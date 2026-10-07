//! `hoppy doctor`: walk you -> router -> internet -> dns -> target.
//!
//! Uses TCP connects instead of ICMP ping, so no root is needed. A refused
//! connection still proves the host is alive.

use crate::net::{self, Iface, Kind, is_self_assigned};
use crate::style::{bold, dim, emit, green, json_on, red, say, yellow};
use serde_json::json;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Ports a home/office router usually answers on: DNS, and its admin page.
pub(crate) const ROUTER_PORTS: [u16; 3] = [53, 80, 443];
/// Windows retries a refused connect for about a second before reporting it,
/// so the timeout has to be longer than that.
const ROUTER_TIMEOUT: Duration = Duration::from_millis(1500);
/// Public servers that are effectively always up, addressed by raw IP so this
/// step doesn't depend on DNS: Cloudflare, then Google.
const INTERNET_PROBES: [Ipv4Addr; 2] = [Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(8, 8, 8, 8)];
const INTERNET_TIMEOUT: Duration = Duration::from_secs(2);
const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const TARGET_TIMEOUT: Duration = Duration::from_secs(3);
/// How many of a name's addresses to try before giving up.
const MAX_TARGET_ADDRS: usize = 4;
/// Name to resolve when no target (or a bare IP) is given.
const DEFAULT_DNS_NAME: &str = "example.com";
const DEFAULT_PORT: u16 = 443;

/// Where the user wants to get to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
}

impl Target {
    pub(crate) fn label(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    pub(crate) fn ip(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
}

/// Accept a host, `host:port`, URL, or bare IP (v4 or v6) and work out what
/// to connect to. With no port given, the URL scheme decides, else 443.
pub fn parse_target(input: &str) -> Result<Target, String> {
    let trimmed = input.trim();
    let bad = |why: &str| {
        format!(
            "can't understand target \"{trimmed}\": {why}. \
             Try a host (example.com), host:port (nas.local:445), a URL, or an IP."
        )
    };

    let (scheme, rest) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, trimmed),
    };
    let default_port = match scheme.as_deref() {
        Some("http" | "ws") => 80,
        Some("ssh" | "sftp") => 22,
        Some("ftp") => 21,
        _ => DEFAULT_PORT,
    };

    // Keep only the host[:port] part of a URL: drop path, query, credentials.
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);

    // `fe80::1%eth0` pins a link-local IPv6 address to one interface (a
    // "zone"). Rust's address parser doesn't take zones, so say so clearly.
    if authority.contains('%') {
        return Err(bad(
            "IPv6 zone IDs (the %eth0 part) aren't supported yet, use the device's IPv4 address",
        ));
    }

    let parse_port = |text: &str| match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(bad("the port must be a number from 1 to 65535")),
    };

    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        // IPv6 addresses contain colons, so URLs wrap them: [::1]:8080
        let (host, after) = inner
            .split_once(']')
            .ok_or_else(|| bad("missing the closing ]"))?;
        let port = match after.strip_prefix(':') {
            Some(text) => parse_port(text)?,
            None if after.is_empty() => default_port,
            None => return Err(bad("unexpected text after ]")),
        };
        (host, port)
    } else if authority.parse::<Ipv6Addr>().is_ok() {
        (authority, default_port)
    } else if let Some((host, port_text)) = authority.rsplit_once(':') {
        (host, parse_port(port_text)?)
    } else {
        (authority, default_port)
    };

    if host.is_empty() {
        return Err(bad("there's no host in it"));
    }
    if host.chars().any(char::is_whitespace) {
        return Err(bad("hosts can't contain spaces"));
    }
    Ok(Target {
        host: host.to_string(),
        port,
    })
}

/// Result of one TCP connect attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// Connected: something is listening.
    Open(Duration),
    /// Refused: host is alive, nothing on that port.
    Refused(Duration),
    /// No answer in time.
    Silent,
}

impl Probe {
    /// Latency if the host proved it's alive (open *or* refused).
    pub fn alive(self) -> Option<Duration> {
        match self {
            Probe::Open(t) | Probe::Refused(t) => Some(t),
            Probe::Silent => None,
        }
    }
}

/// Classify a connect result. "Connection refused" counts as alive.
pub fn classify(result: &io::Result<()>, elapsed: Duration) -> Probe {
    match result {
        Ok(()) => Probe::Open(elapsed),
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Probe::Refused(elapsed),
        Err(_) => Probe::Silent,
    }
}

