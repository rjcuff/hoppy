//! `hoppy ports` and `hoppy port <n>`: what is listening, and who owns a port.

use crate::net::port_label;
use crate::style::{bold, cyan, dim, green, red, say, table, yellow};
use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::net::IpAddr;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// Who can connect to a listening socket. Ordered from narrowest to widest so
/// merging duplicates can keep the widest with `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reach {
    ThisMachine,
    OneInterface,
    WholeNetwork,
}

impl Reach {
    pub fn label(self) -> &'static str {
        match self {
            Reach::ThisMachine => "this machine only",
            Reach::OneInterface => "one interface",
            Reach::WholeNetwork => "whole network",
        }
    }

    fn colored(self) -> String {
        match self {
            Reach::ThisMachine => dim(self.label()),
            Reach::OneInterface => self.label().to_string(),
            Reach::WholeNetwork => yellow(self.label()),
        }
    }
}

/// One listening socket, ready to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRow {
    pub port: u16,
    pub proto: Proto,
    pub process: String,
    pub pid: u32,
    pub reach: Reach,
}

/// Translate a listen address into who can reach it.
pub fn reach_of(ip: IpAddr) -> Reach {
    // `::ffff:127.0.0.1` is IPv4 wearing an IPv6 costume; unwrap it first.
    let ip = ip.to_canonical();
    if ip.is_loopback() {
        Reach::ThisMachine
    } else if ip.is_unspecified() {
        Reach::WholeNetwork
    } else {
        Reach::OneInterface
    }
}

/// Merge duplicate rows (IPv4 + IPv6 sockets of one listener), keeping the
/// widest reach. Sorted by port.
pub fn collapse(rows: Vec<PortRow>) -> Vec<PortRow> {
    let mut merged: BTreeMap<(u16, Proto, u32), PortRow> = BTreeMap::new();
    for row in rows {
        merged
            .entry((row.port, row.proto, row.pid))
            .and_modify(|kept| kept.reach = kept.reach.max(row.reach))
            .or_insert(row);
    }
    merged.into_values().collect()
}

/// Everything listening: TCP always, bound UDP sockets when asked.
pub fn load(include_udp: bool) -> Result<Vec<PortRow>, String> {
    let all = listeners::get_all().map_err(|e| {
        format!(
            "couldn't read the list of listening ports ({e}). \
             Try again, or run with sudo/as Administrator to see every process."
        )
    })?;

    let rows = all
        .into_iter()
        .filter_map(|l| {
            let proto = match l.protocol {
                listeners::Protocol::TCP => Proto::Tcp,
                listeners::Protocol::UDP => Proto::Udp,
            };
            let wanted = match proto {
                Proto::Tcp => l.state == listeners::SocketState::Listen,
                Proto::Udp => include_udp,
            };
            wanted.then(|| PortRow {
                port: l.socket.port(),
                proto,
                process: process_name(&l.process.name),
                pid: l.process.pid,
                reach: reach_of(l.socket.ip()),
            })
        })
        .collect();

    Ok(collapse(rows))
}

fn process_name(raw: &str) -> String {
    if raw.trim().is_empty() {
        "(unknown)".to_string()
    } else {
        raw.to_string()
    }
}

fn header() -> Vec<String> {
    ["PORT", "PROTO", "PROCESS", "PID", "REACHABLE FROM", ""]
        .iter()
        .map(|h| dim(h))
        .collect()
}

fn display_row(row: &PortRow) -> Vec<String> {
    vec![
        bold(&row.port.to_string()),
        row.proto.label().to_string(),
        cyan(&row.process),
        row.pid.to_string(),
        row.reach.colored(),
        dim(port_label(row.port).unwrap_or("")),
    ]
}

fn print_rows(rows: &[PortRow]) {
    let mut cells = vec![header()];
    cells.extend(rows.iter().map(display_row));
    for line in table(&cells) {
        say(&format!("  {line}"));
    }
}

/// `hoppy ports [--udp]`
pub fn run_list(include_udp: bool) -> Result<(), String> {
    let rows = load(include_udp)?;
    if rows.is_empty() {
        say("Nothing is listening on this machine.");
        return Ok(());
    }

    say("");
    print_rows(&rows);
    say("");
    let hint = if include_udp {
        "hoppy port <n> to inspect one"
    } else {
        "hoppy port <n> to inspect one · --udp to include UDP"
    };
    say(&dim(&format!("  {} listening · {hint}", rows.len())));
    Ok(())
}

