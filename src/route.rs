//! `hoppy route <target>`: which adapter would traffic to this target use?
//!
//! With Wi-Fi, Ethernet, a VPN, and Docker all up at once, "which way does
//! this go?" is a real question. The OS already knows the answer, and asking
//! it costs nothing: opening a UDP socket "to" the target makes the OS pick a
//! source address without sending a single packet.

use crate::doctor::{parse_target, resolve};
use crate::net::{self, Iface, Kind};
use crate::style::{bold, dim, emit, green, json_on, say, table, yellow};
use netdev::ipnet::Ipv4Net;
use serde_json::json;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

const DNS_TIMEOUT: Duration = Duration::from_secs(4);

/// How traffic gets from here to the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum How {
    /// The target is this machine.
    ThisMachine,
    /// Same network: delivered straight to the device, no router involved.
    Direct(Ipv4Net),
    /// Handed to a router to forward. `None` when the OS reports no gateway
    /// for the adapter, as with most VPN tunnels.
    Gateway(Option<Ipv4Addr>),
    /// The OS picked a source address hoppy can't match to an adapter.
    Unknown,
}

impl How {
    fn key(&self) -> &'static str {
        match self {
            How::ThisMachine => "this-machine",
            How::Direct(_) => "direct",
            How::Gateway(_) => "gateway",
            How::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Path<'a> {
    pub source: IpAddr,
    pub iface: Option<&'a Iface>,
    pub how: How,
}

/// The source address the OS would use to reach `dest`. Connecting a UDP
/// socket only records the destination; nothing goes on the wire.
fn source_for(dest: IpAddr) -> io::Result<IpAddr> {
    let local: SocketAddr = match dest {
        IpAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local)?;
    socket.connect((dest, 53))?;
    Ok(socket.local_addr()?.ip())
}

/// Match the OS's chosen source address to an adapter and work out whether
/// the target is local or behind a router.
pub fn explain<'a>(dest: IpAddr, source: IpAddr, ifaces: &'a [Iface]) -> Path<'a> {
    let iface = ifaces.iter().find(|iface| {
        iface
            .ipv4
            .iter()
            .any(|net| IpAddr::V4(net.addr()) == source)
    });
    let how = if dest.is_loopback() || dest == source {
        How::ThisMachine
    } else {
        match (iface, dest) {
            (Some(iface), IpAddr::V4(dest)) => iface
                .ipv4
                .iter()
                .find(|net| net.contains(&dest))
                .map_or(How::Gateway(iface.gateway), |net| How::Direct(net.trunc())),
            _ => How::Unknown,
        }
    };
    Path { source, iface, how }
}

/// One sentence describing the path.
pub fn sentence(path: &Path, target: &str) -> String {
    let adapter = path.iface.map_or("an unknown adapter", |i| i.name.as_str());
    let vpn = path.iface.is_some_and(|i| i.kind == Kind::Vpn);
    match &path.how {
        How::ThisMachine => format!("{target} is this machine. Traffic never leaves it."),
        How::Direct(net) => format!(
            "{target} is on {net}, the same network as {adapter}. Traffic goes straight \
             there with no router in between."
        ),
        How::Gateway(_) if vpn => {
            format!("Traffic to {target} goes through your VPN ({adapter}).")
        }
        How::Gateway(Some(gateway)) => format!(
            "{target} isn't on any local network, so traffic leaves through {adapter} and \
             the router at {gateway} forwards it."
        ),
        How::Gateway(None) => format!("Traffic to {target} leaves through {adapter}."),
        How::Unknown => format!(
            "Traffic to {target} leaves from {}, but hoppy can't tell which adapter that is.",
            path.source
        ),
    }
}

fn next_hop(how: &How) -> String {
    match how {
        How::ThisMachine => dim("none, it's this machine"),
        How::Direct(net) => format!("direct {}", dim(&format!("(on {net})"))),
        How::Gateway(Some(gateway)) => format!("{gateway} {}", dim("(router)")),
        How::Gateway(None) | How::Unknown => dim("not reported"),
    }
}

