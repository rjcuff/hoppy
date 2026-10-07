//! hoppy: everyday networking, painless.

mod dns;
mod doctor;
mod ip;
mod lookup;
mod names;
mod neighbors;
mod net;
mod oui;
mod overview;
mod pool;
mod ports;
mod route;
mod scan;
mod style;
mod watch;

use clap::{Parser, Subcommand};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "hoppy",
    version,
    about = "Everyday networking, painless.",
    long_about = "Everyday networking, painless.\n\n\
                  Run `hoppy` with no arguments for your network at a glance."
)]
struct Cli {
    /// Also show loopback, down, and IP-less interfaces
    #[arg(short, long)]
    all: bool,

    /// Plain text, no colors (also honors the NO_COLOR env var)
    #[arg(long, global = true)]
    no_color: bool,

    /// Print JSON instead of text, for scripts and `jq`
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Everything listening on this machine
    Ports {
        /// Only rows matching this: a port number, process name, or service
        filter: Option<String>,
        /// Include UDP as well as TCP
        #[arg(long)]
        udp: bool,
        /// Only ports other devices can reach
        #[arg(short = 'x', long)]
        exposed: bool,
    },
    /// Who's using this port?
    Port {
        /// Port number, 1-65535
        #[arg(value_parser = clap::value_parser!(u16).range(1..))]
        number: u16,
        /// Stop the process using the port
        #[arg(long)]
        kill: bool,
        /// Don't ask for confirmation before killing
        #[arg(short, long, requires = "kill")]
        yes: bool,
    },
    /// Where is it broken? Checks you -> router -> internet -> dns -> target
    Doctor {
        /// Host, host:port, URL, or IP to test at the end of the chain
        target: Option<String>,
    },
    /// Find the devices on your network
    Scan {
        /// A range (192.168.1.0/24) or adapter name. Default: your main network
        target: Option<String>,
    },
    /// Ask your DNS and three public ones the same question, side by side
    Dns {
        /// Name to look up. An IP address is looked up in reverse
        name: String,
        /// Record type: a, aaaa, cname, mx, ns, ptr, or txt
        #[arg(short = 't', long = "type", value_name = "TYPE")]
        kind: Option<String>,
    },
    /// Which adapter would traffic to this target leave through?
    Route {
        /// Host, host:port, URL, or IP
        target: String,
    },
    /// Live latency to your router, the internet, and an optional target
    Watch {
        /// Host, host:port, URL, or IP to watch as well
        target: Option<String>,
        /// Stop after this many rounds
        #[arg(short = 'n', long, value_parser = clap::value_parser!(u32).range(1..))]
        count: Option<u32>,
        /// Seconds between rounds
        #[arg(short, long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=3600))]
        interval: u64,
    },
    /// Your local and public IP addresses
    Ip,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    style::init(cli.no_color, cli.json);

    let result = match cli.command {
        None => overview::run(cli.all).map(|()| true),
        Some(Cmd::Ports {
            filter,
            udp,
            exposed,
        }) => ports::run_list(udp, filter.as_deref(), exposed).map(|()| true),
        Some(Cmd::Port { number, kill, yes }) => ports::run_port(number, kill, yes).map(|()| true),
        Some(Cmd::Doctor { target }) => doctor::run(target.as_deref()),
        Some(Cmd::Scan { target }) => scan::run(target.as_deref()),
        Some(Cmd::Dns { name, kind }) => lookup::run(&name, kind.as_deref()),
        Some(Cmd::Route { target }) => route::run(&target),
        Some(Cmd::Watch {
            target,
            count,
            interval,
        }) => watch::run(target.as_deref(), count, Duration::from_secs(interval)),
        Some(Cmd::Ip) => ip::run(),
    };

    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("hoppy: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn port_zero_is_rejected() {
        assert!(Cli::try_parse_from(["hoppy", "port", "0"]).is_err());
        assert!(Cli::try_parse_from(["hoppy", "port", "70000"]).is_err());
        assert!(Cli::try_parse_from(["hoppy", "port", "3000"]).is_ok());
    }

    #[test]
    fn yes_requires_kill() {
        assert!(Cli::try_parse_from(["hoppy", "port", "3000", "-y"]).is_err());
        assert!(Cli::try_parse_from(["hoppy", "port", "3000", "--kill", "-y"]).is_ok());
    }

    #[test]
    fn json_works_before_or_after_the_subcommand() {
        assert!(Cli::try_parse_from(["hoppy", "--json"]).unwrap().json);
        assert!(
            Cli::try_parse_from(["hoppy", "ports", "--json"])
                .unwrap()
                .json
        );
        assert!(
            Cli::try_parse_from(["hoppy", "--json", "scan"])
                .unwrap()
                .json
        );
    }

    #[test]
    fn new_commands_parse() {
        for args in [
            vec!["hoppy", "ports", "node", "--exposed"],
            vec!["hoppy", "scan"],
            vec!["hoppy", "scan", "10.0.0.0/24"],
            vec!["hoppy", "dns", "example.com", "-t", "mx"],
            vec!["hoppy", "route", "example.com"],
            vec!["hoppy", "watch", "-n", "5", "-i", "2"],
            vec!["hoppy", "ip"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }
        assert!(Cli::try_parse_from(["hoppy", "watch", "-n", "0"]).is_err());
        assert!(Cli::try_parse_from(["hoppy", "dns"]).is_err());
    }
}
