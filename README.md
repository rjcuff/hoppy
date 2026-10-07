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

hoppy scan                # every device on your network, named
hoppy dns example.com     # your DNS vs. three public ones, side by side
hoppy route nas.local     # which adapter does traffic to this use?
hoppy watch               # live latency to your router and the internet
hoppy ip                  # local and public IP

hoppy ports --json | jq   # every command speaks JSON
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

  More: hoppy ports · hoppy scan · hoppy doctor · hoppy --help
```

### What it replaces

| You used to run                         | Now                    |
| --------------------------------------- | ---------------------- |
| `ifconfig` / `ip addr` / `ipconfig`     | `hoppy`                 |
| `netstat -an` / `ss -tlnp` / `lsof -i`  | `hoppy ports`           |
| `lsof -i :3000`, then `kill <pid>`      | `hoppy port 3000 --kill` |
| `ping`, `traceroute`, `nslookup`        | `hoppy doctor`          |
| `arp -a`, `nmap -sn`                    | `hoppy scan`            |
| `dig @1.1.1.1`, `dig @8.8.8.8`, ...     | `hoppy dns`             |
| `ip route get` / `route get`            | `hoppy route`           |
| `ping -t`, `mtr`                        | `hoppy watch`           |
| `curl ifconfig.me`                      | `hoppy ip`              |

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

Narrow the list with a filter or `--exposed`:

```sh
hoppy ports node          # by process name
hoppy ports postgres      # by what the port is usually for
hoppy ports 8080          # by port number
hoppy ports --exposed     # only what other devices can reach
```

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

### `hoppy scan [range | adapter]`

```console
$ hoppy scan

  Scanning 192.168.1.0/24 on en0 (254 addresses)...

  IP            NAME               MAC                MADE BY       LOOKS LIKE
  192.168.1.1   router.lan         b4:fb:e4:00:00:01  Ubiquiti      router             dns, http, https
  192.168.1.20  office-nas         00:11:32:00:00:02  Synology      PC or file server  ssh, http, smb (file sharing)
  192.168.1.31  living-room.local  3c:22:fb:00:00:03  Apple         AirPlay device (Apple TV, Mac, speaker)
  192.168.1.42  -                  f0:18:98:00:00:04  Apple         this machine       ssh
  192.168.1.57  pi.local           dc:a6:32:00:00:05  Raspberry Pi  computer (ssh)     ssh
  192.168.1.80  -                  da:a1:19:00:00:06  private address

  6 devices found on 192.168.1.0/24 in 1.9 s
```

Finds every device that's switched on, including ones that ignore pings, in
about two seconds and without root. hoppy sends one tiny packet toward each
address so the OS has to look the device up, then reads the answers from the
OS's own neighbor (ARP) table. Each device is then asked for its name (mDNS,
your DNS server, NetBIOS) and checked for a few telltale ports.

```sh
hoppy scan                # the network that carries your internet traffic
hoppy scan Ethernet       # the network on a specific adapter
hoppy scan 10.10.0.0/24   # a specific range
```

Ranges are capped at 1022 addresses (`/22`). `MADE BY` comes from a short
built-in list of common manufacturers; `private address` means the device made
up a random MAC, as phones and laptops do for privacy.

Only scan networks you own or are allowed to test.

### `hoppy dns <name>`

```console
$ hoppy dns ads.example.com

  ads.example.com A

  192.168.1.1  your router  0.0.0.0 (blocked)  2 ms
  1.1.1.1      Cloudflare   203.0.113.20       11 ms
  8.8.8.8      Google       203.0.113.20       14 ms
  9.9.9.9      Quad9        203.0.113.20       12 ms

  ✗ Your DNS is blocking ads.example.com (it answers 0.0.0.0). That's an ad blocker or filter such as Pi-hole doing its job.