/// `hoppy route <target>`
pub fn run(input: &str) -> Result<bool, String> {
    let target = parse_target(input)?;
    let dest = match target.ip() {
        Some(ip) => ip,
        None => {
            let (addrs, _) = resolve(&target.host, target.port, DNS_TIMEOUT).ok_or_else(|| {
                format!(
                    "couldn't look up {0}. Run `hoppy dns {0}` to see why.",
                    target.host
                )
            })?;
            // IPv4 first, to match what `hoppy doctor` connects to.
            addrs
                .iter()
                .find(|addr| addr.is_ipv4())
                .unwrap_or(&addrs[0])
                .ip()
        }
    };
    let source = source_for(dest).map_err(|_| {
        format!("no adapter has a route to {dest}. Run `hoppy` to see your networks.")
    })?;

    let ifaces = net::load_interfaces();
    let path = explain(dest, source, &ifaces);
    let summary = sentence(&path, &target.host);

    if json_on() {
        emit(&json!({
            "target": target.host,
            "address": dest.to_string(),
            "source": source.to_string(),
            "interface": path.iface.map(|i| i.name.as_str()),
            "kind": path.iface.map(|i| i.kind.label()),
            "how": path.how.key(),
            "gateway": match path.how {
                How::Gateway(gateway) => gateway.map(|g| g.to_string()),
                _ => None,
            },
            "summary": summary,
        }));
        return Ok(true);
    }

    let shown_target = if target.ip().is_some() {
        bold(&dest.to_string())
    } else {
        format!("{} {}", bold(&target.host), dim(&format!("({dest})")))
    };
    let leaves = match path.iface {
        Some(iface) => format!(
            "{} {} {}",
            bold(&iface.name),
            dim(&format!("({})", iface.kind.label())),
            dim(&format!("as {source}"))
        ),
        None => format!("{source}"),
    };
    let rows = vec![
        vec![dim("target"), shown_target],
        vec![dim("leaves via"), leaves],
        vec![dim("next hop"), next_hop(&path.how)],
    ];
    say("");
    for line in table(&rows) {
        say(&format!("  {line}"));
    }
    say("");
    let mark = if path.how == How::Unknown {
        yellow("?")
    } else {
        green("✓")
    };
    say(&format!("  {mark} {}", bold(&summary)));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::iface;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn two_networks() -> Vec<Iface> {
        let mut vpn = iface("utun3", &["10.8.0.2/24"], None);
        vpn.kind = Kind::Vpn;
        vec![
            iface("Wi-Fi", &["192.168.1.42/24"], Some("192.168.1.1")),
            iface("Dante", &["10.10.0.5/24"], None),
            vpn,
        ]
    }

    #[test]
    fn neighbor_on_the_same_subnet_is_direct() {
        let ifaces = two_networks();
        let path = explain(ip("10.10.0.77"), ip("10.10.0.5"), &ifaces);
        assert_eq!(path.iface.unwrap().name, "Dante");
        assert_eq!(path.how, How::Direct("10.10.0.0/24".parse().unwrap()));
        assert!(sentence(&path, "mixer").contains("same network as Dante"));
    }

    #[test]
    fn internet_targets_use_the_gateway() {
        let ifaces = two_networks();
        let path = explain(ip("93.184.216.34"), ip("192.168.1.42"), &ifaces);
        assert_eq!(path.how, How::Gateway(Some(Ipv4Addr::new(192, 168, 1, 1))));
        let text = sentence(&path, "example.com");
        assert!(text.contains("leaves through Wi-Fi"));
        assert!(text.contains("192.168.1.1"));
    }

    #[test]
    fn vpn_routes_are_called_out() {
        let ifaces = two_networks();
        let path = explain(ip("172.16.5.5"), ip("10.8.0.2"), &ifaces);
        assert_eq!(path.how, How::Gateway(None));
        assert!(sentence(&path, "intranet").contains("through your VPN (utun3)"));
    }

    #[test]
    fn own_addresses_and_loopback_never_leave() {
        let ifaces = two_networks();
        let own = explain(ip("192.168.1.42"), ip("192.168.1.42"), &ifaces);
        assert_eq!(own.how, How::ThisMachine);
        let loopback = explain(ip("127.0.0.1"), ip("127.0.0.1"), &ifaces);
        assert_eq!(loopback.how, How::ThisMachine);
        assert!(sentence(&loopback, "localhost").contains("this machine"));
    }

    #[test]
    fn unmatched_source_is_unknown() {
        let ifaces = two_networks();
        let path = explain(ip("2606:4700::1111"), ip("2001:db8::5"), &ifaces);
        assert!(path.iface.is_none());
        assert_eq!(path.how, How::Unknown);
        assert!(sentence(&path, "one.one").contains("2001:db8::5"));
    }
}