/// `hoppy port <n> [--kill] [-y]`
pub fn run_port(port: u16, kill: bool, yes: bool) -> Result<(), String> {
    let rows: Vec<PortRow> = load(true)?.into_iter().filter(|r| r.port == port).collect();

    if rows.is_empty() {
        say(&format!(
            "{} Nothing is using :{port}. It's free.",
            green("✓")
        ));
        if let Some(label) = port_label(port) {
            say(&dim(&format!("  :{port} is usually used for {label}.")));
        }
        return Ok(());
    }

    say("");
    print_rows(&rows);
    say("");

    if kill {
        kill_owners(port, &rows, yes)
    } else {
        say(&dim(&format!("  hoppy port {port} --kill to stop it")));
        Ok(())
    }
}

/// Stop every process holding the port, after confirming.
fn kill_owners(port: u16, rows: &[PortRow], yes: bool) -> Result<(), String> {
    let mut owners: Vec<(u32, &str)> = rows.iter().map(|r| (r.pid, r.process.as_str())).collect();
    owners.sort();
    owners.dedup_by_key(|(pid, _)| *pid);

    if !yes {
        // Never kill on a guess: with no keyboard attached there's nobody to
        // say yes, so the caller has to opt in explicitly.
        if !std::io::stdin().is_terminal() {
            return Err(format!(
                "not killing anything without confirmation. \
                 Run `hoppy port {port} --kill -y` to skip the prompt."
            ));
        }
        let names: Vec<String> = owners
            .iter()
            .map(|(pid, name)| format!("{name} (PID {pid})"))
            .collect();
        crate::style::ask(&format!("  Kill {}? [y/N] ", names.join(", ")));
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|e| format!("couldn't read your answer ({e}). Use -y to skip the prompt."))?;
        if !matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
            say("  Left it running.");
            return Ok(());
        }
    }

    let mut failures = Vec::new();
    for (pid, name) in owners {
        let outcome = match refuse_to_kill(pid, std::process::id()) {
            Some(why) => Err(why),
            None => kill_pid(pid),
        };
        match outcome {
            Ok(()) if wait_until_released(port, pid) => say(&format!(
                "  {} Stopped {name} (PID {pid}). :{port} is free.",
                green("✓")
            )),
            Ok(()) => {
                say(&format!(
                    "  {} Asked {name} (PID {pid}) to stop, but it's still holding :{port}. \
                     Give it a moment, then run `hoppy port {port}` again.",
                    yellow("?")
                ));
                failures.push(pid);
            }
            Err(why) => {
                say(&format!("  {} {name} (PID {pid}): {why}", red("✗")));
                failures.push(pid);
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("couldn't stop everything on :{port}. See above."))
    }
}

/// PIDs hoppy must never signal. On Unix, `kill 0` hits our own process group.
pub fn refuse_to_kill(pid: u32, own_pid: u32) -> Option<String> {
    if pid == 0 {
        Some("that's the kernel, not a process hoppy can stop.".to_string())
    } else if pid == own_pid {
        Some("that's hoppy itself.".to_string())
    } else {
        None
    }
}

/// Wait up to ~2s for `pid` to let go of `port`. A stop request is only a
/// request: the process may take a moment to shut down, or ignore it.
fn wait_until_released(port: u16, pid: u32) -> bool {
    const ATTEMPTS: u32 = 10;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(200);
    for attempt in 0..ATTEMPTS {
        let still_held = load(true)
            .map(|rows| rows.iter().any(|r| r.port == port && r.pid == pid))
            .unwrap_or(false);
        if !still_held {
            return true;
        }
        if attempt + 1 < ATTEMPTS {
            std::thread::sleep(PAUSE);
        }
    }
    false
}

/// Ask a process to exit: SIGTERM on Unix (the polite "please shut down"
/// signal), `taskkill` on Windows.
fn kill_pid(pid: u32) -> Result<(), String> {
    let pid_arg = pid.to_string();
    let output = if cfg!(windows) {
        // Windows has no SIGTERM. Plain taskkill only works on apps with a
        // window, so servers need /F (force).
        Command::new("taskkill")
            .args(["/PID", &pid_arg, "/F"])
            .output()
    } else {
        Command::new("kill").arg(&pid_arg).output()
    };

    let output = output.map_err(|e| {
        let tool = if cfg!(windows) { "taskkill" } else { "kill" };
        format!("couldn't run `{tool}` ({e}). Stop PID {pid} from your task manager instead.")
    })?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(kill_failure_message(pid, stderr.trim()))
}

