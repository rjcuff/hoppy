//! `hoppy` with no arguments: the network at a glance.

use crate::net::{self, Iface, port_label};
use crate::ports::{self, PortRow, Reach};
use crate::style::{bold, cyan, dim, green, say, table, yellow};
use std::net::IpAddr;

/// How many listening ports the overview shows before pointing at
/// `hoppy ports`.
const TOP_PORTS: usize = 6;

/// Ports from here up are "ephemeral": handed out at random by the OS for
/// short-lived use, so they're rarely the ones you're looking for.
const EPHEMERAL_START: u16 = 49152;

/// Print the overview. `show_all` includes loopback, down, and IP-less
/// interfaces.
pub fn run(show_all: bool) -> Result<(), String> {
    let all = net::load_interfaces();
    let live: Vec<Iface> = all.iter().filter(|i| i.is_interesting()).cloned().collect();
    let shown: &[Iface] = if show_all { &all } else { &live };

    say("");
    if shown.is_empty() {
        say(&format!(
            "  {} No active network connections. Join Wi-Fi or plug in a cable, \
             then run `hoppy --all` to see every adapter.",
            yellow("!")
        ));
    } else {
        for line in table(&interface_rows(shown)) {
            say(&format!("  {line}"));
        }
    }

    let dns = dns_servers(&live);
    if !dns.is_empty() {
        let list: Vec<String> = dns.iter().map(IpAddr::to_string).collect();
        say("");
        say(&format!("  {}  {}", dim("DNS"), list.join(", ")));
    }

    print_listening();

    let warnings = net::warnings(&live);
    if !live.is_empty() && !warnings.is_empty() {
        say("");
        for warning in warnings {
            say(&format!("  {} {warning}", yellow("!")));
        }
    }

    let hidden = all.len() - shown.len();
    say("");
    if hidden > 0 {
        say(&dim(&format!(
            "  {hidden} inactive or internal adapters hidden · hoppy --all"
        )));
    }
    say(&dim(
        "  More: hoppy ports · hoppy port <n> · hoppy doctor [target]",
    ));
    Ok(())
}

/// One table row per interface, plus an indented row for each extra IP.
fn interface_rows(ifaces: &[Iface]) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for iface in ifaces {
        let dot = if iface.up { green("●") } else { dim("○") };
        let first_ip = iface
            .ipv4
            .first()
            .map_or_else(|| dim("no IP"), |net| net.to_string());
        let gateway = match iface.gateway {
            Some(gw) => format!("{} {gw}", dim("gw")),
            None => dim("no gateway"),
        };
        let note = if iface.is_default {
            green("← internet")
        } else if !iface.up {
            dim("down")
        } else {
            String::new()
        };

        rows.push(vec![
            dot,
            bold(&iface.name),
            iface.kind.label().to_string(),
            first_ip,
            gateway,
            note,
        ]);
        // An interface can hold several addresses at once (common when a
        // static IP is added alongside DHCP to reach AV gear).
        for extra in iface.ipv4.iter().skip(1) {
            rows.push(vec![
                String::new(),
                String::new(),
                String::new(),
                format!("{} {extra}", dim("+")),
            ]);
        }
    }
    rows
}

/// DNS servers across live interfaces, in interface order, no repeats.
fn dns_servers(ifaces: &[Iface]) -> Vec<IpAddr> {
    let mut seen = Vec::new();
    for ip in ifaces.iter().flat_map(|i| &i.dns) {
        if !is_placeholder_dns(ip) && !seen.contains(ip) {
            seen.push(*ip);
        }
    }
    seen
}

/// Windows lists `fec0:0:0:ffff::1..3` on adapters with no real IPv6 DNS.
/// They're long-deprecated "site-local" defaults that never answer.
fn is_placeholder_dns(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfec0,
        IpAddr::V4(_) => false,
    }
}

/// Pick the listeners most worth a glance: well-known ports before
/// ephemeral ones, network-reachable before local-only, then by port.
fn top_ports(rows: &[PortRow], limit: usize) -> Vec<&PortRow> {
    let mut picked: Vec<&PortRow> = rows.iter().collect();
    picked.sort_by_key(|r| {
        (
            r.port >= EPHEMERAL_START,
            port_label(r.port).is_none(),
            r.reach != Reach::WholeNetwork,
            r.port,
        )
    });
    picked.truncate(limit);
    picked.sort_by_key(|r| r.port);
    picked
}

