//! Headless scenario tests: run the real binary and compare state hashes across processes.

use std::path::PathBuf;
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_sim-headless"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sim-headless {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn hash_of(output: &str) -> String {
    output
        .split_whitespace()
        .find_map(|w| w.strip_prefix("hash="))
        .expect("output has a hash")
        .to_string()
}

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

const SCENARIO: &[&str] = &[
    "run", "--seed", "42", "--ticks", "1000", "--agents", "300", "--spawn", "100:50", "--spawn",
    "500:25",
];

/// The Phase 0 "done when": two headless runs with the same seed produce the same hash.
#[test]
fn two_processes_same_seed_same_hash() {
    let a = run(SCENARIO);
    let b = run(SCENARIO);
    assert_eq!(hash_of(&a), hash_of(&b));
    assert!(a.contains("agents=375"), "{a}");
}

#[test]
fn different_seed_different_hash() {
    let a = run(&["run", "--seed", "1", "--ticks", "100"]);
    let b = run(&["run", "--seed", "2", "--ticks", "100"]);
    assert_ne!(hash_of(&a), hash_of(&b));
}

#[test]
fn thread_count_does_not_change_hash() {
    let mut one = vec!["--threads", "1"];
    one.extend_from_slice(SCENARIO);
    let mut many = vec!["--threads", "8"];
    many.extend_from_slice(SCENARIO);
    assert_eq!(hash_of(&run(&one)), hash_of(&run(&many)));
}

#[test]
fn snapshot_and_log_reproduce_the_run() {
    let snap = tmp("scenario.frmc");
    let log = tmp("scenario.frmclog");
    let mut args = SCENARIO[..SCENARIO.len()].to_vec();
    let (s, l) = (snap.to_str().unwrap(), log.to_str().unwrap());
    args.extend_from_slice(&["--save-snapshot", s, "--save-log", l]);
    let full = hash_of(&run(&args));

    assert_eq!(
        hash_of(&run(&["resume", s])),
        full,
        "snapshot load preserves the hash"
    );
    let replay = run(&["replay", l]);
    assert!(replay.contains("replay ok"), "{replay}");
    assert_eq!(hash_of(&replay), full);

    // Resuming from a snapshot and running on matches one long run.
    let longer = hash_of(&run(&[
        "run", "--seed", "42", "--ticks", "1200", "--agents", "300", "--spawn", "100:50",
        "--spawn", "500:25",
    ]));
    assert_eq!(hash_of(&run(&["resume", s, "--ticks", "200"])), longer);
}

/// Pins the scenario's exact outcome. If this fails after an *intentional* change to simulation
/// behaviour, RNG streams or serialized state, update the constant (and bump the save format
/// version if serialized state changed). If it fails unexpectedly, determinism broke.
#[test]
fn scenario_hash_is_pinned() {
    const EXPECTED: &str = "878fa6a2a1d0c705";
    let got = hash_of(&run(SCENARIO));
    assert_eq!(got, EXPECTED, "Phase 0 scenario hash changed");
}
