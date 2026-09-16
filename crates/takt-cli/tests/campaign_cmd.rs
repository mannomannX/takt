//! `takt campaign` (13.7): Tabelle, Aufzeichnung je Lauf, `stop_on`,
//! Wiedergabe eines Laufs aus seiner Aufzeichnung.

use std::path::PathBuf;
use std::process::{Command, Output};

const PROGRAM: &str = "corpus-try/44_campaign.takt";

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn every_run_lands_in_the_table_and_replays_from_its_recording() {
    let out_dir = std::env::temp_dir().join(format!("takt-campaign-{}", std::process::id()));
    let dir = out_dir.to_str().expect("Pfad");
    let out = takt(&["campaign", PROGRAM, "gain_sweep", "--ticks", "12", "--out", dir]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "zwei Laeufe scheitern:\n{text}");
    assert!(text.starts_with("Lauf  GAIN  Wdh  Verdikt  Messwerte\n"), "{text}");
    assert!(text.contains("\n1     1     1    PASS     peak=10\n"), "{text}");
    assert!(text.contains("\n2     1     2    PASS     peak=10\n"), "{text}");
    assert!(text.contains("\n7     4     1    FAIL     peak=40\n"), "{text}");
    assert!(text.ends_with("8 Laeufe, 2 FAIL\n"), "{text}");

    let failed = out_dir.join("gain_sweep-008.trace");
    let head = std::fs::read_to_string(&failed).expect("Aufzeichnung");
    for line in ["#! takt-aufzeichnung 2", "#! profil QUAL", "#! param GAIN 4", "#! param LIMIT 30"] {
        assert!(head.contains(line), "{line} fehlt:\n{head}");
    }
    let replay = takt(&["replay", PROGRAM, "--record", failed.to_str().expect("Pfad")]);
    assert!(!replay.status.success(), "{}", String::from_utf8_lossy(&replay.stderr));
    assert!(String::from_utf8_lossy(&replay.stdout).contains("verdict-final FAIL"));
    let passed = out_dir.join("gain_sweep-001.trace");
    let replay = takt(&["replay", PROGRAM, "--record", passed.to_str().expect("Pfad")]);
    assert!(replay.status.success(), "{}", String::from_utf8_lossy(&replay.stderr));
    std::fs::remove_dir_all(&out_dir).expect("aufraeumen");
}

#[test]
fn stop_on_fail_ends_the_campaign_after_the_first_failure() {
    let out = takt(&["campaign", PROGRAM, "stop_first", "--ticks", "12"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(text.contains("\n1     4     1    FAIL     peak=40\n"), "{text}");
    assert!(text.ends_with("abgebrochen nach Lauf 1 von 2 (stop_on fail)\n"), "{text}");
}

#[test]
fn a_missing_name_lists_the_campaigns() {
    let out = takt(&["campaign", PROGRAM, "--ticks", "1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("vorhanden: gain_sweep, stop_first"));
}
