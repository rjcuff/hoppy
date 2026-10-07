# Contributing to hoppy

Thanks for helping. Bug reports, ideas, and pull requests are all welcome.

## Reporting a bug

Open an [issue] with:

- what you ran and what you expected,
- what hoppy printed (`hoppy <command> --json` output is the most useful),
- your OS and `hoppy --version`.

hoppy's output contains your IP addresses, MAC addresses, and device names.
Replace anything you'd rather not share before posting.

Networking differs a lot between machines, so "it mislabels my adapter" or
"scan misses my printer" are good bug reports, not nitpicks.

## Building

You need [Rust] 1.85 or newer.

```sh
git clone https://github.com/rjcuff/hoppy
cd hoppy
cargo run -- doctor     # run a command from source
```

Before opening a pull request, run what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs these on Linux, macOS, and Windows. If you can only test on one, say
so in the pull request.

## How the code is laid out

Each command is one file. Shared building blocks sit beside them.

| File                  | What it does                                                   |
| --------------------- | -------------------------------------------------------------- |
| `src/main.rs`         | Command-line definition and dispatch                           |
| `src/overview.rs`     | `hoppy`                                                        |
| `src/ports.rs`        | `hoppy ports`, `hoppy port`                                    |
| `src/doctor.rs`       | `hoppy doctor`, plus the TCP probes other commands reuse       |
| `src/scan.rs`         | `hoppy scan`                                                   |
| `src/lookup.rs`       | `hoppy dns`                                                    |
| `src/route.rs`        | `hoppy route`                                                  |
| `src/watch.rs`        | `hoppy watch`                                                  |
| `src/ip.rs`           | `hoppy ip`                                                     |
| `src/net.rs`          | Interfaces, adapter kinds, warnings, port labels               |
| `src/dns.rs`          | A small DNS client: build queries, parse replies               |
| `src/names.rs`        | Device names over mDNS, DNS, and NetBIOS                       |
| `src/neighbors.rs`    | Reads and parses the OS neighbor (ARP) table                   |
| `src/oui.rs`          | MAC address to manufacturer                                    |
| `src/pool.rs`         | Runs many network calls at once                                |
| `src/style.rs`        | Colors, tables, printing, JSON output                          |

## Ground rules

These are what make hoppy hoppy. Changes that break them need a strong reason.

- **No root.** Every command must work for a normal user. That rules out raw
  sockets and ICMP on most systems; use TCP, UDP, and what the OS already
  exposes.
- **Same behavior on Linux, macOS, and Windows.** If something can't work on
  one of them, it should say so clearly instead of failing oddly.
- **Plain English.** Output says what is wrong and what to do next. Explain a
  networking term the first time it appears.
- **Fast by default.** Commands run their network calls in parallel and use
  short timeouts. Nothing should hang.
- **Few dependencies.** hoppy is a small single binary. Prefer the standard
  library; open an issue before adding a crate.
- **Quiet on the network.** A command only contacts what it needs to. Any new
  third-party endpoint must be listed in the README.
- **Never trust the network.** Bytes from the network are parsed with bounds
  checks and never panic, and text from the network goes through
  `dns::clean` before it is printed.

## Writing a change

- Keep the decision-making separate from the I/O. Most files have pure
  functions (`verdict`, `plan`, `explain`, `parse`, `looks_like`) that take
  plain data and are tested with made-up inputs, and a thin `run` that does
  the real network calls. New logic should follow that shape so it can be
  tested without a network.
- Add tests next to the code, in the `#[cfg(test)] mod tests` at the bottom of
  the file. Tests must not need network access.
- New output needs a `--json` form too.
- Add a line to `CHANGELOG.md` under `Unreleased`.
- Use real-looking but fake addresses in docs and tests: `192.168.1.x`,
  `10.x`, or the documentation ranges `192.0.2.0/24` and `203.0.113.0/24`.

### Easy first contributions

- Add a port to `port_label` in `src/net.rs`.
- Add an adapter name to `interface_kind` in `src/net.rs` when hoppy calls
  your VPN or virtual adapter "Other".
- Add a manufacturer prefix to `src/oui.rs` (check it against the
  [IEEE registry] first).
- Add a device guess to `looks_like` in `src/scan.rs`.

## Pull requests

- One change per pull request.
- Commit messages follow [Conventional Commits]: `feat: ...`, `fix: ...`,
  `docs: ...`, `refactor: ...`, `test: ...`, `chore: ...`.
- Describe how you tested it, and on which OS.

## Releasing (maintainers)

1. Move the `Unreleased` entries in `CHANGELOG.md` under a new version heading
   and update the links at the bottom.
2. Bump `version` in `Cargo.toml`, then run `cargo build` to update
   `Cargo.lock`.
3. Commit, then `cargo publish`.
4. Tag and push: `git tag v0.x.y && git push origin main v0.x.y`. The release
   workflow builds the binaries and creates the GitHub release.

## License

By contributing you agree that your work is released under the [MIT license].

[conventional commits]: https://www.conventionalcommits.org
[ieee registry]: https://standards-oui.ieee.org
[issue]: https://github.com/rjcuff/hoppy/issues
[mit license]: LICENSE
[rust]: https://rustup.rs
