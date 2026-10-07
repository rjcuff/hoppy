//! `hoppy scan`: find the devices on your network, with no root.
//!
//! Discovery leans on the OS: sending one tiny packet toward every address
//! makes the OS ask the LAN "who has this IP?" for each, and every device
//! that's switched on has to answer. The answers land in the neighbor table,
//! which anyone can read. Each device found is then asked for its name and
//! checked for a handful of telltale ports.

use crate::doctor::{Probe, tcp_probe};
use crate::names;
use crate::neighbors;
use crate::net::{self, Iface, Kind, is_self_assigned, port_label};
use crate::oui;
use crate::pool;
use crate::style::{bold, cyan, dim, emit, green, json_on, say, table};
use netdev::ipnet::Ipv4Net;
use serde_json::json;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

/// Smallest prefix scanned: a /22 is 1022 addresses.
const MIN_PREFIX: u8 = 22;
const WORKERS: usize = 128;
/// The "discard" port. What's sent doesn't matter, only that the OS has to
/// find the device in order to send it.
const NUDGE_PORT: u16 = 9;
const NUDGE_ROUNDS: u32 = 2;
const NUDGE_PAUSE: Duration = Duration::from_millis(350);
const PORT_TIMEOUT: Duration = Duration::from_millis(400);
/// Ports tried when the neighbor table can't be used (a network this machine
/// isn't directly on): a host is "found" if any of them answers.
const SWEEP_PORTS: [u16; 5] = [22, 80, 443, 445, 3389];
const SWEEP_TIMEOUT: Duration = Duration::from_millis(500);
/// Name lookups each hold a socket open for up to 1.5 s, so fewer run at once.
const NAME_WORKERS: usize = 64;
/// Ports that say something about what a device is.
const TELLTALE_PORTS: [u16; 19] = [
    22, 53, 80, 443, 445, 548, 554, 631, 1883, 3389, 5000, 5900, 7000, 8009, 8080, 8123, 9100,
    32400, 62078,
];

/// What gets scanned, and the adapter it's attached to (if any).
#[derive(Debug, Clone)]
pub struct Plan<'a> {
    pub net: Ipv4Net,
    /// `None` when the range isn't on a network this machine is plugged into.
    pub iface: Option<&'a Iface>,
}

fn too_big(net: Ipv4Net) -> String {
    let [a, b, c, _] = net.addr().octets();
    format!(
        "{} is too big to scan in one go. Pick a smaller range, for example \
         `hoppy scan {a}.{b}.{c}.0/24`.",
        net.trunc()
    )
}

/// Decide what to scan from the argument: a range (`10.0.0.0/24`), an adapter
/// name, or nothing (the network that carries internet traffic).
pub fn plan<'a>(target: Option<&str>, live: &'a [Iface]) -> Result<Plan<'a>, String> {
    let owner = |net: &Ipv4Net| {
        live.iter()
            .find(|iface| iface.ipv4.iter().any(|own| own.contains(&net.network())))
    };
    let of_iface = |iface: &'a Iface| {
        iface
            .real_ipv4()
            .or(iface.ipv4.first())
            .map(|net| Plan {
                net: *net,
                iface: Some(iface),
            })
            .ok_or_else(|| format!("{} has no IPv4 address to scan from.", iface.name))
    };

    let plan = match target {
        Some(text) => match text.parse::<Ipv4Net>() {
            Ok(net) => Plan {
                net,
                iface: owner(&net),
            },
            Err(_) if text.contains('/') || text.parse::<Ipv4Addr>().is_ok() => {
                return Err(format!(
                    "\"{text}\" isn't a range. Use the form 192.168.1.0/24, or an adapter name."
                ));
            }
            Err(_) => {
                let iface = live
                    .iter()
                    .find(|iface| iface.name.eq_ignore_ascii_case(text))
                    .ok_or_else(|| {
                        let names: Vec<&str> = live.iter().map(|i| i.name.as_str()).collect();
                        format!(
                            "no active adapter is called \"{text}\". Yours are: {}.",
                            names.join(", ")
                        )
                    })?;
                of_iface(iface)?
            }
        },
        None => {
            // With a VPN up, internet traffic goes through the tunnel, but
            // the devices worth finding are on the Wi-Fi or cable under it.
            let physical = |i: &&Iface| matches!(i.kind, Kind::WiFi | Kind::Ethernet);
            let iface = live
                .iter()
                .find(|i| i.is_default && i.kind != Kind::Vpn)
                .or_else(|| live.iter().find(physical))
                .or(live.first())
                .ok_or("no active network to scan. Join Wi-Fi or plug in a cable.")?;
            of_iface(iface)?
        }
    };

    if plan.net.prefix_len() < MIN_PREFIX {
        return Err(too_big(plan.net));
    }
    Ok(plan)
}

