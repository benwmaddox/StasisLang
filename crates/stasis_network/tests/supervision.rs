use std::time::Duration;

use stasis_network::supervision::{parse_args, PEER_PROTOCOL_LINE};

#[test]
fn consumer_contract_is_explicit_and_bounded() {
    let options = parse_args(
        [
            "--authority",
            "authority.exe",
            "--peer",
            "peer.exe",
            "--startup-ms",
            "30000",
            "--action-ms",
            "45000",
            "--shutdown-ms",
            "10000",
            "--",
            "fixture-argument",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("valid supervisor contract");
    assert_eq!(PEER_PROTOCOL_LINE, "stasis-network-supervision-v1");
    assert_eq!(options.startup, Duration::from_secs(30));
    assert_eq!(options.action, Duration::from_secs(45));
    assert_eq!(options.shutdown, Duration::from_secs(10));
    assert_eq!(options.peer_args, ["fixture-argument"]);
}

#[test]
fn every_lifecycle_bound_is_required() {
    let base: Vec<String> = [
        "--authority",
        "authority.exe",
        "--peer",
        "peer.exe",
        "--startup-ms",
        "1",
        "--action-ms",
        "1",
        "--shutdown-ms",
        "1",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();

    for option in ["--startup-ms", "--action-ms", "--shutdown-ms"] {
        for invalid in ["0", "900001", "18446744073709551616"] {
            let mut args = base.clone();
            let index = args.iter().position(|arg| arg == option).unwrap() + 1;
            args[index] = invalid.into();
            assert!(parse_args(args).is_err(), "accepted {option}={invalid}");
        }

        let mut missing = base.clone();
        let index = missing.iter().position(|arg| arg == option).unwrap();
        missing.drain(index..=index + 1);
        assert!(parse_args(missing).is_err(), "accepted missing {option}");
    }
}

#[test]
fn parser_diagnostics_never_repeat_untrusted_values() {
    let sentinel = "private-credential-sentinel";
    let unknown = parse_args(["--unknown", sentinel].into_iter().map(str::to_string)).unwrap_err();
    let missing = parse_args(["--authority"].into_iter().map(str::to_string)).unwrap_err();
    assert!(!unknown.contains(sentinel));
    assert!(!missing.contains("--authority"));
}
