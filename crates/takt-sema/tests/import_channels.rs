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

/// `MACHINE`, das zusaetzlich `extra` liest.
fn reading(extra: &str) -> String {
    MACHINE.replace("daq1_ai0 > 100 bar", &format!("daq1_ai0 > 100 bar and {extra}"))
}

/// 8.2: Ein Kanal, aus dem sich keine Deklaration bilden laesst, ist ein
/// Fehler am Import (Pruefung 2), sobald die Quelle ihn nennt — und nur
/// dieser: Der abgelehnte Name meldet keinen Folgefehler (FB-407).
#[test]
fn an_undeclarable_channel_is_one_error_at_the_import() {
    for (section, name, want) in [
        ("[channel x/y]\nraw = bool\n", "x_y", "Kanal `x/y`: ohne `direction`"),
        ("[channel x/z]\ndirection = input\nraw = Nope\n", "x_z", "Kanal `x/z`: `Nope` ist nicht definiert"),
    ] {
        let (_, errors) = compile(&reading(name), Some(&format!("{SITE}\n{section}")));
        assert_eq!(errors.len(), 1, "{name}: {errors:?}");
        assert!(errors[0].contains("SC-2") && errors[0].contains(want), "{name}: {errors:?}");
    }
}

/// 8.2: Der Name ist die Adresse als Bezeichner. `a/b_c` und `a_b/c`
/// ergeben beide `a_b_c`, und ein importierter Name, den das Programm an
/// einer anderen Adresse schon vergeben hat, ist ebenso doppelt (SC-2).
#[test]
fn imported_names_collide_like_any_other() {
    let twins = "[channel a/b_c]\ndirection = input\nraw = bool\n\n[channel a_b/c]\ndirection = input\nraw = bool\n";
    let (_, errors) = compile(&reading("a_b_c"), Some(&format!("{SITE}\n{twins}")));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("SC-2") && errors[0].contains("Kanal `a_b/c`: `a_b_c` ist schon definiert"),
        "{errors:?}"
    );

    let body = format!("output gpio_valve : bool @ hw(\"other/valve\") with safe = false\n\n{MACHINE}");
    let (_, errors) = compile(&body, Some(SITE));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("SC-2") && errors[0].contains("Kanal `gpio/valve`: `gpio_valve` ist schon definiert"),
        "{errors:?}"
    );
}

/// Eine Adresse bindet je Richtung einmal (Pruefung 7, lower/decl.rs): Ein
/// geschriebener Output an der Adresse eines importierten Inputs gewinnt
/// nicht (andere Richtung, 8.2), beide stehen nebeneinander.
#[test]
fn a_written_channel_of_the_other_direction_stands_beside_the_import() {
    let body = format!(
        "output probe : bool @ hw(\"daq1/ai0\") with safe = false\n\n{}",
        MACHINE.replace("            gpio_valve", "            probe = true\n            gpio_valve")
    );
    let (p, errors) = compile(&body, Some(SITE));
    assert!(errors.is_empty(), "{errors:?}");
    let names: Vec<(String, Direction)> =
        p.expect("Programm").channels.iter().map(|c| (c.name.clone(), c.dir)).collect();
    assert_eq!(
        names,
        [
            ("probe".to_string(), Direction::Output),
            ("daq1_ai0".to_string(), Direction::Input),
            ("gpio_valve".to_string(), Direction::Output)
        ]
    );
}

/// 8.3: Im Sim-Build braucht ein importierter `hw`-Input eine `sim`-Quelle
/// wie ein geschriebener; in `takt sim` ist das eine Warnung (Pruefung 13).
#[test]
fn an_imported_input_needs_a_sim_source_in_the_sim_build() {
    let mut options = Options { policy: Policy::default(), build: Build::Sim, ..Default::default() };
    options.channel_imports.insert("site1.hw".to_string(), SITE.to_string());
    let out = takt_sema::compile(&format!("{HEAD}{MACHINE}"), &options);
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    let warned: Vec<String> = out.diagnostics.iter().filter(|d| d.code == "SC-13").map(|d| d.message.clone()).collect();
    assert_eq!(warned, ["Input `daq1_ai0` hat keine `sim`-Quelle"]);
}

