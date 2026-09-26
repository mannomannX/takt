//! Das eingebaute Geraet `sys` (12.7, 7.4) in Pruefung 60, die
//! vordefinierten `EfuseBlock` und `EfuseCmd`, und das Profil-Enum aus
//! `system: target` (12.8).

use takt_diag::Policy;
use takt_mir::program::RuntimeProfile;
use takt_mir::{Program, hardware};
use takt_sema::{Build, Options};

fn compile(src: &str) -> Result<Program, Vec<String>> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if errors.is_empty() { Ok(out.program.expect("Programm")) } else { Err(errors) }
}

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

/// Die Warnungen von Pruefung 60, ohne und mit Konfiguration.
fn deep_sleep_warnings(src: &str, hw: Option<&str>) -> Vec<String> {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&format!("{HEAD}{src}"), &options);
    let program = out.program.expect("Programm");
    let mut diags = out.diagnostics;
    if let Some(hw) = hw {
        diags.extend(takt_sema::calibrated::check_bindings(&program, &hardware::parse(hw).expect("Konfiguration")));
    }
    diags
        .iter()
        .filter(|d| d.code == "SC-60" && format!("{d}").contains("DEEP_SLEEP"))
        .map(|d| format!("{d}"))
        .collect()
}

/// Ein Programm, das mit `command` in den Tiefschlaf geht, und dazu `extra`.
fn sleeper(command: &str, extra: &str) -> String {
    format!(
        "output reboot : RebootCmd @ hw(\"sys/reboot\") with safe = NONE\n{extra}\
         machine m:\n    initial RUN\n    state RUN:\n        after 5 ms: -> OFF\n    \
         state OFF:\n        enter:\n            reboot = {command}\n"
    )
}

const ALL: &str = "
input  boot_reason     : BootReason @ hw(\"sys/boot_reason\")
input  image_state     : ImageState @ hw(\"sys/image_state\")
input  reset_count     : int        @ hw(\"sys/reset_count\")
output image_confirm   : bool       @ hw(\"sys/image_confirm\") with safe = false
output reboot          : RebootCmd  @ hw(\"sys/reboot\")        with safe = NONE
input  efuse           : EfuseBlock @ hw(\"sys/efuse\")
input  image_confirmed : [2] bool   @ hw(\"sys/image_confirmed\")
output boot_jump       : u8         @ hw(\"sys/jump\")          with safe = 0
output efuse_burn      : EfuseCmd   @ hw(\"sys/efuse_burn\")    with safe = NONE
input  wall_time       : Duration   @ hw(\"sys/clock\")
output boot_sim        : BootReason @ sim(\"sys/boot_reason\")
output version         : u32        @ hw(\"o/version\")         with safe = 0
machine m:
    initial RUN
    state RUN:
        loop:
            version = efuse.min_version
            image_confirm = efuse.secure_boot and image_confirmed[0]
";

#[test]
fn every_system_channel_type_checks_without_a_configuration() {
    let p = compile(&format!("{HEAD}{ALL}")).expect("uebersetzt");
    assert_eq!(p.channels.len(), 12);
    // Pruefung 60 mit einer Konfiguration, die `sys` nicht fuehrt: kein Befund.
    let hw =
        hardware::parse("# takt-hw 3\n[channel o/version]\ndirection = output\nraw = u32\n").expect("Konfiguration");
    let diags = takt_sema::calibrated::check_bindings(&p, &hw);
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:?}");
}

#[test]
fn a_wrong_address_direction_or_type_is_rejected() {
    for (line, want) in [
        ("input  x : ImageState @ hw(\"sys/image_stat\")", "kennt `sys/image_stat` nicht"),
        ("output x : BootReason @ hw(\"sys/boot_reason\") with safe = POWER_ON", "ist am Geraet `sys` ein Input"),
        ("input  x : int @ hw(\"sys/boot_reason\")", "verlangt `BootReason`"),
        ("input  x : [3] bool @ hw(\"sys/image_confirmed\")", "verlangt `[2] bool`"),
        ("output x : u16 @ hw(\"sys/jump\") with safe = 0", "verlangt `u8`"),
        ("output x : bool @ sim(\"sys/image_confirm\")", "ist am Geraet `sys` ein Output"),
    ] {
        let src =
            format!("{HEAD}{line}\nmachine m:\n    initial RUN\n    state RUN:\n        loop:\n            pass\n");
        let e = compile(&src).expect_err(line).join("\n");
        assert!(e.contains("SC-60") && e.contains(want), "{line}:\n{e}");
    }
}

#[test]
fn the_target_is_one_of_the_four_profiles() {
    let body = "machine m:\n    initial RUN\n    state RUN:\n        loop:\n            pass\n";
    for name in ["linux_rt", "baremetal", "rtos", "boot"] {
        let p =
            compile(&format!("system:\n    language = 1\n    tick = 1 ms\n    target = {name}\n\n{body}")).expect(name);
        assert_eq!(p.config.runtime_profile().map(RuntimeProfile::name), Some(name));
    }
    let e = compile(&format!("system:\n    language = 1\n    tick = 1 ms\n    target = x86_64\n\n{body}"))
        .expect_err("kein Profil");
    assert!(e.join("\n").contains("unbekanntes Laufzeitprofil `x86_64`"), "{e:?}");
}

/// **Pruefung 60 warnt vor einem Tiefschlaf, aus dem nichts weckt** (12.7).
///
/// `DEEP_SLEEP` schlaeft ohne Zeitgeber; ohne Wake-Quelle wacht das Geraet
/// nur durch einen Reset auf. Mit Konfiguration zaehlt nur eine Quelle, die
/// sie als `deep_wake` fuehrt. `DEEP_SLEEP_FOR` hat einen Zeitgeber.
#[test]
fn a_deep_sleep_without_a_wake_source_warns() {
    let button = "input  btn : bool @ hw(\"gpio/btn\") with wake = true\n";
    assert_eq!(deep_sleep_warnings(&sleeper("DEEP_SLEEP", ""), None).len(), 1);
    assert!(deep_sleep_warnings(&sleeper("DEEP_SLEEP", button), None).is_empty());
    assert!(deep_sleep_warnings(&sleeper("DEEP_SLEEP_FOR(duration = 10 s)", ""), None).is_empty());

    let config = |deep: bool| {
        format!("# takt-hw 8\n[channel gpio/btn]\ndirection = input\ndeep_wake = {deep}\n[channel ui/led]\n")
    };
    let only_idle = deep_sleep_warnings(&sleeper("DEEP_SLEEP", button), Some(&config(false)));
    assert!(only_idle.len() == 1 && only_idle[0].contains("Konfiguration"), "{only_idle:?}");
    assert!(deep_sleep_warnings(&sleeper("DEEP_SLEEP", button), Some(&config(true))).is_empty());
}
