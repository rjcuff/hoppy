# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-10-07

### Added

- `hoppy scan [range | adapter]`: finds every device on the network without
  root by reading the OS neighbor table, then names each one (mDNS, DNS,
  NetBIOS), shows who made it, and guesses what it is from its open ports.
- `hoppy dns <name> [-t type]`: asks your DNS servers and Cloudflare, Google,
  and Quad9 the same question at once and explains the difference: blocked,
  private name, dead resolver, or a typo. Also detects networks that intercept
  DNS. An IP address is looked up in reverse.
- `hoppy route <target>`: shows which adapter and source address traffic to a
  target uses, and whether it goes direct, through the router, or through a VPN.
- `hoppy watch [target] [-n count] [-i seconds]`: live latency graph to the
  router, the internet, and an optional target, with loss and jitter, and a
  hint about where drops start.
- `hoppy ip`: local and public IP addresses.
- `--json` on every command.
- `hoppy ports [filter]` matches by port, process name, or service, and
  `--exposed` (`-x`) shows only ports other devices can reach.
- `CONTRIBUTING.md`.

### Changed

- Names and text received from the network have control characters removed
  before they are printed.

## [0.1.0] - 2026-10-06

### Added

- `hoppy`: network overview with interface kinds in plain English, the
  default-route interface marked, DNS servers, top listening ports, and
  warnings for self-assigned addresses, overlapping subnets, and missing or
  multiple gateways.
- `hoppy ports [--udp]`: every listening socket with process, PID, and who can
  reach it. IPv4 and IPv6 duplicates are collapsed.
- `hoppy port <n> [--kill] [-y]`: show what is using a port and optionally stop
  it, with confirmation.
- `hoppy doctor [target]`: checks you, router, internet, DNS, and an optional
  target over TCP, then gives a one-sentence verdict. Exits `1` when unhealthy.
- Friendly labels for common ports, including PTP and Dante.
- Color on terminals only, with `--no-color` and `NO_COLOR` support.
- `install.sh` and a release workflow that builds binaries for Linux, macOS,
  and Windows.

[Unreleased]: https://github.com/rjcuff/hoppy/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/rjcuff/hoppy/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/rjcuff/hoppy/releases/tag/v0.1.0
