//! Musterpruefung im erzeugten Code (8.7, 11.2).
//!
//! **Warum hier und nicht in der Runtime.** Die Stromzugriffe laufen
//! ueber `takt_stream_*`, weil die Puffer der Runtime gehoeren. Der
//! Mustervergleich gehoert dagegen in den erzeugten Code: 11.2 sagt
//! „Handler-Dispatch als Schleife ueber das Fenster mit vorkompilierten
//! DFA-Tabellen" und verlangt, dass die Tabellen in `takt size`
//! erscheinen (11.5) und auf Zielen mit XIP-Flash im RAM liegen (12.3).
//! Beides geht nur, wenn sie Daten des Programms sind — eine Runtime
//! haette sie nicht zur Uebersetzungszeit.
//!
//! **Was der Automat entscheidet.** Ob das Muster trifft. Was in den
//! Captures steht, holt die Extraktion (`captures`), denn ein Automat
//! ueber Zeichenklassen kennt die Grenzen, aber nicht die Werte.

use takt_mir::pattern::Dfa;

use crate::emit::{Module, Reg};
use crate::expr::NotYet;

/// Der Name der Klassenabbildung eines Musters.
fn classes_name(id: u32) -> String {
    format!("@takt_dfa{id}_classes")
}

/// Der Name der Uebergangstabelle eines Musters.
fn table_name(id: u32) -> String {
    format!("@takt_dfa{id}_table")
}

/// Schreibt die Tabellen eines Automaten als Konstanten (11.2).
///
/// Drei Felder: die Klassenabbildung (256 Byte), die Uebergaenge
/// (`states × classes`, je `i32`) und die akzeptierenden Zustaende. Sie
/// sind `constant`, damit sie im Flash liegen duerfen; 12.3 verlangt
/// fuer XIP-Ziele eine Kopie im RAM, und die zieht die Runtime.
pub fn declare(id: u32, dfa: &Dfa, m: &mut Module) {
    let classes: Vec<String> = dfa.classes.iter().map(|c| format!("i8 {c}")).collect();
    let table: Vec<String> = dfa.table.iter().map(|t| format!("i32 {t}")).collect();
    let mut text = format!("\n; DFA eines Musters (8.7, 11.2): {} Klassen", dfa.class_count);
    let states = table.len() / dfa.class_count.max(1) as usize;
    text.push_str(&format!("\n;   {states} Zustaende, {} akzeptierend", dfa.accept.len()));
    text.push_str(&format!("\n{} = private constant [256 x i8] [{}]", classes_name(id), classes.join(", ")));
    text.push_str(&format!("\n{} = private constant [{} x i32] [{}]", table_name(id), table.len(), table.join(", ")));
    // Die akzeptierenden Zustaende brauchen keine Tabelle: Sie stehen
    // als Vergleichskette in der IR (siehe `run`).
    m.declare(&text);
}

/// Laesst den Automaten ueber einen Text laufen (8.7).
///
/// `text` zeigt auf `{ i32 len, [N x i8] bytes, ... }` — den Aufbau von
/// `str<N>` und `line<N>` (`ty::lower`). Das Ergebnis ist ein `i1`:
/// trifft das Muster oder nicht.
///
/// Die Schleife ist beschraenkt durch die Laenge des Texts, und die ist
/// durch `N` beschraenkt (3.9) — 4.1 verlangt genau das.
pub fn run(id: u32, dfa: &Dfa, text: Reg, m: &mut Module) -> Result<Reg, NotYet> {
    let k = m.next_label();
    let (head, body) = (format!("dfa{k}"), format!("dfa{k}_rumpf"));
    let (check, done) = (format!("dfa{k}_pruef"), format!("dfa{k}_fertig"));

    // Laenge und Bytes des Texts.
    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 4"));

    let state_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {state_ptr}"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {len}"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{check}"));

    m.label(&body);
    let at = m.inst(&format!("getelementptr inbounds i8, ptr {bytes}, i32 {i}"));
    let byte = m.inst(&format!("load i8, ptr {at}"));
    let idx = m.inst(&format!("zext i8 {byte} to i32"));
    let class_at = m.inst(&format!("getelementptr inbounds [256 x i8], ptr {}, i32 0, i32 {idx}", classes_name(id)));
    let class8 = m.inst(&format!("load i8, ptr {class_at}"));
    let class = m.inst(&format!("zext i8 {class8} to i32"));
    // `zeile = zustand * klassen + klasse`, der Eintrag der Tabelle.
    let state = m.inst(&format!("load i32, ptr {state_ptr}"));
    let row = m.inst(&format!("mul i32 {state}, {}", dfa.class_count));
    let off = m.inst(&format!("add i32 {row}, {class}"));
    let cell = m.inst(&format!(
        "getelementptr inbounds [{} x i32], ptr {}, i32 0, i32 {off}",
        dfa.table.len(),
        table_name(id)
    ));
    let next = m.inst(&format!("load i32, ptr {cell}"));
    m.void_inst(&format!("store i32 {next}, ptr {state_ptr}"));
    let next_i = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {next_i}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    // Am Ende: Ist der erreichte Zustand akzeptierend?
    //
    // Die Menge steht zur Uebersetzungszeit fest, also wird sie zu einer
    // Kette von Vergleichen statt zu einer Schleife ueber eine Tabelle:
    // Sie ist typisch ein- bis dreielementig, und LLVM macht daraus
    // einen `switch`, wenn es sich lohnt. Eine Tabelle brauchte einen
    // Zaehler, eine Schleife und drei Marken fuer dieselbe Frage.
    m.label(&check);
    let end_state = m.inst(&format!("load i32, ptr {state_ptr}"));
    let mut hit = m.inst("and i1 false, false");
    for a in &dfa.accept {
        let same = m.inst(&format!("icmp eq i32 {end_state}, {a}"));
        hit = m.inst(&format!("or i1 {hit}, {same}"));
    }
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    Ok(hit)
}
