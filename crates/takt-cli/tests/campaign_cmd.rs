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
    let out_dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-campaign-{}", std::process::id()));
    let dir = out_dir.to_str().expect("Pfad");
    let out = takt(&["campaign", PROGRAM, "gain_sweep", "--ticks", "12", "--out", dir]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "zwei Laeufe scheitern:\n{text}");
    assert!(text.starts_with("Lauf  GAIN  Wdh  Verdikt  Messwerte\n"), "{text}");
    assert!(text.contains("\n1     1     1    PASS     peak=10\n"), "{text}");
    assert!(text.contains("\n2     1     2    PASS     peak=10\n"), "{text}");
    assert!(text.contains("\n7     4     1    FAIL     peak=40\n"), "{text}");
    assert!(text.ends_with("8 Laeufe, 2 FAIL\nKampagne: FAIL (13.7)\n"), "{text}");

    let failed = out_dir.join("gain_sweep-008.trace");
    let head = std::fs::read_to_string(&failed).expect("Aufzeichnung");
    let version = format!("#! takt-aufzeichnung {}", takt_interp::record::RECORDING_VERSION);
    for line in [version.as_str(), "#! profil QUAL", "#! param GAIN 4", "#! param LIMIT 30"] {
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
    assert!(text.ends_with("abgebrochen nach Lauf 1 von 2 (stop_on fail)\nKampagne: FAIL (13.7)\n"), "{text}");
}

#[test]
fn a_missing_name_lists_the_campaigns() {
    let out = takt(&["campaign", PROGRAM, "--ticks", "1"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("vorhanden: gain_sweep, stop_first"));
}

/// 13.7: Das Urteil der Kampagne folgt 13.5 ueber alle Laeufe - FAIL, sobald
/// ein Lauf FAIL ist, PASS nur, wenn jeder PASS ist, sonst INCONCLUSIVE; nur
/// PASS endet mit Exit 0.
#[test]
fn the_campaign_verdict_follows_its_runs() {
    let inconclusive = takt(&["campaign", PROGRAM, "gain_sweep", "--ticks", "1"]);
    let text = String::from_utf8_lossy(&inconclusive.stdout);
    assert!(!inconclusive.status.success(), "Laeufe ohne Aussage bestanden:\n{text}");
    assert!(text.ends_with("8 Laeufe, 0 FAIL\nKampagne: INCONCLUSIVE (13.7)\n"), "{text}");

    let failing = takt(&["campaign", PROGRAM, "gain_sweep", "--ticks", "12"]);
    assert!(String::from_utf8_lossy(&failing.stdout).ends_with("Kampagne: FAIL (13.7)\n"));

    let dir =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-campaign-pass-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join("44_campaign.takt");
    let text = std::fs::read_to_string(root().join(PROGRAM)).expect("Programm");
    std::fs::write(&file, text.replace("sweep GAIN = 1..4 step 1", "sweep GAIN = 1..3 step 1")).expect("Programm");
    let passing = takt(&["campaign", file.to_str().expect("Pfad"), "gain_sweep", "--ticks", "12"]);
    let text = String::from_utf8_lossy(&passing.stdout);
    assert!(passing.status.success(), "{text}\n{}", String::from_utf8_lossy(&passing.stderr));
    assert!(text.ends_with("6 Laeufe, 0 FAIL\nKampagne: PASS (13.7)\n"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
