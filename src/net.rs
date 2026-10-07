//! Interface kinds, warnings, and port names. No printing here.

use netdev::interface::types::InterfaceType;
use netdev::ipnet::Ipv4Net;
use std::net::{IpAddr, Ipv4Addr};

/// What an interface is, in words a person would use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    WiFi,
    Ethernet,
    Vpn,
    Docker,
    Vm,
    Bridge,
    AirDrop,
    Cellular,
    Bluetooth,
    Loopback,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::WiFi => "Wi-Fi",
            Kind::Ethernet => "Ethernet",
            Kind::Vpn => "VPN",
            Kind::Docker => "Docker",
            Kind::Vm => "VM",
            Kind::Bridge => "Bridge",
            Kind::AirDrop => "AirDrop",
            Kind::Cellular => "Cellular",
            Kind::Bluetooth => "Bluetooth",
            Kind::Loopback => "Loopback",
            Kind::Other => "Other",
        }
    }
}

/// The slice of an interface hoppy cares about, detached from the OS so the
/// logic below can be tested with made-up interfaces.
#[derive(Debug, Clone)]
pub struct Iface {
    /// The name a person would recognise: `Wi-Fi` on Windows, `en0`/`eth0`
    /// elsewhere.
    pub name: String,
    pub kind: Kind,
    pub ipv4: Vec<Ipv4Net>,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<IpAddr>,
    /// True for the interface that carries the default route.
    pub is_default: bool,
    pub up: bool,
    pub loopback: bool,
}

impl Iface {
    /// Worth showing by default: up, not loopback, and has an IPv4 address.
    pub fn is_interesting(&self) -> bool {
        self.up && !self.loopback && !self.ipv4.is_empty()
    }
}

/// Read every interface from the OS, most important first: the default-route
/// interface, then others with a gateway, then the rest by name.
pub fn load_interfaces() -> Vec<Iface> {
    let mut ifaces: Vec<Iface> = netdev::get_interfaces().iter().map(from_netdev).collect();
    ifaces.sort_by(|a, b| {
        let rank = |i: &Iface| (!i.is_default, i.gateway.is_none(), !i.up);
        rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name))
    });
    ifaces
}

fn from_netdev(raw: &netdev::Interface) -> Iface {
    use netdev::interface::state::OperState;

    let friendly = raw.friendly_name.as_deref().unwrap_or("");
    let description = raw.description.as_deref().unwrap_or("");
    // On Windows the "name" is a GUID; the friendly name is what people see.
    let name = if cfg!(windows) && !friendly.is_empty() {
        friendly.to_string()
    } else {
        raw.name.clone()
    };
    // "Up" has two layers: the OS has the interface enabled, and the link is
    // actually carrying signal (cable plugged in, Wi-Fi joined).
    let link_dead = matches!(
        raw.oper_state,
        OperState::Down | OperState::LowerLayerDown | OperState::NotPresent
    );

    Iface {
        name,
        kind: interface_kind(
            &raw.name,
            friendly,
            description,
            raw.if_type,
            raw.is_loopback(),
        ),
        ipv4: raw.ipv4.clone(),
        gateway: raw.gateway.as_ref().and_then(|gw| gw.ipv4.first().copied()),
        dns: raw.dns_servers.clone(),
        is_default: raw.default,
        up: raw.is_up() && !link_dead,
        loopback: raw.is_loopback(),
    }
}

