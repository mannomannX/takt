//! Reduktionen ueber Arrays und Abtastfenster (8.9, 3.9).
//!
//! **Was hier steht.** `xs.min()`, `.max()`, `.mean()`, `.rms()` und die
//! Zugriffe `.count`/`.last` auf `samples<T, N>` und `array<T, N>`. Die
//! Laenge steht im Typ, also ist jede Schleife statisch beschraenkt —
//! 4.1 verlangt genau das.
//!
//! **An der Stelle, nicht als Wert.** Ein Feld, das einen Ort hat, wird
//! nicht geladen, um es zu reduzieren: Ein `samples<f64, 64>` als Wert
//! sind 64 `extractvalue`, die LLVM erst wieder in Ladebefehle zerlegt
//! (FB-214). Kurze Felder werden abgerollt — die Kette aus Vergleich und
//! `select` hat keine Verzweigung, und LLVM vektorisiert sie, wo das
//! Ziel es hergibt. Lange laufen als Schleife, sonst wuechse der Code
//! mit `N`.
//!
//! **Die Reihenfolge der Summation ist Semantik, nicht Geschmack.**
//! `mean` und `rms` summieren von links nach rechts, wie `Ctx::reduce`
//! im Interpreter. Eine paarweise Summation waere numerisch besser — und
//! liefe gegen 4.2, das bitgleiche Ergebnisse ueber alle Targets
//! verlangt. Der Codegen rechnet darum dieselbe Folge in derselben
//! Reihenfolge, abgerollt wie als Schleife, und das Modul traegt keine
//! Fast-Math-Flags, damit LLVM sie nicht umordnet (`emit`).
//!
//! **`min`/`max` ohne Fliesskomma-Intrinsic.** `llvm.minnum` behandelt
//! NaN eigen; 4.1 schliesst NaN zwar aus, aber die Zusage steht in der
//! Analyse, nicht im erzeugten Code. Ein geordneter Vergleich mit
//! `select` ist das, was der Interpreter tut (`Value::compare`), und
//! bleibt auch ohne diese Zusage richtig.

use takt_mir::expr::Accessor;

use crate::emit::{Module, Reg, float_literal};
use crate::expr::{Lowered, NotYet};
use crate::ty::LlvmType;

/// Bis zu dieser Laenge wird abgerollt; darueber laeuft eine Schleife.
const UNROLLED: u32 = 8;

/// Das Feld einer Reduktion: an seiner Stelle oder als Wert.
pub enum Field {
    /// Eine Stelle; sie wird elementweise gelesen (FB-214).
    At(Reg, LlvmType),
    /// Ein gerechneter Wert; er hat keinen Ort.
    Of(Lowered),
}

impl Field {
    fn ty(&self) -> &LlvmType {
        match self {
            Field::At(_, ty) => ty,
            Field::Of(x) => &x.ty,
        }
    }

    /// Das `i`-te Element.
    fn at(&self, i: u32, elem: &LlvmType, m: &mut Module) -> String {
        match self {
            Field::At(ptr, ty) => {
                let p = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {i}"));
                m.inst(&format!("load {elem}, ptr {p}")).to_string()
            }
            Field::Of(x) => m.inst(&format!("extractvalue {} {}, {i}", x.ty, x.value)).to_string(),
        }
    }

    /// Die Stelle; ein Wert bekommt dafuer einen Slot.
    fn place(&mut self, m: &mut Module) -> Reg {
        match self {
            Field::At(ptr, _) => *ptr,
            Field::Of(x) => {
                let ty = x.ty.clone();
                let slot = m.alloca(&ty);
                m.write(&ty, &x.value, &slot.to_string());
                *self = Field::At(slot, ty);
                slot
            }
        }
    }
}

/// Ob `which` eine Reduktion ueber ein Feld ist.
pub fn reduces(which: Accessor) -> bool {
    matches!(which, Accessor::Count | Accessor::Last | Accessor::Min | Accessor::Max | Accessor::Mean | Accessor::Rms)
}

/// Senkt einen Zugriff auf ein Array oder Abtastfenster (8.9).
///
/// `signed` sagt, wie Ganzzahlen verglichen werden (3.2). `None` heisst:
/// kein Zugriff dieser Art auf einem Feld — der Aufrufer entscheidet
/// dann weiter.
pub fn access(
    which: Accessor,
    x: Field,
    signed: bool,
    want: &LlvmType,
    m: &mut Module,
) -> Option<Result<Lowered, NotYet>> {
    let LlvmType::Array(elem, n) = x.ty() else { return None };
    let (elem, n) = (elem.as_ref().clone(), *n);
    Some(match which {
        // `.count` ist die Laenge; sie steht im Typ (8.9).
        Accessor::Count => Ok(Lowered { value: n.to_string(), ty: want.clone() }),
        Accessor::Last => last(&x, &elem, n, m),
        Accessor::Min | Accessor::Max => min_max(which, x, &elem, signed, n, m),
        Accessor::Mean | Accessor::Rms => mean_rms(which, x, &elem, n, m),
        _ => return None,
    })
}

