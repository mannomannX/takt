//! Die Grenze zwischen Rahmen und Randkern (12.6, FB-397).
//!
//! Der Rahmen deklariert die Einstiege des Randkerns in C
//! (`takt_frame::edge::PROTOTYPES`), der Randkern definiert sie in Rust
//! (`takt-native-abi/src/edge.rs`). Der Linker sieht keine Typen: Ein
//! Unterschied liesse den Kern still falsche Argumente lesen. Dieser Test
//! uebersetzt jede `extern "C"`-Signatur nach C und vergleicht. Die
//! Strukturen selbst prueft der C-Compiler mit `_Static_assert` gegen das
//! Layout aus `takt-hal`.

use takt_frame::edge::PROTOTYPES;
use takt_hal::contract::{Delivery, Device, Event, Track, Window};
use takt_hal::quality::{Bounds, Gate};

/// Ein Rust-Typ der Grenze in C.
fn c_type(rust: &str) -> String {
    let rust = rust.trim();
    if let Some(inner) = rust.strip_prefix("*mut ") {
        return format!("{} *", c_type(inner));
    }
    if let Some(inner) = rust.strip_prefix("*const ") {
        return format!("const {} *", c_type(inner));
    }
    let shared = [
        ("Track", Track::C_LAYOUT.name),
        ("Device", Device::C_LAYOUT.name),
        ("Delivery", Delivery::C_LAYOUT.name),
        ("Window", Window::C_LAYOUT.name),
        ("Event", Event::C_LAYOUT.name),
        ("Gate", Gate::C_LAYOUT.name),
        ("Bounds", Bounds::C_LAYOUT.name),
    ];
    if let Some((_, c)) = shared.iter().find(|(r, _)| *r == rust) {
        return format!("struct {c}");
    }
    match rust {
        "bool" => "_Bool",
        "u8" => "unsigned char",
        "u32" => "unsigned",
        "i32" => "int",
        "i64" => "long long",
        "f64" => "double",
        other => panic!("Typ `{other}` an der Grenze ohne C-Entsprechung im Test"),
    }
    .to_string()
}

/// Jede `extern "C"`-Funktion `takt_edge_*` der Quelle als C-Prototyp.
fn prototypes(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("extern \"C\" fn takt_edge_") {
        let sig = &rest[at + "extern \"C\" fn ".len()..];
        let open = sig.find('(').expect("Parameterliste");
        let close = sig.find(')').expect("Ende der Parameterliste");
        let name = &sig[..open];
        let params: Vec<String> = sig[open + 1..close]
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| c_type(p.split_once(':').expect("Name: Typ").1))
            .collect();
        let body = sig[close + 1..].find('{').expect("Rumpf") + close + 1;
        let ret = sig[close + 1..body].trim().strip_prefix("->").map_or("void".to_string(), c_type);
        out.push(format!("{ret} {name}({});", params.join(", ")));
        rest = &sig[body..];
    }
    out
}

/// **Jeder Prototyp des Rahmens ist die Signatur des Randkerns**, und der
/// Rahmen deklariert jeden Einstieg, den der Randkern anbietet.
#[test]
fn the_frame_declares_the_edge_core_as_it_is_defined() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../takt-native-abi/src/edge.rs");
    let src = std::fs::read_to_string(path).expect("takt-native-abi lesbar");
    let mut defined = prototypes(&src);
    let mut declared: Vec<String> = PROTOTYPES.iter().map(|p| p.to_string()).collect();
    defined.sort();
    declared.sort();
    assert!(defined.len() >= 5, "nur {} Einstiege gefunden", defined.len());
    assert_eq!(declared, defined, "Rahmen (links) und Randkern (rechts) deklarieren verschieden");
}
