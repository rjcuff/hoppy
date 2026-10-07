//! The OS's neighbor (ARP) table: which IP on the LAN has which MAC address.
//!
//! Before a machine can send anything to a LAN neighbor it has to ask "who
//! has this IP?" and remember the answer. Reading that memory needs no
//! special permissions, and devices can't refuse to answer the question, so
//! it finds hosts that ignore every other kind of probe.

use std::net::Ipv4Addr;
use std::process::Command;

/// The raw table, in whatever format this OS prints it.
pub fn read_table() -> Option<String> {
    if cfg!(target_os = "linux") {
        if let Ok(text) = std::fs::read_to_string("/proc/net/arp") {
            return Some(text);
        }
        return run("ip", &["-4", "neigh"]);
    }
    if cfg!(windows) {
        run("arp", &["-a"])
    } else {
        // -n: don't look up names, which can stall for seconds per entry.
        run("arp", &["-an"])
    }
}

fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Accepts `aa:bb:cc:dd:ee:ff`, Windows' `aa-bb-cc-dd-ee-ff`, and macOS's
/// habit of dropping leading zeros (`0:1c:42:0:0:8`).
pub fn parse_mac(token: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = token.split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut mac = [0u8; 6];
    for (slot, part) in mac.iter_mut().zip(parts) {
        if part.is_empty() || part.len() > 2 {
            return None;
        }
        *slot = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

/// A real device's address: not empty, not broadcast, and not one of the
/// multicast group addresses Windows lists alongside real neighbors.
fn is_device(mac: [u8; 6]) -> bool {
    mac != [0; 6] && mac[0] & 1 == 0
}

/// Every (IP, MAC) pair in the table. Works on the output of `arp -a`
/// (Windows, macOS, BSD), `ip neigh`, and `/proc/net/arp` by looking for one
/// IP and one MAC on each line, wherever they sit.
pub fn parse(table: &str) -> Vec<(Ipv4Addr, [u8; 6])> {
    table
        .lines()
        .filter_map(|line| {
            let tokens = || line.split_whitespace();
            let ip = tokens().find_map(|token| {
                token
                    .trim_matches(|c| c == '(' || c == ')')
                    .parse::<Ipv4Addr>()
                    .ok()
            })?;
            let mac = tokens().find_map(parse_mac)?;
            is_device(mac).then_some((ip, mac))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(table: &str) -> Vec<String> {
        parse(table).iter().map(|(ip, _)| ip.to_string()).collect()
    }

    #[test]
    fn parses_windows_arp() {
        let table = "\
Interface: 192.168.1.42 --- 0x5
  Internet Address      Physical Address      Type
  192.168.1.1           b4-fb-e4-11-22-33     dynamic
  192.168.1.50          dc-a6-32-0a-0b-0c     dynamic
  192.168.1.255         ff-ff-ff-ff-ff-ff     static
  224.0.0.22            01-00-5e-00-00-16     static
";
        assert_eq!(ips(table), ["192.168.1.1", "192.168.1.50"]);
        assert_eq!(parse(table)[0].1, [0xb4, 0xfb, 0xe4, 0x11, 0x22, 0x33]);
    }

    #[test]
    fn parses_macos_arp_with_short_octets() {
        let table = "\
? (192.168.1.1) at b4:fb:e4:11:22:33 on en0 ifscope [ethernet]
? (192.168.1.7) at 0:1c:42:0:0:8 on en0 ifscope [ethernet]
? (192.168.1.9) at (incomplete) on en0 ifscope [ethernet]
? (224.0.0.251) at 1:0:5e:0:0:fb on en0 ifscope permanent [ethernet]
";
        let found = parse(table);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].1, [0x00, 0x1c, 0x42, 0x00, 0x00, 0x08]);
    }

    #[test]
    fn parses_linux_proc_and_ip_neigh() {
        let proc_table = "\
IP address       HW type     Flags       HW address            Mask     Device
192.168.1.1      0x1         0x2         b4:fb:e4:11:22:33     *        wlan0
192.168.1.77     0x1         0x0         00:00:00:00:00:00     *        wlan0
";
        assert_eq!(ips(proc_table), ["192.168.1.1"]);

        let ip_neigh = "\
192.168.1.1 dev wlan0 lladdr b4:fb:e4:11:22:33 REACHABLE
192.168.1.80 dev wlan0  FAILED
";
        assert_eq!(ips(ip_neigh), ["192.168.1.1"]);
    }

    #[test]
    fn rejects_things_that_only_look_like_macs() {
        assert_eq!(parse_mac("fe80::1"), None);
        assert_eq!(parse_mac("aa:bb:cc:dd:ee"), None);
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:fff"), None);
        assert_eq!(parse_mac("zz:bb:cc:dd:ee:ff"), None);
        assert_eq!(
            parse_mac("12-34-56-78-9a-bc"),
            Some([0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc])
        );
    }
}