pub(crate) fn tcp_probe(addr: SocketAddr, timeout: Duration) -> Probe {
    let start = Instant::now();
    let result = TcpStream::connect_timeout(&addr, timeout).map(drop);
    classify(&result, start.elapsed())
}

/// Probe several ports on one host at once and return the fastest sign of
/// life, so a silent host costs one timeout instead of one per port.
pub(crate) fn probe_any(ip: IpAddr, ports: &[u16], timeout: Duration) -> Option<Duration> {
    let (tx, rx) = mpsc::channel();
    for &port in ports {
        let tx = tx.clone();
        thread::spawn(move || {
            let _ = tx.send(tcp_probe(SocketAddr::new(ip, port), timeout));
        });
    }
    drop(tx);
    rx.iter().find_map(Probe::alive)
}

/// Resolve a name with a timeout. The OS resolver has none of its own.
pub(crate) fn resolve(
    host: &str,
    port: u16,
    timeout: Duration,
) -> Option<(Vec<SocketAddr>, Duration)> {
    let (tx, rx) = mpsc::channel();
    let host = host.to_string();
    let start = Instant::now();
    thread::spawn(move || {
        let found = (host.as_str(), port)
            .to_socket_addrs()
            .map(|addrs| addrs.collect::<Vec<_>>());
        let _ = tx.send(found);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(addrs)) if !addrs.is_empty() => Some((addrs, start.elapsed())),
        _ => None,
    }
}

/// State of the first link: does this machine have a usable address?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Ok,
    /// Only a 169.254.x.x address on the named interface.
    SelfAssigned(String),
    /// Has an address but no route to the internet.
    NoGateway(String),
    /// No interface has an address at all.
    Disconnected,
}

/// Work out the link state from the live interfaces, and which one carries
/// internet traffic.
pub fn check_link(live: &[Iface]) -> (Link, Option<&Iface>) {
    let real = |i: &&Iface| i.ipv4.iter().any(|n| !is_self_assigned(n.addr()));

    let default = live.iter().find(|i| i.is_default);
    if let Some(iface) = default {
        return if real(&iface) {
            (Link::Ok, Some(iface))
        } else {
            (Link::SelfAssigned(iface.name.clone()), Some(iface))
        };
    }
    // No default route found. Blame a physical adapter if there is one,
    // rather than a Docker bridge or VM switch that never had a gateway.
    let physical = |i: &&Iface| matches!(i.kind, Kind::WiFi | Kind::Ethernet | Kind::Cellular);
    let with_ip = || live.iter().filter(real);
    if let Some(iface) = with_ip().find(physical).or_else(|| with_ip().next()) {
        return (Link::NoGateway(iface.name.clone()), Some(iface));
    }
    match live.first() {
        Some(iface) => (Link::SelfAssigned(iface.name.clone()), Some(iface)),
        None => (Link::Disconnected, None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Router {
    Answered,
    /// Didn't respond to probes. Not proof it's down: many routers ignore them.
    Silent,
    /// There's no gateway to probe.
    Missing,
}

/// Everything doctor learned, for [`verdict`] to summarise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Findings {
    pub link: Link,
    pub router: Router,
    pub internet: bool,
    pub dns: bool,
    /// `None` when no target was given.
    pub target: Option<TargetState>,
}

/// What happened when connecting to the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetState {
    /// Connected: the service is there.
    Open,
    /// The host answered but nothing is listening on that port. The machine
    /// is alive; the service isn't.
    Refused,
    /// No answer at all.
    Down,
}

impl Findings {
    pub fn healthy(&self) -> bool {
        // The OS doesn't always report its default route. If the internet
        // answers anyway, the missing gateway clearly isn't a problem.
        let link_ok = match self.link {
            Link::Ok => true,
            Link::NoGateway(_) => self.internet,
            Link::SelfAssigned(_) | Link::Disconnected => false,
        };
        link_ok
            && self.internet
            && self.dns
            && matches!(self.target, None | Some(TargetState::Open))
    }
}

/// One sentence saying where the problem is, picking the earliest broken
/// link in the chain since everything after it fails as a consequence.
pub fn verdict(f: &Findings, target: Option<&str>) -> String {
    match &f.link {
        Link::Disconnected => {
            return "You're not connected to any network. Join Wi-Fi or plug in a cable.".into();
        }
        Link::SelfAssigned(name) => {
            return format!(
                "{name} gave itself a 169.254 address because nothing answered DHCP. \
                 Check the cable and that the router or DHCP server is on \
                 (normal on a Dante/AV network, but those have no internet)."
            );
        }
        Link::NoGateway(name) if !f.internet => {
            return format!(
                "{name} has an address but no gateway, so there's no way out to the \
                 internet. Connect to a network with a router."
            );
        }
        Link::NoGateway(_) | Link::Ok => {}
    }

    if !f.internet {
        return match f.router {
            Router::Answered => "Your router is up, but the internet isn't. The problem is \
                                 upstream: modem, Starlink, or your ISP."
                .into(),
            Router::Silent | Router::Missing => {
                "Can't reach the internet, and your router didn't answer either. \
                 Power-cycle the router, then check the modem or Starlink."
                    .into()
            }
        };
    }
    if !f.dns {
        return "Internet works but DNS doesn't. Try 1.1.1.1 as your DNS server.".into();
    }
    let name = target.unwrap_or("the target");
    match f.target {
        Some(TargetState::Down) => format!(
            "Your network is fine; the target itself is down or the port is wrong ({name})."
        ),
        Some(TargetState::Refused) => format!(
            "Your network is fine and the host is up, but nothing is listening at {name}. \
             Start the service or check the port."
        ),
        Some(TargetState::Open) => format!("Everything works, and {name} is reachable."),
        None => "Everything works. Your network is healthy.".into(),
    }
}

pub(crate) fn millis(t: Duration) -> String {
    match t.as_millis() {
        0 => "<1 ms".to_string(),
        ms => format!("{ms} ms"),
    }
}

/// Milliseconds to one decimal place, for JSON output.
pub(crate) fn ms(t: Duration) -> f64 {
    (t.as_secs_f64() * 10_000.0).round() / 10.0
}

/// How one step of the checkup went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mark {
    Ok,
    /// Inconclusive: worth showing, not worth failing over.
    Unsure,
    Fail,
}