/// Name an interface in plain English. Virtual adapters usually claim to be
/// Ethernet, so names are checked before the hardware type.
pub fn interface_kind(
    name: &str,
    friendly_name: &str,
    description: &str,
    if_type: InterfaceType,
    loopback: bool,
) -> Kind {
    if loopback || if_type == InterfaceType::Loopback {
        return Kind::Loopback;
    }

    let name = name.to_lowercase();
    let text = format!("{name} {friendly_name} {description}").to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| name.starts_with(p));

    // Checked before Docker because Windows' "vEthernet" starts with "veth".
    if has(&[
        "vethernet",
        "hyper-v",
        "vmware",
        "virtualbox",
        "vmnet",
        "vboxnet",
        "virbr",
        "wsl",
        "parallels",
    ]) || starts(&["vnet"])
    {
        return Kind::Vm;
    }
    // Container bridges and veth pairs: Docker, Podman, LXD, Kubernetes.
    if has(&["docker"])
        || starts(&[
            "br-", "veth", "podman", "cni", "flannel", "cali", "lxdbr", "kube",
        ])
    {
        return Kind::Docker;
    }
    // awdl = Apple Wireless Direct Link (AirDrop/AirPlay); llw is its
    // low-latency sibling.
    if starts(&["awdl", "llw"]) {
        return Kind::AirDrop;
    }
    if has(&[
        "vpn",
        "wireguard",
        "tailscale",
        "zerotier",
        "wintun",
        "tap-windows",
        "openvpn",
        "nordlynx",
        "anyconnect",
    ]) || starts(&["utun", "tun", "tap", "wg", "ppp", "ipsec", "zt"])
    {
        return Kind::Vpn;
    }
    if has(&["bluetooth"]) {
        return Kind::Bluetooth;
    }
    if if_type == InterfaceType::Bridge || starts(&["bridge", "br"]) {
        return Kind::Bridge;
    }

    match if_type {
        InterfaceType::Wireless80211 | InterfaceType::PeerToPeerWireless => Kind::WiFi,
        InterfaceType::Wwan | InterfaceType::Wwanpp | InterfaceType::Wwanpp2 => Kind::Cellular,
        InterfaceType::Tunnel | InterfaceType::Ppp => Kind::Vpn,
        InterfaceType::Ethernet
        | InterfaceType::Ethernet3Megabit
        | InterfaceType::FastEthernetT
        | InterfaceType::FastEthernetFx
        | InterfaceType::GigabitEthernet => {
            // Linux reports Wi-Fi cards as Ethernet; the name gives it away.
            if starts(&["wl"]) || has(&["wi-fi", "wifi", "wireless"]) {
                Kind::WiFi
            } else {
                Kind::Ethernet
            }
        }
        _ => Kind::Other,
    }
}

/// Self-assigned (link-local) address: 169.254.x.x, picked by the device
/// itself when no DHCP server answers.
pub fn is_self_assigned(ip: Ipv4Addr) -> bool {
    ip.is_link_local()
}

/// Do two networks share any addresses? True when either one contains the
/// other's first address (`192.168.1.0/24` sits inside `192.168.0.0/16`).
pub fn subnets_overlap(a: &Ipv4Net, b: &Ipv4Net) -> bool {
    a.contains(&b.network()) || b.contains(&a.network())
}

/// Pairs of interfaces on the same subnet, where the OS may send traffic out
/// of either one. Self-assigned addresses are skipped.
pub fn subnet_overlaps(ifaces: &[Iface]) -> Vec<(String, String, Ipv4Net)> {
    let mut found = Vec::new();
    for (i, a) in ifaces.iter().enumerate() {
        for b in &ifaces[i + 1..] {
            let shared = a.ipv4.iter().find(|net_a| {
                !is_self_assigned(net_a.addr())
                    && b.ipv4.iter().any(|net_b| {
                        !is_self_assigned(net_b.addr()) && subnets_overlap(net_a, net_b)
                    })
            });
            if let Some(net) = shared {
                found.push((a.name.clone(), b.name.clone(), net.trunc()));
            }
        }
    }
    found
}

