//! Das eingebaute Geraet `sys` (12.7, 7.4) in Pruefung 60 — Anfang und Ende
//! eines Laufs und die Wanduhr — und das Profil-Enum aus `system: target`
//! (12.8).

use takt_diag::Policy;
use takt_mir::program::RuntimeProfile;
use takt_mir::{Program, hardware};
use takt_sema::{Build, Options};

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

/// Die Warnungen von Pruefung 60 zu `ON_WAKE`, ohne und mit Konfiguration.
fn wake_warnings(src: &str, hw: Option<&str>) -> Vec<String> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&format!("{HEAD}{src}"), &options);
    let program = out.program.expect("Programm");
    let mut diags = out.diagnostics;
    if let Some(hw) = hw {
        diags.extend(takt_sema::calibrated::check_bindings(&program, &hardware::parse(hw).expect("Konfiguration")));
    }
    diags.iter().filter(|d| d.code == "SC-60" && format!("{d}").contains("ON_WAKE")).map(|d| format!("{d}")).collect()
}

/// Ein Programm, das seinen Lauf mit `next` beendet, und dazu `extra`.
fn ending(next: &str, extra: &str) -> String {
    format!(
        "output next_run : NextRun @ hw(\"sys/next_run\") with safe = NONE\n{extra}\
         machine m:\n    initial RUN\n    state RUN:\n        after 5 ms: -> OFF\n    \
         state OFF:\n        enter:\n            next_run = {next}\n"
    )
}

const ALL: &str = "
input  previous_run : PreviousRun @ hw(\"sys/previous_run\")
output next_run     : NextRun     @ hw(\"sys/next_run\")     with safe = NONE
input  wall_time    : Duration    @ hw(\"sys/clock\")
output previous_sim : PreviousRun @ sim(\"sys/previous_run\")
output led          : bool        @ hw(\"o/led\")            with safe = false
machine m:
    initial RUN
    state RUN:
        loop:
            led = previous_run.or(NONE) == WATCHDOG
        after 5 ms: -> OFF
    state OFF:
        enter:
            next_run = AFTER(delay = 1 s)
";

#[test]
fn every_system_channel_type_checks_without_a_configuration() {
    let p = compile(&format!("{HEAD}{ALL}")).expect("uebersetzt");
    assert_eq!(p.channels.len(), 5);
    // Pruefung 60 mit einer Konfiguration, die `sys` nicht fuehrt: kein Befund.
    let hw = hardware::parse("# takt-hw 3\n[channel o/led]\ndirection = output\nraw = bool\n").expect("Konfiguration");
    let diags = takt_sema::calibrated::check_bindings(&p, &hw);
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:?}");
}

#[test]
fn a_wrong_address_direction_or_type_is_rejected() {
    for (line, want) in [
        ("input  x : PreviousRun @ hw(\"sys/previous_rnu\")", "kennt `sys/previous_rnu` nicht"),
        ("output x : PreviousRun @ hw(\"sys/previous_run\") with safe = NONE", "ist am Geraet `sys` ein Input"),
        ("input  x : int @ hw(\"sys/previous_run\")", "verlangt `PreviousRun`"),
        ("output x : PreviousRun @ hw(\"sys/next_run\") with safe = NONE", "verlangt `NextRun`"),
        ("output x : NextRun @ sim(\"sys/next_run\")", "ist am Geraet `sys` ein Output"),
    ] {
        let src =
            format!("{HEAD}{line}\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            pass\n");
        let e = compile(&src).expect_err(line).join("\n");
        assert!(e.contains("SC-60") && e.contains(want), "{line}:\n{e}");
    }
}

/// Was nur ein Programm braucht, das den Chip besitzt oder selbst
/// Startstufe ist, kennt das Geraet nicht mehr (FB-360).
#[test]
fn the_channels_of_a_boot_stage_are_gone() {
    for address in ["sys/boot_reason", "sys/reset_count", "sys/reboot", "sys/jump", "sys/efuse", "sys/image_state"] {
        let src = format!(
            "{HEAD}input  x : bool @ hw(\"{address}\")\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            pass\n"
        );
        let e = compile(&src).expect_err(address).join("\n");
        assert!(e.contains(&format!("kennt `{address}` nicht")), "{address}:\n{e}");
    }
}

#[test]
fn the_target_is_one_of_the_three_profiles() {
    let body = "machine m:\n    initial RUN\n    state RUN:\n        loop:\n            pass\n";
    for name in ["linux_rt", "baremetal", "rtos"] {
        let p =
            compile(&format!("system:\n    language = 1\n    tick = 1 ms\n    target = {name}\n\n{body}")).expect(name);
        assert_eq!(p.config.runtime_profile().map(RuntimeProfile::name), Some(name));
    }
    for name in ["x86_64", "boot"] {
        let e = compile(&format!("system:\n    language = 1\n    tick = 1 ms\n    target = {name}\n\n{body}"))
            .expect_err("kein Profil");
        assert!(e.join("\n").contains(&format!("unbekanntes Laufzeitprofil `{name}`")), "{e:?}");
    }
}

/// **Pruefung 60 warnt vor einem Ende, nach dem nichts weckt** (12.7).
///
/// Nach `ON_WAKE` beginnt der naechste Lauf, wenn eine Wake-Quelle weckt;
/// ohne eine beginnt er erst mit dem naechsten Start, und das sagt
/// `ON_START` ehrlicher. Mit Konfiguration zaehlt nur eine Quelle, die sie
/// als `deep_wake` fuehrt. `AFTER` hat einen Zeitgeber, `ON_START` will
/// nicht geweckt werden.
#[test]
fn an_end_on_wake_without_a_wake_source_warns() {
    let button = "input  btn : bool @ hw(\"gpio/btn\") with wake = true\n";
    let alone = wake_warnings(&ending("ON_WAKE", ""), None);
    assert!(alone.len() == 1 && alone[0].contains("ON_START"), "{alone:?}");
    assert!(wake_warnings(&ending("ON_WAKE", button), None).is_empty());
    assert!(wake_warnings(&ending("AFTER(delay = 10 s)", ""), None).is_empty());
    assert!(wake_warnings(&ending("ON_START", ""), None).is_empty());

    let config = |deep: bool| {
        format!("# takt-hw 8\n[channel gpio/btn]\ndirection = input\ndeep_wake = {deep}\n[channel ui/led]\n")
    };
    let only_idle = wake_warnings(&ending("ON_WAKE", button), Some(&config(false)));
    assert!(only_idle.len() == 1 && only_idle[0].contains("Konfiguration"), "{only_idle:?}");
    assert!(wake_warnings(&ending("ON_WAKE", button), Some(&config(true))).is_empty());
}
