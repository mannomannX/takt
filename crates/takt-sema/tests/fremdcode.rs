//! Befunde aus fremden Programmen (`feedback/`, FB-93 bis FB-100).
//!
//! Die Programme in `feedback/` sind gegen aeltere Referenzstaende
//! geschrieben und benutzen die Sprache anders als der eigene Korpus.
//! Sechs Stellen, an denen der Compiler von der Referenz abwich, kamen
//! erst dadurch ans Licht — jede hier mit dem Fall, der sie zeigt.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn options() -> Options {
    Options { policy: Policy::default(), build: Build::Sim, profile: None }
}

/// Uebersetzt ein Programm und verlangt Fehlerfreiheit.
fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let out = takt_sema::compile(&src, &options());
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Fehler eines Programms, je Zeile eine Meldung.
fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let out = takt_sema::compile(&src, &options());
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

/// Laeuft ein Programm und liefert den Trace.
fn simulate(body: &str, ticks: u64) -> String {
    let program = compile(body);
    let out = run(&program, &Trace::default(), &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    out.trace.render()
}

// --- FB-95: die Untergrenze einer signierten Breite ----------------------

/// Der Zweierkomplementbereich ist asymmetrisch, und genau seine
/// Untergrenze fiel durch: Das unaere Minus ist ein eigener Knoten, also
/// wurde der Betrag `32768` gegen `i16` gehalten.
#[test]
fn the_lowest_value_of_a_signed_width_is_writable() {
    for (ty, lo) in [("i8", "-128"), ("i16", "-32768"), ("i32", "-2147483648")] {
        let body = format!("fn f() -> {ty}:\n    var x : {ty} = {lo}\n    return x\n");
        let e = errors(&body);
        assert!(e.is_empty(), "{lo} passt in {ty}, wurde aber abgelehnt:\n{e}");
    }
}

/// Einen Schritt darunter bleibt es ein Fehler — und wird genau einmal
/// gemeldet, nicht zweimal fuer Vorzeichen und Betrag.
#[test]
fn one_below_the_lowest_value_is_reported_once() {
    let e = errors("fn f() -> i16:\n    var x : i16 = -32769\n    return x\n");
    assert_eq!(e.matches("passt nicht in `i16`").count(), 1, "genau eine Meldung, gefunden:\n{e}");
}

// --- FB-96: eigene Methoden einer Blockinstanz ---------------------------

const STOPWATCH: &str = "\
output y : Duration @ sim(\"o\")

block stopwatch():
    var acc : Duration = 0 s
    var run : bool = false
    start():
        run = true
    stop():
        run = false
    elapsed() -> Duration:
        return acc

machine m every 1 ms:
    var sw = stopwatch()
    initial A
    state A:
        enter:
            sw.start()
        loop:
            y = sw.elapsed()
        after 1 s:
            sw.stop()
            -> A
";

/// 11.4 fuehrt `stopwatch` mit `start`/`stop`/`elapsed` als Beispiel, und
/// die Grammatik sieht `method_decl` ausdruecklich vor. Erkannt wurden
/// Methodenanweisungen aber an einer festen Namensliste, in der nur die
/// Methoden der eingebauten Typen stehen; ein eigener Name fiel vorher
/// durch und landete in der Ausdruckspruefung.
#[test]
fn a_block_instance_answers_to_its_own_methods() {
    let e = errors(STOPWATCH);
    assert!(e.is_empty(), "der Block aus 11.4 uebersetzt nicht:\n{e}");
}

/// Ein Block ohne `step` hat nur eigene Methoden — fuer ihn galt die
/// Namensliste ueberhaupt nicht.
#[test]
fn a_block_without_step_is_usable() {
    let p = compile(STOPWATCH);
    assert!(p.blocks.iter().any(|b| b.name == "stopwatch" && b.step.is_none()));
}

// --- FB-97: `pulse` in einer Sequenz ------------------------------------

/// `pulse` ist Zucker fuer zwei Anweisungen (7.5) und wird vor der
/// Einzelsenkung aufgeloest. Die Sequenz senkte jedes Element einzeln und
/// lief darum in den Zweig, dessen einzige Aufgabe die Fehlermeldung ist.
#[test]
fn pulse_works_in_a_sequence_as_it_does_in_a_loop() {
    let block = "\
output e : bool @ hw(\"gpio/e\") with safe = false

machine m every 1 ms:
    initial A
    state A:
        sequence:
            pulse e = true for 5 ms
            wait 2 ms
            -> A
";
    let e = errors(block);
    assert!(e.is_empty(), "`pulse` in `sequence` abgelehnt:\n{e}");
}

// --- FB-98: das Element eines Skalarstroms ------------------------------

/// 8.6 nennt `u8` als Elementart, und jedes Element traegt neben `.t` und
/// `.seq` seinen Inhalt. Bei `bytes` und `line` heisst er `.data` bzw.
/// `.text`; beim Skalar fehlte er ganz, womit ein Byte-Strom aus einem
/// Handler heraus nicht verarbeitbar war.
#[test]
fn a_scalar_stream_element_carries_its_value() {
    let body = "\
stream<u8> q with capacity = 8, overflow = fault
output y : int @ sim(\"o\")

machine w every 1 ms:
    initial A
    state A:
        loop:
            send q, 7
        after 1 s: -> A

machine m every 1 ms:
    initial A
    state A:
        on q as b:
            y = b.data as int
        after 1 s: -> A
";
    let trace = simulate(body, 3);
    // Unit-Delay: was in Tick 0 gesendet wird, liest der Konsument in Tick 1.
    assert!(trace.contains("t=1 out y 7"), "das Element kam nicht an:\n{trace}");
}

// --- FB-99: Sichtbarkeit veroeffentlichter Groessen ---------------------

/// Grundentscheidung 6: Kommunikation laeuft mit Unit-Delay, „dadurch ist
/// die Ausfuehrungsreihenfolge im Tick beweisbar irrelevant". Die
/// *Deklarations*reihenfolge entschied dennoch, ob ein Programm
/// uebersetzt — zwei Maschinen mit Gegenkopplung sind aber nicht beide
/// zuerst deklarierbar, und das ist der Normalfall.
#[test]
fn a_published_variable_is_visible_before_its_machine_is_declared() {
    let body = "\
output y : int @ sim(\"o\")

machine a every 1 ms:
    initial A
    state A:
        loop:
            y = b.flag
        after 1 s: -> A

machine b every 1 ms:
    pub var flag : int in 0..9 = 4
    initial B
    state B:
        loop:
            flag = 7
        after 1 s: -> B
";
    let e = errors(body);
    assert!(e.is_empty(), "die Vorwaertsreferenz wurde abgelehnt:\n{e}");
}

/// Die Vorschau legt die Variablenliste deckungsgleich an, weil `VarId`
/// ein Index in sie ist. Faenden sich dort nur die oeffentlichen, zeigte
/// jede aufgeloeste Referenz um die nicht-oeffentlichen verschoben — und
/// das faellt nicht als Fehler auf, sondern als falscher Wert.
#[test]
fn a_forward_reference_reads_the_variable_it_names() {
    let body = "\
output y : int @ sim(\"o\")

machine a every 1 ms:
    initial A
    state A:
        loop:
            y = b.zweite
        after 1 s: -> A

machine b every 1 ms:
    var intern : int in 0..9 = 3
    pub var erste : int in 0..9 = 1
    pub var zweite : int in 0..9 = 2
    initial B
    state B:
        loop:
            erste = intern
            zweite = 7
        after 1 s: -> B
";
    let trace = simulate(body, 3);
    assert!(trace.contains("t=0 out y 2"), "gelesen wurde nicht `zweite`:\n{trace}");
    assert!(trace.contains("t=1 out y 7"), "der Folgewert stimmt nicht:\n{trace}");
}

/// Ein Signal wird ebenso vorwaerts gelesen wie eine `pub var`.
#[test]
fn a_signal_is_visible_before_its_machine_is_declared() {
    let body = "\
output y : bool @ sim(\"o\")

machine a every 1 ms:
    initial A
    state A:
        loop:
            y = b.ready
        after 1 s: -> A

machine b every 1 ms:
    signal ready
    initial B
    state B:
        loop:
            raise ready
        after 1 s: -> B
";
    let e = errors(body);
    assert!(e.is_empty(), "das Signal wurde nicht gefunden:\n{e}");
}

// --- FB-100: die Diskriminante eines Enums ------------------------------

/// Ein Enum bleibt ohne Cast: Die Rueckrichtung waere partiell, und
/// `match` traegt die Erschoepfung (3.7). `layout u8` nennt die Breite
/// fuer das Drahtformat, nicht fuer die Rechnung — die Meldung sagt das
/// jetzt, statt nur abzulehnen.
#[test]
fn casting_an_enum_names_the_way_to_its_discriminant() {
    let e = errors("enum Cmd layout u8: START = 0x01, STOP = 0x02\n\nfn f(c: Cmd) -> u8:\n    return c as u8\n");
    assert!(e.contains("`as` auf `Cmd`"), "die Ablehnung fehlt:\n{e}");
    assert!(e.contains("layout"), "die Meldung nennt den Weg nicht:\n{e}");
}