/// Addresses worth trying: everything in the range except the network and
/// broadcast addresses.
fn hosts(net: Ipv4Net) -> Vec<Ipv4Addr> {
    net.hosts().collect()
}

/// Make the OS look for every host, then give the answers time to arrive.
fn nudge(source: Ipv4Addr, hosts: &[Ipv4Addr]) {
    let Ok(socket) = UdpSocket::bind((source, 0)) else {
        return;
    };
    for _ in 0..NUDGE_ROUNDS {
        for host in hosts {
            // Errors are expected here (some systems report "host down" on
            // the second try) and don't matter.
            let _ = socket.send_to(&[0], (*host, NUDGE_PORT));
        }
        thread::sleep(NUDGE_PAUSE);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Read from the OS neighbor table: finds everything that's switched on.
    Neighbors,
    /// Connection attempts only: misses devices with no common port open.
    Sweep,
}

/// Keep the neighbor-table entries that belong to the scanned range.
pub fn in_range(
    entries: Vec<(Ipv4Addr, [u8; 6])>,
    net: Ipv4Net,
) -> BTreeMap<Ipv4Addr, Option<[u8; 6]>> {
    entries
        .into_iter()
        .filter(|(ip, _)| hosts(net).contains(ip))
        .map(|(ip, mac)| (ip, Some(mac)))
        .collect()
}

fn discover(plan: &Plan) -> (BTreeMap<Ipv4Addr, Option<[u8; 6]>>, Method) {
    let all = hosts(plan.net);

    if let Some(iface) = plan.iface {
        let source = iface
            .ipv4
            .iter()
            .find(|own| plan.net.contains(&own.addr()))
            .map_or(Ipv4Addr::UNSPECIFIED, Ipv4Net::addr);
        nudge(source, &all);
        if let Some(table) = neighbors::read_table() {
            let mut found = in_range(neighbors::parse(&table), plan.net);
            // An empty table means this adapter doesn't do neighbor lookups
            // at all (VPN tunnels don't), so fall through to the sweep.
            if !found.is_empty() {
                // This machine isn't in its own neighbor table.
                if !source.is_unspecified() {
                    found.insert(source, iface.mac);
                }
                return (found, Method::Neighbors);
            }
        }
    }

    let pairs: Vec<(Ipv4Addr, u16)> = all
        .iter()
        .flat_map(|ip| SWEEP_PORTS.map(|port| (*ip, port)))
        .collect();
    let alive = pool::map(pairs, WORKERS, |(ip, port)| {
        let addr = SocketAddr::new(IpAddr::V4(ip), port);
        tcp_probe(addr, SWEEP_TIMEOUT).alive().map(|_| ip)
    });
    let found = alive.into_iter().flatten().map(|ip| (ip, None)).collect();
    (found, Method::Sweep)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    ThisMachine,
    Router,
    Other,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub ip: Ipv4Addr,
    pub mac: Option<[u8; 6]>,
    pub name: Option<String>,
    pub open: Vec<u16>,
    pub role: Role,
}

/// A plain-English guess at what a device is, from the ports it has open.
/// The most specific clue wins.
pub fn looks_like(role: Role, open: &[u16]) -> &'static str {
    let has = |port: u16| open.contains(&port);
    match role {
        Role::ThisMachine => return "this machine",
        Role::Router => return "router",
        Role::Other => {}
    }
    if has(62078) {
        "iPhone or iPad"
    } else if has(9100) || has(631) {
        "printer"
    } else if has(8009) {
        "Chromecast or Google TV"
    } else if has(32400) {
        "Plex server"
    } else if has(8123) {
        "Home Assistant"
    } else if has(554) {
        "camera or recorder"
    } else if has(7000) {
        "AirPlay device (Apple TV, Mac, speaker)"
    } else if has(3389) {
        "Windows PC"
    } else if has(548) {
        "Mac or NAS"
    } else if has(445) {
        "PC or file server"
    } else if has(1883) {
        "smart-home hub"
    } else if has(22) {
        "computer (ssh)"
    } else if has(53) {
        "DNS server"
    } else if has(80) || has(443) || has(8080) {
        "has a web page"
    } else {
        ""
    }
}

