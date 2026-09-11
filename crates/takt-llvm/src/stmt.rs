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
use takt_mir::stmt::{Block, Observe, Place, Stmt, StmtKind};

use crate::abi::Abi;
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
    /// Die Nummer der Maschine im Programm; sie geht in jeden
    /// Runtime-Aufruf (`crate::abi`).
    pub machine_index: u32,
    /// Wie viele Meldungsstellen die Maschine schon hat.
    ///
    /// Der Index identifiziert die Stelle im Trace; die Reihenfolge ist
    /// die der Erzeugung und damit die des Quelltexts.
    sites: u32,
}

impl<'a> Ctx<'a> {
    /// Ein Kontext fuer eine Maschine.
    pub fn new(machine: &'a Machine, state: &'a StateStruct, program: &'a Program) -> Ctx<'a> {
        let machine_index = program.machines.iter().position(|m| m.name == machine.name).unwrap_or(0) as u32;
        Ctx { machine, state, program, checks: 0, machine_index, sites: 0 }
    }

    /// Eine frische Nummer fuer eine Meldungsstelle (9.3).
    pub fn next_site(&mut self) -> u32 {
        let n = self.sites;
        self.sites += 1;
        n
    }

    /// Eine frische Nummer fuer eine Marke.
    ///
    /// Marken muessen je erzeugter Verzweigung eindeutig sein, nicht je
    /// Zustand oder Anweisung: Derselbe Block kann mehrfach erzeugt
    /// werden, etwa der `loop:` einer Zwischenebene je Blatt darunter
    /// (5.2).
    pub fn next_label(&mut self) -> u32 {
        self.checks += 1;
        self.checks
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
    /// Ein Feld des Abbild-Eintrags eines Channels (`crate::image`).
    fn image_slot(&self, channel: takt_mir::ChannelId, slot: crate::image::Slot, m: &mut Module) -> Option<Lowered> {
        let entry = crate::image::entry_type(channel, self.program)?;
        let LlvmType::Struct(fields) = &entry else { return None };
        let ty = fields.get(slot as usize)?.clone();
        let off = crate::image::offset_of(channel, self.program)?;
        // Der Versatz wird aufsummiert, weil die Eintraege verschieden
        // gross sind; `image` begruendet das.
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
        let field = m.inst(&format!("getelementptr inbounds {entry}, ptr {at}, i32 0, i32 {}", slot as usize));
        let v = m.inst(&format!("load {ty}, ptr {field}"));
        Some(Lowered { value: v.to_string(), ty })
    }

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
    ///
    /// Der Aufbau steht in `crate::image`: je Channel ein Eintrag aus
    /// Wert, Qualitaet, Grund und Alter.
    fn input(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        self.image_slot(channel, crate::image::Slot::Value, m)
    }

    /// Ein Feld des Abbild-Eintrags (3.5).
    fn quality(&self, channel: takt_mir::ChannelId, slot: crate::image::Slot, m: &mut Module) -> Option<Lowered> {
        self.image_slot(channel, slot, m)
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

    /// Ein Command (8.5): ein `bool` im Prozessabbild, hinter den
    /// Channels. Die Runtime setzt es vor dem Schritt und loescht es
    /// danach — im erzeugten Code ist es ein gewoehnlicher Ladevorgang.
    fn command(&self, id: takt_mir::CommandId, m: &mut Module) -> Option<Lowered> {
        let after = self.program.channels.len();
        Some(self.slot("%1", &LlvmType::Int(1), after + id.index(), m))
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
        StmtKind::Observe(o) => observe(o, ctx, m),
        StmtKind::Abort { .. } => {
            // 5.4: `abort` faultet *alle* Maschinen im selben Tick. Der
            // erzeugte Code kann das nicht selbst — er kennt die anderen
            // nicht —, also ruft er die Runtime und verlaesst den Schritt.
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(i32 {}, i32 {site})", Abi::ABORT, ctx.machine_index));
            m.void_inst("ret void");
            Ok(())
        }
        other => Err(NotYet { what: crate::scope::stmt_name(other) }),
    }
}

/// `x = e`: Wert berechnen, in den Speicherort schreiben.
fn assign(target: &Place, value: &Expr, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let v = lower_expr(value, ctx.program, m, &vars)?;
    let (ptr, _) = place(target, ctx, m)?;
    m.void_inst(&format!("store {} {}, ptr {ptr}", v.ty, v.value));
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

/// Eine Beobachtung (9.3): `alert`, `log`, `measure`, `verify`.
///
/// Beobachtungen aendern den Zustand nicht — sie tragen etwas nach
/// draussen. Der erzeugte Code ruft dafuer die Runtime (`crate::abi`);
/// der Text steht als Index in einer Tabelle, nicht als Zeichenkette im
/// Aufruf.
fn observe(o: &Observe, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let machine = ctx.machine_index;
    match o {
        Observe::Alert { cond, .. } => {
            let vars = ctx.vars();
            let c = lower_expr(cond, ctx.program, m, &vars)?;
            let site = ctx.next_site();
            // Die Flanke bildet die Runtime: Sie kennt den vorigen Wert,
            // der erzeugte Code muesste ihn sonst im Zustand fuehren.
            m.void_inst(&format!("call void @{}(i32 {machine}, i32 {site}, i1 {})", Abi::ALERT, c.value));
            Ok(())
        }
        Observe::Log(_) => {
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(i32 {machine}, i32 {site})", Abi::LOG,));
            Ok(())
        }
        Observe::Measure { value, .. } => {
            let vars = ctx.vars();
            let v = lower_expr(value, ctx.program, m, &vars)?;
            // Der Report rechnet in `double`, unabhaengig von der Breite
            // des Programms (13.2): Ein Messwert ist eine Zahl fuer
            // Menschen, keine, mit der weitergerechnet wird.
            let as_double = match v.ty {
                LlvmType::F64 => v.value.clone(),
                LlvmType::F32 => m.inst(&format!("fpext float {} to double", v.value)).to_string(),
                LlvmType::Int(_) => m.inst(&format!("sitofp {} {} to double", v.ty, v.value)).to_string(),
                _ => return Err(NotYet { what: "`measure` auf diesem Typ" }),
            };
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(i32 {machine}, i32 {site}, double {as_double})", Abi::MEASURE));
            Ok(())
        }
        Observe::Verify { cond, .. } => {
            let vars = ctx.vars();
            let c = lower_expr(cond, ctx.program, m, &vars)?;
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(i32 {machine}, i32 {site}, i1 {})", Abi::VERIFY, c.value));
            Ok(())
        }
        _ => Err(NotYet { what: "Beobachtung" }),
    }
}

/// Der Speicherort eines Zuweisungsziels und sein Typ (11.2).
///
/// Felder und Elemente werden ueber `getelementptr` erreicht, nicht ueber
/// `insertvalue` in einen geladenen Wert: Eine Zuweisung an `s.f` soll
/// *das Feld* schreiben, nicht den ganzen Record neu bauen. Der Unterschied
/// ist bei einem `bytes<256>` der zwischen einem Byte und 256.
fn place(target: &Place, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(Reg, LlvmType), NotYet> {
    match target {
        Place::Var(id) => {
            let def = ctx.machine.vars.get(id.index()).ok_or(NotYet { what: "Variable" })?;
            let ty = ty::lower(def.ty, ctx.program).ok_or(NotYet { what: "Variablentyp" })?;
            let ptr = ctx.field(Role::Var, id.index(), m).ok_or(NotYet { what: "Variable im Zustand" })?;
            Ok((ptr, ty))
        }
        // 9.2: Ein Output wird in den Latch geschrieben; die Runtime
        // committet ihn (12.1). `%3` ist der Latch (11.2).
        Place::Output(c) => {
            let ch = ctx.program.channels.get(c.index()).ok_or(NotYet { what: "Channel" })?;
            let ty = ty::lower(ch.ty, ctx.program).ok_or(NotYet { what: "Channeltyp" })?;
            let ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr %3, i32 {}", c.index()));
            Ok((ptr, ty))
        }
        Place::Field(base, field) => {
            let (ptr, ty) = place(base, ctx, m)?;
            let LlvmType::Struct(fields) = &ty else { return Err(NotYet { what: "Feld eines Nicht-Records" }) };
            let inner = fields.get(*field as usize).cloned().ok_or(NotYet { what: "Feldnummer" })?;
            let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {field}"));
            Ok((at, inner))
        }
        Place::Index(base, index) => {
            let (ptr, ty) = place(base, ctx, m)?;
            let LlvmType::Array(elem, _) = &ty else { return Err(NotYet { what: "Index auf Nicht-Array" }) };
            let vars = ctx.vars();
            let i = lower_expr(index, ctx.program, m, &vars)?;
            // Die Grenze prueft der `Checked`-Knoten der MIR (4.1); hier
            // steht nur der Zugriff.
            let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, {} {}", i.ty, i.value));
            Ok((at, (**elem).clone()))
        }
        Place::Index2(..) => Err(NotYet { what: "Matrixelement als Ziel" }),
    }
}