/// Turn the OS's kill error into something that says what to do next.
pub fn kill_failure_message(pid: u32, stderr: &str) -> String {
    let lower = stderr.to_lowercase();
    if lower.contains("not permitted")
        || lower.contains("permission denied")
        || lower.contains("access is denied")
    {
        if cfg!(windows) {
            format!(
                "permission denied. PID {pid} belongs to another user or the system; \
                 run hoppy from an Administrator terminal."
            )
        } else {
            format!(
                "permission denied. PID {pid} belongs to another user or the system; \
                 try again with sudo."
            )
        }
    } else if lower.contains("no such process") || lower.contains("not found") {
        format!("PID {pid} already exited. Run `hoppy ports` to see what's left.")
    } else if stderr.is_empty() {
        format!("the OS refused to stop PID {pid}. Try again with sudo/Administrator.")
    } else {
        format!("{stderr}. Try again with sudo/Administrator.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(port: u16, proto: Proto, pid: u32, reach: Reach) -> PortRow {
        PortRow {
            port,
            proto,
            process: format!("proc{pid}"),
            pid,
            reach,
        }
    }

    #[test]
    fn reach_in_plain_english() {
        let reach = |s: &str| reach_of(s.parse().unwrap());
        assert_eq!(reach("127.0.0.1"), Reach::ThisMachine);
        assert_eq!(reach("127.0.0.53"), Reach::ThisMachine);
        assert_eq!(reach("::1"), Reach::ThisMachine);
        assert_eq!(reach("0.0.0.0"), Reach::WholeNetwork);
        assert_eq!(reach("::"), Reach::WholeNetwork);
        assert_eq!(reach("192.168.1.20"), Reach::OneInterface);
        assert_eq!(reach("fe80::1"), Reach::OneInterface);
    }

    #[test]
    fn reach_unwraps_ipv4_mapped_ipv6() {
        assert_eq!(
            reach_of("::ffff:127.0.0.1".parse().unwrap()),
            Reach::ThisMachine
        );
        assert_eq!(
            reach_of("::ffff:0.0.0.0".parse().unwrap()),
            Reach::WholeNetwork
        );
    }

    #[test]
    fn reach_labels() {
        assert_eq!(Reach::ThisMachine.label(), "this machine only");
        assert_eq!(Reach::WholeNetwork.label(), "whole network");
        assert_eq!(Reach::OneInterface.label(), "one interface");
    }

    #[test]
    fn collapse_merges_ipv4_and_ipv6_duplicates() {
        let rows = vec![
            row(3000, Proto::Tcp, 10, Reach::WholeNetwork),
            row(3000, Proto::Tcp, 10, Reach::WholeNetwork),
        ];
        assert_eq!(collapse(rows).len(), 1);
    }

    #[test]
    fn collapse_keeps_widest_reach() {
        let rows = vec![
            row(8080, Proto::Tcp, 10, Reach::ThisMachine),
            row(8080, Proto::Tcp, 10, Reach::WholeNetwork),
            row(8080, Proto::Tcp, 10, Reach::OneInterface),
        ];
        let merged = collapse(rows);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].reach, Reach::WholeNetwork);
    }

    #[test]
    fn collapse_keeps_different_protocols_and_processes_apart() {
        let rows = vec![
            row(53, Proto::Tcp, 10, Reach::ThisMachine),
            row(53, Proto::Udp, 10, Reach::ThisMachine),
            row(53, Proto::Tcp, 11, Reach::ThisMachine),
        ];
        assert_eq!(collapse(rows).len(), 3);
    }

    #[test]
    fn collapse_sorts_by_port() {
        let rows = vec![
            row(8080, Proto::Tcp, 1, Reach::ThisMachine),
            row(22, Proto::Tcp, 2, Reach::WholeNetwork),
            row(443, Proto::Tcp, 3, Reach::WholeNetwork),
        ];
        let ports: Vec<u16> = collapse(rows).iter().map(|r| r.port).collect();
        assert_eq!(ports, [22, 443, 8080]);
    }

    #[test]
    fn kill_failure_suggests_next_step() {
        let denied = kill_failure_message(1, "kill: (1): Operation not permitted");
        assert!(denied.contains("permission denied"));
        assert!(denied.contains("sudo") || denied.contains("Administrator"));

        let windows = kill_failure_message(4, "ERROR: Access is denied.");
        assert!(windows.contains("permission denied"));

        let gone = kill_failure_message(99, "kill: (99): No such process");
        assert!(gone.contains("already exited"));
    }

    #[test]
    fn never_kills_pid_zero_or_itself() {
        assert!(refuse_to_kill(0, 500).is_some());
        assert!(refuse_to_kill(500, 500).is_some());
        assert!(refuse_to_kill(1234, 500).is_none());
    }

    #[test]
    fn blank_process_names_get_a_placeholder() {
        assert_eq!(process_name(""), "(unknown)");
        assert_eq!(process_name("node"), "node");
    }
}