/// Ask every device for its name and check its telltale ports, both at once.
fn inspect(found: BTreeMap<Ipv4Addr, Option<[u8; 6]>>, plan: &Plan, dns: &[IpAddr]) -> Vec<Device> {
    let ips: Vec<Ipv4Addr> = found.keys().copied().collect();
    let pairs: Vec<(Ipv4Addr, u16)> = ips
        .iter()
        .flat_map(|ip| TELLTALE_PORTS.map(|port| (*ip, port)))
        .collect();

    let (names, open) = thread::scope(|scope| {
        let names =
            scope.spawn(|| pool::map(ips.clone(), NAME_WORKERS, |ip| names::device_name(ip, dns)));
        let open = pool::map(pairs, WORKERS, |(ip, port)| {
            let addr = SocketAddr::new(IpAddr::V4(ip), port);
            matches!(tcp_probe(addr, PORT_TIMEOUT), Probe::Open(_)).then_some((ip, port))
        });
        (names.join().unwrap_or_default(), open)
    });

    let own: Vec<Ipv4Addr> = plan
        .iface
        .map(|iface| iface.ipv4.iter().map(Ipv4Net::addr).collect())
        .unwrap_or_default();
    let gateway = plan.iface.and_then(|iface| iface.gateway);

    found
        .into_iter()
        .enumerate()
        .map(|(index, (ip, mac))| Device {
            ip,
            mac,
            name: names.get(index).cloned().flatten(),
            open: open
                .iter()
                .flatten()
                .filter(|(owner, _)| *owner == ip)
                .map(|(_, port)| *port)
                .collect(),
            role: if own.contains(&ip) {
                Role::ThisMachine
            } else if gateway == Some(ip) {
                Role::Router
            } else {
                Role::Other
            },
        })
        .collect()
}

fn device_json(device: &Device) -> serde_json::Value {
    json!({
        "ip": device.ip.to_string(),
        "name": device.name,
        "mac": device.mac.map(oui::format_mac),
        "vendor": device.mac.and_then(oui::vendor),
        "role": match device.role {
            Role::ThisMachine => "this-machine",
            Role::Router => "router",
            Role::Other => "device",
        },
        "open_ports": device.open,
        "looks_like": Some(looks_like(device.role, &device.open)).filter(|s| !s.is_empty()),
    })
}

fn device_row(device: &Device) -> Vec<String> {
    let missing = || dim("-");
    let services: Vec<&str> = device
        .open
        .iter()
        .map(|port| port_label(*port).unwrap_or("?"))
        .collect();
    let guess = looks_like(device.role, &device.open);
    let guess = match device.role {
        Role::ThisMachine | Role::Router => green(guess),
        Role::Other => guess.to_string(),
    };
    vec![
        bold(&device.ip.to_string()),
        device.name.as_deref().map_or_else(missing, cyan),
        device
            .mac
            .map_or_else(missing, |mac| dim(&oui::format_mac(mac))),
        device
            .mac
            .and_then(oui::vendor)
            .map_or_else(missing, str::to_string),
        guess,
        dim(&services.join(", ")),
    ]
}

