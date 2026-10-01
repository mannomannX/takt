//! `takt check --hw-export` (Referenz 12.4): Der Compiler exportiert die
//! `safe`-Werte der gebundenen Outputs in die Hardware-Konfiguration, damit
//! ein Geraet sie bei Heartbeat-Verlust selbst anwendet.

use std::path::PathBuf;
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

#[test]
fn the_safe_values_land_in_the_configuration() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-hw-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let config = dir.join("site.hw");
    // Die Datei gehoert einem Menschen: Ihr Kommentar bleibt stehen.
    std::fs::write(&config, "# takt-hw 9\n# Von Hand.\n[channel ui/led]\nport = \"PC13\"   # die LED\n")
        .expect("schreiben");
    let out = takt(&["check", "corpus-try/33_enum_param.takt", "--hw-export", config.to_str().expect("Pfad")]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&config).expect("lesen");
    assert!(text.contains("# Von Hand.\n"), "{text}");
    assert!(
        text.contains("[channel ui/led]\nport = \"PC13\"   # die LED\ndirection = output\nsafe = false\n"),
        "{text}"
    );
    assert!(text.contains("[channel o/mode]\ndirection = output\nsafe = SLOW\n"), "{text}");
    let hw = takt_mir::hardware::parse(&text).expect("lesbar");
    assert_eq!(hw.channel("o/mode").and_then(|c| c.safe.as_deref()), Some("SLOW"));
    let _ = std::fs::remove_dir_all(&dir);
}
