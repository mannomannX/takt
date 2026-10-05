//! Registerports in der Simulation (Referenz 12.10, v1.2).
//!
//! **Was hier belegt wird.** Ein Port ist im Sim-Build ein Channel-Paar:
//! Gelesen wird der Modellwert des Ticks, geschrieben wird in einen
//! Strom, der *jeden* Vorgang einzeln und in Reihenfolge fuehrt. Das ist
//! der Unterschied zu einem Output, der am Tickende einmal committet —
//! und er ist der Grund, warum ein Treiber ueberhaupt einen Port braucht.
//!
//! Dazu die Zugriffsarten am Bitfeld (3.7): Ein `w1c`-Feld senkt auf
//! einen einzelnen Schreibvorgang mit Einzelbitmaske, nicht auf
//! Lese-Modifiziere-Schreibe. Nur so kann ein ungesehenes Ereignis nicht
//! verlorengehen.
//!
//! Nativ bildet der Rahmen die Adresse auf dasselbe Modell ab (FB-261);
//! jedes Szenario laeuft darum auch uebersetzt gegen den Interpreter.

use takt_conformance::compare;
use takt_diag::Policy;
use takt_interp::{RunOptions, Trace};
use takt_mir::program::Program;

mod common;

fn compile(src: &str) -> Program {
    let options = takt_sema::Options {
        policy: Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &options);
    let fehler: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(fehler.is_empty(), "{}", fehler.join("\n"));
    out.program.expect("Programm")
}

fn run(src: &str, ticks: u64) -> String {
    let options = RunOptions { ticks, ..Default::default() };
    takt_interp::run(&compile(src), &Trace::default(), &options).expect("Lauf").trace.render()
}

const KOPF: &str = "\
system:
    language = 1
    tick     = 10 ms

record Ctrl layout little:
    flags : u8 with bits:
        a : bool at 0
        b : bool at 1 w1c

port reg : Ctrl @ mmio(0x50000000)

