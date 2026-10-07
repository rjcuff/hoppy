//! hoppy: everyday networking, painless.

mod doctor;
mod net;
mod overview;
mod ports;
mod style;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

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

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Everything listening on this machine
    Ports {
        /// Include UDP as well as TCP
        #[arg(long)]
        udp: bool,
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    style::init(cli.no_color);

    let result = match cli.command {
        None => overview::run(cli.all).map(|()| true),
        Some(Cmd::Ports { udp }) => ports::run_list(udp).map(|()| true),
        Some(Cmd::Port { number, kill, yes }) => ports::run_port(number, kill, yes).map(|()| true),
        Some(Cmd::Doctor { target }) => doctor::run(target.as_deref()),
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
}
