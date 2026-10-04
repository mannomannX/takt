//! Blockinstanzen und ihre Methoden (5.7).
//!
//! Ein Block ist ein Stueck Zustand mit Methoden: ein Filter, ein Regler,
//! eine Entprellung. Seine Instanz lebt in der Maschine, die ihn haelt,
//! und ueberdauert den Tick — anders als eine reine Funktion (4.4), die
//! nichts behaelt.
//!
//! **Der Instanz-Struct traegt Parameter, Zustandsvariablen, das Ergebnis
//! des Schritts und ein Flag.**
//!
//! ```text
//! { params…, state_vars…, result: R, fault: i32, stepped: i1 }
//! ```
//!
//! Das ist 5.7: Eine Instanz hat je Aktivierung genau ein Ergebnis. Der
//! erste `step` schreitet und legt Wert und Fault-Art ab; jeder weitere
//! liefert sie, ohne zu schreiten — sonst liefe ein Filter, den ein
//! Selbstuebergang im Entry-Modus erneut ruft (9.3), zweimal und haette
//! einen Tick uebersprungen. `result` und `fault` gibt es nur bei einem
//! Block mit `step`. Der Schritt der Maschine setzt das Flag zu Beginn
//! zurueck.
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
    /// Ergebnis und Fault-Art des Schritts, zuletzt das `stepped`-Flag.
    pub fields: Vec<LlvmType>,
    /// Wie viele davon Parameter sind.
    pub params: usize,
    /// Wie viele Parameter und Zustandsvariablen.
    pub vars: usize,
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

    /// Die Indizes von Ergebnis und Fault-Art des Schritts dieser
    /// Aktivierung; ohne `step` gibt es sie nicht. Die Art ist null, wenn
    /// der Schritt einen Wert lieferte.
    pub fn memo(&self) -> Option<(u32, u32)> {
        (self.fields.len() == self.vars + 3).then(|| (self.vars as u32, self.vars as u32 + 1))
    }

    /// Der Index einer Variablen der Instanz.
    ///
    /// Die MIR nummeriert Parameter und Zustandsvariablen in einem Raum
    /// (plan/mir.md, Abschnitt 7), und der Struct uebernimmt diese
    /// Reihenfolge — sonst muesste jede Methode umrechnen.
    pub fn var(&self, id: takt_mir::VarId) -> Option<u32> {
        (id.index() < self.vars).then(|| id.index() as u32)
    }
}

/// Baut den Instanz-Struct eines Blocks.
///
/// Die Reihenfolge ist die des Interpreters (`call_block_method`): erst
/// die Konstruktionsparameter, dann die Zustandsvariablen, dann Ergebnis
/// und Fault-Art des Schritts und das `stepped`-Flag. Die Parameter stehen
/// mit im Struct, obwohl sie sich nie aendern — die MIR nummeriert sie in
/// denselben Raum, und eine zweite Nummerierung waere eine zweite
/// Gelegenheit, sie zu verwechseln.
pub fn instance_of(b: &BlockDef, p: &Program) -> Option<Instance> {
    let mut fields = Vec::with_capacity(b.params.len() + b.state_vars.len() + 3);
    for v in b.params.iter() {
        fields.push(ty::lower(v.ty, p)?);
    }
    for v in &b.state_vars {
        fields.push(ty::lower(v.ty, p)?);
    }
    let params = b.params.len();
    let vars = fields.len();
    // Ergebnis und Flag stehen hinten, damit die Variablennummern der MIR
    // ohne Versatz stimmen.
    if let Some(step) = b.step {
        fields.push(ty::lower(p.fns.get(step.index())?.ret?, p)?);
        fields.push(LlvmType::Int(32));
    }
    fields.push(LlvmType::Int(1));
    Some(Instance { fields, params, vars })
}

/// Der Name der Typdefinition eines Blocks.
pub fn type_name(b: &BlockDef) -> String {
    format!("%{}_block", crate::fns::sanitized(&b.name))
}

/// Der Name einer Methode im erzeugten Code.
///
/// Der Name in der MIR ist bereits `block.methode`; der Punkt ist in
/// einem Symbol nicht zulaessig und wird zu einem Unterstrich. Das
/// Praefix trennt die Methoden von den reinen Funktionen (4.4), die
/// `takt_fn_` tragen.
pub fn method_symbol(b: &BlockDef, method: &str) -> String {
    let _ = b;
    // Der Punkt trennt Block und Methode (`counter.step`); im Symbol
    // wird er zum Unterstrich, damit der Name lesbar bleibt. Alles
    // andere, was LLVM nicht annimmt, bereinigt `sanitized`.
    format!("takt_block_{}", crate::fns::sanitized(&method.replace('.', "_")))
}

/// Schreibt die Typdefinition eines Blocks.
pub fn declare(b: &BlockDef, inst: &Instance, m: &mut Module) {
    let mut text = format!("\n; Blockinstanz `{}` (5.7)", b.name);
    let memo = if inst.memo().is_some() { ", Ergebnis, Fault-Art" } else { "" };
    text.push_str(&format!(
        "\n;   {} Parameter, {} Zustandsvariablen{memo}, `stepped`",
        inst.params,
        b.state_vars.len()
    ));
    text.push_str(&format!("\n{} = type {}", type_name(b), inst.llvm()));
    m.declare(&text);
}

/// Schreibt die Zustandsvariablen einer Instanz auf ihre Initialwerte und
/// nimmt das `stepped`-Flag zurueck (5.7). Die Initialwerte sehen die
/// Parameter, wie im Interpreter (`instantiate_block`).
pub fn init_state(
    ptr: Reg,
    def: &BlockDef,
    inst: &Instance,
    exit: String,
    p: &Program,
    m: &mut Module,
) -> Result<(), NotYet> {
    let struct_ty = inst.llvm();
    let vars = crate::fns::BlockVars::of_instance(ptr, inst, exit);
    for (i, v) in def.state_vars.iter().enumerate() {
        let field = inst.params + i;
        let ty = inst.fields.get(field).ok_or(NotYet { what: "Zustandsvariable" })?.clone();
        let value = match &v.init {
            Some(init) => crate::expr::lower(init, p, m, &vars)?.value,
            None => "zeroinitializer".to_string(),
        };
        let at = m.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {field}"));
        m.write(&ty, &value, &at.to_string());
    }
    let flag = m.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {}", inst.stepped()));
    m.void_inst(&format!("store i1 false, ptr {flag}"));
    Ok(())
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
