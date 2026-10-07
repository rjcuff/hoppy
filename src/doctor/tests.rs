use super::*;
use crate::net::tests::iface;

fn target(host: &str, port: u16) -> Target {
    Target {
        host: host.to_string(),
        port,
    }
}

fn healthy() -> Findings {
    Findings {
        link: Link::Ok,
        router: Router::Answered,
        internet: true,
        dns: true,
        target: None,
    }
}

#[test]
fn parses_bare_host_with_default_port() {
    assert_eq!(parse_target("example.com"), Ok(target("example.com", 443)));
    assert_eq!(parse_target("  nas.local "), Ok(target("nas.local", 443)));
}

#[test]
fn parses_host_and_port() {
    assert_eq!(parse_target("nas.local:445"), Ok(target("nas.local", 445)));
    assert_eq!(
        parse_target("localhost:3000"),
        Ok(target("localhost", 3000))
    );
}

#[test]
fn parses_urls() {
    assert_eq!(
        parse_target("https://example.com/path?q=1#frag"),
        Ok(target("example.com", 443))
    );
    assert_eq!(
        parse_target("http://example.com"),
        Ok(target("example.com", 80))
    );
    assert_eq!(
        parse_target("http://localhost:8080/api"),
        Ok(target("localhost", 8080))
    );
    assert_eq!(
        parse_target("HTTPS://user:pw@example.com:8443/"),
        Ok(target("example.com", 8443))
    );
    assert_eq!(parse_target("ssh://box.lan"), Ok(target("box.lan", 22)));
}

#[test]
fn parses_bare_ips() {
    assert_eq!(parse_target("192.168.1.1"), Ok(target("192.168.1.1", 443)));
    assert_eq!(
        parse_target("192.168.1.1:80"),
        Ok(target("192.168.1.1", 80))
    );
    assert_eq!(parse_target("::1"), Ok(target("::1", 443)));
    assert_eq!(parse_target("fe80::1"), Ok(target("fe80::1", 443)));
}

#[test]
fn parses_bracketed_ipv6() {
    assert_eq!(parse_target("[::1]:8080"), Ok(target("::1", 8080)));
    assert_eq!(
        parse_target("[2606:4700::1111]"),
        Ok(target("2606:4700::1111", 443))
    );
    assert_eq!(parse_target("http://[::1]/x"), Ok(target("::1", 80)));
}

#[test]
fn rejects_bad_targets_with_a_hint() {
    for bad in [
        "",
        "   ",
        "host:notaport",
        "host:0",
        "host:70000",
        "[::1",
        "https://",
        ":443",
    ] {
        let err = parse_target(bad).unwrap_err();
        assert!(err.contains("Try a host"), "no hint for {bad:?}: {err}");
    }
}

#[test]
fn rejects_ipv6_zone_ids_clearly() {
    for zoned in ["fe80::1%eth0", "[fe80::1%25en0]:80"] {
        let err = parse_target(zoned).unwrap_err();
        assert!(err.contains("zone"), "{zoned}: {err}");
    }
}

#[test]
fn no_gateway_blames_a_physical_adapter_first() {
    let mut docker = iface("docker0", &["172.17.0.1/16"], None);
    docker.kind = Kind::Docker;
    let live = [docker, iface("eth0", &["10.10.0.5/24"], None)];
    assert_eq!(check_link(&live).0, Link::NoGateway("eth0".to_string()));
}

#[test]
fn undetected_gateway_is_healthy_when_internet_works() {
    let f = Findings {
        link: Link::NoGateway("eth0".to_string()),
        router: Router::Missing,
        ..healthy()
    };
    assert!(f.healthy());
    assert!(verdict(&f, None).contains("Everything works"));
}

#[test]
fn target_label_brackets_ipv6() {
    assert_eq!(target("example.com", 443).label(), "example.com:443");
    assert_eq!(target("::1", 8080).label(), "[::1]:8080");
}

#[test]
fn refused_connection_counts_as_alive() {
    let t = Duration::from_millis(3);
    let refused = Err(io::Error::from(io::ErrorKind::ConnectionRefused));
    assert_eq!(classify(&refused, t), Probe::Refused(t));
    assert_eq!(classify(&refused, t).alive(), Some(t));
    assert_eq!(classify(&Ok(()), t), Probe::Open(t));
}