/// Plain-English warnings about the interfaces passed in. Callers should
/// pass only the live ones (see [`Iface::is_interesting`]).
pub fn warnings(ifaces: &[Iface]) -> Vec<String> {
    let mut out = Vec::new();

    for iface in ifaces {
        for net in iface.ipv4.iter().filter(|n| is_self_assigned(n.addr())) {
            out.push(format!(
                "{} has a self-assigned address ({}): nothing gave it an IP, check cable or DHCP \
                 (normal on a Dante/AV network with no DHCP).",
                iface.name,
                net.addr()
            ));
        }
    }

    for (a, b, net) in subnet_overlaps(ifaces) {
        out.push(format!(
            "{a} and {b} are both on {net}: traffic may leave the wrong port. \
             Unplug one or move it to a different subnet."
        ));
    }

    let with_gateway: Vec<&Iface> = ifaces.iter().filter(|i| i.gateway.is_some()).collect();
    if with_gateway.is_empty() {
        out.push(
            "No default gateway: this machine can reach its direct neighbors but not the \
             internet. Connect to a network with a router, or run `hoppy doctor`."
                .to_string(),
        );
    } else if with_gateway.len() > 1 {
        let names: Vec<&str> = with_gateway.iter().map(|i| i.name.as_str()).collect();
        let winner = match ifaces.iter().find(|i| i.is_default) {
            Some(i) => format!("Internet traffic uses {}.", i.name),
            None => "The OS picks one by metric; run `hoppy doctor` to test it.".to_string(),
        };
        out.push(format!(
            "{} gateways ({}). {winner}",
            with_gateway.len(),
            names.join(", ")
        ));
    }

    out
}

