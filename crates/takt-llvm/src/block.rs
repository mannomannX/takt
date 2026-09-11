//! Blockinstanzen und ihre Methoden (5.7).
//!
//! Ein Block ist ein Stueck Zustand mit Methoden: ein Filter, ein Regler,
//! eine Entprellung. Seine Instanz lebt in der Maschine, die ihn haelt,
//! und ueberdauert den Tick — anders als eine reine Funktion (4.4), die
//! nichts behaelt.
//!
//! **Der Instanz-Struct traegt Parameter, Zustandsvariablen und ein
//! Flag.**
//!
//! ```text
//! { params…, state_vars…, stepped: i1 }
//! ```
//!
//! Das Flag ist 5.7: „`step` hoechstens einmal je Aktivierung". Ohne es
//! liefe ein Filter, der in zwei Zweigen desselben Ticks gerufen wird,
//! zweimal — und sein Zustand haette einen Tick uebersprungen, ohne dass
//! es jemand saehe. Die Runtime setzt es zu Tick-Beginn zurueck.
//!
//! **Die Methoden sind gewoehnliche Funktionen mit einem Zeiger als
//! erstem Parameter.** Sie aendern den Zustand der Instanz, also bekommen
//! sie ihn als Speicherort, nicht als Wert — dieselbe Ueberlegung wie bei
//! den Sammlungen (3.9).

use takt_mir::BlockId;
use takt_mir::fns::BlockDef;
use takt_mir::program::Program;

use crate::emit::{Module, Reg};
use crate::expr::NotYet;
use crate::ty::{self, LlvmType};

/// Der Aufbau einer Blockinstanz (5.7).
#[derive(Clone, Debug, PartialEq)]
pub struct Instance {
    /// Die Felder: erst die Parameter, dann die Zustandsvariablen, dann
    /// das `stepped`-Flag.
    pub fields: Vec<LlvmType>,
    /// Wie viele davon Parameter sind.
    pub params: usize,
}

impl Instance {
    /// Der Struct als LLVM-Typ.
    pub fn llvm(&self) -> LlvmType {
        LlvmType::Struct(self.fields.clone())
    }

    /// Der Index des `stepped`-Flags (5.7).
    pub fn stepped(&self) -> u32 {
        self.fields.len() as u32 - 1
    }

    /// Der Index einer Variablen der Instanz.
    ///
    /// Die MIR nummeriert Parameter und Zustandsvariablen in einem Raum
    /// (plan/mir.md, Abschnitt 7), und der Struct uebernimmt diese
    /// Reihenfolge — sonst muesste jede Methode umrechnen.
    pub fn var(&self, id: takt_mir::VarId) -> Option<u32> {
        (id.index() < self.fields.len() - 1).then(|| id.index() as u32)
    }
}

/// Baut den Instanz-Struct eines Blocks.
///
/// Die Reihenfolge ist die des Interpreters (`call_block_method`): erst
/// die Konstruktionsparameter, dann die Zustandsvariablen, dann das
/// `stepped`-Flag. Die Parameter stehen mit im Struct, obwohl sie sich
/// nie aendern — die MIR nummeriert sie in denselben Raum, und eine
/// zweite Nummerierung waere eine zweite Gelegenheit, sie zu verwechseln.
pub fn instance_of(b: &BlockDef, p: &Program) -> Option<Instance> {
    let mut fields = Vec::with_capacity(b.params.len() + b.state_vars.len() + 1);
    for v in b.params.iter() {
        fields.push(ty::lower(v.ty, p)?);
    }
    for v in &b.state_vars {
        fields.push(ty::lower(v.ty, p)?);
    }
    let params = b.params.len();
    // Das Flag steht am Ende, damit die Variablennummern der MIR ohne
    // Versatz stimmen.
    fields.push(LlvmType::Int(1));
    Some(Instance { fields, params })
}

/// Der Name der Typdefinition eines Blocks.
pub fn type_name(b: &BlockDef) -> String {
    format!("%{}_block", b.name)
}

/// Der Name einer Methode im erzeugten Code.
///
/// Der Name in der MIR ist bereits `block.methode`; der Punkt ist in
/// einem Symbol nicht zulaessig und wird zu einem Unterstrich. Das
/// Praefix trennt die Methoden von den reinen Funktionen (4.4), die
/// `takt_fn_` tragen.
pub fn method_symbol(b: &BlockDef, method: &str) -> String {
    let _ = b;
    format!("takt_block_{}", method.replace('.', "_"))
}

/// Schreibt die Typdefinition eines Blocks.
pub fn declare(b: &BlockDef, inst: &Instance, m: &mut Module) {
    let mut text = format!("\n; Blockinstanz `{}` (5.7)", b.name);
    text.push_str(&format!("\n;   {} Parameter, {} Zustandsvariablen, `stepped`", inst.params, b.state_vars.len()));
    text.push_str(&format!("\n{} = type {}", type_name(b), inst.llvm()));
    m.declare(&text);
}

/// Setzt das `stepped`-Flag einer Instanz (5.7).
///
/// 5.7: „`step` hoechstens einmal je Aktivierung". Der erzeugte Code
/// prueft das Flag vor dem Aufruf; die Runtime setzt es zu Tick-Beginn
/// zurueck. Dass es *hier* geprueft wird und nicht erst in der Methode,
/// ist die Bedingung dafuer, dass ein zweiter Aufruf gar nicht erst
/// stattfindet — 5.7 macht ihn zu einem Fehler, nicht zu einer Wiederholung.
pub fn guard_stepped(ptr: Reg, inst: &Instance, block: &BlockDef, label: u32, m: &mut Module) -> Result<(), NotYet> {
    let ty = type_name(block);
    let flag = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {}", inst.stepped()));
    let done = m.inst(&format!("load i1, ptr {flag}"));
    let (weiter, ende) = (format!("step{label}"), format!("step{label}_ende"));
    m.void_inst(&format!("br i1 {done}, label %{ende}, label %{weiter}"));
    m.label(&weiter);
    m.void_inst(&format!("store i1 true, ptr {flag}"));
    // Der Aufrufer schreibt den Aufruf; `finish_stepped` schliesst.
    let _ = ende;
    Ok(())
}

/// Schliesst den Zweig aus [`guard_stepped`].
pub fn finish_stepped(label: u32, m: &mut Module) {
    let ende = format!("step{label}_ende");
    m.void_inst(&format!("br label %{ende}"));
    m.label(&ende);
}

/// Setzt alle Felder einer Instanz auf ihren Anfangswert (5.7).
///
/// `reset()` stellt den Zustand her, den die Instanz beim Start hatte —
/// die Parameter bleiben, die Zustandsvariablen gehen auf ihre
/// Initialwerte zurueck.
pub fn reset_symbol(b: &BlockDef) -> String {
    method_symbol(b, "reset")
}

/// Der Block zu einer Id.
pub fn block_of(p: &Program, id: BlockId) -> Option<&BlockDef> {
    p.blocks.get(id.index())
}
