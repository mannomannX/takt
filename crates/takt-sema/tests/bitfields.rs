//! Benannte Bitfelder in `layout`-Records (Referenz 3.7): Lesen, Schreiben
//! und der Weg ueber `encode`/`decode`.
//!
//! Ein Bitfeld ist eine *Sicht* auf sein Traegerfeld, kein eigener Speicher:
//! Lesen wird zu `bit`/`bits`, Schreiben zu `with_bit` beziehungsweise zum
//! Einsetzen mit Maske (3.10). Damit traegt der Codec sie ohne Zutun mit.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

/// Ein Statusregister, wie es ein Treiber deklariert.
const STATUS: &str = "\
record Status layout little:
    flags : u16 with bits:
        ready : bool at 0
        busy  : bool at 3
        level : u8 at 4..7

";

fn simulate(body: &str, ticks: u64) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    let program = out.program.expect("Programm");
    let stim = Trace::parse("").expect("leer");
    run(&program, &stim, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn a_named_bit_reads_as_bool_and_a_field_as_its_type() {
    // 3.7: `bool at 0` liest ein einzelnes Bit, `u8 at 4..7` einen Bereich.
    let trace = simulate(
        &format!(
            "{STATUS}\
output r : bool @ hw(\"o/r\") with safe = false
output b : bool @ hw(\"o/b\") with safe = false
output l : int in 0..99 @ hw(\"o/l\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0x0059)
            r = s.flags.ready
            b = s.flags.busy
            l = s.flags.level as int
"
        ),
        2,
    );
    // 0x59 = 0101_1001: Bit 0 und 3 gesetzt, Bits 4..7 sind 5.
    assert!(trace.contains("t=0 out r true\n"), "Bit 0: {trace}");
    assert!(trace.contains("t=0 out b true\n"), "Bit 3: {trace}");
    assert!(trace.contains("t=0 out l 5\n"), "Bits 4..7: {trace}");
}

#[test]
fn writing_a_bitfield_leaves_its_neighbours_alone() {
    // 3.7: das Bitfeld ist eine Sicht; das Schreiben setzt genau seine Bits
    // und laesst den Rest des Traegers stehen.
    let trace = simulate(
        &format!(
            "{STATUS}\
output f : int in 0..65535 @ hw(\"o/f\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0xFFFF)
            s.flags.level = 5
            f = s.flags as int
"
        ),
        2,
    );
    // 0xFFFF mit Bits 4..7 auf 5 ist 0xFF5F.
    assert!(trace.contains("t=0 out f 65375\n"), "nur die eigenen Bits: {trace}");
}

#[test]
fn a_bit_survives_encode_and_decode() {
    // 3.7: der Codec serialisiert das Traegerfeld, die Bitfelder fahren mit.
    let trace = simulate(
        &format!(
            "{STATUS}\
output r : bool @ hw(\"o/r\") with safe = false
output l : int in 0..99 @ hw(\"o/l\") with safe = 0
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = Status(flags = 0)
            s.flags.ready = true
            s.flags.level = 5
            var wire = s.encode()
            n = wire.len as int
            var back = Status.decode(wire)
            if back.valid:
                r = back.flags.ready
                l = back.flags.level as int
"
        ),
        2,
    );
    assert!(trace.contains("t=0 out n 2\n"), "zwei Byte Draht: {trace}");
    assert!(trace.contains("t=0 out r true\n"), "das Bit kam zurueck: {trace}");
    assert!(trace.contains("t=0 out l 5\n"), "das Feld kam zurueck: {trace}");
}

#[test]
fn an_unknown_bitfield_names_the_declared_ones() {
    let read = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            var s : Status = default
            n = 1 if s.flags.nope else 0
"
    ));
    assert!(read.contains("nope"), "der Name steht in der Meldung: {read}");

    let write = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            s.flags.nope = true
            n = 1
"
    ));
    assert!(write.contains("Bitfeld `nope`"), "auch beim Schreiben: {write}");
}

#[test]
fn a_bitfield_takes_only_plain_assignment() {
    // `+=` und Verwandte muessten lesen, rechnen und einsetzen; die Absicht
    // bleibt lesbarer, wenn der Rechenschritt dasteht.
    let out = errors(&format!(
        "{STATUS}\
output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var s : Status = default
    initial RUN
    state RUN:
        loop:
            s.flags.level += 1
            n = 1
"
    ));
    assert!(out.contains("nur `=`"), "zusammengesetzte Zuweisung abgelehnt: {out}");
}