impl Mark {
    fn symbol(self) -> String {
        match self {
            Mark::Ok => green("✓"),
            Mark::Unsure => yellow("?"),
            Mark::Fail => red("✗"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Mark::Ok => "ok",
            Mark::Unsure => "unsure",
            Mark::Fail => "fail",
        }
    }
}

/// Prints each step as it finishes and keeps a copy for `--json`.
#[derive(Default)]
struct Report {
    steps: Vec<serde_json::Value>,
}

impl Report {
    /// Record one step: mark, name, detail, and latency if there is one.
    fn step(&mut self, mark: Mark, name: &str, detail: &str, latency: Option<Duration>) {
        self.steps.push(json!({
            "name": name,
            "status": mark.key(),
            "detail": detail,
            "ms": latency.map(ms),
        }));
        if json_on() {
            return;
        }
        let name = crate::style::pad(&bold(name), 9);
        let latency = latency.map_or_else(String::new, |t| dim(&format!("  {}", millis(t))));
        say(&format!("  {} {name}  {detail}{latency}", mark.symbol()));
    }
}

fn check_router(report: &mut Report, gateway: Option<Ipv4Addr>) -> Router {
    let Some(gw) = gateway else {
        report.step(Mark::Fail, "router", "no gateway on this network", None);
        return Router::Missing;
    };
    match probe_any(IpAddr::V4(gw), &ROUTER_PORTS, ROUTER_TIMEOUT) {
        Some(t) => {
            report.step(Mark::Ok, "router", &format!("{gw} answered"), Some(t));
            Router::Answered
        }
        None => {
            report.step(
                Mark::Unsure,
                "router",
                &format!("{gw} didn't answer (some routers ignore probes), carrying on"),
                None,
            );
            Router::Silent
        }
    }
}

fn check_internet(report: &mut Report) -> bool {
    for ip in INTERNET_PROBES {
        let addr = SocketAddr::new(IpAddr::V4(ip), 443);
        if let Some(t) = tcp_probe(addr, INTERNET_TIMEOUT).alive() {
            report.step(Mark::Ok, "internet", &format!("reached {ip}"), Some(t));
            return true;
        }
    }
    report.step(
        Mark::Fail,
        "internet",
        "couldn't reach 1.1.1.1 or 8.8.8.8",
        None,
    );
    false
}

/// Resolve the target's name (or example.com when the target is an IP or
/// absent). Returns the addresses found, if any.
fn check_dns(report: &mut Report, target: Option<&Target>) -> Option<Vec<SocketAddr>> {
    let named = target.filter(|t| t.ip().is_none());
    let (name, port) = named.map_or((DEFAULT_DNS_NAME, DEFAULT_PORT), |t| (&t.host, t.port));

    match resolve(name, port, DNS_TIMEOUT) {
        Some((addrs, t)) => {
            let first = addrs[0].ip();
            report.step(Mark::Ok, "dns", &format!("{name} is {first}"), Some(t));
            Some(addrs)
        }
        None => {
            report.step(Mark::Fail, "dns", &format!("couldn't look up {name}"), None);
            None
        }
    }
}

/// Connect to the target. `resolved` holds DNS results when the target is a
/// name; a bare IP needs no lookup.
fn check_target(
    report: &mut Report,
    target: &Target,
    resolved: Option<&[SocketAddr]>,
) -> TargetState {
    let label = target.label();
    let candidates: Vec<SocketAddr> = match target.ip() {
        Some(ip) => vec![SocketAddr::new(ip, target.port)],
        None => resolved.map(<[SocketAddr]>::to_vec).unwrap_or_default(),
    };
    if candidates.is_empty() {
        report.step(
            Mark::Fail,
            "target",
            &format!("{label} has no address to try"),
            None,
        );
        return TargetState::Down;
    }

    // Try IPv4 first: plenty of networks hand out IPv6 addresses that go
    // nowhere, and a dead IPv6 route shouldn't fail a working target.
    let mut ordered = candidates;
    ordered.sort_by_key(|a| !a.is_ipv4());
    let mut refused = false;
    for addr in ordered.iter().take(MAX_TARGET_ADDRS) {
        match tcp_probe(*addr, TARGET_TIMEOUT) {
            Probe::Open(t) => {
                report.step(Mark::Ok, "target", &format!("{label} is open"), Some(t));
                return TargetState::Open;
            }
            // One address refusing doesn't settle it: a name can map to
            // several addresses and another may be open.
            Probe::Refused(_) => refused = true,
            Probe::Silent => {}
        }
    }
    if refused {
        report.step(
            Mark::Fail,
            "target",
            &format!(
                "{} is up, but nothing is listening on port {}",
                target.host, target.port
            ),
            None,
        );
        return TargetState::Refused;
    }
    report.step(
        Mark::Fail,
        "target",
        &format!("{label} didn't answer"),
        None,
    );
    TargetState::Down
}

/// Run the checkup. Returns whether everything was healthy.
pub fn run(target: Option<&str>) -> Result<bool, String> {
    let target = target.map(parse_target).transpose()?;

    let all = net::load_interfaces();
    let live: Vec<Iface> = all.into_iter().filter(Iface::is_interesting).collect();
    let (link, iface) = check_link(&live);
    let mut report = Report::default();

    if !json_on() {
        say("");
    }
    match (&link, iface) {
        (Link::Ok, Some(i)) => {
            let ip = i
                .real_ipv4()
                .map_or_else(String::new, |n| n.addr().to_string());
            report.step(Mark::Ok, "connected", &format!("{} has {ip}", i.name), None);
        }
        (Link::NoGateway(name), _) => report.step(
            Mark::Unsure,
            "connected",
            &format!("{name} has an address but no gateway"),
            None,
        ),
        (Link::SelfAssigned(name), _) => report.step(
            Mark::Fail,
            "connected",
            &format!("{name} only has a self-assigned 169.254 address"),
            None,
        ),
        _ => report.step(Mark::Fail, "connected", "no network connection", None),
    }

    let mut findings = Findings {
        link,
        router: Router::Missing,
        internet: false,
        dns: false,
        target: None,
    };

    // With no usable address nothing further can work, so stop at the cause.
    let usable = matches!(findings.link, Link::Ok | Link::NoGateway(_));
    if usable {
        findings.router = check_router(&mut report, iface.and_then(|i| i.gateway));
        findings.internet = check_internet(&mut report);
        let resolved = check_dns(&mut report, target.as_ref());
        findings.dns = resolved.is_some();
        if let Some(t) = &target {
            findings.target = Some(check_target(&mut report, t, resolved.as_deref()));
        }
    }

    let healthy = findings.healthy();
    let label = target.as_ref().map(Target::label);
    let sentence = verdict(&findings, label.as_deref());
    if json_on() {
        emit(&json!({
            "healthy": healthy,
            "verdict": sentence,
            "target": label,
            "steps": report.steps,
        }));
        return Ok(healthy);
    }
    say("");
    let mark = if healthy { Mark::Ok } else { Mark::Fail };
    say(&format!("  {} {}", mark.symbol(), bold(&sentence)));
    Ok(healthy)
}

#[cfg(test)]
mod tests;