#[test]
fn timeouts_and_unreachable_are_silent() {
    let t = Duration::from_secs(2);
    for kind in [
        io::ErrorKind::TimedOut,
        io::ErrorKind::HostUnreachable,
        io::ErrorKind::NetworkUnreachable,
    ] {
        let result = Err(io::Error::from(kind));
        assert_eq!(classify(&result, t), Probe::Silent);
        assert_eq!(classify(&result, t).alive(), None);
    }
}

#[test]
fn link_ok_when_default_interface_has_real_ip() {
    let mut wifi = iface("Wi-Fi", &["192.168.1.20/24"], Some("192.168.1.1"));
    wifi.is_default = true;
    let live = [iface("Dante", &["169.254.3.4/16"], None), wifi];
    let (link, picked) = check_link(&live);
    assert_eq!(link, Link::Ok);
    assert_eq!(picked.unwrap().name, "Wi-Fi");
}

#[test]
fn link_self_assigned_when_only_169_254() {
    let live = [iface("Ethernet", &["169.254.3.4/16"], None)];
    assert_eq!(
        check_link(&live).0,
        Link::SelfAssigned("Ethernet".to_string())
    );
}

#[test]
fn link_no_gateway_and_disconnected() {
    let live = [iface("Dante", &["10.10.0.5/24"], None)];
    assert_eq!(check_link(&live).0, Link::NoGateway("Dante".to_string()));
    assert_eq!(check_link(&[]).0, Link::Disconnected);
}

#[test]
fn verdict_healthy() {
    let f = healthy();
    assert!(f.healthy());
    assert!(verdict(&f, None).contains("healthy"));

    let with_target = Findings {
        target: Some(TargetState::Open),
        ..healthy()
    };
    assert!(with_target.healthy());
    assert!(verdict(&with_target, Some("nas.local:445")).contains("nas.local:445"));
}

#[test]
fn verdict_silent_router_is_still_healthy() {
    let f = Findings {
        router: Router::Silent,
        ..healthy()
    };
    assert!(f.healthy());
}

#[test]
fn verdict_blames_upstream_when_router_up_but_internet_down() {
    let f = Findings {
        internet: false,
        dns: false,
        ..healthy()
    };
    assert!(!f.healthy());
    let v = verdict(&f, None);
    assert!(v.contains("router is up, but the internet isn't"));
    assert!(v.contains("Starlink"));
}

#[test]
fn verdict_mentions_router_when_it_was_silent_too() {
    let f = Findings {
        router: Router::Silent,
        internet: false,
        dns: false,
        ..healthy()
    };
    assert!(verdict(&f, None).contains("router didn't answer"));
}

#[test]
fn verdict_dns() {
    let f = Findings {
        dns: false,
        ..healthy()
    };
    assert!(!f.healthy());
    assert!(verdict(&f, None).contains("Internet works but DNS doesn't. Try 1.1.1.1"));
}

#[test]
fn verdict_target_down() {
    let f = Findings {
        target: Some(TargetState::Down),
        ..healthy()
    };
    assert!(!f.healthy());
    assert!(
        verdict(&f, Some("example.com:81"))
            .contains("Your network is fine; the target itself is down or the port is wrong")
    );
}

#[test]
fn verdict_target_refused_is_not_healthy() {
    let f = Findings {
        target: Some(TargetState::Refused),
        ..healthy()
    };
    assert!(!f.healthy());
    let v = verdict(&f, Some("localhost:3000"));
    assert!(v.contains("host is up"));
    assert!(v.contains("nothing is listening at localhost:3000"));
}

#[test]
fn verdict_link_problems_come_first() {
    let base = Findings {
        router: Router::Missing,
        internet: false,
        dns: false,
        ..healthy()
    };
    let self_assigned = Findings {
        link: Link::SelfAssigned("Ethernet".to_string()),
        ..base.clone()
    };
    assert!(verdict(&self_assigned, None).contains("169.254"));

    let unplugged = Findings {
        link: Link::Disconnected,
        ..base.clone()
    };
    assert!(verdict(&unplugged, None).contains("not connected"));

    let no_gateway = Findings {
        link: Link::NoGateway("Dante".to_string()),
        ..base
    };
    assert!(!no_gateway.healthy());
    assert!(verdict(&no_gateway, None).contains("no gateway"));
}

#[test]
fn millis_never_prints_zero() {
    assert_eq!(millis(Duration::from_micros(300)), "<1 ms");
    assert_eq!(millis(Duration::from_millis(14)), "14 ms");
}

#[test]
fn json_milliseconds_keep_one_decimal() {
    assert_eq!(ms(Duration::from_micros(1250)), 1.3);
    assert_eq!(ms(Duration::from_millis(14)), 14.0);
}
