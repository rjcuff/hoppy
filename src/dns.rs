//! A small DNS client: build a question, send it over UDP, read the answer.
//!
//! The OS resolver only says "found" or "not found". Asking a server directly
//! shows which server answered, how fast, and exactly what it said.

use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DNS_PORT: u16 = 53;
/// Response code for "this name does not exist" (NXDOMAIN).
pub const NO_SUCH_NAME: u8 = 3;
/// The normal "internet" class every everyday query uses.
pub const CLASS_IN: u16 = 1;

const HEADER_LEN: usize = 12;
const MAX_NAME_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;
/// Names can point back into the packet to save space ("compression"). A
/// hostile packet can make those pointers loop, so reading gives up here.
const MAX_NAME_STEPS: usize = 128;

/// The kind of record being asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    A,
    Ns,
    Cname,
    Ptr,
    Mx,
    Txt,
    Aaaa,
}

impl Kind {
    pub fn code(self) -> u16 {
        match self {
            Kind::A => 1,
            Kind::Ns => 2,
            Kind::Cname => 5,
            Kind::Ptr => 12,
            Kind::Mx => 15,
            Kind::Txt => 16,
            Kind::Aaaa => 28,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::A => "A",
            Kind::Ns => "NS",
            Kind::Cname => "CNAME",
            Kind::Ptr => "PTR",
            Kind::Mx => "MX",
            Kind::Txt => "TXT",
            Kind::Aaaa => "AAAA",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        let kind = match text.to_ascii_lowercase().as_str() {
            "a" => Kind::A,
            "ns" => Kind::Ns,
            "cname" => Kind::Cname,
            "ptr" => Kind::Ptr,
            "mx" => Kind::Mx,
            "txt" => Kind::Txt,
            "aaaa" => Kind::Aaaa,
            _ => return None,
        };
        Some(kind)
    }
}

/// One answer from a DNS server.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Record {
    A(Ipv4Addr),
    Aaaa(Ipv6Addr),
    /// "This name is an alias for that one."
    Cname(String),
    /// A host name: the answer to a PTR (reverse) or NS query.
    Name(String),
    Mx(String),
    Txt(String),
}

impl Record {
    pub fn is(&self, kind: Kind) -> bool {
        matches!(
            (self, kind),
            (Record::A(_), Kind::A)
                | (Record::Aaaa(_), Kind::Aaaa)
                | (Record::Cname(_), Kind::Cname)
                | (Record::Name(_), Kind::Ptr | Kind::Ns)
                | (Record::Mx(_), Kind::Mx)
                | (Record::Txt(_), Kind::Txt)
        )
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Record::A(ip) => write!(f, "{ip}"),
            Record::Aaaa(ip) => write!(f, "{ip}"),
            Record::Cname(name) | Record::Name(name) | Record::Mx(name) => write!(f, "{name}"),
            Record::Txt(text) => write!(f, "\"{text}\""),
        }
    }
}

/// A parsed reply: the server's status code and its answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub code: u8,
    pub records: Vec<Record>,
    /// The server had more to say than fits in one packet, so some answers
    /// are missing.
    pub truncated: bool,
}

/// Build a query packet asking `name` for records of type `kind_code`.
pub fn build_query(id: u16, name: &str, kind_code: u16, class: u16) -> Result<Vec<u8>, String> {
    let name = name.trim_end_matches('.');
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return Err(format!("\"{name}\" isn't a valid name to look up"));
    }

    let mut packet = Vec::with_capacity(HEADER_LEN + name.len() + 6);
    packet.extend_from_slice(&id.to_be_bytes());
    // Flags: a standard query that asks the server to chase the answer for us
    // ("recursion desired").
    packet.extend_from_slice(&[0x01, 0x00]);
    // One question, no answers or extras.
    packet.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
    for label in name.split('.') {
        if label.is_empty() || label.len() > MAX_LABEL_LEN {
            return Err(format!("\"{name}\" isn't a valid name to look up"));
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    packet.extend_from_slice(&kind_code.to_be_bytes());
    packet.extend_from_slice(&class.to_be_bytes());
    Ok(packet)
}

/// Text from the network goes straight to a terminal, so control characters
/// (which could move the cursor or recolor the screen) are replaced.
pub fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .map(|c| {
            if c.is_control() || is_invisible(c) {
                '?'
            } else {
                c
            }
        })
        .collect()
}