```

Asks your own DNS servers and three public ones the same question at once, and
says what the difference means:

| What happened                                 | Verdict                                              |
| --------------------------------------------- | ---------------------------------------------------- |
| Yours answers `0.0.0.0`, public ones resolve  | Your DNS is blocking it (ad blocker or filter)       |
| Yours is silent, public ones resolve          | Your DNS server isn't answering                      |
| Only yours resolves                           | A private name on this network: normal               |
| Nobody resolves                               | The name doesn't exist: check the spelling           |
| Answers differ                                | Normal for big sites that steer you to a near server |

It also notices when the network answers DNS meant for other servers (many
routers and ISPs do), since then every row is really the same resolver.

```sh
hoppy dns example.com -t mx   # a, aaaa, cname, mx, ns, ptr, or txt
hoppy dns 8.8.8.8             # an IP is looked up in reverse
```

### `hoppy route <target>`

```console
$ hoppy route mixer.local

  target      mixer.local (10.10.0.77)
  leaves via  en7 (Ethernet) as 10.10.0.5
  next hop    direct (on 10.10.0.0/24)

  ✓ mixer.local is on 10.10.0.0/24, the same network as en7. Traffic goes straight there with no router in between.
```

With Wi-Fi, Ethernet, a VPN, and Docker all up at once, this answers "which way
does it actually go?". hoppy asks the OS which source address it would use,
which sends nothing on the wire.

### `hoppy watch [target]`

```console
$ hoppy watch example.com

  router    192.168.1.1      ▂▂▂▂▂▂▂▂▂▂▂▂▂▂▂▂  2 ms   avg 2 ms · max 3 ms · jitter <1 ms · loss 0%
  internet  1.1.1.1          ▂▂▂▃▂×▂▂█▂▂×▂▂▃▂  11 ms  avg 14 ms · max 96 ms · jitter 9 ms · loss 12%
  target    example.com:443  ▂▂▃▂▂×▂▂█▃▂×▂▂▂▂  13 ms  avg 16 ms · max 99 ms · jitter 9 ms · loss 12%
  ! The router is solid but the internet drops: the problem is upstream (modem, Starlink, or ISP).
```

A live graph of latency to each hop, updated every second, for the "it keeps
cutting out" problems that a single test can't catch. Comparing the hops says
where the drops start: between you and the router (Wi-Fi, cable), or upstream.

```sh
hoppy watch -n 60        # stop after 60 rounds and print a summary
hoppy watch -i 5         # one round every 5 seconds
hoppy watch --json       # one JSON object per round, for logging
```

### `hoppy ip`

```console
$ hoppy ip

  local   192.168.1.42  en0 (Wi-Fi)
  public  203.0.113.7   as seen by Amazon
```

The public address is found with one plain-text web request to
`checkip.amazonaws.com` (falling back to `icanhazip.com`, then
`api.ipify.org`). No other hoppy command contacts a third party except
`doctor` and `watch`, which open a connection to `1.1.1.1`, and `dns`, which
queries the public resolvers listed above.

## Configuration

hoppy has no config file.

### Flags

- `--all`, `-a`
  - Also shows loopback, down, and IP-less adapters in the overview.
- `--no-color`
  - Prints plain text.
- `--json`
  - Prints JSON instead of text. Works with every command. `hoppy watch --json`
    prints one object per line.

### Environment variables

- `NO_COLOR`
  - When set to any non-empty value, hoppy prints plain text. See [no-color.org].

Color is only used when stdout is a terminal, so piping hoppy into another
program always gives plain text.

### Scripting

Every command takes `--json`, and exit codes mean what you'd expect: `doctor`,
`dns`, `watch`, and `ip` exit `1` when the thing they check is broken.

```sh
hoppy ports --json | jq -r '.[] | select(.reach == "whole-network") | .process'
hoppy scan --json | jq -r '.devices[] | "\(.ip)\t\(.name // "-")"'
hoppy doctor --json | jq -r .verdict
hoppy dns example.com --json | jq .intercepted
```

## Roadmap

- [x] `hoppy scan`: find devices on the LAN
- [x] `hoppy route`: which adapter traffic uses
- [x] `--json` output
- [x] Public IP
- [ ] `hoppy trace`: every hop to a target, without root
- [ ] IPv6 in the overview and scan
- [ ] Full manufacturer database for `scan`
- [ ] Homebrew tap

## Contributing

Bug reports, ideas, and pull requests are welcome. See [CONTRIBUTING.md].

## Building

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

[MIT][license]

[changelog]: CHANGELOG.md
[contributing.md]: CONTRIBUTING.md
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
