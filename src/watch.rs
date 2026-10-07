//! `hoppy watch [target]`: live latency to the router, the internet, and an
//! optional target, so a flaky connection shows where it's flaky.
//!
//! Like `doctor`, this times TCP connections instead of ICMP pings, so it
//! needs no root.

use crate::doctor::{ROUTER_PORTS, millis, ms, parse_target, probe_any, resolve, tcp_probe};
use crate::net::{self, Iface};
use crate::pool;
use crate::style::{ask, bold, color_on, dim, emit_line, json_on, red, say, table, yellow};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::thread;
use std::time::{Duration, Instant};

/// Windows takes about a second to report a refused connection, so the
/// timeout has to be longer than that.
const TIMEOUT: Duration = Duration::from_millis(1500);
const DNS_TIMEOUT: Duration = Duration::from_secs(4);
const INTERNET: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 443);
/// How many recent samples the graph shows.
const WINDOW: usize = 30;
/// How far back the live hint looks.
const HINT_WINDOW: usize = 60;
const NO_WRAP: &str = "\x1b[?7l";
const WRAP: &str = "\x1b[?7h";
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// One round trip: `Some(latency)` or `None` for no answer.
type Sample = Option<Duration>;

enum Probe {
    /// Any of these ports answering (open or refused) proves the host is up.
    AnyPort(IpAddr, &'static [u16]),
    Port(SocketAddr),
}

impl Probe {
    fn sample(&self) -> Sample {
        match self {
            Probe::AnyPort(ip, ports) => probe_any(*ip, ports, TIMEOUT),
            Probe::Port(addr) => tcp_probe(*addr, TIMEOUT).alive(),
        }
    }
}

struct Track {
    name: &'static str,
    shown: String,
    probe: Probe,
    samples: Vec<Sample>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    pub sent: usize,
    pub lost: usize,
    pub min: Option<Duration>,
    pub avg: Option<Duration>,
    pub max: Option<Duration>,
    /// Average change between one answer and the next. High jitter is what
    /// makes calls and games stutter even when the average looks fine.
    pub jitter: Option<Duration>,
}

impl Stats {
    pub fn loss_percent(&self) -> usize {
        if self.sent == 0 {
            0
        } else {
            self.lost * 100 / self.sent
        }
    }

    /// True when the host never answered at all.
    fn silent(&self) -> bool {
        self.lost == self.sent
    }
}

fn mean(times: &[Duration]) -> Option<Duration> {
    let count = u32::try_from(times.len()).ok().filter(|n| *n > 0)?;
    Some(times.iter().sum::<Duration>() / count)
}

pub fn stats(samples: &[Sample]) -> Stats {
    let answered: Vec<Duration> = samples.iter().flatten().copied().collect();
    let changes: Vec<Duration> = answered.windows(2).map(|w| w[0].abs_diff(w[1])).collect();
    Stats {
        sent: samples.len(),
        lost: samples.len() - answered.len(),
        min: answered.iter().min().copied(),
        avg: mean(&answered),
        max: answered.iter().max().copied(),
        jitter: mean(&changes),
    }
}

/// A bar chart of the most recent samples, one character each. Lost samples
/// show as `×`.
pub fn sparkline(samples: &[Sample], window: usize) -> String {
    let recent = &samples[samples.len().saturating_sub(window)..];
    let fastest = recent.iter().flatten().min().copied().unwrap_or_default();
    let slowest = recent.iter().flatten().max().copied().unwrap_or_default();
    // Leave headroom above a steady line so normal wobble doesn't look like
    // a spike: the top of the chart is at least three times the fastest.
    let top = slowest.max(fastest * 3).as_secs_f64().max(0.001);
    recent
        .iter()
        .map(|sample| match sample {
            Some(t) => {
                let level = (t.as_secs_f64() / top * 7.0).round() as usize;
                BARS[level.min(7)].to_string()
            }
            None => red("×"),
        })
        .collect()
}

/// Where the trouble is, judged by which hops are dropping probes.
pub fn diagnose(router: Option<&Stats>, internet: &Stats) -> Option<String> {
    if internet.lost == 0 {
        return None;
    }
    // A router that never answers is ignoring probes, not broken, so it
    // can't be used as evidence either way.
    let hint = match router.filter(|r| !r.silent()) {
        Some(r) if r.lost > 0 => {
            "Drops start between you and the router: weak Wi-Fi, a bad cable, or a busy router."
        }
        Some(_) => {
            "The router is solid but the internet drops: the problem is upstream (modem, \
             Starlink, or ISP)."
        }
        None => "The internet is dropping probes.",
    };
    Some(hint.to_string())
}

fn summary(s: &Stats) -> String {
    match (s.avg, s.max, s.jitter) {
        (Some(avg), Some(max), jitter) => {
            let jitter = jitter.map_or_else(String::new, |j| format!(" · jitter {}", millis(j)));
            format!(
                "avg {} · max {}{jitter} · loss {}%",
                millis(avg),
                millis(max),
                s.loss_percent()
            )
        }
        _ => "no answer yet".to_string(),
    }
}

fn find<'a>(tracks: &'a [Track], name: &str) -> Option<&'a Track> {
    tracks.iter().find(|track| track.name == name)
}