/// Characters that print nothing but change how the text around them is
/// shown: zero-width marks and the overrides that reverse text direction.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
    )
}

pub fn u16_at(buf: &[u8], pos: usize) -> Option<u16> {
    let bytes = buf.get(pos..pos.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// Read a name starting at `start`. Returns the name and the position just
/// past it in the original (uncompressed) stream.
pub fn read_name(buf: &[u8], start: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut pos = start;
    let mut resume = None;
    for _ in 0..MAX_NAME_STEPS {
        let len = usize::from(*buf.get(pos)?);
        if len == 0 {
            return Some((labels.join("."), resume.unwrap_or(pos + 1)));
        }
        if len & 0xC0 == 0xC0 {
            // A pointer: the rest of the name lives elsewhere in the packet.
            let low = usize::from(*buf.get(pos + 1)?);
            resume.get_or_insert(pos + 2);
            pos = ((len & 0x3F) << 8) | low;
            continue;
        }
        if len > MAX_LABEL_LEN {
            return None;
        }
        labels.push(clean(buf.get(pos + 1..pos + 1 + len)?));
        pos += 1 + len;
    }
    None
}

fn parse_record(buf: &[u8], kind_code: u16, at: usize, data: &[u8]) -> Option<Record> {
    match kind_code {
        1 => <[u8; 4]>::try_from(data).ok().map(|b| Record::A(b.into())),
        28 => <[u8; 16]>::try_from(data)
            .ok()
            .map(|b| Record::Aaaa(b.into())),
        5 => read_name(buf, at).map(|(name, _)| Record::Cname(name)),
        2 | 12 => read_name(buf, at).map(|(name, _)| Record::Name(name)),
        // MX data starts with a 2-byte priority, then the mail server's name.
        15 => read_name(buf, at + 2).map(|(name, _)| Record::Mx(name)),
        16 => {
            // TXT data is one or more length-prefixed strings.
            let mut text = String::new();
            let mut rest = data;
            while let Some((&len, tail)) = rest.split_first() {
                let chunk = tail.get(..usize::from(len))?;
                text.push_str(&clean(chunk));
                rest = &tail[usize::from(len)..];
            }
            Some(Record::Txt(text))
        }
        _ => None,
    }
}

/// Parse a reply packet. With `expect_id`, replies to some other question
/// are rejected.
pub fn parse_reply(buf: &[u8], expect_id: Option<u16>) -> Result<Reply, String> {
    let malformed = || "the reply was malformed".to_string();
    if buf.len() < HEADER_LEN {
        return Err(malformed());
    }
    let id = u16_at(buf, 0).ok_or_else(malformed)?;
    let flags = u16_at(buf, 2).ok_or_else(malformed)?;
    let questions = u16_at(buf, 4).ok_or_else(malformed)?;
    let answers = u16_at(buf, 6).ok_or_else(malformed)?;
    if expect_id.is_some_and(|expected| expected != id) || flags & 0x8000 == 0 {
        return Err("that wasn't a reply to our question".to_string());
    }

    let mut pos = HEADER_LEN;
    for _ in 0..questions {
        let (_, next) = read_name(buf, pos).ok_or_else(malformed)?;
        pos = next + 4;
    }

    let mut records = Vec::new();
    for _ in 0..answers {
        // A reply cut short (too big for one UDP packet) still has useful
        // answers at the front, so stop quietly instead of failing.
        let Some((_, next)) = read_name(buf, pos) else {
            break;
        };
        let (Some(kind_code), Some(len)) = (u16_at(buf, next), u16_at(buf, next + 8)) else {
            break;
        };
        let at = next + 10;
        let Some(data) = buf.get(at..at + usize::from(len)) else {
            break;
        };
        records.extend(parse_record(buf, kind_code, at, data));
        pos = at + usize::from(len);
    }

    Ok(Reply {
        code: (flags & 0x000F) as u8,
        records,
        truncated: flags & 0x0200 != 0,
    })
}

/// A query ID that's hard to guess, so a stray or forged packet isn't
/// mistaken for the answer.
fn next_id() -> u16 {
    static COUNTER: AtomicU16 = AtomicU16::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let step = COUNTER.fetch_add(1, Ordering::Relaxed);
    (nanos as u16 ^ (nanos >> 16) as u16 ^ std::process::id() as u16).wrapping_add(step)
}

/// Send `packet` to `server` and wait for a reply that `accept` recognises.
/// Packets it rejects are ignored and the wait continues.
pub fn exchange<T>(
    server: SocketAddr,
    packet: &[u8],
    timeout: Duration,
    accept: impl Fn(&[u8]) -> Option<T>,
) -> Result<(T, Duration), String> {
    let explain = |e: io::Error| match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => "no answer".to_string(),
        // The host answered "nothing listens on that port".
        io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionRefused => "refused".to_string(),
        _ => format!("network error ({e})"),
    };

    let local: SocketAddr = match server {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).map_err(explain)?;
    // Connecting a UDP socket makes the OS drop packets from anyone else.
    socket.connect(server).map_err(explain)?;

    let start = Instant::now();
    socket.send(packet).map_err(explain)?;
    let mut buf = [0u8; 4096];
    loop {
        let left = timeout
            .checked_sub(start.elapsed())
            .filter(|left| !left.is_zero())
            .ok_or("no answer")?;
        socket.set_read_timeout(Some(left)).map_err(explain)?;
        let len = socket.recv(&mut buf).map_err(explain)?;
        if let Some(found) = accept(&buf[..len]) {
            return Ok((found, start.elapsed()));
        }
    }
}

/// Ask one server one question.
pub fn query(
    server: IpAddr,
    name: &str,
    kind: Kind,
    timeout: Duration,
) -> Result<(Reply, Duration), String> {
    let id = next_id();
    let mut packet = build_query(id, name, kind.code(), CLASS_IN)?;
    // Without this extra record, servers cap replies at 512 bytes and long
    // answers (big TXT or MX sets) get cut off. It says "I can take 4096".
    packet[11] = 1;
    packet.extend_from_slice(&[0, 0, 41, 0x10, 0x00, 0, 0, 0, 0, 0, 0]);
    exchange(
        SocketAddr::new(server, DNS_PORT),
        &packet,
        timeout,
        |bytes| parse_reply(bytes, Some(id)).ok(),
    )
}

/// The name a reverse lookup asks for: `1.2.3.4` becomes
/// `4.3.2.1.in-addr.arpa`.
pub fn reverse_name(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        IpAddr::V6(v6) => {
            let nibbles: Vec<String> = v6
                .octets()
                .iter()
                .rev()
                .flat_map(|byte| [byte & 0x0F, byte >> 4])
                .map(|nibble| format!("{nibble:x}"))
                .collect();
            format!("{}.ip6.arpa", nibbles.join("."))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A reply to "example.com A?" with a CNAME and an A record, both using
    /// compression pointers back to the question.
    pub(crate) fn sample_reply(id: u16) -> Vec<u8> {
        let mut p = build_query(id, "example.com", 1, CLASS_IN).unwrap();
        p[2] = 0x81; // reply, recursion desired
        p[3] = 0x80; // recursion available, code 0
        p[7] = 2; // two answers
        // example.com CNAME www.example.com
        p.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 6]);
        p.extend_from_slice(&[3, b'w', b'w', b'w', 0xC0, 12]);
        // www.example.com A 93.184.216.34 (name points at the CNAME data)
        let cname_at = (p.len() - 6) as u8;
        p.extend_from_slice(&[0xC0, cname_at, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        p.extend_from_slice(&[93, 184, 216, 34]);
        p
    }

    #[test]
    fn builds_a_standard_query() {
        let q = build_query(0xBEEF, "example.com.", 1, CLASS_IN).unwrap();
        assert_eq!(&q[..2], &[0xBE, 0xEF]);
        assert_eq!(&q[4..6], &[0, 1]);
        assert_eq!(
            &q[12..],
            b"\x07example\x03com\x00\x00\x01\x00\x01".as_slice()
        );
    }

    #[test]
    fn rejects_names_that_cannot_be_encoded() {
        assert!(build_query(1, "", 1, CLASS_IN).is_err());
        assert!(build_query(1, "a..b", 1, CLASS_IN).is_err());
        assert!(build_query(1, &"x".repeat(64), 1, CLASS_IN).is_err());
        assert!(build_query(1, &"abc.".repeat(70), 1, CLASS_IN).is_err());
    }

    #[test]
    fn parses_answers_with_compressed_names() {
        let reply = parse_reply(&sample_reply(7), Some(7)).unwrap();
        assert_eq!(reply.code, 0);
        assert_eq!(
            reply.records,
            [
                Record::Cname("www.example.com".to_string()),
                Record::A(Ipv4Addr::new(93, 184, 216, 34)),
            ]
        );
    }

    #[test]
    fn rejects_wrong_id_and_non_replies() {
        assert!(parse_reply(&sample_reply(7), Some(8)).is_err());
        let question = build_query(7, "example.com", 1, CLASS_IN).unwrap();
        assert!(parse_reply(&question, Some(7)).is_err());
        assert!(parse_reply(&[0; 5], None).is_err());
    }

    #[test]
    fn reports_no_such_name() {
        let mut p = build_query(9, "nope.invalid", 1, CLASS_IN).unwrap();
        p[2] = 0x81;
        p[3] = 0x83;
        let reply = parse_reply(&p, Some(9)).unwrap();
        assert_eq!(reply.code, NO_SUCH_NAME);
        assert!(reply.records.is_empty());
    }

    #[test]
    fn truncated_replies_keep_the_answers_that_fit() {
        let full = sample_reply(7);
        let cut = &full[..full.len() - 3];
        let reply = parse_reply(cut, Some(7)).unwrap();
        assert_eq!(reply.records.len(), 1);
        assert!(!reply.truncated);
    }

    #[test]
    fn truncation_flag_is_reported() {
        let mut p = sample_reply(7);
        p[2] |= 0x02;
        assert!(parse_reply(&p, Some(7)).unwrap().truncated);
    }

    #[test]
    fn clean_replaces_direction_overrides() {
        assert_eq!(clean("a\u{202E}b\u{200B}c".as_bytes()), "a?b?c");
        assert_eq!(clean("caf\u{e9}".as_bytes()), "caf\u{e9}");
    }

    #[test]
    fn pointer_loops_do_not_hang() {
        // A name that points at itself.
        let mut p = vec![0u8; HEADER_LEN];
        p.extend_from_slice(&[0xC0, 12]);
        assert_eq!(read_name(&p, 12), None);
    }

    #[test]
    fn parses_txt_and_strips_control_characters() {
        let data = [5, b'h', b'e', 0x1b, b'l', b'o', 2, b'!', b'!'];
        assert_eq!(
            parse_record(&data, 16, 0, &data),
            Some(Record::Txt("he?lo!!".to_string()))
        );
    }

    #[test]
    fn reverse_names() {
        assert_eq!(
            reverse_name("192.168.1.42".parse().unwrap()),
            "42.1.168.192.in-addr.arpa"
        );
        let v6 = reverse_name("2001:db8::1".parse().unwrap());
        assert!(v6.starts_with("1.0.0.0."));
        assert!(v6.ends_with("8.b.d.0.1.0.0.2.ip6.arpa"));
        assert_eq!(v6.split('.').count(), 34);
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [
            Kind::A,
            Kind::Ns,
            Kind::Cname,
            Kind::Ptr,
            Kind::Mx,
            Kind::Txt,
            Kind::Aaaa,
        ] {
            assert_eq!(Kind::parse(kind.label()), Some(kind));
        }
        assert_eq!(Kind::parse("bogus"), None);
    }
}