fn print_listening() {
    // The overview should still work if the port list can't be read.
    let Ok(rows) = ports::load(false) else {
        return;
    };
    if rows.is_empty() {
        return;
    }

    say("");
    say(&format!("  {}", dim("Listening")));
    let cells: Vec<Vec<String>> = top_ports(&rows, TOP_PORTS)
        .into_iter()
        .map(|r| {
            vec![
                bold(&format!(":{}", r.port)),
                cyan(&r.process),
                r.reach.label().to_string(),
                dim(port_label(r.port).unwrap_or("")),
            ]
        })
        .collect();
    for line in table(&cells) {
        say(&format!("    {line}"));
    }
    if rows.len() > TOP_PORTS {
        say(&dim(&format!(
            "    +{} more · hoppy ports",
            rows.len() - TOP_PORTS
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::iface;
    use crate::ports::Proto;
    use crate::style::visible_width;

    fn row(port: u16, reach: Reach) -> PortRow {
        PortRow {
            port,
            proto: Proto::Tcp,
            process: "p".to_string(),
            pid: 1,
            reach,
        }
    }

    #[test]
    fn extra_ips_get_their_own_indented_row() {
        let ifaces = [iface(
            "eth0",
            &["192.168.1.20/24", "10.10.0.5/24"],
            Some("192.168.1.1"),
        )];
        let rows = interface_rows(&ifaces);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][3], "192.168.1.20/24");
        assert!(rows[1][0].is_empty());
        assert!(rows[1][3].contains("10.10.0.5/24"));
    }

    #[test]
    fn default_interface_is_marked() {
        let mut wifi = iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1"));
        wifi.is_default = true;
        let rows = interface_rows(&[wifi, iface("Dante", &["10.0.0.2/24"], None)]);
        assert!(rows[0][5].contains("← internet"));
        assert!(rows[1][5].is_empty());
        assert!(rows[1][4].contains("no gateway"));
    }

    #[test]
    fn interface_table_columns_line_up() {
        let rows = interface_rows(&[
            iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1")),
            iface("USB Ethernet adapter", &["10.0.0.2/8"], None),
        ]);
        let lines = table(&rows);
        let ip_column = |line: &str, ip: &str| visible_width(&line[..line.find(ip).unwrap()]);
        assert_eq!(
            ip_column(&lines[0], "192.168.1.20"),
            ip_column(&lines[1], "10.0.0.2")
        );
    }

    #[test]
    fn dns_is_deduplicated_and_skips_windows_placeholders() {
        let mut a = iface("a", &["192.168.1.2/24"], None);
        a.dns = vec![
            "192.168.1.1".parse().unwrap(),
            "fec0:0:0:ffff::1".parse().unwrap(),
        ];
        let mut b = iface("b", &["10.0.0.2/24"], None);
        b.dns = vec!["192.168.1.1".parse().unwrap(), "1.1.1.1".parse().unwrap()];

        let dns: Vec<String> = dns_servers(&[a, b]).iter().map(|d| d.to_string()).collect();
        assert_eq!(dns, ["192.168.1.1", "1.1.1.1"]);
    }

    #[test]
    fn top_ports_prefers_known_ports_over_ephemeral() {
        let rows = vec![
            row(49664, Reach::WholeNetwork),
            row(49665, Reach::WholeNetwork),
            row(3000, Reach::ThisMachine),
            row(22, Reach::WholeNetwork),
            row(12345, Reach::ThisMachine),
        ];
        let ports: Vec<u16> = top_ports(&rows, 3).iter().map(|r| r.port).collect();
        assert_eq!(ports, [22, 3000, 12345]);
    }

    #[test]
    fn top_ports_respects_the_limit() {
        let rows: Vec<PortRow> = (1..=20).map(|p| row(p, Reach::ThisMachine)).collect();
        assert_eq!(top_ports(&rows, TOP_PORTS).len(), TOP_PORTS);
    }
}
