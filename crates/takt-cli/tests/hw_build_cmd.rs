//! Der Hardware-Build linkt kein Plant-Modell und traegt das gewaehlte
//! Parameterprofil (8.3, 8.4, 11.3; FB-414, FB-418): Auf dem Ziel liest das
//! Programm den Sensor, nicht das Modell, und `--params-profile` wirkt im
//! erzeugten Code wie im Interpreter.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PLANT: &str = "\
system:
    language = 1
    tick     = 10 ms

input  p     : float[bar] in 0..10 bar @ hw(\"daq1/ai0\") with max_age = 50 ms
output p_sim : float[bar] in 0..10 bar @ sim(\"daq1/ai0\")
output v     : bool                    @ hw(\"do1/0\") with safe = false
output g     : int in 0..10            @ hw(\"do1/1\") with safe = 0

param GAIN : int in 0..10 = 1

profile HIGH:
    GAIN = 7

machine model:
    initial FEED

    state FEED:
        loop:
            p_sim = 3 bar

machine ctrl:
    initial IDLE

    state IDLE:
        loop:
            v = p.valid and p > 2 bar
            g = GAIN
";

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).args(args).output().expect("takt startet")
}

fn host_triple() -> &'static str {
    if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" }
}

/// Baut die Lieferform fuer `build` in ein eigenes Verzeichnis `tag` (die
/// Tests laufen parallel) und liefert es.
fn deliver(tag: &str, build: &str, extra: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("hw_build_cmd_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let src = dir.join("plant.takt");
    std::fs::write(&src, PLANT).expect("Programm");
    let out = dir.join("out");
    let (src, out_s) = (src.to_str().expect("Pfad"), out.to_str().expect("Pfad"));
    let mut args = vec!["build", src, "--emit", "embed", "--target", host_triple(), "--form", "own"];
    args.extend(["--build", build, "--out", out_s]);
    args.extend(extra);
    let run = takt(&args);
    assert!(run.status.success(), "{build}: {}", String::from_utf8_lossy(&run.stderr));
    out
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn the_hardware_build_reads_the_sensor_and_links_no_model() {
    let hw = deliver("sensor_hw", "hw", &[]);
    assert!(!read(&hw, "plant.ll").contains("model"), "das Modell ist im HW-Build gelinkt");
    assert!(!read(&hw, "plant_frame.c").contains("plant_model_"), "der Rahmen ruft das Modell");
    let manifest = read(&hw, "plant.manifest");
    assert!(manifest.contains("plant_in_daq1_ai0"), "kein Treiber fuer den Sensor:\n{manifest}");

    // Im Sim-Build speist das Modell den Eingang, und es laeuft mit.
    let sim = deliver("sensor_sim", "sim", &[]);
    assert!(read(&sim, "plant_frame.c").contains("plant_model_step"), "das Modell fehlt im Sim-Build");
    assert!(!read(&sim, "plant.manifest").contains("plant_in_daq1_ai0"), "der Sim-Build hat einen Sensortreiber");
}

#[test]
#[cfg_attr(not(target_arch = "x86_64"), ignore = "das Tripel des Wirts ist hier x86-64")]
fn the_chosen_profile_is_built_in_and_named() {
    let hw = deliver("profile_high", "hw", &["--params-profile", "HIGH"]);
    assert!(read(&hw, "plant_frame.c").contains("= 7; /* GAIN */"), "der Rahmen setzt den Default statt des Profils");
    assert!(read(&hw, "plant.manifest").contains("params_profile = HIGH"));

    let plain = deliver("profile_none", "hw", &[]);
    assert!(read(&plain, "plant_frame.c").contains("= 1; /* GAIN */"));
    assert!(read(&plain, "plant.manifest").contains("params_profile = none"));
}

#[test]
fn an_unknown_profile_is_refused() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hw_build_cmd_unknown");
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let src = dir.join("plant.takt");
    std::fs::write(&src, PLANT).expect("Programm");
    let run = takt(&["check", src.to_str().expect("Pfad"), "--params-profile", "LOW"]);
    assert!(!run.status.success());
    let text = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(text.contains("SC-2") && text.contains("`LOW`") && text.contains("HIGH"), "{text}");
}
