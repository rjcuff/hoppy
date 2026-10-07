<!-- markdownlint-configure-file {
  "MD013": {
    "code_blocks": false,
    "tables": false
  },
  "MD033": false,
  "MD041": false
} -->

<div align="center">

# hoppy

[![CI][ci-badge]][ci]
[![crates.io][crates.io-badge]][crates.io]
[![License: MIT][license-badge]][license]

hoppy is a **friendlier ifconfig, netstat, and ping**, in one small binary.

It answers the questions you actually have: which adapter is on which network,
what's listening, and where the connection is broken.<br />
hoppy works the same on macOS, Linux, and Windows, with no sudo.

[Getting started](#getting-started) •
[Installation](#installation) •
[Commands](#commands) •
[Configuration](#configuration) •
[Roadmap](#roadmap)

</div>

## Getting started

![hoppy demo][demo]

```sh
hoppy                     # your network at a glance
hoppy --all               # include loopback, down, and IP-less adapters

hoppy ports               # everything listening on this machine
hoppy ports --udp         # ...including UDP

hoppy port 3000           # who's using port 3000?
hoppy port 3000 --kill    # stop it (asks first)
hoppy port 3000 --kill -y # stop it, no questions

hoppy doctor              # you -> router -> internet -> dns: where is it broken?
hoppy doctor nas.local:445        # ...and can I reach this?
hoppy doctor https://example.com  # hosts, host:port, URLs, and IPs all work
```

```console
$ hoppy

  ●  en0    Wi-Fi     192.168.1.42/24  gw 192.168.1.1  ← internet
  ●  en7    Ethernet  169.254.18.7/16  no gateway
  ●  utun3  VPN       10.8.0.2/24      no gateway

  DNS  192.168.1.1, 1.1.1.1

  Listening
    :22    sshd      whole network      ssh
    :3000  node      whole network      dev server
    :5432  postgres  this machine only  postgres
    :8080  python3   whole network      dev server

  ! en7 has a self-assigned address (169.254.18.7): nothing gave it an IP, check
    cable or DHCP (normal on a Dante/AV network with no DHCP).

  More: hoppy ports · hoppy port <n> · hoppy doctor [target]
```

### What it replaces

| You used to run                         | Now                    |
| --------------------------------------- | ---------------------- |
| `ifconfig` / `ip addr` / `ipconfig`     | `hoppy`                 |
| `netstat -an` / `ss -tlnp` / `lsof -i`  | `hoppy ports`           |
| `lsof -i :3000`, then `kill <pid>`      | `hoppy port 3000 --kill` |
| `ping`, `traceroute`, `nslookup`        | `hoppy doctor`          |

## Installation

hoppy is a single binary with no runtime dependencies.

<details>
<summary>Linux / WSL</summary>

> ```sh
> curl -sSfL https://raw.githubusercontent.com/rjcuff/hoppy/main/install.sh | sh
> ```
>
> Or build it with [Rust]:
>
> ```sh
> cargo install hoppy --locked
> ```

</details>

<details>
<summary>macOS</summary>

> ```sh
> curl -sSfL https://raw.githubusercontent.com/rjcuff/hoppy/main/install.sh | sh
> ```
>
> Or build it with [Rust]:
>
> ```sh
> cargo install hoppy --locked
> ```
>
> A Homebrew tap is planned.

</details>

<details>
<summary>Windows</summary>

> hoppy works in PowerShell, Windows Terminal, cmd, and Git Bash.
>
> Download `hoppy-<version>-x86_64-pc-windows-msvc.zip` from the
> [releases page][releases] and put `hoppy.exe` somewhere on your `PATH`.
>
> In Git Bash, MSYS2, or Cygwin you can use the install script:
>
> ```sh
> curl -sSfL https://raw.githubusercontent.com/rjcuff/hoppy/main/install.sh | sh
> ```
>
> Or build it with [Rust]:
>
> ```sh
> cargo install hoppy --locked
> ```

</details>

<details>
<summary>From source</summary>

> ```sh
> git clone https://github.com/rjcuff/hoppy
> cd hoppy
> cargo install --path . --locked
> ```

</details>

Building from source needs [Rust] 1.85 or newer. See the [changelog] for
release history.

## Commands

### `hoppy`

Your network at a glance: every live adapter, named for what it is (Wi-Fi,
Ethernet, VPN, Docker, VM, Bridge, AirDrop), with its address and gateway. The
adapter carrying internet traffic is marked `← internet`.

hoppy warns you, in plain English, when something looks off:

| Situation                               | Warning                                                         |
| --------------------------------------- | --------------------------------------------------------------- |
| An adapter has a `169.254.x.x` address  | Nothing gave it an IP: check cable or DHCP                      |
| Two adapters are on the same subnet     | Traffic may leave the wrong port                                |
| No adapter has a gateway                | You can reach direct neighbors but not the internet             |
| Several adapters have a gateway         | Says which one internet traffic actually uses                   |

### `hoppy ports`

```console
$ hoppy ports

  PORT  PROTO  PROCESS   PID    REACHABLE FROM
  22    tcp    sshd      812    whole network      ssh
  3000  tcp    node      48213  whole network      dev server
  5432  tcp    postgres  1337   this machine only  postgres
  8080  tcp    python3   50122  whole network      dev server

  4 listening · hoppy port <n> to inspect one · --udp to include UDP
```

`REACHABLE FROM` translates the listen address:

| Listening on        | Means             |
| ------------------- | ----------------- |
| `127.0.0.1` / `::1` | this machine only |
| `0.0.0.0` / `::`    | whole network     |
| a specific address  | one interface     |

IPv4 and IPv6 duplicates collapse into one row, and well-known ports are
labeled: ssh, dns, http, postgres, redis, dev servers, mdns, wireguard, and AV
gear such as PTP clocks (319/320) and Dante control (8700-8708).

### `hoppy port <n>`

```console
$ hoppy port 5173
✓ Nothing is using :5173. It's free.
  :5173 is usually used for dev server (vite).

$ hoppy port 3000 --kill

  PORT  PROTO  PROCESS  PID    REACHABLE FROM
  3000  tcp    node     48213  whole network   dev server

  Kill node (PID 48213)? [y/N] y
  ✓ Stopped node (PID 48213). :3000 is free.
```

`--kill` asks before stopping anything. Without a terminal attached it refuses
unless you pass `-y`, and it only reports success once the port is really free.

### `hoppy doctor [target]`

```console
$ hoppy doctor example.com:81

  ✓ connected  en0 has 192.168.1.42
  ✓ router     192.168.1.1 answered  2 ms
  ✓ internet   reached 1.1.1.1  11 ms
  ✓ dns        example.com is 93.184.216.34  14 ms
  ✗ target     example.com:81 didn't answer

  ✗ Your network is fine; the target itself is down or the port is wrong (example.com:81).
```

Each link in the chain is tested in order, and the verdict names the first one
that broke:

| What broke | Verdict                                                                                |
| ---------- | -------------------------------------------------------------------------------------- |
| You        | Ethernet gave itself a 169.254 address because nothing answered DHCP                   |
| Internet   | Your router is up, but the internet isn't. The problem is upstream: modem, Starlink, or your ISP |
| DNS        | Internet works but DNS doesn't. Try 1.1.1.1 as your DNS server                         |
| Target     | Your network is fine; the target itself is down or the port is wrong                   |

hoppy doctor exits with `0` when everything is healthy and `1` otherwise. It
needs no root: it uses TCP connections with short timeouts instead of ICMP.

## Configuration

hoppy has no config file.

### Flags

- `--all`, `-a`
  - Also shows loopback, down, and IP-less adapters in the overview.
- `--no-color`
  - Prints plain text.

### Environment variables

- `NO_COLOR`
  - When set to any non-empty value, hoppy prints plain text. See [no-color.org].

Color is only used when stdout is a terminal, so piping hoppy into another
program always gives plain text.

## Roadmap

- [ ] `hoppy explain`: routes and subnets in plain English
- [ ] `hoppy scan`: find devices on the LAN
- [ ] `--json` output
- [ ] Public IP in the overview
- [ ] Live TUI mode
- [ ] Homebrew tap and prebuilt binaries

## Building

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

[MIT][license]

[changelog]: CHANGELOG.md
[crates.io-badge]: https://img.shields.io/crates/v/hoppy?logo=rust&logoColor=white&style=flat-square
[crates.io]: https://crates.io/crates/hoppy
[demo]: assets/demo.gif
[ci-badge]: https://github.com/rjcuff/hoppy/actions/workflows/ci.yml/badge.svg
[ci]: https://github.com/rjcuff/hoppy/actions/workflows/ci.yml
[license-badge]: https://img.shields.io/badge/license-MIT-blue?style=flat-square
[license]: LICENSE
[no-color.org]: https://no-color.org
[releases]: https://github.com/rjcuff/hoppy/releases
[rust]: https://rustup.rs