/// The last `limit` samples, so a live hint reflects what's happening now
/// instead of a blip from an hour ago.
fn recent(samples: &[Sample], limit: usize) -> &[Sample] {
    &samples[samples.len().saturating_sub(limit)..]
}

fn hint(tracks: &[Track], limit: usize) -> Option<String> {
    let of = |name| find(tracks, name).map(|t| stats(recent(&t.samples, limit)));
    diagnose(of("router").as_ref(), &of("internet")?)
}

/// The lines of one screen update.
fn frame(tracks: &[Track]) -> Vec<String> {
    let rows: Vec<Vec<String>> = tracks
        .iter()
        .map(|track| {
            let last = match track.samples.last() {
                Some(Some(t)) => bold(&millis(*t)),
                Some(None) => red("timeout"),
                None => String::new(),
            };
            vec![
                bold(track.name),
                dim(&track.shown),
                sparkline(&track.samples, WINDOW),
                last,
                dim(&summary(&stats(&track.samples))),
            ]
        })
        .collect();
    let mut lines: Vec<String> = table(&rows).into_iter().map(|l| format!("  {l}")).collect();
    // Always present, so every frame is the same height and can be redrawn
    // in place.
    let hint = hint(tracks, HINT_WINDOW);
    lines.push(hint.map_or_else(String::new, |h| format!("  {} {h}", yellow("!"))));
    lines
}

/// Draw a frame. On a terminal each frame replaces the previous one; when
/// piped, each round is one plain line.
fn draw(tracks: &[Track], round: u32) {
    if !color_on() {
        let parts: Vec<String> = tracks
            .iter()
            .map(|track| {
                let last = track.samples.last().copied().flatten();
                format!(
                    "{} {}",
                    track.name,
                    last.map_or_else(|| "timeout".to_string(), millis)
                )
            })
            .collect();
        say(&format!("  {round:>4}  {}", parts.join(" · ")));
        return;
    }
    ask(&redraw(&frame(tracks), round > 1));
}

/// The text that paints a frame. After the first frame, the cursor first
/// moves back up so the new frame lands on top of the old one.
///
/// Moving up by the number of lines only works if each line takes one row,
/// so line wrapping is switched off while drawing: on a narrow terminal the
/// right-hand side is cut off instead of spilling onto a second row.
fn redraw(lines: &[String], over_previous: bool) -> String {
    let mut out = String::from(NO_WRAP);
    if over_previous {
        out.push_str(&format!("\x1b[{}A", lines.len()));
    }
    for line in lines {
        // Clear the old line before writing the new one over it.
        out.push_str(&format!("\x1b[2K{line}\n"));
    }
    out.push_str(WRAP);
    out
}