/// What a well-known port is usually used for.
pub fn port_label(port: u16) -> Option<&'static str> {
    let label = match port {
        22 => "ssh",
        25 => "smtp (mail)",
        53 => "dns",
        67 | 68 => "dhcp",
        80 => "http",
        123 => "ntp (time)",
        135 => "windows rpc",
        137..=139 => "netbios",
        319 | 320 => "PTP clock (Dante/AV sync)",
        443 => "https",
        445 => "smb (file sharing)",
        500 | 4500 => "ipsec vpn",
        631 => "printing (ipp)",
        1900 => "ssdp (device discovery)",
        3000 => "dev server",
        3306 => "mysql",
        3389 => "rdp (remote desktop)",
        4440 | 4444 | 4455 => "Dante audio control",
        5000 | 7000 => "airplay",
        5173 => "dev server (vite)",
        5353 => "mdns/bonjour",
        5355 => "llmnr (name lookup)",
        5432 => "postgres",
        5900 => "vnc",
        6379 => "redis",
        8000 | 8080 | 8888 => "dev server",
        8700..=8708 => "Dante control",
        27017 => "mongodb",
        51820 => "wireguard",
        _ => return None,
    };
    Some(label)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a fake live interface for tests.
    pub(crate) fn iface(name: &str, ipv4: &[&str], gateway: Option<&str>) -> Iface {
        Iface {
            name: name.to_string(),
            kind: Kind::Ethernet,
            ipv4: ipv4.iter().map(|s| s.parse().unwrap()).collect(),
            gateway: gateway.map(|g| g.parse().unwrap()),
            dns: Vec::new(),
            is_default: false,
            up: true,
            loopback: false,
        }
    }

    fn kind(name: &str, friendly: &str, desc: &str, ty: InterfaceType) -> Kind {
        interface_kind(name, friendly, desc, ty, false)
    }

    #[test]
    fn port_labels_cover_everyday_services() {
        assert_eq!(port_label(22), Some("ssh"));
        assert_eq!(port_label(53), Some("dns"));
        assert_eq!(port_label(80), Some("http"));
        assert_eq!(port_label(443), Some("https"));
        assert_eq!(port_label(5432), Some("postgres"));
        assert_eq!(port_label(6379), Some("redis"));
        assert_eq!(port_label(3306), Some("mysql"));
        assert_eq!(port_label(27017), Some("mongodb"));
        assert_eq!(port_label(5353), Some("mdns/bonjour"));
        assert_eq!(port_label(51820), Some("wireguard"));
        for dev in [3000, 5173, 8080] {
            assert!(port_label(dev).unwrap().contains("dev server"));
        }
    }

    #[test]
    fn port_labels_cover_dante_and_ptp() {
        assert!(port_label(319).unwrap().contains("PTP"));
        assert!(port_label(320).unwrap().contains("PTP"));
        for port in 8700..=8708 {
            assert_eq!(port_label(port), Some("Dante control"));
        }
        for port in [4440, 4444, 4455] {
            assert!(port_label(port).unwrap().contains("Dante"));
        }
        assert_eq!(port_label(8709), None);
    }

    #[test]
    fn unknown_ports_have_no_label() {
        assert_eq!(port_label(1), None);
        assert_eq!(port_label(49664), None);
    }

    #[test]
    fn kind_from_hardware_type() {
        assert_eq!(
            kind("en0", "Wi-Fi", "", InterfaceType::Wireless80211),
            Kind::WiFi
        );
        assert_eq!(
            kind("eth0", "", "", InterfaceType::Ethernet),
            Kind::Ethernet
        );
        assert_eq!(
            kind(
                "{GUID}",
                "Ethernet",
                "Realtek USB GbE",
                InterfaceType::Ethernet
            ),
            Kind::Ethernet
        );
        assert_eq!(kind("wwan0", "", "", InterfaceType::Wwan), Kind::Cellular);
        assert_eq!(
            kind("mystery0", "", "", InterfaceType::Unknown),
            Kind::Other
        );
    }

    #[test]
    fn kind_linux_wifi_reported_as_ethernet() {
        assert_eq!(kind("wlan0", "", "", InterfaceType::Ethernet), Kind::WiFi);
        assert_eq!(kind("wlp3s0", "", "", InterfaceType::Ethernet), Kind::WiFi);
    }

    #[test]
    fn kind_virtual_adapters_by_name() {
        assert_eq!(
            kind("docker0", "", "", InterfaceType::Ethernet),
            Kind::Docker
        );
        assert_eq!(
            kind("br-3f2a9c1d", "", "", InterfaceType::Ethernet),
            Kind::Docker
        );
        assert_eq!(
            kind("veth12ab", "", "", InterfaceType::Ethernet),
            Kind::Docker
        );
        assert_eq!(
            kind("podman0", "", "", InterfaceType::Ethernet),
            Kind::Docker
        );
        assert_eq!(kind("cni0", "", "", InterfaceType::Ethernet), Kind::Docker);
        assert_eq!(
            kind("flannel.1", "", "", InterfaceType::Ethernet),
            Kind::Docker
        );
        assert_eq!(kind("vnet0", "", "", InterfaceType::Ethernet), Kind::Vm);
        assert_eq!(kind("utun3", "", "", InterfaceType::Unknown), Kind::Vpn);
        assert_eq!(kind("wg0", "", "", InterfaceType::Unknown), Kind::Vpn);
        assert_eq!(
            kind("tailscale0", "", "", InterfaceType::Unknown),
            Kind::Vpn
        );
        assert_eq!(
            kind("awdl0", "", "", InterfaceType::Ethernet),
            Kind::AirDrop
        );
        assert_eq!(
            kind("bridge0", "", "", InterfaceType::Ethernet),
            Kind::Bridge
        );
        assert_eq!(kind("vmnet8", "", "", InterfaceType::Ethernet), Kind::Vm);
        assert_eq!(kind("virbr0", "", "", InterfaceType::Ethernet), Kind::Vm);
    }

    #[test]
    fn kind_windows_virtual_adapters_by_description() {
        // "vEthernet" starts with "veth" but is Hyper-V, not Docker.
        assert_eq!(
            kind(
                "{GUID}",
                "vEthernet (WSL (Hyper-V firewall))",
                "Hyper-V Virtual Ethernet Adapter",
                InterfaceType::Ethernet
            ),
            Kind::Vm
        );
        assert_eq!(
            kind(
                "{GUID}",
                "Local Area Connection",
                "TAP-Windows Adapter V9",
                InterfaceType::Ethernet
            ),
            Kind::Vpn
        );
        assert_eq!(
            kind(
                "{GUID}",
                "Bluetooth Network Connection",
                "Bluetooth Device (Personal Area Network)",
                InterfaceType::Ethernet
            ),
            Kind::Bluetooth
        );
    }

    #[test]
    fn kind_loopback_wins() {
        assert_eq!(
            interface_kind("lo", "", "", InterfaceType::Ethernet, true),
            Kind::Loopback
        );
        assert_eq!(kind("lo0", "", "", InterfaceType::Loopback), Kind::Loopback);
    }

    #[test]
    fn self_assigned_is_169_254_only() {
        assert!(is_self_assigned("169.254.10.20".parse().unwrap()));
        assert!(is_self_assigned("169.254.255.255".parse().unwrap()));
        assert!(!is_self_assigned("169.253.0.1".parse().unwrap()));
        assert!(!is_self_assigned("192.168.1.10".parse().unwrap()));
        assert!(!is_self_assigned("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn overlap_same_and_nested_subnets() {
        let net = |s: &str| s.parse::<Ipv4Net>().unwrap();
        assert!(subnets_overlap(
            &net("192.168.1.10/24"),
            &net("192.168.1.99/24")
        ));
        assert!(subnets_overlap(
            &net("192.168.1.10/24"),
            &net("192.168.0.1/16")
        ));
        assert!(subnets_overlap(&net("10.0.0.1/8"), &net("10.20.30.40/24")));
        assert!(!subnets_overlap(
            &net("192.168.1.10/24"),
            &net("192.168.2.10/24")
        ));
        assert!(!subnets_overlap(&net("10.0.0.1/24"), &net("172.16.0.1/12")));
    }

    #[test]
    fn overlap_detected_between_interfaces() {
        let ifaces = [
            iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1")),
            iface("Ethernet", &["192.168.1.21/24"], Some("192.168.1.1")),
            iface("Dante", &["10.10.0.5/24"], None),
        ];
        let overlaps = subnet_overlaps(&ifaces);
        assert_eq!(overlaps.len(), 1);
        assert_eq!(overlaps[0].0, "Wi-Fi");
        assert_eq!(overlaps[0].1, "Ethernet");
        assert_eq!(overlaps[0].2.to_string(), "192.168.1.0/24");
    }

    #[test]
    fn overlap_ignores_same_interface_and_self_assigned() {
        let two_ips_one_port = [iface("eth0", &["10.0.0.5/24", "10.0.0.6/24"], None)];
        assert!(subnet_overlaps(&two_ips_one_port).is_empty());

        let two_dante_ports = [
            iface("Dante A", &["169.254.1.5/16"], None),
            iface("Dante B", &["169.254.9.9/16"], None),
        ];
        assert!(subnet_overlaps(&two_dante_ports).is_empty());
    }

    #[test]
    fn warns_about_self_assigned_address() {
        let ifaces = [
            iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1")),
            iface("Dante", &["169.254.44.7/16"], None),
        ];
        let warnings = warnings(&ifaces);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Dante"));
        assert!(warnings[0].contains("169.254.44.7"));
        assert!(warnings[0].contains("check cable or DHCP"));
    }

    #[test]
    fn warns_when_no_gateway() {
        let ifaces = [iface("eth0", &["10.0.0.5/24"], None)];
        let warnings = warnings(&ifaces);
        assert!(warnings.iter().any(|w| w.contains("No default gateway")));
    }

    #[test]
    fn warns_about_multiple_gateways_and_names_the_winner() {
        let mut starlink = iface("Starlink A", &["192.168.1.20/24"], Some("192.168.1.1"));
        starlink.is_default = true;
        let ifaces = [
            starlink,
            iface("Starlink B", &["192.168.100.20/24"], Some("192.168.100.1")),
        ];
        let warnings = warnings(&ifaces);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("2 gateways"));
        assert!(warnings[0].contains("Internet traffic uses Starlink A"));
    }

    #[test]
    fn healthy_single_network_has_no_warnings() {
        let mut wifi = iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1"));
        wifi.is_default = true;
        assert!(warnings(&[wifi]).is_empty());
    }
}
