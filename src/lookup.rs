//! `hoppy dns <name>`: ask every resolver the same question and compare.
//!
//! "DNS is broken" usually means one particular server is broken, blocking,
//! or out of date. Asking your own DNS and three public ones side by side
//! shows which.

use crate::dns::{self, Kind, Record, Reply};
use crate::doctor::{millis, ms, parse_target};
use crate::net::{self, Iface};
use crate::pool;
use crate::style::{bold, dim, emit, green, json_on, red, say, table, yellow};
use serde_json::json;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::thread;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(2);
/// How many answers to show per resolver before summarising the rest.
const MAX_SHOWN: usize = 3;
/// An address set aside for documentation: no DNS server can live there, so
/// an answer from it means something on the way is answering in its place.
const CANARY: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 53));
const CANARY_TIMEOUT: Duration = Duration::from_millis(500);
const PUBLIC: [(Ipv4Addr, &str); 3] = [
    (Ipv4Addr::new(1, 1, 1, 1), "Cloudflare"),
    (Ipv4Addr::new(8, 8, 8, 8), "Google"),
    (Ipv4Addr::new(9, 9, 9, 9), "Quad9"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolver {
    pub addr: IpAddr,
    pub who: String,
    /// True for the servers this machine is configured to use.
    pub yours: bool,
}

/// This machine's own DNS servers first, then the public ones it isn't
/// already using.
pub fn resolvers(system: &[IpAddr], gateway: Option<Ipv4Addr>) -> Vec<Resolver> {
    let public_name = |ip: &IpAddr| {
        PUBLIC
            .iter()
            .find(|(addr, _)| IpAddr::V4(*addr) == *ip)
            .map(|(_, who)| *who)
    };
    let mut list: Vec<Resolver> = system
        .iter()
        .map(|ip| Resolver {
            addr: *ip,
            who: match public_name(ip) {
                Some(who) => format!("your DNS ({who})"),
                None if gateway.map(IpAddr::V4) == Some(*ip) => "your router".to_string(),
                None => "your DNS".to_string(),
            },
            yours: true,
        })
        .collect();
    for (addr, who) in PUBLIC {
        if !system.contains(&IpAddr::V4(addr)) {
            list.push(Resolver {
                addr: IpAddr::V4(addr),
                who: who.to_string(),
                yours: false,
            });
        }
    }
    list
}

/// What one resolver said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Answers(Vec<Record>),
    NoSuchName,
    /// The name exists but has no records of the type asked for.
    Empty,
    Failed(String),
}

impl Outcome {
    /// Blockers such as Pi-hole answer `0.0.0.0` instead of "no such name".
    fn is_blocked(&self) -> bool {
        match self {
            Outcome::Answers(records) => records.iter().all(|record| match record {
                Record::A(ip) => ip.is_unspecified(),
                Record::Aaaa(ip) => ip.is_unspecified(),
                _ => false,
            }),
            _ => false,
        }
    }

    /// A real, usable answer.
    fn is_good(&self) -> bool {
        matches!(self, Outcome::Answers(_)) && !self.is_blocked()
    }

    fn key(&self) -> &'static str {
        match self {
            Outcome::Answers(_) => "answers",
            Outcome::NoSuchName => "no-such-name",
            Outcome::Empty => "empty",
            Outcome::Failed(_) => "failed",
        }
    }
}

/// Boil a reply down to the records of the type that was asked for.
pub fn outcome(result: Result<Reply, String>, kind: Kind) -> Outcome {
    let reply = match result {
        Ok(reply) => reply,
        Err(why) => return Outcome::Failed(why),
    };
    match reply.code {
        0 => {}
        dns::NO_SUCH_NAME => return Outcome::NoSuchName,
        2 => return Outcome::Failed("server failure".to_string()),
        5 => return Outcome::Failed("refused to answer".to_string()),
        code => return Outcome::Failed(format!("server error (code {code})")),
    }
    let truncated = reply.truncated;
    let wanted: BTreeSet<Record> = reply.records.into_iter().filter(|r| r.is(kind)).collect();
    if wanted.is_empty() && truncated {
        // Saying "no records" here would be wrong: they exist, they just
        // didn't fit.
        Outcome::Failed("answer too big for one packet".to_string())
    } else if wanted.is_empty() {
        Outcome::Empty
    } else {
        Outcome::Answers(wanted.into_iter().collect())
    }
}

pub struct Row {
    pub resolver: Resolver,
    pub outcome: Outcome,
    pub took: Option<Duration>,
}

