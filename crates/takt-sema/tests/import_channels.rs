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

/// Was die Quelle nicht nennt, zeichnet die Runtime auf (8.2): jeden Input
/// mit der Art seines Werts, keinen Output und keinen, aus dem sich kein Typ
/// bilden laesst. Die Logik bleibt dieselbe — ein Kanal mehr in der
/// Konfiguration aendert den Logik-Hash nicht, wohl aber den Programm-Hash.
#[test]
fn an_unread_input_is_recorded_without_changing_the_logic() {
    use takt_mir::program::RecordedValue;
    use takt_mir::types::{FloatWidth, IntWidth};
    let extra = "
[channel daq1/ai3]
direction = input
raw = i16
unit = bar
range = 0..250

[channel daq1/di0]
direction = input
raw = bool

[channel daq1/mode]
direction = input
raw = Mode

[channel daq1/count]
direction = input
raw = u32

[channel daq1/odd]
direction = input
raw = Nothing
";
    let body = format!("enum Mode: IDLE, RUN\n\n{MACHINE}");
    let (base, errors) = compile(&body, Some(SITE));
    assert!(errors.is_empty(), "{errors:?}");
    let (more, errors) = compile(&body, Some(&format!("{SITE}{extra}")));
    assert!(errors.is_empty(), "{errors:?}");
    let (base, more) = (base.expect("Programm"), more.expect("Programm"));
    assert!(base.recorded.is_empty(), "`pwm/ch0` ist ein Output: {:?}", base.recorded);
    let mode = takt_mir::EnumId(more.enums.iter().position(|e| e.name == "Mode").expect("Mode") as u32);
    let recorded: Vec<(&str, RecordedValue, Option<&str>)> =
        more.recorded.iter().map(|r| (r.name.as_str(), r.value, r.unit.as_deref())).collect();
    assert_eq!(
        recorded,
        [
            ("daq1_ai3", RecordedValue::Float(FloatWidth::F64), Some("bar")),
            ("daq1_count", RecordedValue::Int(IntWidth::U32), None),
            ("daq1_di0", RecordedValue::Bool, None),
            ("daq1_mode", RecordedValue::Enum(mode), None),
        ]
    );
    assert_eq!(takt_mir::hash::logic_hash(&base), takt_mir::hash::logic_hash(&more));
    assert_ne!(takt_mir::hash::program_hash(&base), takt_mir::hash::program_hash(&more));
    let (_, back) = takt_mir::format::read_program(&takt_mir::format::write_program(&more, "test")).expect("lesbar");
    assert_eq!(back.recorded, more.recorded, "die MIR traegt sie (Feld 21)");
}

/// Ein Kanal mit `max_rate_hz` ist ein Strom (8.2, 8.6): genannt wird er
/// `stream<raw>`, ungenannt je Element aufgezeichnet — ein `u8` als Zahl,
/// alles andere in der Drahtform mit der Groesse seines Elements.
#[test]
fn a_channel_with_a_maximum_rate_is_a_stream() {
    use takt_mir::program::{RecordedStream, RecordedValue};
    use takt_mir::types::{IntWidth, Type};
    let streams = "
[channel uart1/rx]
direction = input
raw = line<80>
max_rate_hz = 200
framing = lines

[channel can0/rx]
direction = input
raw = Frame
max_rate_hz = 1000

[channel uart2/rx]
direction = input
raw = u8
max_rate_hz = 1000
";
    let body = format!(
        "record Frame:\n    id   : u16\n    data : bytes<8>\n\n{MACHINE}\nmachine reader:\n    var n : int in 0..100 = 0\n    \
         initial RUN\n    state RUN:\n        on uart1_rx as e:\n            n = min(n + 1, 100)\n"
    );
    let (p, errors) = compile(&body, Some(&format!("{SITE}{streams}")));
    assert!(errors.is_empty(), "{errors:?}");
    let p = p.expect("Programm");
    let rx = p.channels.iter().find(|c| c.name == "uart1_rx").expect("`uart1_rx` gebunden");
    let Type::Stream(elem) = p.types.list[rx.ty.index()] else { panic!("kein Strom") };
    assert_eq!(p.types.list[elem.index()], Type::Line { cap: 80 });
    let recorded: Vec<(&str, RecordedValue, Option<RecordedStream>)> =
        p.recorded.iter().map(|r| (r.name.as_str(), r.value, r.stream)).collect();
    assert_eq!(
        recorded,
        [
            // `u16` und `bytes<8>`: 2 + 4 + 8 Byte in kanonischer Form (5.9).
            ("can0_rx", RecordedValue::Wire, Some(RecordedStream { max_rate_hz: 1000, bytes: 14 })),
            ("uart2_rx", RecordedValue::Int(IntWidth::U8), Some(RecordedStream { max_rate_hz: 1000, bytes: 1 })),
        ]
    );
}
