//! Die Hardware-Konfiguration (8.10) und die Pruefungen, die an ihr
//! haengen: 60 (Bindung), 39 (Speicher), 28 (Jitter).

use takt_diag::{Policy, Span};
use takt_mir::hardware::{self, Hardware};
use takt_mir::program::Direction;
use takt_sema::calibrated::{check, check_bindings};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> takt_mir::Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Hw, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn hw(text: &str) -> Hardware {
    hardware::parse(&format!("# takt-hw {}\n{text}", hardware::FORMAT_VERSION)).unwrap_or_else(|e| panic!("{e}"))
}

fn codes(diags: &[takt_diag::Diagnostic]) -> Vec<String> {
    diags.iter().map(|d| format!("{} {}", d.code, d.message)).collect()
}

const PROGRAM: &str = "
input  p   : float[bar] in 0..250 bar @ hw(\"daq1/ai0\") with max_age = 200 ms
output v   : bool                     @ hw(\"gpio/valve\") with safe = false
output pwm : float in 0..1            @ hw(\"pwm/ch0\") with safe = 0, jitter = 100 us

machine m:
    initial RUN
    state RUN:
        loop:
            v = p > 100 bar
            pwm = 0.5
";

const CONFIG: &str = "
[target.thumbv7em]
core_hz = 84000000
i32 = 11905
i64 = 35715
f32 = 11905
f64 = 1190500
mem = 23810
call = 47620
t_io = 120000
ram = 65536
flash = 262144

[device.daq1]
driver = \"ads131\"

[channel daq1/ai0]
direction = input
raw = i16
unit = bar
range = 0..250
device = daq1
port = \"SPI1 CS0\"
latency_ns = 50000

[channel gpio/valve]
direction = output
raw = bool
safe = false
port = \"PB0\"

[channel pwm/ch0]
direction = output
raw = float
safe = 0
port = \"TIM3 CH1\"
jitter_ns = 50000
";

#[test]
fn a_configuration_reads_back_with_devices_and_channels() {
    let cfg = hw(CONFIG);
    assert_eq!(cfg.format_version, hardware::FORMAT_VERSION);
    assert_eq!(cfg.target("thumbv7em").expect("Ziel").memory.ram, Some(65536));
    assert_eq!(cfg.devices["daq1"].driver.as_deref(), Some("ads131"));
    let ai0 = cfg.channel("daq1/ai0").expect("Kanal");
    assert_eq!(ai0.direction, Some(Direction::Input));
    assert_eq!(ai0.unit.as_deref(), Some("bar"));
    assert_eq!(ai0.range, Some((0.0, 250.0)));
    assert_eq!(ai0.port.as_deref(), Some("SPI1 CS0"));
    assert_eq!(cfg.channel("pwm/ch0").expect("Kanal").jitter_ns, Some(50_000));
}

#[test]
fn rendering_round_trips_the_whole_file() {
    let cfg = hw(CONFIG);
    let again = hardware::parse(&hardware::render(&cfg)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(cfg, again);
}

#[test]
fn an_older_file_still_reads() {
    // 11.3: Leser akzeptieren aeltere Versionen ihres Formats.
    let cfg = hardware::parse("# takt-hw 1\n[target.x]\ncore_hz = 1000\n").expect("Version 1");
    assert_eq!(cfg.format_version, 1);
    assert!(cfg.channels.is_empty());
}

#[test]
fn an_unknown_section_or_key_is_an_error() {
    assert!(hardware::parse("# takt-hw 3\n[gadget.x]\n").is_err());
    assert!(hardware::parse("# takt-hw 3\n[channel a/b]\ncolour = red\n").is_err());
    assert!(hardware::parse("# takt-hw 3\n[channel a/b]\ndirection = sideways\n").is_err());
}

#[test]
fn matching_bindings_pass_check_60() {
    let p = compile(PROGRAM);
    let diags = check_bindings(&p, &hw(CONFIG));
    let errors: Vec<String> = codes(&diags).into_iter().filter(|c| c.starts_with("SC-60")).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_address_the_configuration_does_not_know_is_an_error() {
    // 8.10: welche Channels eine Plattform anbietet, steht in ihrer
    // Konfiguration.
    let p = compile(PROGRAM);
    let cfg = hw(&CONFIG.replace("[channel gpio/valve]", "[channel gpio/other]"));
    let diags = check_bindings(&p, &cfg);
    assert!(codes(&diags).iter().any(|c| c.contains("SC-60") && c.contains("gpio/valve")), "{diags:?}");
}

#[test]
fn a_unit_mismatch_is_an_error() {
    // FB-48: `float[bar]` gebunden, `psi` in der Konfiguration.
    let p = compile(PROGRAM);
    let cfg = hw(&CONFIG.replace("unit = bar", "unit = psi"));
    let diags = check_bindings(&p, &cfg);
    assert!(codes(&diags).iter().any(|c| c.contains("SC-60") && c.contains("psi")), "{diags:?}");
}

#[test]
fn a_program_range_outside_the_device_range_is_an_error() {
    let p = compile(PROGRAM);
    let cfg = hw(&CONFIG.replace("range = 0..250", "range = 0..100"));
    let diags = check_bindings(&p, &cfg);
    assert!(codes(&diags).iter().any(|c| c.contains("SC-60") && c.contains("liefert")), "{diags:?}");
}

#[test]
fn a_direction_mismatch_is_an_error() {
    let p = compile(PROGRAM);
    let cfg =
        hw(&CONFIG.replace("[channel gpio/valve]\ndirection = output", "[channel gpio/valve]\ndirection = input"));
    let diags = check_bindings(&p, &cfg);
    assert!(codes(&diags).iter().any(|c| c.contains("SC-60") && c.contains("Output")), "{diags:?}");
}

#[test]
fn a_safe_value_mismatch_is_an_error() {
    // 12.4: der Treiber kennt denselben Safe-Wert wie das Programm.
    let p = compile(PROGRAM);
    let cfg = hw(&CONFIG.replace("safe = false", "safe = true"));
    let diags = check_bindings(&p, &cfg);
    assert!(codes(&diags).iter().any(|c| c.contains("SC-60") && c.contains("safe")), "{diags:?}");
}

#[test]
fn a_jitter_requirement_above_the_measurement_passes_and_below_fails() {
    // Pruefung 28: `jitter = 100 us` verlangt, 50 us gemessen — passt.
    let p = compile(PROGRAM);
    let ok = check_bindings(&p, &hw(CONFIG));
    assert!(!codes(&ok).iter().any(|c| c.contains("SC-28")), "{ok:?}");
    let cfg = hw(&CONFIG.replace("jitter_ns = 50000", "jitter_ns = 250000"));
    let bad = check_bindings(&p, &cfg);
    assert!(codes(&bad).iter().any(|c| c.contains("SC-28") && c.contains("250000")), "{bad:?}");
}

#[test]
fn a_tick_granular_output_adds_the_tick_to_its_jitter() {
    // 7.5: Schreibt die Runtime nur zu Tickbeginn, faellt `at T` auf den
    // Tick, der `T` enthaelt — 2 us gemessen plus 1 ms Tick sprengen die
    // verlangten 100 us, und `at` wirkt tick-granular.
    let granular = CONFIG.replace("jitter_ns = 50000", "jitter_ns = 2000\ntick_granular = true");
    let p = compile(PROGRAM);
    let diags = check_bindings(&p, &hw(&granular));
    assert!(codes(&diags).iter().any(|c| c.contains("SC-28") && c.contains("1002000")), "{diags:?}");
    let p = compile(&PROGRAM.replace("            pwm = 0.5", "            at now + 5 ms:\n                pwm = 0.5"));
    let diags = check_bindings(&p, &hw(&granular));
    assert!(codes(&diags).iter().any(|c| c.contains("SC-28") && c.contains("tick-granular")), "{diags:?}");
}

#[test]
fn jitter_is_read_at_an_output_only() {
    // 7.5: `o.jitter` liest die Bindung eines Outputs, nicht einen Wert.
    let src = format!("{HEAD}{}", PROGRAM.replace("v = p > 100 bar", "v = p.jitter > 1 us"));
    let options = Options { build: Build::Hw, ..Default::default() };
    let errors: Vec<String> = takt_sema::compile(&src, &options)
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.message.clone())
        .collect();
    assert!(errors.iter().any(|e| e.contains("`jitter` gibt es nur an einem Output")), "{errors:?}");
    compile(&PROGRAM.replace("v = p > 100 bar", "v = pwm.jitter > 1 us"));
}

#[test]
fn the_gate_names_what_it_could_not_decide() {
    // 8.10, M10 Schritt 13: Eine fehlende Zahl ist eine Meldung, keine
    // Annahme; was das Programm nicht hat, ist ohne Belang.
    use takt_sema::calibrated::{Gate, gate, polling};
    let p = compile(PROGRAM);
    let bare: Vec<(u32, Gate)> = gate(&p, None, None, &[]);
    let of = |rows: &[(u32, Gate)], n: u32| rows.iter().find(|(k, _)| *k == n).map(|(_, g)| g.clone()).expect("Zeile");
    assert!(matches!(of(&bare, 12), Gate::NotApplicable(_)), "{bare:?}");
    assert!(matches!(of(&bare, 28), Gate::Undecidable(ref m) if m.contains("pwm")), "{bare:?}");
    assert!(matches!(of(&bare, 32), Gate::Undecidable(ref m) if m.contains("Kalibrierung")), "{bare:?}");
    assert!(matches!(of(&bare, 60), Gate::Undecidable(ref m) if m.contains("Konfiguration")), "{bare:?}");

    let cfg = hw(&CONFIG.replace("jitter_ns = 50000", "jitter_ns = 250000"));
    let target = cfg.target("thumbv7em").expect("Ziel");
    let mut diags = check(&p, target, Span::default());
    diags.extend(check_bindings(&p, &cfg));
    diags.extend(polling(&p, &cfg, Some(target)));
    let judged = gate(&p, Some(target), Some(&cfg), &diags);
    assert!(matches!(of(&judged, 28), Gate::Violated(ref m) if m.contains("250000")), "{judged:?}");
    assert!(matches!(of(&judged, 60), Gate::Ok), "{judged:?}");
}

#[test]
fn the_memory_budget_is_judged_against_the_target() {
    // Pruefung 39: 64 KiB passen; 64 Byte nicht.
    let p = compile(PROGRAM);
    let cfg = hw(CONFIG);
    let ok = check(&p, cfg.target("thumbv7em").expect("Ziel"), Span::default());
    assert!(!codes(&ok).iter().any(|c| c.contains("SC-39")), "{ok:?}");
    let cfg = hw(&CONFIG.replace("ram = 65536", "ram = 64"));
    let bad = check(&p, cfg.target("thumbv7em").expect("Ziel"), Span::default());
    assert!(codes(&bad).iter().any(|c| c.contains("SC-39") && c.contains("RAM")), "{bad:?}");
}

#[test]
fn the_corpus_configuration_matches_the_heartbeat_program() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try");
    let text = std::fs::read_to_string(format!("{root}/hw/stm32f401.hw")).expect("hw lesbar");
    let cfg = hardware::parse(&text).unwrap_or_else(|e| panic!("{e}"));
    let src = std::fs::read_to_string(format!("{root}/29_heartbeat.takt")).expect("Programm lesbar");
    let options = Options { policy: Policy::default(), build: Build::Hw, profile: None, ..Default::default() };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    let diags = check_bindings(&p, &cfg);
    assert!(diags.iter().all(|d| !d.is_error()), "{:?}", codes(&diags));
    let diags = check(&p, cfg.target("thumbv7em").expect("Ziel"), Span::default());
    assert!(!codes(&diags).iter().any(|c| c.contains("SC-39")), "{:?}", codes(&diags));
}
