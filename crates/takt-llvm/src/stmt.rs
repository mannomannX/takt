//! Anweisungen nach LLVM-IR (11.2).
//!
//! 11.2 gibt die Form vor: „`loop:`-Koerper als Straight-Line-Code;
//! `check` → Vergleich + bedingter Sprung in den Fault-Trampolin der
//! Maschine; `->` → Setzen der Goto-Vormerkung + Sprung ans Kettenende."
//!
//! **Der Zustand liegt im Speicher, nicht in Registern.** Eine Variable
//! ist ein Feld des Zustands-Structs (11.2), also `getelementptr` und
//! `load`/`store`. Das sieht teuer aus und ist es nicht: LLVM hebt mit
//! `mem2reg` heraus, was nicht im Speicher bleiben muss. Der Codegen
//! selbst entscheidet das nicht — er waere dabei schlechter als LLVM und
//! muesste die Entscheidung gegen jede Optimierung verteidigen.

use takt_mir::expr::Expr;
use takt_mir::machine::Machine;
use takt_mir::program::Program;
use takt_mir::stmt::{Block, Place, Stmt, StmtKind};

use crate::emit::{Module, Reg};
use crate::expr::{Lowered, NotYet, Vars, lower as lower_expr};
use crate::machine::{Role, StateStruct};
use crate::ty::{self, LlvmType};

/// Was beim Senken einer Anweisung gebraucht wird.
///
/// Die Felder stehen zusammen, weil sie zusammen gehoeren: Ohne den
/// Struct kennt man die Feldindizes nicht, ohne die Maschine nicht den
/// Namen des Trampolins.
pub struct Ctx<'a> {
    /// Die Maschine, deren Schritt entsteht.
    pub machine: &'a Machine,
    /// Ihr Zustands-Struct (11.2).
    pub state: &'a StateStruct,
    /// Das Programm.
    pub program: &'a Program,
    /// Wie oft schon ein Fault-Zweig entstanden ist; die Marken muessen
    /// eindeutig sein.
    checks: u32,
}

impl<'a> Ctx<'a> {
    /// Ein Kontext fuer eine Maschine.
    pub fn new(machine: &'a Machine, state: &'a StateStruct, program: &'a Program) -> Ctx<'a> {
        Ctx { machine, state, program, checks: 0 }
    }

    /// Die Variablenabbildung dieser Maschine.
    pub fn vars(&self) -> StateVars<'a> {
        StateVars { machine: self.machine, state: self.state, program: self.program }
    }

    /// Der Zeiger auf das `n`-te Feld einer Rolle im Zustands-Struct.
    ///
    /// Das ist die einzige Stelle, an der ein Feldindex in Code wird —
    /// `StateStruct` ist die Quelle, und hier wird sie gelesen.
    pub fn field(&self, role: Role, nth: usize, m: &mut Module) -> Option<Reg> {
        let i = self.state.index_of(role, nth)?;
        let ty = format!("%{}_state", self.machine.name);
        Some(m.inst(&format!("getelementptr inbounds {ty}, ptr %0, i32 0, i32 {i}")))
    }

    /// Der Name des Fault-Trampolins dieser Maschine.
    pub fn trampoline(&self) -> String {
        format!("fault_{}", self.machine.name)
    }
}

/// Die Variablen einer Maschine: Felder des Zustands-Structs (11.2).
///
/// Sie stehen im Speicher, nicht in Registern. Das sieht teuer aus und
/// ist es nicht — LLVM hebt mit `mem2reg` heraus, was nicht im Speicher
/// bleiben muss. Der Codegen waere dabei schlechter als LLVM.
pub struct StateVars<'a> {
    /// Die Maschine.
    pub machine: &'a Machine,
    /// Ihr Zustands-Struct.
    pub state: &'a StateStruct,
    /// Das Programm, fuer die Typen.
    pub program: &'a Program,
}

impl StateVars<'_> {
    /// Der Zeiger auf das `n`-te Feld eines Abbilds.
    ///
    /// Prozessabbild, Ψ und Latch sind Arrays je Channel — ihre
    /// Reihenfolge ist die der `ChannelId`, wie im Interpreter. Eine
    /// zweite Reihenfolge waere eine zweite Gelegenheit, sie verschieden
    /// zu waehlen.
    fn slot(&self, base: &str, ty: &LlvmType, index: usize, m: &mut Module) -> Lowered {
        let ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {base}, i32 {index}"));
        let v = m.inst(&format!("load {ty}, ptr {ptr}"));
        Lowered { value: v.to_string(), ty: ty.clone() }
    }
}