/// One sentence explaining the results, and whether the name is usable from
/// this machine.
pub fn verdict(name: &str, kind: Kind, rows: &[Row]) -> (bool, String) {
    let of = |yours: bool| {
        rows.iter()
            .filter(move |row| row.resolver.yours == yours)
            .map(|row| &row.outcome)
    };
    let failed = |o: &Outcome| matches!(o, Outcome::Failed(_));

    if rows.iter().all(|row| failed(&row.outcome)) {
        return (
            false,
            "No resolver answered. DNS is blocked on this network or you're offline: \
             run `hoppy doctor`."
                .to_string(),
        );
    }
    let blocked_here = of(true).any(Outcome::is_blocked);
    if !rows.iter().any(|row| row.outcome.is_good()) && !blocked_here {
        let sentence = if rows.iter().any(|row| row.outcome == Outcome::NoSuchName) {
            format!("{name} doesn't exist. Check the spelling.")
        } else {
            format!("{name} exists but has no {} records.", kind.label())
        };
        return (false, sentence);
    }

    let yours_good = of(true).any(Outcome::is_good);
    let public_good = of(false).any(Outcome::is_good);
    if of(true).next().is_some() && !yours_good {
        let sentence = if blocked_here {
            format!(
                "Your DNS is blocking {name} (it answers 0.0.0.0). That's an ad blocker or \
                 filter such as Pi-hole doing its job."
            )
        } else if of(true).all(failed) {
            "Your DNS server isn't answering, but public ones are. Restart the router, or \
             switch this machine's DNS to 1.1.1.1."
                .to_string()
        } else {
            format!(
                "Your DNS can't find {name}, but public resolvers can. Your DNS is filtering \
                 it or is out of date."
            )
        };
        return (false, sentence);
    }
    if !public_good {
        let sentence = if of(false).all(failed) {
            "Your DNS works. Public resolvers didn't answer, so this network blocks outside \
             DNS."
                .to_string()
        } else {
            format!(
                "Only your DNS knows {name}: it's a private name on this network. Normal for \
                 .lan, .local, and work names."
            )
        };
        return (true, sentence);
    }

    let distinct: BTreeSet<&Vec<Record>> = rows
        .iter()
        .filter_map(|row| match &row.outcome {
            Outcome::Answers(records) => Some(records),
            _ => None,
        })
        .collect();
    let sentence = if distinct.len() > 1 {
        "Resolvers give different answers. Normal for big sites: each one is pointed at a \
         nearby server."
            .to_string()
    } else {
        "All resolvers agree.".to_string()
    };
    (true, sentence)
}

/// Work out what to ask. An IP address becomes a reverse lookup.
pub fn question(input: &str, kind: Option<&str>) -> Result<(String, Kind), String> {
    let host = parse_target(input)?.host;
    if !host.is_ascii() {
        return Err(format!(
            "hoppy can't look up names with accented or non-Latin letters yet. Use the \
             encoded form of \"{host}\" (it starts with xn--)."
        ));
    }
    let asked = kind
        .map(|text| {
            Kind::parse(text).ok_or_else(|| {
                format!("unknown record type \"{text}\". Use a, aaaa, cname, mx, ns, ptr, or txt.")
            })
        })
        .transpose()?;
    match host.parse::<IpAddr>() {
        Ok(ip) if matches!(asked, None | Some(Kind::Ptr)) => Ok((dns::reverse_name(ip), Kind::Ptr)),
        Ok(_) => Err(format!(
            "{host} is an address, so the only thing to look up is its name (drop --type)."
        )),
        Err(_) => Ok((host, asked.unwrap_or(Kind::A))),
    }
}

fn answer_cell(outcome: &Outcome, kind: Kind) -> String {
    match outcome {
        Outcome::Answers(records) => {
            let shown: Vec<String> = records
                .iter()
                .take(MAX_SHOWN)
                .map(Record::to_string)
                .collect();
            let more = records.len().saturating_sub(MAX_SHOWN);
            let list = shown.join(", ");
            let text = if more > 0 {
                format!("{list} {}", dim(&format!("+{more}")))
            } else {
                list
            };
            if outcome.is_blocked() {
                yellow(&format!("{text} (blocked)"))
            } else {
                text
            }
        }
        Outcome::NoSuchName => red("no such name"),
        Outcome::Empty => dim(&format!("no {} records", kind.label())),
        Outcome::Failed(why) => yellow(why),
    }
}

/// Is DNS being intercepted? Many routers and ISPs quietly answer every DNS
/// question themselves, whichever server it was addressed to. When they do,
/// asking different resolvers proves nothing: it's one resolver every time.
fn intercepted(name: &str, kind: Kind) -> bool {
    dns::query(CANARY, name, kind, CANARY_TIMEOUT).is_ok()
}

