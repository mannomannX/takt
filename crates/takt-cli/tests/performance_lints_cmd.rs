//! `takt check --target` reicht den Kern des Ziels an die Pruefungen 40 und
//! 41 weiter (12.8, FB-384): Ein Programm ohne `system: target` bekommt die
//! Hinweise fuer einen schmalen Kern, wenn der Bau ihn nennt.

use std::process::Command;

const SRC: &str = "\
system:
    language = 1
    tick     = 10 ms

output total : int @ sim(\"t\")

machine m:
    var acc : int = 0
    initial RUN
    state RUN:
        loop:
            for i in range(4):
                acc = acc + i
            total = acc
";

fn check(target: Option<&str>) -> String {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("performance_lints_cmd");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let file = dir.join("lints.takt");
    std::fs::write(&file, SRC).expect("Programm");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_takt"));
    cmd.arg("check");
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    let out = cmd.arg(&file).output().expect("takt startet");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn a_bare_metal_target_brings_both_lints() {
    let text = check(Some("thumbv7em"));
    assert!(text.contains("SC-40") && text.contains("SC-41"), "{text}");
}

#[test]
fn a_64_bit_target_and_no_target_bring_none() {
    for target in [Some("x86_64"), None] {
        let text = check(target);
        assert!(!text.contains("SC-40") && !text.contains("SC-41"), "{target:?}: {text}");
    }
}