/// `xs.last` (8.9): das letzte Element.
fn last(x: &Field, elem: &LlvmType, n: u32, m: &mut Module) -> Result<Lowered, NotYet> {
    Ok(Lowered { value: x.at(checked(n)? - 1, elem, m), ty: elem.clone() })
}

/// `xs.min()` und `xs.max()` (8.9).
fn min_max(
    which: Accessor,
    x: Field,
    elem: &LlvmType,
    signed: bool,
    n: u32,
    m: &mut Module,
) -> Result<Lowered, NotYet> {
    let n = checked(n)?;
    // `o`-Praefix heisst „geordnet": falsch bei NaN, wie in `expr`.
    let pred = match (which, elem.is_float(), signed) {
        (Accessor::Min, true, _) => "fcmp olt",
        (Accessor::Max, true, _) => "fcmp ogt",
        (Accessor::Min, false, true) => "icmp slt",
        (Accessor::Max, false, true) => "icmp sgt",
        (Accessor::Min, false, false) => "icmp ult",
        (Accessor::Max, false, false) => "icmp ugt",
        _ => return Err(NotYet { what: "Reduktion" }),
    };
    let first = x.at(0, elem, m);
    let step = |best: &str, cur: &str, m: &mut Module| {
        let better = m.inst(&format!("{pred} {elem} {cur}, {best}"));
        m.inst(&format!("select i1 {better}, {elem} {cur}, {elem} {best}")).to_string()
    };
    Ok(Lowered { value: fold(x, elem, 1, n, first, &step, m), ty: elem.clone() })
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
fn mean_rms(which: Accessor, x: Field, elem: &LlvmType, n: u32, m: &mut Module) -> Result<Lowered, NotYet> {
    let n = checked(n)?;
    if !elem.is_float() {
        // 8.9 nennt `mean` und `rms` fuer Abtastfenster, deren Elemente
        // Fliesskomma sind. Auf Ganzzahlen bliebe die Rundung offen, und
        // eine stille Antwort waere die falsche (4.1).
        return Err(NotYet { what: "`mean`/`rms` auf einem Ganzzahlfeld" });
    }
    let step = |acc: &str, cur: &str, m: &mut Module| {
        let term = if which == Accessor::Rms {
            m.inst(&format!("fmul {elem} {cur}, {cur}")).to_string()
        } else {
            cur.to_string()
        };
        m.inst(&format!("fadd {elem} {acc}, {term}")).to_string()
    };
    let acc = fold(x, elem, 0, n, float_literal(0.0, elem), &step, m);
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

/// Faltet die Elemente ab `from` von links nach rechts in `init`.
///
/// Abgerollt bis `UNROLLED`, darueber als Schleife mit Zaehler und
/// Akkumulator im Slot — dieselbe Folge in derselben Reihenfolge (4.2).
fn fold(
    mut x: Field,
    elem: &LlvmType,
    from: u32,
    n: u32,
    init: String,
    step: &dyn Fn(&str, &str, &mut Module) -> String,
    m: &mut Module,
) -> String {
    if n <= UNROLLED {
        let mut acc = init;
        for i in from..n {
            let cur = x.at(i, elem, m);
            acc = step(&acc, &cur, m);
        }
        return acc;
    }
    let ty = x.ty().clone();
    let ptr = x.place(m);
    let k = m.next_label();
    let (head, done) = (format!("falte{k}"), format!("falte{k}_ende"));
    let i_at = m.alloca(&LlvmType::Int(32));
    let acc_at = m.alloca(elem);
    m.void_inst(&format!("store i32 {from}, ptr {i_at}"));
    m.void_inst(&format!("store {elem} {init}, ptr {acc_at}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_at}"));
    let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {i}"));
    let cur = m.inst(&format!("load {elem}, ptr {at}"));
    let acc = m.inst(&format!("load {elem}, ptr {acc_at}"));
    let next = step(&acc.to_string(), &cur.to_string(), m);
    m.void_inst(&format!("store {elem} {next}, ptr {acc_at}"));
    let i_next = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {i_next}, ptr {i_at}"));
    let more = m.inst(&format!("icmp ult i32 {i_next}, {n}"));
    m.void_inst(&format!("br i1 {more}, label %{head}, label %{done}"));
    m.label(&done);
    m.inst(&format!("load {elem}, ptr {acc_at}")).to_string()
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