/// 8.2, 8.6: Typ und Attribute je Kanalart aus der Konfiguration — eine
/// Ganzzahl mit Range, ein Enum ueber `raw`, ein Input ohne `max_age` mit
/// dem Default aus 3.5 (das Doppelte der kuerzesten lesenden Periode), ein
/// Strom mit `max_rate`, `framing` und Default-`capacity`, Stroeme aus
/// `Edge` und `bytes<N>`.
#[test]
fn each_kind_of_channel_gets_its_type_and_attributes() {
    use takt_mir::program::Framing;
    use takt_mir::types::{IntWidth, Type};
    let kinds = "
[channel daq1/level]
direction = input
raw = i16
range = -10..10

[channel daq1/mode]
direction = input
raw = Mode

[channel uart1/rx]
direction = input
raw = line<80>
max_rate_hz = 200
framing = lines

[channel gpio/btn]
direction = input
raw = Edge
max_rate_hz = 50

[channel can0/rx]
direction = input
raw = bytes<16>
max_rate_hz = 1000
";
    let body = format!(
        "enum Mode: IDLE, RUN\n\n{}\nmachine reader:\n    var n : int in 0..100 = 0\n    initial RUN\n    state RUN:\n        \
         on uart1_rx as e:\n            n = min(n + 1, 100)\n        on gpio_btn as e:\n            n = 0\n        \
         on can0_rx as e:\n            n = 1\n",
        reading("daq1_level > 0 and daq1_mode == RUN")
    );
    let (p, errors) = compile(&body, Some(&format!("{SITE}{kinds}")));
    assert!(errors.is_empty(), "{errors:?}");
    let p = p.expect("Programm");
    let channel = |name: &str| p.channels.iter().find(|c| c.name == name).unwrap_or_else(|| panic!("`{name}` fehlt"));
    let ty = |name: &str| p.types.list[channel(name).ty.index()].clone();

    let Type::Int { width, range, .. } = ty("daq1_level") else { panic!("{:?}", ty("daq1_level")) };
    assert_eq!(width, IntWidth::I16);
    let range = range.expect("Range");
    assert_eq!((range.lo, range.hi), (takt_mir::types::Const::Int(-10), takt_mir::types::Const::Int(10)));

    let mode = p.enums.iter().position(|e| e.name == "Mode").expect("Mode");
    assert_eq!(ty("daq1_mode"), Type::Enum(takt_mir::EnumId(mode as u32)));
    // 3.5: zwei Perioden des schnellsten Lesers (`m` und `reader` je 1 ms).
    assert_eq!(channel("daq1_mode").attrs.max_age, Some(2_000_000));

    let stream_of = |name: &str| {
        let Type::Stream(elem) = ty(name) else { panic!("`{name}` ist kein Strom: {:?}", ty(name)) };
        p.types.list[elem.index()].clone()
    };
    assert_eq!(stream_of("uart1_rx"), Type::Line { cap: 80 });
    let rx = &channel("uart1_rx").attrs;
    assert_eq!(rx.framing, Some(Framing::Lines));
    assert!(
        matches!(rx.max_rate.as_ref().map(|e| &e.kind), Some(takt_mir::expr::ExprKind::Float(f)) if *f == 200.0),
        "{:?}",
        rx.max_rate
    );
    assert!(rx.capacity.is_some_and(|c| c > 0), "Default-Kapazitaet: {:?}", rx.capacity);
    let edge = p.records.iter().position(|r| r.name == "Edge").expect("Edge");
    assert_eq!(stream_of("gpio_btn"), Type::Record(takt_mir::RecordId(edge as u32)));
    assert!(matches!(stream_of("can0_rx"), Type::Bytes { cap: 16 }), "{:?}", stream_of("can0_rx"));
}

/// 8.2: Eine Konfiguration, die sich nicht lesen laesst, ist ein Fehler am
/// Import mit Datei und Zeile; ebenso eine unbekannte Richtung. Die Namen
/// ihrer Kanaele kennt dann niemand, ihre Verwendungen bleiben unbekannt.
#[test]
fn an_unreadable_configuration_names_its_file_and_line() {
    for (what, site) in [
        ("kaputter Abschnitt", format!("{SITE}\n[channel x/y\ndirection = input\n")),
        ("unbekannte Richtung", format!("{SITE}\n[channel x/y]\ndirection = sideways\nraw = bool\n")),
    ] {
        let (_, errors) = compile(MACHINE, Some(&site));
        assert!(errors[0].contains("[SC-2]") && errors[0].contains("`site1.hw`, Zeile "), "{what}: {errors:?}");
        assert!(errors[1..].iter().all(|e| e.contains("ist nicht definiert")), "{what}: {errors:?}");
    }
}

/// 8.8: Ein importierter Ausgabestrom braucht keinen `safe`-Wert und wird
/// wie ein geschriebener beschrieben.
#[test]
fn an_imported_output_stream_is_written_by_send() {
    let site = format!("{SITE}\n[channel uart0/tx]\ndirection = output\nraw = u8\nmax_rate_hz = 1000\n");
    let body = MACHINE.replace("            gpio_valve", "            send uart0_tx, 7\n            gpio_valve");
    let (p, errors) = compile(&body, Some(&site));
    assert!(errors.is_empty(), "{errors:?}");
    let p = p.expect("Programm");
    let tx = p.channels.iter().find(|c| c.name == "uart0_tx").expect("uart0_tx");
    assert_eq!(tx.dir, Direction::Output);
    assert!(tx.attrs.safe.is_none());
}
