//! `hoppy ip`: this machine's address on the LAN and on the internet.
//!
//! The public address is the one the rest of the internet sees, usually the
//! router's. The only way to learn it is to ask a server outside "what
//! address did this come from?", so this makes one small web request.

use crate::doctor::resolve;
use crate::net::{self, Iface};
use crate::style::{bold, dim, emit, json_on, say, table, yellow};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(3);
/// More than enough for a status line, headers, and one address.
const MAX_REPLY: usize = 4096;

/// Services that reply with the caller's address as plain text. Tried in
/// order until one answers.
const MIRRORS: [(&str, &str); 3] = [
    ("checkip.amazonaws.com", "Amazon"),
    ("icanhazip.com", "Cloudflare"),
    ("api.ipify.org", "ipify"),
];

/// The address in an HTTP reply whose body is just an IP.
pub fn address_in(reply: &str) -> Option<Ipv4Addr> {
    let (head, body) = reply.split_once("\r\n\r\n")?;
    let status = head.lines().next()?;
    if !status.starts_with("HTTP/") || status.split(' ').nth(1) != Some("200") {
        return None;
    }
    body.trim().parse().ok()
}

fn ask(host: &str) -> Option<Ipv4Addr> {
    let (addrs, _) = resolve(host, 80, TIMEOUT)?;
    // Connecting over IPv4 is what makes the answer an IPv4 address.
    let addr = addrs.iter().find(|addr| addr.is_ipv4())?;
    let mut stream = TcpStream::connect_timeout(addr, TIMEOUT).ok()?;
    stream.set_read_timeout(Some(TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(TIMEOUT)).ok()?;
    let request = format!(
        "GET / HTTP/1.0\r\nHost: {host}\r\nUser-Agent: hoppy/{}\r\nConnection: close\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes()).ok()?;
    // Read in pieces against one overall deadline, so a server that trickles
    // a byte at a time can't keep this waiting forever. A read error after
    // some data still leaves a usable reply, so it's judged by its contents.
    let deadline = Instant::now() + TIMEOUT;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 512];
    while reply.len() < MAX_REPLY && Instant::now() < deadline {
        match stream.read(&mut chunk) {
            Ok(len) if len > 0 => reply.extend_from_slice(&chunk[..len]),
            _ => break,
        }
    }
    address_in(&String::from_utf8_lossy(&reply))
}

/// This machine's public IPv4 address and who reported it.
pub fn public_ipv4() -> Option<(Ipv4Addr, &'static str)> {
    MIRRORS
        .iter()
        .find_map(|(host, who)| ask(host).map(|ip| (ip, *who)))
}

/// `hoppy ip`. Returns whether the public address was found.
pub fn run() -> Result<bool, String> {
    let live: Vec<Iface> = net::load_interfaces()
        .into_iter()
        .filter(Iface::is_interesting)
        .collect();
    let local = live
        .iter()
        .find(|i| i.is_default)
        .or_else(|| live.iter().find(|i| i.real_ipv4().is_some()))
        .and_then(|i| i.real_ipv4().map(|net| (net.addr(), i)));
    let public = public_ipv4();
    // With no router rewriting addresses in between (NAT), the public
    // address is one of this machine's own.
    let direct = public.is_some_and(|(ip, _)| {
        live.iter()
            .any(|i| i.ipv4.iter().any(|net| net.addr() == ip))
    });

    if json_on() {
        emit(&json!({
            "local": local.map(|(ip, iface)| json!({
                "ip": ip.to_string(),
                "interface": iface.name,
            })),
            "public": public.map(|(ip, who)| json!({
                "ip": ip.to_string(),
                "seen_by": who,
            })),
            "nat": public.map(|_| !direct),
        }));
        return Ok(public.is_some());
    }

    let rows = vec![
        match local {
            Some((ip, iface)) => vec![
                dim("local"),
                bold(&ip.to_string()),
                dim(&format!("{} ({})", iface.name, iface.kind.label())),
            ],
            None => vec![dim("local"), dim("not connected")],
        },
        match public {
            Some((ip, who)) => vec![
                dim("public"),
                bold(&ip.to_string()),
                dim(&format!("as seen by {who}")),
            ],
            None => vec![dim("public"), dim("unknown")],
        },
    ];
    say("");
    for line in table(&rows) {
        say(&format!("  {line}"));
    }
    if direct {
        say("");
        say(&format!(
            "  {} This machine is directly on the internet, with no router in between. \
             Everything in `hoppy ports --exposed` is reachable by anyone.",
            yellow("!")
        ));
    } else if public.is_none() {
        say("");
        say(&format!(
            "  {} Couldn't find the public address. Run `hoppy doctor` to see if you're online.",
            yellow("!")
        ));
    }
    Ok(public.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_address_from_a_plain_text_body() {
        let reply = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n203.0.113.7\n";
        assert_eq!(address_in(reply), Some(Ipv4Addr::new(203, 0, 113, 7)));
    }

    #[test]
    fn ignores_errors_and_pages_that_are_not_an_address() {
        assert_eq!(address_in(""), None);
        assert_eq!(
            address_in("HTTP/1.1 200 OK\r\n\r\n<html>login</html>"),
            None
        );
        assert_eq!(address_in("HTTP/1.1 503 Busy\r\n\r\n203.0.113.7"), None);
        assert_eq!(address_in("203.0.113.7"), None);
    }
}
