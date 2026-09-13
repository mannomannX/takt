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

use takt_mir::expr::{Expr, ExprKind};
use takt_mir::machine::Machine;
use takt_mir::program::Program;
use takt_mir::stmt::{Block, Method, Observe, Place, Stmt, StmtKind};

use crate::abi::Abi;
use crate::collection;
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
    /// Das Blatt, dessen Zweig gerade entsteht; sein Fault-Ziel gilt.
    pub leaf: Option<takt_mir::StateId>,
    /// Wie viele Meldungsstellen die Maschine schon hat.
    ///
    /// Der Index identifiziert die Stelle im Trace; die Reihenfolge ist
    /// die der Erzeugung und damit die des Quelltexts.
    sites: u32,
    /// Sprungziele der laufenden Schleifen; `break` nimmt das oberste.
    breaks: Vec<String>,
}

impl<'a> Ctx<'a> {
    /// Ein Kontext fuer eine Maschine.
    pub fn new(machine: &'a Machine, state: &'a StateStruct, program: &'a Program) -> Ctx<'a> {
        let machine_index = program.machines.iter().position(|m| m.name == machine.name).unwrap_or(0) as u32;
        Ctx { machine, state, program, checks: 0, machine_index, leaf: None, sites: 0, breaks: Vec::new() }
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
        StateVars { machine: self.machine, leaf: self.leaf, state: self.state, program: self.program }
    }

    /// Der Zeiger auf das `n`-te Feld einer Rolle im Zustands-Struct.
    ///
    /// Das ist die einzige Stelle, an der ein Feldindex in Code wird —
    /// `StateStruct` ist die Quelle, und hier wird sie gelesen.
    pub fn field(&self, role: Role, nth: usize, m: &mut Module) -> Option<Reg> {
        let i = self.state.index_of(role, nth)?;
        let ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        Some(m.inst(&format!("getelementptr inbounds {ty}, ptr %0, i32 0, i32 {i}")))
    }

    /// Der Name des Fault-Trampolins des laufenden Blatts (5.3).
    ///
    /// Je Blatt einer, weil das Fault-Ziel am innersten Zustand haengt,
    /// der eines deklariert (Fault-Wald).
    pub fn trampoline(&self) -> String {
        format!("fault_{}_{}", self.machine.name, self.leaf.map_or(0, |s| s.index()))
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
    /// Das Blatt, dessen Zweig entsteht; sein Fault-Ziel gilt (5.3).
    pub leaf: Option<takt_mir::StateId>,
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
}

impl Vars for StateVars<'_> {
    fn fault_label(&self) -> Option<String> {
        Some(format!("fault_{}_{}", self.machine.name, self.leaf?.index()))
    }

    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered> {
        let def = self.machine.vars.get(id.index())?;
        let ty = ty::lower(def.ty, self.program)?;
        let i = self.state.index_of(Role::Var, id.index())?;
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
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

    /// Eine eingebaute Groesse (3.3).
    ///
    /// `tick` ist eine Konstante des Programms und steht direkt in der
    /// IR. `now` fuehrt die Runtime, weil alle Maschinen dieselbe Uhr
    /// lesen — eine Kopie je Maschine waere eine zweite Quelle fuer
    /// dieselbe Zahl. `time_in_state` steht im Zustand.
    fn builtin(&self, b: takt_mir::expr::Builtin, p: &Program, m: &mut Module) -> Option<Lowered> {
        use takt_mir::expr::Builtin as B;
        let dur = LlvmType::Int(64);
        match b {
            B::Tick => Some(Lowered { value: p.config.tick.to_string(), ty: dur }),
            B::Now => {
                let v = m.inst(&format!("call i64 @{}()", crate::abi::Abi::NOW));
                Some(Lowered { value: v.to_string(), ty: dur })
            }
            B::TimeInState => {
                let i = self.state.index_of(Role::TimeInState, 0)?;
                let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
                let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
                let cell =
                    m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", self.state.depth));
                let ticks = m.inst(&format!("load i64, ptr {cell}"));
                // Der Zaehler zaehlt Aktivierungen; die Zeit ist ihre
                // Zahl mal der Periode (7.2), wie in `after`.
                let per = i64::from(self.machine.period.max(1)).saturating_mul(p.config.tick);
                let ns = m.inst(&format!("mul i64 {ticks}, {per}"));
                Some(Lowered { value: ns.to_string(), ty: dur })
            }
            B::LastFault | B::Event => None,
        }
    }

    /// Der Wert eines Parameters; `%2` traegt Ψ und den Parametervektor.
    fn param(&self, id: takt_mir::ParamId, m: &mut Module) -> Option<Lowered> {
        let p = self.program.params.get(id.index())?;
        let ty = ty::lower(p.ty, self.program)?;
        let off = crate::image::param_offset(id, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %2, i64 {off}"));
        let v = m.inst(&format!("load {ty}, ptr {at}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    /// Der Latch eines eigenen Outputs; `%3` ist der Latch (11.2).
    fn output(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        let c = self.program.channels.get(channel.index())?;
        let ty = ty::lower(c.ty, self.program)?;
        let off = crate::image::latch_offset(channel, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %3, i64 {off}"));
        let v = m.inst(&format!("load {ty}, ptr {at}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    /// Ein Command (8.5): ein `bool` im Prozessabbild, hinter den
    /// Channels. Die Runtime setzt es vor dem Schritt und loescht es
    /// danach — im erzeugten Code ist es ein gewoehnlicher Ladevorgang.
    fn command(&self, id: takt_mir::CommandId, m: &mut Module) -> Option<Lowered> {
        let off = crate::image::command_offset(id, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
        let raw = m.inst(&format!("load i8, ptr {at}"));
        // Ein Command ist ein Byte im Abbild; `bool` ist `i1`.
        let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
        Some(Lowered { value: b.to_string(), ty: LlvmType::Int(1) })
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
        StmtKind::Match { subject, arms } => match_stmt(subject, arms, ctx, m),
        StmtKind::MethodCall { target, receiver, method, args } => {
            method_call(target.as_ref(), receiver, *method, args, ctx, m)
        }
        StmtKind::Abort { .. } => {
            // 5.4: `abort` faultet *alle* Maschinen im selben Tick. Der
            // erzeugte Code kann das nicht selbst — er kennt die anderen
            // nicht —, also ruft er die Runtime und verlaesst den Schritt.
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(i32 {}, i32 {site})", Abi::ABORT, ctx.machine_index));
            m.void_inst("ret void");
            Ok(())
        }
        StmtKind::ForRange { var, count, body } => for_range(*var, count, body, ctx, m),
        StmtKind::Break => {
            // 4.1: Die Schleife hat eine statische Schranke; `break`
            // verlaesst sie vorzeitig.
            let Some(target) = ctx.breaks.last().cloned() else {
                return Err(NotYet { what: "`break` ausserhalb einer Schleife" });
            };
            m.void_inst(&format!("br label %{target}"));
            Ok(())
        }
        StmtKind::Pass => Ok(()),
        StmtKind::Send { stream, value, len_max } => send(*stream, value, *len_max, ctx, m),
        StmtKind::Every { period, counter, body } => every(period, *counter, body, ctx, m),
        other => Err(NotYet { what: crate::scope::stmt_name(other) }),
    }
}

/// `send o, e` (8.8): Der Wert geht in den Sendepuffer des Stroms.
///
/// Der Text entsteht in einem Puffer auf dem Stack — seine Hoechstlaenge
/// steht in `len_max`, und 8.8 prueft statisch, dass die Summe aller
/// erreichbaren `send` die `capacity` nicht uebersteigt. Die Runtime
/// nimmt ihn entgegen; passt er nicht in `tx.free`, ist das ein
/// `StreamOverflow` (8.8), und der Fault-Trampolin faengt ihn.
fn send(
    stream: takt_mir::expr::StreamRef,
    value: &Expr,
    len_max: u32,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let sid = match stream {
        takt_mir::expr::StreamRef::Channel(c) => i64::from(c.0),
        takt_mir::expr::StreamRef::Internal(s) => -1 - i64::from(s.0),
        _ => return Err(NotYet { what: "`send` auf einem Strom ohne feste Nummer" }),
    };
    // Der Puffer: `{ i32 len, [len_max x i8] }`, wie `str<N>` (3.9).
    let ty = LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), len_max)]);
    let buffer = m.inst(&format!("alloca {ty}"));
    let vars = ctx.vars();
    match &value.kind {
        // Der haeufige Fall: ein Formatstring (8.8). Er wird an Ort und
        // Stelle gebaut, statt als Wert erzeugt und dann kopiert.
        takt_mir::expr::ExprKind::Format(f) => {
            crate::format::render(f, buffer, &ty, len_max, ctx.program, m, &vars)?;
        }
        // Ein Literal ohne Platzhalter bleibt `Str` (3.9); es ist ein
        // Formatstring aus einem einzigen Textbaustein.
        takt_mir::expr::ExprKind::Str(lit) => {
            let f = takt_mir::pattern::Format::text(lit);
            crate::format::render(&f, buffer, &ty, len_max, ctx.program, m, &vars)?;
        }
        // Alles andere ist ein fertiger Wert — `bytes<N>` aus
        // `frame.encode()` etwa (8.8).
        _ => {
            let v = lower_expr(value, ctx.program, m, &vars)?;
            let LlvmType::Struct(_) = v.ty else {
                return Err(NotYet { what: "`send` mit einem Wert ohne Laenge" });
            };
            m.void_inst(&format!("store {} {}, ptr {buffer}", v.ty, v.value));
        }
    }
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 1"));
    let ok = m.inst(&format!("call i1 @{}(i32 {sid}, ptr {bytes}, i32 {len})", crate::stream::Streams::SEND));
    // 8.8: `len > tx.free` ist ein `StreamOverflow`.
    ctx.checks += 1;
    let go_on = format!("gesendet{}_{}", ctx.checks, ctx.machine.name);
    m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{}", ctx.trampoline()));
    m.label(&go_on);
    Ok(())
}

/// `every d:` (5.8): der Block laeuft, wenn die Uhr den naechsten
/// Zeitpunkt erreicht hat.
///
/// **Die Uhr haengt am Ort.** Steht das `every` in einem Zustand, ist sie
/// `t_in_state`; im maschinenweiten `loop:` ist sie `now` (5.8). Der
/// Grund steht dort: Ein Block im Maschinen-`loop:` gehoert keinem
/// Zustand, dessen Eintritt ihn neu startete, und verstummte mit
/// `t_in_state` nach dem ersten Wechsel. Mit `now` ueberlebt seine Phase
/// Zustandswechsel und Fault-Pfade — die gewollte Phasenstarrheit fuer
/// Takterzeuger.
///
/// **Der Zaehler steht im Zustand**, ein `i64` je Aufrufstelle
/// (`Role::EveryNext`, 11.2). Der Interpreter haelt ihn zusaetzlich je
/// Schleifenindex (5.6); der Codegen senkt `every` in einer `for`-
/// Schleife darum noch nicht — mit einem Feld je Stelle zaehlten alle
/// Durchlaeufe gemeinsam, und das waere still falsch.
fn every(
    period: &Expr,
    counter: takt_mir::CounterId,
    body: &Block,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let site = ctx
        .machine
        .layout
        .every_counters
        .get(counter.index())
        .copied()
        .ok_or(NotYet { what: "`every` ohne Zaehlerstelle" })?;
    let idx = ctx.state.index_of(Role::EveryNext, counter.index()).ok_or(NotYet { what: "`every` im Zustand" })?;
    let vars = ctx.vars();
    let d = lower_expr(period, ctx.program, m, &vars)?;
    if d.ty != LlvmType::Int(64) {
        return Err(NotYet { what: "`every` mit einer Dauer, die keine Dauer ist" });
    }
    // Die Uhr: `t_in_state` in einem Zustand, sonst `now` (5.8).
    let clock = match site.state {
        Some(_) => time_in_state_ns(ctx, m)?,
        None => {
            // `takt_now` steht im Modulkopf (`Abi::declare`).
            m.inst(&format!("call i64 @{}()", crate::abi::Abi::NOW)).to_string()
        }
    };
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let slot = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {idx}"));
    let next = m.inst(&format!("load i64, ptr {slot}"));
    // `-1` heisst „seit dem Eintritt noch nicht gesetzt": Dann gilt `d`
    // als naechster Zeitpunkt (5.8, Startwert `d`).
    let fresh = m.inst(&format!("icmp slt i64 {next}, 0"));
    let due_at = m.inst(&format!("select i1 {fresh}, i64 {}, i64 {next}", d.value));
    let due = m.inst(&format!("icmp sge i64 {clock}, {due_at}"));

    let k = ctx.next_label();
    let name = &ctx.machine.name;
    let (run, skip) = (format!("every{k}_{name}"), format!("every{k}_{name}_aus"));
    m.void_inst(&format!("br i1 {due}, label %{run}, label %{skip}"));
    m.label(&run);
    // `next += d` — vom faelligen Zeitpunkt aus, nicht von der Uhr: Das
    // Raster bleibt starr, auch wenn eine Aktivierung ausfiel (5.8).
    let advanced = m.inst(&format!("add i64 {due_at}, {}", d.value));
    m.void_inst(&format!("store i64 {advanced}, ptr {slot}"));
    block(body, ctx, m)?;
    m.void_inst(&format!("br label %{skip}"));
    m.label(&skip);
    Ok(())
}

/// `t_in_state` in Nanosekunden (5.8).
///
/// Der Zaehler im Zustand zaehlt Aktivierungen; eine Aktivierung ist
/// `period` Basis-Ticks lang (7.2). Dieselbe Rechnung wie in `after`.
fn time_in_state_ns(ctx: &Ctx<'_>, m: &mut Module) -> Result<String, NotYet> {
    let t_i = ctx.state.index_of(Role::TimeInState, 0).ok_or(NotYet { what: "t_in_state im Zustand" })?;
    let state_ty = format!("%{}_state", crate::fns::sanitized(&ctx.machine.name));
    let base = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    let cell = m.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 0", ctx.state.depth));
    let ticks = m.inst(&format!("load i64, ptr {cell}"));
    let period_ns = i64::from(ctx.machine.period.max(1)).saturating_mul(ctx.program.config.tick);
    Ok(m.inst(&format!("mul i64 {ticks}, {period_ns}")).to_string())
}

/// `for i in range(n)` im Rumpf einer Maschine (4.1).
///
/// Dieselbe Form wie in einer Funktion (`fn_for`), nur liegt der Zaehler
/// im Zustands-Struct statt auf dem Stack: Eine Maschine hat keine
/// Locals, ihre Variablen sind Felder (11.2). Die Schranke ist statisch,
/// weil 4.1 es verlangt und die Kostenrechnung (9.4.3) sie braucht.
///
/// **Warum nicht `fn_for` mitbenutzt.** Die beiden Kontexte
/// unterscheiden sich in mehr als der Variablenquelle: Eine Maschine hat
/// einen Fault-Pfad, Meldungsstellen und ein Blatt, eine Funktion nicht.
/// `Ctx` in `FnCtx` zu pressen hiesse, beide um das zu erweitern, was
/// der andere braucht — der Rumpf ist zwoelf Zeilen, die Naht waere
/// teurer als die Wiederholung.
fn for_range(
    var: takt_mir::VarId,
    count: &Expr,
    body: &Block,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let n = lower_expr(count, ctx.program, m, &vars)?;
    let (ptr, ty) = place(&Place::Var(var), ctx, m)?;
    m.void_inst(&format!("store {ty} 0, ptr {ptr}"));
    let k = ctx.next_label();
    let name = &ctx.machine.name;
    let (head, loop_body, end_at) =
        (format!("fuer{k}_{name}"), format!("fuer{k}_{name}_rumpf"), format!("fuer{k}_{name}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load {ty}, ptr {ptr}"));
    // Vorzeichenbehaftet: `range(n)` laeuft von 0 bis n-1 ueber einem
    // `int` (3.1).
    let go_on = m.inst(&format!("icmp slt {ty} {i}, {}", n.value));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    ctx.breaks.push(end_at.clone());
    let result = block(body, ctx, m);
    ctx.breaks.pop();
    result?;
    // Der Zaehler waechst am Ende des Rumpfs; ein `break` springt daran
    // vorbei, und das ist richtig — er verlaesst die Schleife.
    let cur = m.inst(&format!("load {ty}, ptr {ptr}"));
    let next = m.inst(&format!("add {ty} {cur}, 1"));
    m.void_inst(&format!("store {ty} {next}, ptr {ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&end_at);
    Ok(())
}

/// `for i in range(n)` in einer Funktion (4.1).
///
/// Die Schranke ist statisch — 4.1 verlangt es, und ohne sie waere die
/// Kostenrechnung (9.4.3) nicht moeglich. Der Zaehler liegt in einem
/// Slot wie jede andere lokale Variable; `mem2reg` macht daraus ein
/// Register, wenn er sich nicht entzieht.
fn fn_for<V: Slots>(
    var: takt_mir::VarId,
    count: &Expr,
    body: &Block,
    ctx: &mut FnCtx<'_, V>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let n = lower_expr(count, ctx.program, m, &ctx.vars)?;
    let (ptr, ty) = ctx.vars.slot(var, m).ok_or(NotYet { what: "Schleifenvariable" })?;
    m.void_inst(&format!("store {ty} 0, ptr {ptr}"));
    let k = ctx.next_label(m);
    let (head, loop_body, end_at) = (format!("fuer{k}"), format!("fuer{k}_rumpf"), format!("fuer{k}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load {ty}, ptr {ptr}"));
    // Der Vergleich ist vorzeichenbehaftet: `range(n)` laeuft von 0 bis
    // n-1, und `n` ist ein `int` (3.2).
    let go_on = m.inst(&format!("icmp slt {ty} {i}, {}", n.value));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    ctx.breaks.push(end_at.clone());
    let result = fn_block(body, ctx, m);
    ctx.breaks.pop();
    result?;
    // Der Zaehler waechst am Ende des Rumpfs; ein `break` springt daran
    // vorbei, und das ist richtig — es verlaesst die Schleife.
    let cur = m.inst(&format!("load {ty}, ptr {ptr}"));
    let next = m.inst(&format!("add {ty} {cur}, 1"));
    m.void_inst(&format!("store {ty} {next}, ptr {ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&end_at);
    Ok(())
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
        // `verdict pass | fail` (13.2): das Urteil eines Tests. Wie
        // `verify` eine reine Beobachtung — sie loest nie einen Fault aus
        // (Leitentscheidung 14), also nur ein Aufruf.
        Observe::Verdict { pass, .. } => {
            let site = ctx.next_site();
            let v = u8::from(*pass);
            m.void_inst(&format!("call void @{}(i32 {machine}, i32 {site}, i1 {v})", Abi::VERDICT));
            Ok(())
        }
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
            let off = crate::image::latch_offset(*c, ctx.program).ok_or(NotYet { what: "Versatz im Latch" })?;
            let ptr = m.inst(&format!("getelementptr inbounds i8, ptr %3, i64 {off}"));
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

/// Ein Methodenaufruf (3.9, 5.7).
///
/// Die Sammlungsmethoden aendern ihren Empfaenger *an Ort und Stelle* —
/// sie bekommen darum seinen Zeiger, nicht seinen Wert. Ein `push` auf
/// eine Kopie waere folgenlos, und die Sprache hat keine Referenzen, mit
/// denen man den Unterschied ausdruecken koennte (11.2): Der Empfaenger
/// ist ein `Place`, und das genuegt.
fn method_call(
    target: Option<&Place>,
    receiver: &Place,
    method: Method,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    // 5.7: `step` und `reset` einer Blockinstanz. Der Empfaenger ist die
    // Instanz selbst; sie liegt im Zustands-Struct der Maschine.
    if let Place::Var(id) = receiver
        && let Some(block) = crate::machine::instance_block(ctx.machine, *id)
    {
        return block_method_call(target, *id, block, method, args, ctx, m);
    }
    if !collection::is_collection_method(method) {
        return Err(collection::unsupported(method));
    }
    let (recv, ty) = place(receiver, ctx, m)?;
    let layout = collection::layout_of(&ty).ok_or(NotYet { what: "Methode auf einer Nicht-Sammlung" })?;
    let label = ctx.next_label();
    let ok = match method {
        Method::Push => {
            let vars = ctx.vars();
            let v = lower_expr(args.first().ok_or(NotYet { what: "`push` ohne Argument" })?, ctx.program, m, &vars)?;
            collection::push(recv, &layout, &v, label, m)
        }
        Method::Append => {
            // Die Quelle ist selbst eine Sammlung und damit ein
            // Speicherort; ihr Wert waere eine Kopie, die `memcpy` nicht
            // lesen kann.
            let src = args.first().ok_or(NotYet { what: "`append` ohne Argument" })?;
            let ExprKind::Var(id) = src.kind else { return Err(NotYet { what: "`append` aus einem Ausdruck" }) };
            let (src_ptr, _) = place(&Place::Var(id), ctx, m)?;
            collection::append(recv, src_ptr, &layout, label, m)
        }
        _ => collection::clear(recv, &layout, m),
    };
    // 3.9: Der Rueckgabewert sagt, ob es gelang. Wer ihn nicht verwendet,
    // hat es nach Pruefung 33 ausdruecklich getan.
    if let Some(t) = target {
        let (ptr, _) = place(t, ctx, m)?;
        m.void_inst(&format!("store i1 {ok}, ptr {ptr}"));
    }
    Ok(())
}

/// Der Kontext eines Funktionsrumpfs (4.4).
///
/// Eine Funktion hat keinen Zustands-Struct und keine Maschine: Ihre
/// Variablen sind Locals auf dem Stack, und ein `check` in ihr faultet
/// den Aufrufer, nicht sie selbst. Darum ein eigener Kontext statt eines
/// `Ctx` mit lauter leeren Feldern.
pub struct FnCtx<'a, V: Slots> {
    /// Das Programm, fuer Typen und Konstanten.
    pub program: &'a Program,
    /// Woher die Variablen kommen.
    ///
    /// Eine reine Funktion hat Locals auf dem Stack (4.4), eine
    /// Blockmethode ihre Parameter dort und ihren Zustand in der Instanz
    /// (5.7). Der Rumpf ist derselbe — nur die Quelle unterscheidet sie,
    /// und ein zweiter Satz Senkungen waere eine zweite Gelegenheit, sie
    /// verschieden zu senken.
    pub vars: V,
    /// Zaehler fuer eindeutige Marken.
    pub labels: u32,
    /// Sprungziele der laufenden Schleifen; `break` nimmt das oberste.
    pub breaks: Vec<String>,
}

/// Eine Variablenquelle, in die auch geschrieben werden kann.
pub trait Slots: Vars {
    /// Der Speicherort einer Variablen.
    fn slot(&self, id: takt_mir::VarId, m: &mut Module) -> Option<(Reg, LlvmType)>;
}

impl<V: Slots> FnCtx<'_, V> {
    /// Eine frische Nummer fuer eine Marke.
    ///
    /// Sie kommt aus dem Modul, nicht aus dem Kontext: Marken stehen im
    /// Modul, und zwei Funktionen haetten sonst beide `dann1` (derselbe
    /// Befund wie FB-72 bei den Uebergaengen).
    pub fn next_label(&mut self, m: &mut Module) -> u32 {
        self.labels += 1;
        m.next_label()
    }
}

/// Senkt den Rumpf einer Funktion (4.4).
pub fn fn_block<V: Slots>(b: &Block, ctx: &mut FnCtx<'_, V>, m: &mut Module) -> Result<(), NotYet> {
    for s in &b.stmts {
        fn_stmt(s, ctx, m)?;
    }
    Ok(())
}

/// Eine Anweisung im Rumpf einer Funktion.
///
/// Der Vorrat ist kleiner als in einer Maschine: Eine reine Funktion hat
/// keine Zustaende, keine Outputs und keine Beobachtungen (4.4). Was sie
/// hat, ist Rechnung, Verzweigung und `return`.
fn fn_stmt<V: Slots>(s: &Stmt, ctx: &mut FnCtx<'_, V>, m: &mut Module) -> Result<(), NotYet> {
    match &s.kind {
        StmtKind::Return(e) => {
            let v = lower_expr(e, ctx.program, m, &ctx.vars)?;
            m.void_inst(&format!("ret {} {}", v.ty, v.value));
            Ok(())
        }
        StmtKind::Assign { target, value } => {
            let v = lower_expr(value, ctx.program, m, &ctx.vars)?;
            let (ptr, _) = fn_place(target, ctx, m)?;
            m.void_inst(&format!("store {} {}, ptr {ptr}", v.ty, v.value));
            Ok(())
        }
        StmtKind::If { cond, then, otherwise } => {
            let c = lower_expr(cond, ctx.program, m, &ctx.vars)?;
            let n = ctx.next_label(m);
            let (t, f, end) = (format!("dann{n}"), format!("sonst{n}"), format!("ende{n}"));
            m.void_inst(&format!("br i1 {}, label %{t}, label %{f}", c.value));
            m.label(&t);
            fn_block(then, ctx, m)?;
            m.void_inst(&format!("br label %{end}"));
            m.label(&f);
            fn_block(otherwise, ctx, m)?;
            m.void_inst(&format!("br label %{end}"));
            m.label(&end);
            Ok(())
        }
        StmtKind::MethodCall { target, receiver, method, args } => {
            // 3.9 gilt in Funktionen wie in Maschinen; nur der
            // Speicherort ist ein anderer.
            if !collection::is_collection_method(*method) {
                return Err(collection::unsupported(*method));
            }
            let Place::Var(id) = receiver else { return Err(NotYet { what: "Empfaenger in einer Funktion" }) };
            let (recv, ty) = ctx.vars.slot(*id, m).ok_or(NotYet { what: "lokale Sammlung" })?;
            let layout = collection::layout_of(&ty).ok_or(NotYet { what: "Methode auf einer Nicht-Sammlung" })?;
            let label = ctx.next_label(m);
            let ok = match method {
                Method::Push => {
                    let v = lower_expr(
                        args.first().ok_or(NotYet { what: "`push` ohne Argument" })?,
                        ctx.program,
                        m,
                        &ctx.vars,
                    )?;
                    collection::push(recv, &layout, &v, label, m)
                }
                Method::Append => {
                    let src = args.first().ok_or(NotYet { what: "`append` ohne Argument" })?;
                    // `memcpy` liest aus dem Speicher; eine berechnete
                    // Quelle bekommt dafuer einen Platz (11.2: statischer
                    // Scratch). LLVM hebt die `alloca` in den
                    // Eintrittsblock und entfernt sie, wo sie unnoetig ist.
                    let src_ptr = match src.kind {
                        ExprKind::Var(sid) => ctx.vars.slot(sid, m).ok_or(NotYet { what: "Quelle" })?.0,
                        _ => {
                            let v = lower_expr(src, ctx.program, m, &ctx.vars)?;
                            let tmp = m.inst(&format!("alloca {}", v.ty));
                            m.void_inst(&format!("store {} {}, ptr {tmp}", v.ty, v.value));
                            tmp
                        }
                    };
                    collection::append(recv, src_ptr, &layout, label, m)
                }
                _ => collection::clear(recv, &layout, m),
            };
            if let Some(Place::Var(tid)) = target {
                let (ptr, _) = ctx.vars.slot(*tid, m).ok_or(NotYet { what: "Ziel" })?;
                m.void_inst(&format!("store i1 {ok}, ptr {ptr}"));
            }
            Ok(())
        }
        StmtKind::ForRange { var, count, body } => fn_for(*var, count, body, ctx, m),
        StmtKind::Break => {
            // 4.1: Die Schleife hat eine statische Schranke; `break`
            // verlaesst sie vorzeitig. Das Ziel steht auf dem Stapel der
            // laufenden Schleifen.
            let Some(target) = ctx.breaks.last().cloned() else {
                return Err(NotYet { what: "`break` ausserhalb einer Schleife" });
            };
            m.void_inst(&format!("br label %{target}"));
            Ok(())
        }
        other => Err(NotYet { what: crate::scope::stmt_name(other) }),
    }
}

/// `i.step(...)` oder `i.reset()` einer Blockinstanz (5.7).
///
/// 5.7: „Jede Instanz darf pro Aktivierungs-Tick hoechstens einmal `step`
/// ausfuehren." Die Pruefung ist statisch (Pruefung 52), aber das Flag im
/// Zustand ist die Absicherung: Ein Aufruf in zwei Zweigen desselben
/// Ticks ist statisch nicht immer auszuschliessen, und ein Filter, der
/// zweimal laeuft, hat einen Tick uebersprungen, ohne dass es jemand
/// saehe.
fn block_method_call(
    target: Option<&Place>,
    var: takt_mir::VarId,
    block: takt_mir::BlockId,
    method: Method,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let def = ctx.program.blocks.get(block.index()).ok_or(NotYet { what: "Block" })?;
    let inst = crate::block::instance_of(def, ctx.program).ok_or(NotYet { what: "Blockinstanz" })?;
    let ptr = ctx.field(Role::Var, var.index(), m).ok_or(NotYet { what: "Instanz im Zustand" })?;
    let mut ops = vec![format!("ptr {ptr}")];
    let vars = ctx.vars();
    for a in args {
        let v = lower_expr(a, ctx.program, m, &vars)?;
        ops.push(format!("{} {}", v.ty, v.value));
    }
    let (_, fid) = match method {
        Method::Step => ("step", def.step.ok_or(NotYet { what: "Block ohne `step`" })?),
        Method::Reset => {
            // `reset()` stellt den Anfangszustand her; er steht in den
            // Initialwerten der Zustandsvariablen (5.7). Der Codegen
            // schreibt sie unmittelbar, statt eine Methode zu rufen, die
            // es nicht gibt.
            return reset_instance(ptr, def, &inst, ctx, m);
        }
        _ => return Err(collection::unsupported(method)),
    };
    let f = ctx.program.fns.get(fid.index()).ok_or(NotYet { what: "Methode" })?;
    let ret = match f.ret {
        Some(t) => ty::lower(t, ctx.program).ok_or(NotYet { what: "Rueckgabetyp" })?,
        None => LlvmType::Void,
    };
    // 5.7: hoechstens einmal je Aktivierung. Der Zweig ueberspringt den
    // zweiten Aufruf, statt ihn zu wiederholen.
    let label = ctx.next_label();
    let flag = m.inst(&format!(
        "getelementptr inbounds {}, ptr {ptr}, i32 0, i32 {}",
        LlvmType::Struct(inst.fields.clone()),
        inst.stepped()
    ));
    let done = m.inst(&format!("load i1, ptr {flag}"));
    let (go_on, end_at) = (format!("step{label}"), format!("step{label}_ende"));
    m.void_inst(&format!("br i1 {done}, label %{end_at}, label %{go_on}"));
    m.label(&go_on);
    m.void_inst(&format!("store i1 true, ptr {flag}"));
    let symbol = crate::block::method_symbol(def, &f.name);
    let call = if ret == LlvmType::Void {
        m.void_inst(&format!("call void @{symbol}({})", ops.join(", ")));
        None
    } else {
        Some(m.inst(&format!("call {ret} @{symbol}({})", ops.join(", "))))
    };
    if let (Some(t), Some(v)) = (target, call) {
        let (dst, _) = place(t, ctx, m)?;
        m.void_inst(&format!("store {ret} {v}, ptr {dst}"));
    }
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&end_at);
    Ok(())
}

/// `i.reset()` (5.7): die Zustandsvariablen auf ihre Initialwerte.
fn reset_instance(
    ptr: Reg,
    def: &takt_mir::fns::BlockDef,
    inst: &crate::block::Instance,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let struct_ty = LlvmType::Struct(inst.fields.clone());
    let vars = ctx.vars();
    for (i, v) in def.state_vars.iter().enumerate() {
        let init = v.init.as_ref().ok_or(NotYet { what: "Zustandsvariable ohne Initialwert" })?;
        let value = lower_expr(init, ctx.program, m, &vars)?;
        let at = m.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {i}"));
        m.void_inst(&format!("store {} {}, ptr {at}", value.ty, value.value));
    }
    // Das Flag geht mit zurueck: Nach `reset()` darf `step` wieder laufen.
    let flag = m.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {}", inst.stepped()));
    m.void_inst(&format!("store i1 false, ptr {flag}"));
    Ok(())
}

/// `match` ueber einen Summentyp oder Werte (3.8, 6.1).
///
/// Eine Kette von Vergleichen, kein `switch`: Die Muster koennen Bereiche
/// sein (`case 1..9`), und ein `switch` kann nur einzelne Werte. LLVM
/// macht aus einer Kette gleicher Vergleiche selbst einen `switch`, wo es
/// sich lohnt — der Codegen waere dabei schlechter als er.
///
/// **Die Reihenfolge ist die des Quelltexts.** 6.1 sagt, der erste
/// passende `case` gewinnt; ein Umsortieren (etwa nach Diskriminante)
/// waere eine andere Semantik, sobald sich zwei Muster ueberschneiden.
fn match_stmt(subject: &Expr, arms: &[takt_mir::stmt::Arm], ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let value = lower_expr(subject, ctx.program, m, &vars)?;
    let n = ctx.next_label();
    let name = ctx.machine.name.clone();
    let end_at = format!("match{n}_{name}");
    // Bei einem Summentyp wird die Diskriminante verglichen; die MIR
    // legt sie als Feld 0 ab, wenn die Variante Felder traegt, sonst ist
    // der Wert selbst die Diskriminante (3.7).
    // Woher die Diskriminante kommt, sagt der Typ: Bei `T!E` steht sie im
    // Feld 1 (3.8: Wert, Fehler, Flag), bei einem Summentyp mit Feldern
    // im Feld 0. Ein fieldloses Enum *ist* seine Diskriminante.
    let disc = match (&value.ty, ctx.program.types.list.get(subject.ty.index())) {
        (LlvmType::Struct(_), Some(takt_mir::types::Type::Result { .. })) => {
            let d = m.inst(&format!("extractvalue {} {}, 1", value.ty, value.value));
            Lowered { value: d.to_string(), ty: LlvmType::Int(32) }
        }
        (LlvmType::Struct(_), _) => {
            let d = m.inst(&format!("extractvalue {} {}, 0", value.ty, value.value));
            Lowered { value: d.to_string(), ty: LlvmType::Int(32) }
        }
        _ => value.clone(),
    };
    for (i, arm) in arms.iter().enumerate() {
        let hit = format!("case{n}_{i}_{name}");
        let go_on = format!("case{n}_{i}_sonst_{name}");
        match &arm.pattern {
            takt_mir::stmt::ArmPattern::Wild => {
                // `case _` faengt alles; die folgenden kaemen nie zum Zug.
                block(&arm.body.clone(), ctx, m)?;
                m.void_inst(&format!("br label %{end_at}"));
                m.label(&end_at);
                return Ok(());
            }
            takt_mir::stmt::ArmPattern::Variant { variant, fields } => {
                let def = enum_of(subject.ty, ctx.program).ok_or(NotYet { what: "Enum des `match`" })?;
                let d = def.variants.get(*variant as usize).ok_or(NotYet { what: "Variante" })?.discriminant;
                let ok = m.inst(&format!("icmp eq {} {}, {d}", disc.ty, disc.value));
                m.void_inst(&format!("br i1 {ok}, label %{hit}, label %{go_on}"));
                m.label(&hit);
                // 6.1: Die Felder der Variante werden an gehobene
                // Variablen gebunden, bevor der Rumpf laeuft. Sie liegen
                // im Wert hinter der Diskriminante.
                bind_fields(&value, fields, ctx, m)?;
                block(&arm.body.clone(), ctx, m)?;
                m.void_inst(&format!("br label %{end_at}"));
                m.label(&go_on);
                continue;
            }
            takt_mir::stmt::ArmPattern::Values(values) => {
                let mut ok = "false".to_string();
                for v in values {
                    let lo = lower_expr(&v.lo, ctx.program, m, &vars)?;
                    let hit = match &v.hi {
                        // `case a..b`: einschliesslich beider Grenzen.
                        Some(hi) => {
                            let h = lower_expr(hi, ctx.program, m, &vars)?;
                            let a = m.inst(&format!("icmp sge {} {}, {}", disc.ty, disc.value, lo.value));
                            let b = m.inst(&format!("icmp sle {} {}, {}", disc.ty, disc.value, h.value));
                            m.inst(&format!("and i1 {a}, {b}")).to_string()
                        }
                        None => m.inst(&format!("icmp eq {} {}, {}", disc.ty, disc.value, lo.value)).to_string(),
                    };
                    ok = m.inst(&format!("or i1 {ok}, {hit}")).to_string();
                }
                m.void_inst(&format!("br i1 {ok}, label %{hit}, label %{go_on}"));
            }
        }
        m.label(&hit);
        block(&arm.body.clone(), ctx, m)?;
        m.void_inst(&format!("br label %{end_at}"));
        m.label(&go_on);
    }
    // Kein `case` hat getroffen. Bei einem geschlossenen Enum kann das
    // nicht vorkommen (Pruefung 7 verlangt Vollstaendigkeit); der Sprung
    // steht trotzdem da, weil LLVM einen Terminator braucht.
    m.void_inst(&format!("br label %{end_at}"));
    m.label(&end_at);
    Ok(())
}

/// Das Enum hinter einem `match`-Subjekt.
fn enum_of(ty: takt_mir::TypeId, p: &Program) -> Option<&takt_mir::types::EnumDef> {
    match p.types.list.get(ty.index())? {
        takt_mir::types::Type::Enum(e) => p.enums.get(e.index()),
        // Bei `T!E` steht die Fehlerdiskriminante im Feld 1; das Enum
        // ist das der Fehlerseite (3.8).
        takt_mir::types::Type::Result { err, .. } => p.enums.get(err.index()),
        _ => None,
    }
}

/// Bindet die Felder einer Variante an ihre gehobenen Variablen (6.1).
///
/// Die Felder stehen im Wert hinter der Diskriminante; bei einem `T!E`
/// ist das Feld 0 der Wert und Feld 1 der Fehler (3.8). Mehr als ein Feld
/// braucht den Aufbau der Variante im Wert, den erst die Summentypen mit
/// Feldern mitbringen.
fn bind_fields(value: &Lowered, fields: &[takt_mir::VarId], ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    if fields.is_empty() {
        return Ok(());
    }
    let LlvmType::Struct(parts) = &value.ty else { return Err(NotYet { what: "Variante ohne Felder im Wert" }) };
    if fields.len() > 1 {
        return Err(NotYet { what: "`case` mit mehreren Feldbindungen" });
    }
    // Bei `T!E` traegt Feld 0 den Wert und Feld 1 den Fehler; welches
    // gemeint ist, sagt der Typ der Bindung.
    let var = fields[0];
    let def = ctx.machine.vars.get(var.index()).ok_or(NotYet { what: "Bindung" })?;
    let want = ty::lower(def.ty, ctx.program).ok_or(NotYet { what: "Typ der Bindung" })?;
    let index = parts.iter().position(|t| *t == want).ok_or(NotYet { what: "Feld der Variante" })?;
    let v = m.inst(&format!("extractvalue {} {}, {index}", value.ty, value.value));
    let ptr = ctx.field(Role::Var, var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" })?;
    m.void_inst(&format!("store {want} {v}, ptr {ptr}"));
    Ok(())
}

/// Der Speicherort eines Zuweisungsziels in einer Funktion (4.4).
///
/// Dieselbe Form wie `place` in einer Maschine; nur die Wurzel ist eine
/// andere — eine Funktion hat keine Outputs, nur Locals.
fn fn_place<V: Slots>(target: &Place, ctx: &mut FnCtx<'_, V>, m: &mut Module) -> Result<(Reg, LlvmType), NotYet> {
    match target {
        Place::Var(id) => ctx.vars.slot(*id, m).ok_or(NotYet { what: "lokale Variable" }),
        Place::Field(base, field) => {
            let (ptr, ty) = fn_place(base, ctx, m)?;
            let LlvmType::Struct(fields) = &ty else { return Err(NotYet { what: "Feld eines Nicht-Records" }) };
            let inner = fields.get(*field as usize).cloned().ok_or(NotYet { what: "Feldnummer" })?;
            let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 {field}"));
            Ok((at, inner))
        }
        Place::Index(base, index) => {
            let (ptr, ty) = fn_place(base, ctx, m)?;
            let i = lower_expr(index, ctx.program, m, &ctx.vars)?;
            // Bei einer Sammlung liegt das Element im `data`-Feld (3.9),
            // bei einem Array unmittelbar. Die Grenze prueft der
            // `Checked`-Knoten der MIR (4.1).
            let (array_ty, data) = match &ty {
                LlvmType::Struct(_) => {
                    let l = collection::layout_of(&ty).ok_or(NotYet { what: "Index auf diesem Struct" })?;
                    let d = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 1"));
                    (LlvmType::Array(Box::new(l.elem), l.cap), d)
                }
                LlvmType::Array(..) => (ty.clone(), ptr),
                _ => return Err(NotYet { what: "Index auf diesem Typ" }),
            };
            let LlvmType::Array(elem, _) = &array_ty else { return Err(NotYet { what: "Elementtyp" }) };
            let at = m.inst(&format!("getelementptr inbounds {array_ty}, ptr {data}, i32 0, {} {}", i.ty, i.value));
            Ok((at, (**elem).clone()))
        }
        _ => Err(NotYet { what: "Zuweisungsziel in einer Funktion" }),
    }
}
