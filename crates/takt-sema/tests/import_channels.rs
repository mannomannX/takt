//! `import channels from "…"` (Referenz 8.2): typisierte Channels aus der
//! Hardware-Konfiguration; gebunden wird nur, was die Quelle nennt.

use takt_diag::Policy;
use takt_mir::program::{Binding, Direction};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\nimport channels from \"site1.hw\"\n\n";

const SITE: &str = "# takt-hw 9
[channel daq1/ai0]
direction = input
raw = i16
unit = bar
range = 0..250

[channel gpio/valve]
direction = output
raw = bool
safe = false

[channel pwm/ch0]
direction = output
raw = float
safe = 0
";

fn compile(body: &str, site: Option<&str>) -> (Option<takt_mir::Program>, Vec<String>) {
    let mut options = Options { policy: Policy::default(), build: Build::Hw, ..Default::default() };
    if let Some(text) = site {
        options.channel_imports.insert("site1.hw".to_string(), text.to_string());
    }
    let out = takt_sema::compile(&format!("{HEAD}{body}"), &options);
    let errors = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    (out.program, errors)
}

const MACHINE: &str = "machine m:
    initial RUN
    state RUN:
        loop:
            gpio_valve = daq1_ai0 > 100 bar
";

#[test]
fn a_named_channel_is_bound_with_the_type_of_the_configuration() {
    let (p, errors) = compile(MACHINE, Some(SITE));
    assert!(errors.is_empty(), "{errors:?}");
    let p = p.expect("Programm");
    let names: Vec<(&str, Direction)> = p.channels.iter().map(|c| (c.name.as_str(), c.dir)).collect();
    // `pwm_ch0` nennt die Quelle nicht: nicht gebunden (8.2).
    assert_eq!(names, [("daq1_ai0", Direction::Input), ("gpio_valve", Direction::Output)]);
    let ai0 = &p.channels[0];
    assert!(matches!(&ai0.binding, Binding::Hw(a) if a.text() == "daq1/ai0"));
    let ty = &p.types.list[ai0.ty.index()];
    assert!(matches!(ty, takt_mir::types::Type::Float { unit: Some(_), range: Some(_), .. }), "{ty:?}");
    assert!(p.channels[1].attrs.safe.is_some(), "der `safe`-Wert kommt aus der Konfiguration");
}

#[test]
fn a_written_declaration_of_the_same_address_wins() {
    let body = format!("input p : float[bar] in 0..250 bar @ hw(\"daq1/ai0\")\n\n{}", MACHINE.replace("daq1_ai0", "p"));
    let (p, errors) = compile(&body, Some(SITE));
    assert!(errors.is_empty(), "{errors:?}");
    let names: Vec<String> = p.expect("Programm").channels.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["p", "gpio_valve"]);
}

#[test]
fn an_unread_configuration_is_an_error_at_the_import() {
    let (_, errors) = compile(MACHINE, None);
    assert!(errors.iter().any(|e| e.contains("SC-2") && e.contains("`site1.hw` nicht gelesen")), "{errors:?}");
}

#[test]
fn a_channel_the_program_cannot_declare_is_named_with_its_address() {
    let (_, errors) = compile(MACHINE, Some(&SITE.replace("raw = bool\nsafe = false\n", "raw = bool\n")));
    assert!(errors.iter().any(|e| e.contains("Kanal `gpio/valve`") && e.contains("safe")), "{errors:?}");
    let (_, errors) = compile(MACHINE, Some(&SITE.replace("unit = bar", "unit = furlong")));
    assert!(errors.iter().any(|e| e.contains("Kanal `daq1/ai0`") && e.contains("furlong")), "{errors:?}");
}