impl Vars for StateVars<'_> {
    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered> {
        let def = self.machine.vars.get(id.index())?;
        let ty = ty::lower(def.ty, self.program)?;
        let i = self.state.index_of(Role::Var, id.index())?;
        let state_ty = format!("%{}_state", self.machine.name);
        let ptr = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
        let v = m.inst(&format!("load {ty}, ptr {ptr}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    /// Der Wert eines Inputs; `%1` ist das Prozessabbild (11.2).
    fn input(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        let c = self.program.channels.get(channel.index())?;
        let ty = ty::lower(c.ty, self.program)?;
        Some(self.slot("%1", &ty, channel.index(), m))
    }

    /// Der Wert eines Parameters; `%2` traegt Ψ und den Parametervektor.
    fn param(&self, id: takt_mir::ParamId, m: &mut Module) -> Option<Lowered> {
        let p = self.program.params.get(id.index())?;
        let ty = ty::lower(p.ty, self.program)?;
        Some(self.slot("%2", &ty, id.index(), m))
    }

    /// Der Latch eines eigenen Outputs; `%3` ist der Latch (11.2).
    fn output(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        let c = self.program.channels.get(channel.index())?;
        let ty = ty::lower(c.ty, self.program)?;
        Some(self.slot("%3", &ty, channel.index(), m))
    }
}

/// Senkt einen Block (11.2: Straight-Line-Code).
pub fn block(b: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    for s in &b.stmts {
        stmt(s, ctx, m)?;
    }
    Ok(())
}

/// Senkt eine Anweisung.
pub fn stmt(s: &Stmt, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    match &s.kind {
        StmtKind::Assign { target, value } => assign(target, value, ctx, m),
        StmtKind::Check { cond, kind, .. } => check(cond, *kind, ctx, m),
        StmtKind::If { cond, then, otherwise } => branch(cond, then, otherwise, ctx, m),
        _ => Err(NotYet { what: "Anweisung" }),
    }
}

/// `x = e`: Wert berechnen, in den Speicherort schreiben.
fn assign(target: &Place, value: &Expr, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let v = lower_expr(value, ctx.program, m, &vars)?;
    match target {
        Place::Var(id) => {
            let ptr = ctx.field(Role::Var, id.index(), m).ok_or(NotYet { what: "Variable im Zustand" })?;
            m.void_inst(&format!("store {} {}, ptr {ptr}", v.ty, v.value));
        }
        // 9.2: Ein Output wird in den Latch geschrieben; die Runtime
        // committet ihn (12.1). `%3` ist der Latch (11.2).
        Place::Output(c) => {
            let ptr = m.inst(&format!("getelementptr inbounds {}, ptr %3, i32 {}", v.ty, c.index()));
            m.void_inst(&format!("store {} {}, ptr {ptr}", v.ty, v.value));
        }
        _ => return Err(NotYet { what: "Zuweisungsziel" }),
    }
    Ok(())
}

/// `check c`: Vergleich und bedingter Sprung in den Trampolin (11.2).
///
/// Die Bedingung ist die *gute* Seite: `check p < LIMIT` haelt, solange
/// `p < LIMIT` gilt. Der Sprung geht also bei `false` in den Trampolin —
/// und der `weiter`-Block ist der heisse Pfad, was 11.2 mit „die
/// Fault-Pfade sind `cold`" meint.
fn check(cond: &Expr, kind: takt_mir::stmt::CheckKind, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let c = lower_expr(cond, ctx.program, m, &vars)?;
    if c.ty != LlvmType::Int(1) {
        return Err(NotYet { what: "Bedingung ist kein `bool`" });
    }
    ctx.checks += 1;
    let go_on = format!("weiter{}_{}", ctx.checks, ctx.machine.name);
    let _ = kind;
    m.void_inst(&format!("br i1 {}, label %{go_on}, label %{}", c.value, ctx.trampoline()));
    m.label(&go_on);
    Ok(())
}

/// `if c: … else: …`
fn branch(cond: &Expr, then: &Block, otherwise: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let c = lower_expr(cond, ctx.program, m, &vars)?;
    ctx.checks += 1;
    let n = ctx.checks;
    let name = &ctx.machine.name;
    let (t, f, end) = (format!("dann{n}_{name}"), format!("sonst{n}_{name}"), format!("ende{n}_{name}"));
    m.void_inst(&format!("br i1 {}, label %{t}, label %{f}", c.value));
    m.label(&t);
    block(then, ctx, m)?;
    m.void_inst(&format!("br label %{end}"));
    m.label(&f);
    block(otherwise, ctx, m)?;
    m.void_inst(&format!("br label %{end}"));
    m.label(&end);
    Ok(())
}