input w : stream<Ctrl> @ sim(\"mmio/0x50000000/w\") with capacity = 16, overflow = fault, max_rate = 400 Hz

output seen : int in 0..9 @ hw(\"o/seen\") with safe = 0
";

/// Ein Register ohne `w1c`: Es darf als Ganzes geschrieben werden (3.7).
fn three_writes() -> String {
    format!(
        "{KOPF}\n\
         record Data layout little:\n    value : u8\n\n\
         port dat : Data @ mmio(0x50000010)\n\n\
         input wd : stream<Data> @ sim(\"mmio/0x50000010/w\") with capacity = 16, overflow = fault, \
         max_rate = 400 Hz\n\n\
         output last : int in 0..9 @ hw(\"o/last\") with safe = 0\n\n\
         driver machine d:\n    var n : int in 0..9 = 0\n\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         dat = Data(value = 1)\n                dat = Data(value = 2)\n                \
         dat = Data(value = 3)\n\n        on wd as e:\n            n = n + 1\n            \
         seen = n\n            last = e.data.value as int\n\n        after 1 s: -> RUN\n"
    )
}

/// **Drei Schreibvorgaenge in einem Tick kommen als drei Elemente an, in
/// Reihenfolge.** Ein Output haette am Tickende einen Wert; ein Port
/// fuehrt jeden Vorgang.
#[test]
fn three_writes_in_one_tick_arrive_in_order() {
    let trace = run(&three_writes(), 4);
    // Drei Elemente, und das zuletzt gesehene traegt den dritten Wert:
    // Haette der Strom sie zusammengefasst, stuende hier eine Eins.
    assert!(trace.contains("out seen 3"), "{trace}");
    assert!(trace.contains("out last 3"), "{trace}");
}

fn w1c_write() -> String {
    format!(
        "{KOPF}\n\
         driver machine d:\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         reg.flags.b = true\n\n        on w as e:\n            seen = e.data.flags as int\n\n        \
         after 1 s: -> RUN\n"
    )
}

/// **Ein `w1c`-Feld schreibt nur die eigene Eins.** Die Senkung ist
/// `reg.0 = 0.with_bit(1, true)`: Kein Lesen, kein Zurueckschreiben
/// fremder Bits — sonst loeschte ein Treiber Ereignisse, die er nie
/// gesehen hat (FB-11).
#[test]
fn a_w1c_write_sends_only_its_own_bit() {
    let trace = run(&w1c_write(), 4);
    // Bit 1 allein ist die Zwei. Waere es ein Lese-Modifiziere-Schreibe
    // ueber den ganzen Traeger, stuende hier der gelesene Wert mit
    // gesetztem Bit 1.
    assert!(trace.contains("out seen 2"), "{trace}");
}

fn field_write() -> String {
    format!(
        "{KOPF}\n\
         output model : Ctrl @ sim(\"mmio/0x50000000/r\")\n\
         output got   : int in 0..255 @ hw(\"o/got\") with safe = 0\n\n\
         machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            \
         model = Ctrl(flags = 0x83)\n\n        after 1 s: -> RUN\n\n\
         driver machine d:\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 20 ms:\n                \
         reg.flags.a = false\n\n        on w as e:\n            got = e.data.flags as int\n\n        \
         after 1 s: -> RUN\n"
    )
}

/// **Ein `rw`-Feld unter einem Port liest und schreibt den ganzen
/// Record.** Es hat keinen eigenen Speicher; der Wert kommt vom Modell.
#[test]
fn a_plain_field_under_a_port_goes_through_the_whole_record() {
    let trace = run(&field_write(), 5);
    // Gelesen wurde 0x83, Bit 0 geloescht. Bit 1 ist `w1c` und geht als 0
    // hinaus, sonst loeschte das Schreiben von `a` ein ungesehenes Ereignis
    // (3.7, FB-410): 0x80 = 128. Ohne das Lesen stuende hier 0 — das Feld
    // hat keinen eigenen Speicher.
    assert!(trace.contains("out got 128"), "{trace}");
}

/// Mehr Schreibvorgaenge in einem Tick, als der Schreibstrom fasst
/// (`capacity = 2`, `overflow = fault`, `max_rate` erlaubt zwei je Tick).
/// Einen Ueberlauf des Rings schliesst SC-17 statisch aus, und ein
/// Portzugriff schlaegt nicht fehl (12.10): Der dritte Vorgang kommt nicht
/// an, in beiden Implementierungen gleich, und es gibt keinen Fault.
fn too_many_writes() -> String {
    format!(
        "{KOPF}\n\
         record Data layout little:\n    value : u8\n\n\
         port dat : Data @ mmio(0x50000010)\n\n\
         input wd : stream<Data> @ sim(\"mmio/0x50000010/w\") with capacity = 2, overflow = fault, \
         max_rate = 200 Hz\n\n\
         driver machine d:\n    fault -> HALT\n    var n : int in 0..9 = 0\n\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         dat = Data(value = 1)\n                dat = Data(value = 2)\n                \
         dat = Data(value = 3)\n\n        on wd as e:\n            n = n + 1\n            \
         seen = n\n\n        after 1 s: -> RUN\n\n    state HALT:\n        enter:\n            seen = 9\n"
    )
}

/// Ein Modellwert, der sich jeden Tick aendert (FB-307): Der Treiber liest
/// ihn mit Unit-Delay. `model_first` stellt das Modell vor den Treiber,
/// sonst steht es dahinter — die Reihenfolge der Deklaration aendert nichts.
fn changing_model(model_first: bool) -> String {
    let model = "machine m:\n    var k : int in 0..200 = 0\n    initial RUN\n\n    state RUN:\n        loop:\n            \
                 k = (k + 1) % 200\n            model = Ctrl(flags = k as u8)\n\n        after 1 s: -> RUN\n\n";
    let driver = "driver machine d:\n    initial RUN\n\n    state RUN:\n        loop:\n            \
                  got = reg.flags as int\n\n        after 1 s: -> RUN\n\n";
    let (first, second) = if model_first { (model, driver) } else { (driver, model) };
    format!(
        "{KOPF}\n\
         output model : Ctrl @ sim(\"mmio/0x50000000/r\")\n\
         output got   : int in 0..255 @ hw(\"o/got\") with safe = 0\n\n\
         {first}{second}"
    )
}

/// `w1c` mit `false`: Ein Schreibvorgang geht hinaus, aber ohne die Eins —
/// kein Bit wird geloescht. Der Rahmen schreibt dieselben Bits wie der
/// Interpreter (`bits`).
fn w1c_false() -> String {
    format!(
        "{KOPF}\n\
         output bits : int in 0..255 @ hw(\"o/bits\") with safe = 99\n\n\
         driver machine d:\n    var n : int in 0..9 = 0\n    initial RUN\n\n    \
         state RUN:\n        loop:\n            if now == 0 s:\n                \
         reg.flags.b = false\n\n        on w as e:\n            n = n + 1\n            seen = n\n            \
         bits = e.data.flags as int\n\n        after 1 s: -> RUN\n"
    )
}

/// `ro` liest aus dem Traeger, `wo` schreibt ohne Lesen, die uebrigen Bits
/// als 0 (3.7).
fn ro_and_wo() -> String {
    "system:\n    language = 1\n    tick     = 10 ms\n\n\
     record Stat layout little:\n    flags : u8 with bits:\n        ready : bool at 0 ro\n        go : bool at 1 wo\n\n\
     port st : Stat @ mmio(0x50000020)\n\n\
     input  ws    : stream<Stat> @ sim(\"mmio/0x50000020/w\") with capacity = 16, overflow = fault, max_rate = 400 Hz\n\
     output model : Stat @ sim(\"mmio/0x50000020/r\")\n\
     output ready : bool @ hw(\"o/ready\") with safe = false\n\
     output sent  : int in 0..255 @ hw(\"o/sent\") with safe = 0\n\n\
     machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            model = Stat(flags = 0x81)\n\n        \
     after 1 s: -> RUN\n\n\
     driver machine d:\n    initial RUN\n\n    state RUN:\n        loop:\n            ready = st.flags.ready\n            \
     if now == 20 ms:\n                st.flags.go = true\n\n        on ws as e:\n            sent = e.data.flags as int\n\n        \
     after 1 s: -> RUN\n"
        .to_string()
}

/// **Nativ wie im Interpreter** (Satz 9.4.4, FB-261): Der Rahmen bildet
/// die Adresse auf dasselbe Modell ab, und jede der drei Eigenschaften
/// oben gilt auch fuer den uebersetzten Treiber.
#[test]
fn the_generated_code_maps_ports_like_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    // Dazu die Faelle aus KON2-029: Ueberlauf des Schreibstroms, ein
    // Modellwert, der jeden Tick wechselt, in beiden Reihenfolgen der
    // Maschinen, `w1c` mit `false`, `ro` und `wo`.
    let cases = [
        ("ports_in_order", three_writes(), 4, "out seen 3"),
        ("ports_w1c", w1c_write(), 4, "out seen 2"),
        ("ports_field", field_write(), 5, "out got 128"),
        ("ports_overflow", too_many_writes(), 4, "t=1 out seen 2"),
        ("ports_changing", changing_model(true), 6, "t=3 out got 3"),
        ("ports_changing_swapped", changing_model(false), 6, "t=3 out got 3"),
        ("ports_w1c_false", w1c_false(), 4, "t=1 out bits 0"),
        ("ports_ro_wo", ro_and_wo(), 5, "out sent 2"),
    ];
    for (name, src, ticks, want) in cases {
        let interpreted = run(&src, ticks);
        assert!(interpreted.contains(want), "{name}: `{want}` fehlt im Interpreter:\n{interpreted}");
        let native =
            common::run_native_all(&clang, &compile(&src), name, ticks).unwrap_or_else(|e| panic!("{name}: {e}"));
        let missing: Vec<String> = common::board::output_names(&interpreted)
            .difference(&common::board::output_names(&native))
            .cloned()
            .collect();
        assert!(missing.is_empty(), "{name}: nativ fehlen {missing:?}\n{native}");
        let diffs = compare(&interpreted, &native);
        assert!(diffs.is_empty(), "{name}: {diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    }
}