fn build_tracks(target: Option<&str>) -> Result<Vec<Track>, String> {
    let live: Vec<Iface> = net::load_interfaces()
        .into_iter()
        .filter(Iface::is_interesting)
        .collect();
    let gateway = live.iter().find(|i| i.is_default).and_then(|i| i.gateway);

    let mut tracks = Vec::new();
    let mut add = |name, shown: String, probe| {
        tracks.push(Track {
            name,
            shown,
            probe,
            samples: Vec::new(),
        });
    };
    if let Some(gw) = gateway {
        add(
            "router",
            gw.to_string(),
            Probe::AnyPort(IpAddr::V4(gw), &ROUTER_PORTS),
        );
    }
    add("internet", INTERNET.ip().to_string(), Probe::Port(INTERNET));
    if let Some(input) = target {
        let target = parse_target(input)?;
        let addr = match target.ip() {
            Some(ip) => SocketAddr::new(ip, target.port),
            None => {
                let (addrs, _) = resolve(&target.host, target.port, DNS_TIMEOUT)
                    .ok_or_else(|| format!("couldn't look up {}.", target.host))?;
                *addrs.iter().find(|a| a.is_ipv4()).unwrap_or(&addrs[0])
            }
        };
        add("target", target.label(), Probe::Port(addr));
    }
    Ok(tracks)
}

