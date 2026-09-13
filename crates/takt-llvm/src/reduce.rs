//! Reduktionen ueber Arrays und Abtastfenster (8.9, 3.9).
//!
//! **Was hier steht.** `xs.min()`, `.max()`, `.mean()`, `.rms()` und die
//! Zugriffe `.count`/`.last` auf `samples<T, N>` und `array<T, N>`. Die
//! Laenge steht im Typ, also ist jede Schleife statisch beschraenkt —
//! 4.1 verlangt genau das.
//!
//! **Abgerollt statt als Schleife.** `N` ist bei Abtastfenstern typisch
//! vier bis hundert (8.9), und die Kette aus Vergleich und `select` hat
//! keine Verzweigung: LLVM vektorisiert sie, wo das Ziel es hergibt.
//! Eine Schleife braeuchte drei Marken und einen Zaehler im Speicher
//! fuer dasselbe Ergebnis. Bei sehr grossen `N` waere die Abwaegung eine
//! andere; die Grenze zieht `takt size` (11.5), nicht dieser Code.
//!
//! **Die Reihenfolge der Summation ist Semantik, nicht Geschmack.**
//! `mean` und `rms` summieren von links nach rechts, wie `Ctx::reduce`
//! im Interpreter. Eine paarweise Summation waere numerisch besser — und
//! liefe gegen 4.2, das bitgleiche Ergebnisse ueber alle Targets
//! verlangt. Der Codegen rechnet darum dieselbe Folge in derselben
//! Reihenfolge, und das Modul traegt keine Fast-Math-Flags, damit LLVM
//! sie nicht umordnet (`emit`).
//!
//! **`min`/`max` ohne Fliesskomma-Intrinsic.** `llvm.minnum` behandelt
//! NaN eigen; 4.1 schliesst NaN zwar aus, aber die Zusage steht in der
//! Analyse, nicht im erzeugten Code. Ein geordneter Vergleich mit
//! `select` ist das, was der Interpreter tut (`Value::compare`), und
//! bleibt auch ohne diese Zusage richtig.

use takt_mir::expr::Accessor;

use crate::emit::{Module, float_literal};
use crate::expr::{Lowered, NotYet};
use crate::ty::LlvmType;

/// Senkt einen Zugriff auf ein Array oder Abtastfenster (8.9).
///
/// `None` heisst: kein Zugriff dieser Art auf einem Feld — der Aufrufer
/// entscheidet dann weiter.
pub fn access(which: Accessor, x: &Lowered, want: &LlvmType, m: &mut Module) -> Option<Result<Lowered, NotYet>> {
    let LlvmType::Array(elem, n) = &x.ty else { return None };
    let (elem, n) = (elem.as_ref().clone(), *n);
    Some(match which {
        // `.count` ist die Laenge; sie steht im Typ (8.9).
        Accessor::Count => Ok(Lowered { value: n.to_string(), ty: want.clone() }),
        Accessor::Last => last(x, &elem, n, m),
        Accessor::Min | Accessor::Max => min_max(which, x, &elem, n, m),
        Accessor::Mean | Accessor::Rms => mean_rms(which, x, &elem, n, m),
        _ => return None,
    })
}

/// `xs.last` (8.9): das letzte Element.
fn last(x: &Lowered, elem: &LlvmType, n: u32, m: &mut Module) -> Result<Lowered, NotYet> {
    let v = m.inst(&format!("extractvalue {} {}, {}", x.ty, x.value, checked(n)? - 1));
    Ok(Lowered { value: v.to_string(), ty: elem.clone() })
}

