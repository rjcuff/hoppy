//! What a device on the LAN calls itself.
//!
//! Three places are asked, in order: the device (mDNS, what Apple and Linux
//! machines use for `name.local`), the network's DNS server (routers usually
//! remember the names devices gave when they asked for an address), and the
//! device again over NetBIOS (Windows machines and most NAS boxes).

use crate::dns::{self, Kind, Record};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

const MDNS_PORT: u16 = 5353;
const NETBIOS_PORT: u16 = 137;
const TIMEOUT: Duration = Duration::from_millis(500);
/// mDNS flag asking the device to reply straight to us instead of to the
/// whole network.
const UNICAST_REPLY: u16 = 0x8000;
/// NetBIOS "node status": list the names this machine has registered.
const NBSTAT: u16 = 0x21;
/// Each NetBIOS name entry: 15 characters, a type byte, two flag bytes.
const NETBIOS_ENTRY_LEN: usize = 18;

/// Best name for the device at `ip`, if it will tell us one.
pub fn device_name(ip: Ipv4Addr, dns_servers: &[IpAddr]) -> Option<String> {
    mdns_name(ip)
        .or_else(|| dns_name(ip, dns_servers))
        .or_else(|| netbios_name(ip))
}

fn first_name(records: &[Record]) -> Option<String> {
    records.iter().find_map(|record| match record {
        Record::Name(name) if !name.is_empty() => Some(name.clone()),
        _ => None,
    })
}

fn mdns_name(ip: Ipv4Addr) -> Option<String> {
    let name = dns::reverse_name(IpAddr::V4(ip));
    let packet =
        dns::build_query(0, &name, Kind::Ptr.code(), dns::CLASS_IN | UNICAST_REPLY).ok()?;
    let server = SocketAddr::new(IpAddr::V4(ip), MDNS_PORT);
    let (reply, _) = dns::exchange(server, &packet, TIMEOUT, |bytes| {
        dns::parse_reply(bytes, None).ok()
    })
    .ok()?;
    first_name(&reply.records)
}

fn dns_name(ip: Ipv4Addr, dns_servers: &[IpAddr]) -> Option<String> {
    let server = *dns_servers.first()?;
    let name = dns::reverse_name(IpAddr::V4(ip));
    let (reply, _) = dns::query(server, &name, Kind::Ptr, TIMEOUT).ok()?;
    first_name(&reply.records)
}

fn netbios_query() -> Vec<u8> {
    // Header: id, flags, one question. The name is "*" (any), padded with
    // zero bytes to 16 and spelled in NetBIOS's two-letters-per-byte code.
    let mut packet = vec![0x68, 0x70, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 32];
    packet.extend_from_slice(b"CK");
    packet.extend_from_slice(&[b'A'; 30]);
    packet.push(0);
    packet.extend_from_slice(&NBSTAT.to_be_bytes());
    packet.extend_from_slice(&dns::CLASS_IN.to_be_bytes());
    packet
}

/// Pull the machine's own name out of a node-status reply.
pub fn parse_netbios(buf: &[u8]) -> Option<String> {
    let flags = dns::u16_at(buf, 2)?;
    if flags & 0x8000 == 0 {
        return None;
    }
    let mut pos = 12;
    for _ in 0..dns::u16_at(buf, 4)? {
        pos = dns::read_name(buf, pos)?.1 + 4;
    }
    let (_, next) = dns::read_name(buf, pos)?;
    if dns::u16_at(buf, next)? != NBSTAT {
        return None;
    }
    let count = usize::from(*buf.get(next + 10)?);
    let entries = buf.get(next + 11..)?;
    entries
        .chunks_exact(NETBIOS_ENTRY_LEN)
        .take(count)
        .find_map(|entry| {
            // Type 0x00 without the "group" flag is the computer's own name
            // (the group entry of the same type is its workgroup).
            let is_group = entry[16] & 0x80 != 0;
            let name = dns::clean(&entry[..15]).trim().to_string();
            (entry[15] == 0x00 && !is_group && !name.is_empty()).then_some(name)
        })
}

fn netbios_name(ip: Ipv4Addr) -> Option<String> {
    let server = SocketAddr::new(IpAddr::V4(ip), NETBIOS_PORT);
    dns::exchange(server, &netbios_query(), TIMEOUT, parse_netbios)
        .ok()
        .map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn netbios_reply(entries: &[(&str, u8, u8)]) -> Vec<u8> {
        let mut p = vec![0x68, 0x70, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0, 32];
        p.extend_from_slice(b"CK");
        p.extend_from_slice(&[b'A'; 30]);
        p.push(0);
        p.extend_from_slice(&[0, 0x21, 0, 1, 0, 0, 0, 0, 0, 0]);
        p.push(entries.len() as u8);
        for (name, kind, flags) in entries {
            p.extend_from_slice(format!("{name:<15}").as_bytes());
            p.extend_from_slice(&[*kind, *flags, 0]);
        }
        p
    }

    #[test]
    fn netbios_query_is_fifty_bytes() {
        assert_eq!(netbios_query().len(), 50);
    }

    #[test]
    fn netbios_picks_the_computer_name_not_the_workgroup() {
        let reply = netbios_reply(&[
            ("WORKGROUP", 0x00, 0x84),
            ("NAS-OFFICE", 0x00, 0x04),
            ("NAS-OFFICE", 0x20, 0x04),
        ]);
        assert_eq!(parse_netbios(&reply), Some("NAS-OFFICE".to_string()));
    }

    #[test]
    fn netbios_ignores_garbage() {
        assert_eq!(parse_netbios(&[]), None);
        assert_eq!(parse_netbios(&netbios_query()), None);
        assert_eq!(parse_netbios(&netbios_reply(&[])), None);
    }

    #[test]
    fn first_name_skips_other_records() {
        let records = [
            Record::A(Ipv4Addr::LOCALHOST),
            Record::Name("printer.local".to_string()),
        ];
        assert_eq!(first_name(&records), Some("printer.local".to_string()));
        assert_eq!(first_name(&[]), None);
    }
}