/// `hoppy watch [target] [-n count] [-i seconds]`. Runs until interrupted, or
/// for `count` rounds. Returns whether the internet answered at all.
pub fn run(target: Option<&str>, count: Option<u32>, interval: Duration) -> Result<bool, String> {
    let mut tracks = build_tracks(target)?;
    if !json_on() {
        say("");
    }

    let mut round = 0u32;
    loop {
        round += 1;
        let started = Instant::now();
        let probes: Vec<&Probe> = tracks.iter().map(|track| &track.probe).collect();
        let results = pool::map(probes, tracks.len(), Probe::sample);
        for (track, result) in tracks.iter_mut().zip(results) {
            track.samples.push(result);
        }

        if json_on() {
            let samples: serde_json::Map<String, serde_json::Value> = tracks
                .iter()
                .map(|t| {
                    let last = t.samples.last().copied().flatten();
                    (t.name.to_string(), json!(last.map(ms)))
                })
                .collect();
            emit_line(&json!({ "round": round, "ms": samples }));
        } else {
            draw(&tracks, round);
        }

        if count.is_some_and(|limit| round >= limit) {
            break;
        }
        thread::sleep(interval.saturating_sub(started.elapsed()));
    }

    let internet = find(&tracks, "internet").map(|t| stats(&t.samples));
    if json_on() {
        let totals: serde_json::Map<String, serde_json::Value> = tracks
            .iter()
            .map(|t| {
                let s = stats(&t.samples);
                let value = json!({
                    "sent": s.sent,
                    "lost": s.lost,
                    "min_ms": s.min.map(ms),
                    "avg_ms": s.avg.map(ms),
                    "max_ms": s.max.map(ms),
                    "jitter_ms": s.jitter.map(ms),
                });
                (t.name.to_string(), value)
            })
            .collect();
        emit_line(&json!({ "summary": totals, "hint": hint(&tracks, usize::MAX) }));
    } else if !color_on() {
        for track in &tracks {
            say(&format!(
                "  {}: {}",
                track.name,
                summary(&stats(&track.samples))
            ));
        }
        if let Some(hint) = hint(&tracks, usize::MAX) {
            say(&format!("  ! {hint}"));
        }
    }
    Ok(internet.is_some_and(|s| !s.silent()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: &[u64]) -> Vec<Sample> {
        millis
            .iter()
            .map(|m| (*m > 0).then(|| Duration::from_millis(*m)))
            .collect()
    }

    #[test]
    fn stats_cover_loss_and_spread() {
        // 0 stands for a lost sample.
        let s = stats(&at(&[10, 20, 0, 30]));
        assert_eq!(s.sent, 4);
        assert_eq!(s.lost, 1);
        assert_eq!(s.loss_percent(), 25);
        assert_eq!(s.min, Some(Duration::from_millis(10)));
        assert_eq!(s.avg, Some(Duration::from_millis(20)));
        assert_eq!(s.max, Some(Duration::from_millis(30)));
        assert_eq!(s.jitter, Some(Duration::from_millis(10)));
    }

    #[test]
    fn stats_with_no_answers() {
        let s = stats(&at(&[0, 0]));
        assert!(s.silent());
        assert_eq!(s.loss_percent(), 100);
        assert_eq!(s.avg, None);
        assert_eq!(summary(&s), "no answer yet");
        assert_eq!(stats(&[]).loss_percent(), 0);
    }

    #[test]
    fn sparkline_shows_spikes_and_losses() {
        let line = sparkline(&at(&[10, 10, 100, 0]), WINDOW);
        let chars: Vec<char> = line.chars().collect();
        assert_eq!(chars.len(), 4);
        assert_eq!(chars[0], chars[1]);
        assert_eq!(chars[2], '█');
        assert_eq!(chars[3], '×');
        assert!(chars[0] < chars[2]);
    }

    #[test]
    fn sparkline_keeps_a_steady_line_low() {
        let line = sparkline(&at(&[10, 11, 10, 12]), WINDOW);
        assert!(line.chars().all(|c| c <= '▄'), "{line}");
    }

    #[test]
    fn sparkline_only_shows_the_window() {
        let many = at(&[5; 100]);
        assert_eq!(sparkline(&many, WINDOW).chars().count(), WINDOW);
        assert_eq!(sparkline(&[], WINDOW), "");
    }

    #[test]
    fn diagnose_points_at_the_first_lossy_hop() {
        let clean = stats(&at(&[5, 5, 5]));
        let lossy = stats(&at(&[5, 0, 5]));
        let silent = stats(&at(&[0, 0, 0]));

        assert_eq!(diagnose(Some(&clean), &clean), None);
        assert!(
            diagnose(Some(&lossy), &lossy)
                .unwrap()
                .contains("between you and the router")
        );
        assert!(diagnose(Some(&clean), &lossy).unwrap().contains("upstream"));
        // A router that ignores probes isn't blamed.
        assert_eq!(
            diagnose(Some(&silent), &lossy).unwrap(),
            "The internet is dropping probes."
        );
        assert!(diagnose(None, &lossy).is_some());
    }

    #[test]
    fn redraw_moves_up_by_the_frame_height() {
        let lines = ["a".to_string(), "b".to_string()];
        let body = "\x1b[2Ka\n\x1b[2Kb\n";
        assert_eq!(redraw(&lines, false), format!("{NO_WRAP}{body}{WRAP}"));
        assert_eq!(
            redraw(&lines, true),
            format!("{NO_WRAP}\x1b[2A{body}{WRAP}")
        );
    }

    #[test]
    fn live_hint_forgets_old_trouble() {
        let track = |name, samples| Track {
            name,
            shown: String::new(),
            probe: Probe::Port(INTERNET),
            samples,
        };
        // One early drop, then a long clean run.
        let mut samples = at(&[0]);
        samples.extend(at(&[5; 80]));
        let tracks = [track("internet", samples)];
        assert!(hint(&tracks, usize::MAX).is_some());
        assert_eq!(hint(&tracks, HINT_WINDOW), None);
    }

    #[test]
    fn frames_keep_a_constant_height() {
        let track = |samples| Track {
            name: "internet",
            shown: "1.1.1.1".to_string(),
            probe: Probe::Port(INTERNET),
            samples,
        };
        assert_eq!(frame(&[track(at(&[5, 5]))]).len(), 2);
        assert_eq!(frame(&[track(at(&[5, 0]))]).len(), 2);
    }
}