/// `xs.min()` und `xs.max()` (8.9).
fn min_max(which: Accessor, x: &Lowered, elem: &LlvmType, n: u32, m: &mut Module) -> Result<Lowered, NotYet> {
    let n = checked(n)?;
    let float = elem.is_float();
    let mut best = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value)).to_string();
    for i in 1..n {
        let cur = m.inst(&format!("extractvalue {} {}, {i}", x.ty, x.value));
        // `o`-Praefix heisst „geordnet": falsch bei NaN, wie in `expr`.
        let pred = match (which, float) {
            (Accessor::Min, true) => "fcmp olt",
            (Accessor::Max, true) => "fcmp ogt",
            (Accessor::Min, false) => "icmp slt",
            (Accessor::Max, false) => "icmp sgt",
            _ => return Err(NotYet { what: "Reduktion" }),
        };
        let better = m.inst(&format!("{pred} {elem} {cur}, {best}"));
        best = m.inst(&format!("select i1 {better}, {elem} {cur}, {elem} {best}")).to_string();
    }
    Ok(Lowered { value: best, ty: elem.clone() })
}

/// `xs.mean()` und `xs.rms()` (8.9).
///
/// `rms` quadriert vor der Summe und zieht am Ende die Wurzel. Die
/// Wurzel kommt als `llvm.sqrt`, nicht ueber `libtaktm`: IEEE-754
/// verlangt fuer sie ein *korrekt gerundetes* Ergebnis, und ein korrekt
/// gerundetes Ergebnis ist eindeutig — also plattformunabhaengig, wie
/// die Referenz es fuer 4.2 begruendet (Abschnitt 16). `libtaktm` ruft
/// seinerseits nur `x.sqrt()`; ein Umweg ueber die Runtime braechte
/// dieselbe Zahl und ein Symbol, das jedes Profil bereitstellen muesste.
fn mean_rms(which: Accessor, x: &Lowered, elem: &LlvmType, n: u32, m: &mut Module) -> Result<Lowered, NotYet> {
    let n = checked(n)?;
    if !elem.is_float() {
        // 8.9 nennt `mean` und `rms` fuer Abtastfenster, deren Elemente
        // Fliesskomma sind. Auf Ganzzahlen bliebe die Rundung offen, und
        // eine stille Antwort waere die falsche (4.1).
        return Err(NotYet { what: "`mean`/`rms` auf einem Ganzzahlfeld" });
    }
    let mut acc = float_literal(0.0, elem);
    for i in 0..n {
        let cur = m.inst(&format!("extractvalue {} {}, {i}", x.ty, x.value));
        let term = if which == Accessor::Rms {
            m.inst(&format!("fmul {elem} {cur}, {cur}")).to_string()
        } else {
            cur.to_string()
        };
        acc = m.inst(&format!("fadd {elem} {acc}, {term}")).to_string();
    }
    let count = float_literal(f64::from(n), elem);
    let mean = m.inst(&format!("fdiv {elem} {acc}, {count}"));
    if which == Accessor::Mean {
        return Ok(Lowered { value: mean.to_string(), ty: elem.clone() });
    }
    let s = suffix(elem)?;
    m.needs_intrinsic(&format!("{elem} @llvm.sqrt.{s}({elem})"));
    let root = m.inst(&format!("call {elem} @llvm.sqrt.{s}({elem} {mean})"));
    Ok(Lowered { value: root.to_string(), ty: elem.clone() })
}

/// Das Kuerzel, mit dem LLVM ein Intrinsic ueber die Breite auswaehlt.
fn suffix(elem: &LlvmType) -> Result<&'static str, NotYet> {
    match elem {
        LlvmType::F64 => Ok("f64"),
        LlvmType::F32 => Ok("f32"),
        _ => Err(NotYet { what: "Wurzel auf einem Nicht-Fliesskommatyp" }),
    }
}

/// Eine Reduktion braucht mindestens ein Element (8.9).
///
/// Der Interpreter faultet mit `MissingValue`; hier kann der Fall nicht
/// entstehen, weil `N` im Typ steht. Die Pruefung bleibt, weil eine
/// Annahme, die nicht im Typ steht, irgendwann nicht mehr gilt.
fn checked(n: u32) -> Result<u32, NotYet> {
    if n == 0 { Err(NotYet { what: "Reduktion ueber ein leeres Feld" }) } else { Ok(n) }
}