const INTERCEPT_NOTE: &str = "Something on this network (usually the router) answers DNS meant \
                              for other servers, so the rows above may all be the same \
                              resolver.";

fn to_json(
    name: &str,
    kind: Kind,
    rows: &[Row],
    ok: bool,
    sentence: &str,
    intercepted: bool,
) -> serde_json::Value {
    let resolvers: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let (answers, error) = match &row.outcome {
                Outcome::Answers(records) => {
                    (records.iter().map(Record::to_string).collect(), None)
                }
                Outcome::Failed(why) => (Vec::new(), Some(why.as_str())),
                Outcome::NoSuchName | Outcome::Empty => (Vec::new(), None),
            };
            json!({
                "server": row.resolver.addr.to_string(),
                "who": row.resolver.who,
                "yours": row.resolver.yours,
                "status": row.outcome.key(),
                "answers": answers,
                "error": error,
                "ms": row.took.map(ms),
            })
        })
        .collect();
    json!({
        "name": name,
        "type": kind.label(),
        "ok": ok,
        "verdict": sentence,
        "intercepted": intercepted,
        "resolvers": resolvers,
    })
}

/// `hoppy dns <name> [--type T]`. Returns whether the name resolves here.
pub fn run(input: &str, kind: Option<&str>) -> Result<bool, String> {
    let (name, kind) = question(input, kind)?;

    let live: Vec<Iface> = net::load_interfaces()
        .into_iter()
        .filter(Iface::is_interesting)
        .collect();
    let gateway = live.iter().find(|i| i.is_default).and_then(|i| i.gateway);
    let list = resolvers(&net::queryable_dns(&live), gateway);

    let (rows, intercepted) = thread::scope(|scope| {
        let canary = scope.spawn(|| intercepted(&name, kind));
        let rows = pool::map(list, 8, |resolver| {
            let result = dns::query(resolver.addr, &name, kind, TIMEOUT);
            let took = result.as_ref().ok().map(|(_, took)| *took);
            Row {
                resolver,
                outcome: outcome(result.map(|(reply, _)| reply), kind),
                took,
            }
        });
        (rows, canary.join().unwrap_or(false))
    });
    let (ok, sentence) = verdict(input.trim(), kind, &rows);

    if json_on() {
        emit(&to_json(&name, kind, &rows, ok, &sentence, intercepted));
        return Ok(ok);
    }

    say("");
    say(&format!("  {} {}", bold(&name), dim(kind.label())));
    say("");
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.resolver.addr.to_string(),
                dim(&row.resolver.who),
                answer_cell(&row.outcome, kind),
                row.took.map_or_else(String::new, |t| dim(&millis(t))),
            ]
        })
        .collect();
    for line in table(&cells) {
        say(&format!("  {line}"));
    }
    say("");
    let mark = if ok { green("✓") } else { red("✗") };
    say(&format!("  {mark} {}", bold(&sentence)));
    if intercepted {
        say(&format!("  {} {INTERCEPT_NOTE}", yellow("!")));
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(yours: bool, outcome: Outcome) -> Row {
        Row {
            resolver: Resolver {
                addr: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                who: "x".to_string(),
                yours,
            },
            outcome,
            took: None,
        }
    }

    fn a(ip: &str) -> Outcome {
        Outcome::Answers(vec![Record::A(ip.parse().unwrap())])
    }

    fn failed() -> Outcome {
        Outcome::Failed("no answer".to_string())
    }

    fn say_for(rows: &[Row]) -> (bool, String) {
        verdict("example.com", Kind::A, rows)
    }

    #[test]
    fn resolver_list_puts_yours_first_without_duplicates() {
        let system = ["192.168.1.1".parse().unwrap(), "1.1.1.1".parse().unwrap()];
        let list = resolvers(&system, Some(Ipv4Addr::new(192, 168, 1, 1)));
        let who: Vec<&str> = list.iter().map(|r| r.who.as_str()).collect();
        assert_eq!(
            who,
            ["your router", "your DNS (Cloudflare)", "Google", "Quad9"]
        );
        assert!(list[0].yours && list[1].yours && !list[2].yours);
    }

    #[test]
    fn outcome_keeps_only_the_type_asked_for() {
        let reply = Reply {
            code: 0,
            records: vec![
                Record::Cname("www.example.com".to_string()),
                Record::A(Ipv4Addr::new(2, 2, 2, 2)),
                Record::A(Ipv4Addr::new(1, 1, 1, 1)),
                Record::A(Ipv4Addr::new(2, 2, 2, 2)),
            ],
            truncated: false,
        };
        assert_eq!(
            outcome(Ok(reply.clone()), Kind::A),
            Outcome::Answers(vec![
                Record::A(Ipv4Addr::new(1, 1, 1, 1)),
                Record::A(Ipv4Addr::new(2, 2, 2, 2)),
            ])
        );
        assert_eq!(outcome(Ok(reply), Kind::Txt), Outcome::Empty);
    }

    #[test]
    fn outcome_maps_server_codes() {
        let coded = |code| {
            outcome(
                Ok(Reply {
                    code,
                    records: Vec::new(),
                    truncated: false,
                }),
                Kind::A,
            )
        };
        assert_eq!(coded(3), Outcome::NoSuchName);
        assert!(matches!(coded(2), Outcome::Failed(_)));
        assert!(matches!(coded(5), Outcome::Failed(_)));
        assert_eq!(outcome(Err("no answer".into()), Kind::A), failed());
    }

    #[test]
    fn verdict_all_agree() {
        let rows = [row(true, a("1.2.3.4")), row(false, a("1.2.3.4"))];
        assert_eq!(say_for(&rows), (true, "All resolvers agree.".to_string()));
    }

    #[test]
    fn verdict_differing_answers_are_normal() {
        let rows = [row(true, a("1.2.3.4")), row(false, a("5.6.7.8"))];
        let (ok, sentence) = say_for(&rows);
        assert!(ok);
        assert!(sentence.contains("different answers"));
    }

    #[test]
    fn verdict_spots_a_blocker() {
        let rows = [row(true, a("0.0.0.0")), row(false, a("1.2.3.4"))];
        let (ok, sentence) = say_for(&rows);
        assert!(!ok);
        assert!(sentence.contains("blocking example.com"));
    }

    #[test]
    fn verdict_spots_a_dead_local_resolver() {
        let rows = [row(true, failed()), row(false, a("1.2.3.4"))];
        let (ok, sentence) = say_for(&rows);
        assert!(!ok);
        assert!(sentence.contains("isn't answering"));
    }

    #[test]
    fn verdict_spots_filtering_by_missing_name() {
        let rows = [row(true, Outcome::NoSuchName), row(false, a("1.2.3.4"))];
        assert!(say_for(&rows).1.contains("filtering"));
    }

    #[test]
    fn verdict_private_names_are_fine() {
        let rows = [row(true, a("192.168.1.9")), row(false, Outcome::NoSuchName)];
        let (ok, sentence) = say_for(&rows);
        assert!(ok);
        assert!(sentence.contains("private name"));
    }

    #[test]
    fn verdict_blocked_outside_dns() {
        let rows = [row(true, a("1.2.3.4")), row(false, failed())];
        let (ok, sentence) = say_for(&rows);
        assert!(ok);
        assert!(sentence.contains("blocks outside DNS"));
    }

    #[test]
    fn verdict_nothing_anywhere() {
        let offline = [row(true, failed()), row(false, failed())];
        assert!(say_for(&offline).1.contains("No resolver answered"));

        let typo = [
            row(true, Outcome::NoSuchName),
            row(false, Outcome::NoSuchName),
        ];
        assert!(say_for(&typo).1.contains("doesn't exist"));

        let no_records = [row(true, Outcome::Empty), row(false, Outcome::Empty)];
        assert!(say_for(&no_records).1.contains("no A records"));
    }

    #[test]
    fn question_defaults_to_a_and_accepts_urls() {
        assert_eq!(
            question("https://example.com/path", None),
            Ok(("example.com".to_string(), Kind::A))
        );
        assert_eq!(
            question("example.com", Some("MX")),
            Ok(("example.com".to_string(), Kind::Mx))
        );
        assert!(question("example.com", Some("bogus")).is_err());
    }

    #[test]
    fn question_rejects_names_it_cannot_encode() {
        assert!(
            question("b\u{fc}cher.de", None)
                .unwrap_err()
                .contains("xn--")
        );
    }

    #[test]
    fn verdict_reports_a_block_even_when_nobody_else_answers() {
        let rows = [row(true, a("0.0.0.0")), row(false, failed())];
        let (ok, sentence) = say_for(&rows);
        assert!(!ok);
        assert!(sentence.contains("blocking example.com"));
    }

    #[test]
    fn cut_off_replies_are_not_called_empty() {
        let reply = Reply {
            code: 0,
            records: Vec::new(),
            truncated: true,
        };
        assert!(matches!(outcome(Ok(reply), Kind::Txt), Outcome::Failed(_)));
    }

    #[test]
    fn question_reverses_addresses() {
        assert_eq!(
            question("8.8.8.8", None),
            Ok(("8.8.8.8.in-addr.arpa".to_string(), Kind::Ptr))
        );
        assert!(question("8.8.8.8", Some("mx")).is_err());
    }
}
