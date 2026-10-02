//! Der Produkt-DFA eines Handler-Blocks im erzeugten Code (8.7, 11.2).
//!
//! **Warum hier und nicht in der Runtime.** Die Stromzugriffe laufen
//! ueber `P_stream_*`, weil die Puffer der Runtime gehoeren. Der
//! Mustervergleich gehoert dagegen in den erzeugten Code: 11.2 sagt
//! „Handler-Dispatch als Schleife ueber das Fenster mit vorkompilierten
//! DFA-Tabellen" und verlangt, dass die Tabellen in `takt size`
//! erscheinen (11.5) und auf Zielen mit XIP-Flash im RAM liegen (12.3).
//! Beides geht nur, wenn sie Daten des Programms sind — eine Runtime
//! haette sie nicht zur Uebersetzungszeit.
//!
//! **Was der Automat entscheidet.** Welche Muster des Blocks treffen, in
//! einem Durchlauf je Element. Was in den Captures steht und ob ein Wert
//! im Bereich liegt, holt die Extraktion (`captures`).
//!
//! **Ein Automat je Inhalt.** Die Tabellen heissen nach ihrem Schluessel
//! (`Dfa::key`); zwei Bloecke mit denselben Mustern teilen sie, wie
//! `takt size` sie zaehlt.

use takt_mir::dfa::Dfa;

use crate::emit::{Module, Reg};

/// Der Name einer Tabelle des Automaten.
fn name(dfa: &Dfa, what: &str) -> String {
    format!("@takt_dfa_{:016x}_{what}", dfa.key())
}

/// Der LLVM-Typ eines Uebergangs.
fn state_type(dfa: &Dfa) -> String {
    format!("i{}", dfa.state_width() * 8)
}

/// Der LLVM-Typ eines Eintrags der Trefferliste.
fn accept_type(dfa: &Dfa) -> String {
    format!("i{}", dfa.accept_width() * 8)
}

/// Schreibt die Tabellen als Konstanten, einmal je Automat (11.2):
/// Klassenabbildung (256 Byte), Uebergaenge (`states × classes`, je
/// `i8` oder `i16`) und je Zustand die Muster, die dort treffen. Sie sind
/// `constant`, damit sie im Flash liegen duerfen; 12.3 verlangt fuer
/// XIP-Ziele eine Kopie im RAM, und die zieht die Runtime.
fn declare(dfa: &Dfa, m: &mut Module) {
    let classes: Vec<String> = dfa.classes.iter().map(|c| format!("i8 {c}")).collect();
    let st = state_type(dfa);
    let table: Vec<String> = dfa.table.iter().map(|t| format!("{st} {t}")).collect();
    let at = accept_type(dfa);
    let accept: Vec<String> = dfa.accept.iter().map(|a| format!("{at} {}", *a as i64)).collect();
    let mut text = format!("\n; Produkt-DFA (8.7, 11.2): {} Muster, {} Klassen", dfa.patterns, dfa.class_count);
    text.push_str(&format!("\n;   {} Zustaende", dfa.states()));
    text.push_str(&format!("\n{} = private constant [256 x i8] [{}]", name(dfa, "classes"), classes.join(", ")));
    text.push_str(&format!(
        "\n{} = private constant [{} x {st}] [{}]",
        name(dfa, "table"),
        table.len(),
        table.join(", ")
    ));
    text.push_str(&format!(
        "\n{} = private constant [{} x {at}] [{}]",
        name(dfa, "accept"),
        accept.len(),
        accept.join(", ")
    ));
    if !m.has_declared(&text) {
        m.declare(&text);
    }
}

/// Laesst den Automaten ueber einen Text laufen (8.7) und liefert die
/// Treffer als `i64`: Bit `i` fuer Muster `i` des Blocks.
///
/// `text` zeigt auf `{ i32 len, [N x i8] bytes, ... }` — den Aufbau von
/// `str<N>` und `line<N>` (`ty::lower`). Die Schleife ist beschraenkt
/// durch die Laenge des Texts, und die durch `N` (3.9) — 4.1 verlangt
/// genau das.
pub fn run(dfa: &Dfa, text: Reg, m: &mut Module) -> Reg {
    declare(dfa, m);
    let k = m.next_label();
    let (head, body, done) = (format!("dfa{k}"), format!("dfa{k}_rumpf"), format!("dfa{k}_fertig"));

    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 4"));

    let state_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {state_ptr}"));
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {len}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let at = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {i}"));
    let byte = m.inst(&format!("load i8, ptr {at}"));
    let idx = m.inst(&format!("zext i8 {byte} to i32"));
    let class_at =
        m.inst(&format!("getelementptr inbounds [256 x i8], ptr {}, i32 0, i32 {idx}", name(dfa, "classes")));
    let class8 = m.inst(&format!("load i8, ptr {class_at}"));
    let class = m.inst(&format!("zext i8 {class8} to i32"));
    // `zeile = zustand * klassen + klasse`, der Eintrag der Tabelle.
    let state = m.inst(&format!("load i32, ptr {state_ptr}"));
    let row = m.inst(&format!("mul i32 {state}, {}", dfa.class_count));
    let off = m.inst(&format!("add i32 {row}, {class}"));
    let st = state_type(dfa);
    let cell = m.inst(&format!(
        "getelementptr inbounds [{} x {st}], ptr {}, i32 0, i32 {off}",
        dfa.table.len(),
        name(dfa, "table")
    ));
    let narrow = m.inst(&format!("load {st}, ptr {cell}"));
    let next = m.inst(&format!("zext {st} {narrow} to i32"));
    m.void_inst(&format!("store i32 {next}, ptr {state_ptr}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    let end_state = m.inst(&format!("load i32, ptr {state_ptr}"));
    let at = accept_type(dfa);
    let entry = m.inst(&format!(
        "getelementptr inbounds [{} x {at}], ptr {}, i32 0, i32 {end_state}",
        dfa.accept.len(),
        name(dfa, "accept")
    ));
    let mask = m.inst(&format!("load {at}, ptr {entry}"));
    if at == "i64" { mask } else { m.inst(&format!("zext {at} {mask} to i64")) }
}

/// Ist Bit `bit` der Treffer gesetzt?
pub fn hit(mask: Reg, bit: usize, m: &mut Module) -> Reg {
    let shifted = m.inst(&format!("lshr i64 {mask}, {bit}"));
    m.inst(&format!("trunc i64 {shifted} to i1"))
}
