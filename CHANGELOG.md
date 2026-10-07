# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/rjcuff/hoppy/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/rjcuff/hoppy/releases/tag/v0.1.0