/// `hoppy scan [range-or-adapter]`
pub fn run(target: Option<&str>) -> Result<bool, String> {
    let live: Vec<Iface> = net::load_interfaces()
        .into_iter()
        .filter(Iface::is_interesting)
        .collect();
    let plan = plan(target, &live)?;
    let range = plan.net.trunc();
    let on = plan
        .iface
        .map_or_else(String::new, |iface| format!(" on {}", iface.name));

    if !json_on() {
        say("");
        say(&dim(&format!(
            "  Scanning {range}{on} ({} addresses)...",
            hosts(plan.net).len()
        )));
    }

    let started = Instant::now();
    let (found, method) = discover(&plan);
    let devices = inspect(found, &plan, &net::queryable_dns(&live));
    let took = started.elapsed();

    if json_on() {
        emit(&json!({
            "network": range.to_string(),
            "interface": plan.iface.map(|iface| iface.name.as_str()),
            "method": match method {
                Method::Neighbors => "neighbors",
                Method::Sweep => "sweep",
            },
            "seconds": (took.as_secs_f64() * 10.0).round() / 10.0,
            "devices": devices.iter().map(device_json).collect::<Vec<_>>(),
        }));
        return Ok(true);
    }

    say("");
    if devices.is_empty() {
        say("  Nothing answered.");
    } else {
        let header = ["IP", "NAME", "MAC", "MADE BY", "LOOKS LIKE", ""];
        let mut rows = vec![header.iter().map(|h| dim(h)).collect()];
        rows.extend(devices.iter().map(device_row));
        for line in table(&rows) {
            say(&format!("  {line}"));
        }
    }
    say("");
    say(&dim(&format!(
        "  {} found on {range} in {:.1} s",
        match devices.len() {
            1 => "1 device".to_string(),
            n => format!("{n} devices"),
        },
        took.as_secs_f64()
    )));
    if method == Method::Sweep {
        say(&dim(
            "  The neighbor table had nothing for this range (it's routed, or behind a VPN), \
             so only devices with a common port open show up.",
        ));
    } else if plan
        .iface
        .is_some_and(|iface| iface.ipv4.iter().all(|own| is_self_assigned(own.addr())))
    {
        say(&dim(
            "  This is a self-assigned (169.254) network: devices found here have no DHCP \
             server either.",
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::tests::iface;

    fn home() -> Vec<Iface> {
        let mut wifi = iface("Wi-Fi", &["192.168.1.42/24"], Some("192.168.1.1"));
        wifi.is_default = true;
        let mut docker = iface("docker0", &["172.17.0.1/16"], None);
        docker.kind = Kind::Docker;
        vec![docker, iface("Dante", &["10.10.0.5/24"], None), wifi]
    }

    #[test]
    fn default_plan_scans_the_internet_network() {
        let live = home();
        let plan = plan(None, &live).unwrap();
        assert_eq!(plan.net.trunc().to_string(), "192.168.1.0/24");
        assert_eq!(plan.iface.unwrap().name, "Wi-Fi");
    }

    #[test]
    fn plan_accepts_an_adapter_name_in_any_case() {
        let live = home();
        let plan = plan(Some("dante"), &live).unwrap();
        assert_eq!(plan.net.trunc().to_string(), "10.10.0.0/24");
    }

    #[test]
    fn plan_accepts_a_range_and_finds_its_adapter() {
        let live = home();
        let on_link = plan(Some("10.10.0.0/25"), &live).unwrap();
        assert_eq!(on_link.iface.unwrap().name, "Dante");
        let elsewhere = plan(Some("10.99.0.0/24"), &live).unwrap();
        assert!(elsewhere.iface.is_none());
    }

    #[test]
    fn plan_refuses_huge_ranges_with_a_suggestion() {
        let live = home();
        let err = plan(Some("docker0"), &live).unwrap_err();
        assert!(err.contains("too big"), "{err}");
        assert!(err.contains("hoppy scan 172.17.0.0/24"), "{err}");
        assert!(plan(Some("10.0.0.0/8"), &live).is_err());
        assert!(plan(Some("10.0.0.0/22"), &live).is_ok());
    }

    #[test]
    fn plan_explains_bad_arguments() {
        let live = home();
        assert!(
            plan(Some("192.168.1.5"), &live)
                .unwrap_err()
                .contains("isn't a range")
        );
        assert!(
            plan(Some("10.0.0.0/99"), &live)
                .unwrap_err()
                .contains("isn't a range")
        );
        let unknown = plan(Some("eth9"), &live).unwrap_err();
        assert!(unknown.contains("Yours are: docker0, Dante, Wi-Fi"));
        assert!(plan(None, &[]).unwrap_err().contains("no active network"));
    }

    #[test]
    fn hosts_skip_network_and_broadcast() {
        let all = hosts("192.168.1.42/24".parse().unwrap());
        assert_eq!(all.len(), 254);
        assert_eq!(all[0].to_string(), "192.168.1.1");
        assert_eq!(all[253].to_string(), "192.168.1.254");
    }

    #[test]
    fn neighbor_entries_outside_the_range_are_dropped() {
        let mac = [0xb8, 0x27, 0xeb, 1, 2, 3];
        let entries = vec![
            ("192.168.1.50".parse().unwrap(), mac),
            ("192.168.1.255".parse().unwrap(), mac),
            ("10.0.0.9".parse().unwrap(), mac),
            ("192.168.1.7".parse().unwrap(), mac),
        ];
        let kept = in_range(entries, "192.168.1.42/24".parse().unwrap());
        let ips: Vec<String> = kept.keys().map(Ipv4Addr::to_string).collect();
        assert_eq!(ips, ["192.168.1.7", "192.168.1.50"]);
    }

    #[test]
    fn tiny_ranges_keep_all_their_addresses() {
        let mac = [0xb8, 0x27, 0xeb, 1, 2, 3];
        let entries = vec![
            ("10.0.0.0".parse().unwrap(), mac),
            ("10.0.0.1".parse().unwrap(), mac),
        ];
        assert_eq!(in_range(entries, "10.0.0.0/31".parse().unwrap()).len(), 2);
    }

    #[test]
    fn default_plan_looks_under_a_vpn() {
        let mut vpn = iface("utun3", &["10.8.0.2/24"], Some("10.8.0.1"));
        vpn.kind = Kind::Vpn;
        vpn.is_default = true;
        let live = [vpn, iface("en0", &["192.168.1.42/24"], Some("192.168.1.1"))];
        assert_eq!(plan(None, &live).unwrap().iface.unwrap().name, "en0");
    }

    #[test]
    fn guesses_use_the_most_specific_port() {
        assert_eq!(looks_like(Role::Other, &[80, 443, 62078]), "iPhone or iPad");
        assert_eq!(looks_like(Role::Other, &[80, 631, 9100]), "printer");
        assert_eq!(looks_like(Role::Other, &[22, 445]), "PC or file server");
        assert_eq!(looks_like(Role::Other, &[22]), "computer (ssh)");
        assert_eq!(looks_like(Role::Other, &[80]), "has a web page");
        assert_eq!(looks_like(Role::Other, &[]), "");
    }

    #[test]
    fn role_beats_ports() {
        assert_eq!(looks_like(Role::Router, &[53, 80]), "router");
        assert_eq!(looks_like(Role::ThisMachine, &[22]), "this machine");
    }

    #[test]
    fn device_json_has_stable_shape() {
        let device = Device {
            ip: "192.168.1.50".parse().unwrap(),
            mac: Some([0xb8, 0x27, 0xeb, 1, 2, 3]),
            name: Some("pi.local".to_string()),
            open: vec![22],
            role: Role::Other,
        };
        let value = device_json(&device);
        assert_eq!(value["ip"], "192.168.1.50");
        assert_eq!(value["mac"], "b8:27:eb:01:02:03");
        assert_eq!(value["vendor"], "Raspberry Pi");
        assert_eq!(value["role"], "device");
        assert_eq!(value["open_ports"][0], 22);
        assert_eq!(value["looks_like"], "computer (ssh)");
    }
}
