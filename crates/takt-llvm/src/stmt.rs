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

use takt_mir::expr::{Expr, ExprKind, JobField};
use takt_mir::machine::Machine;
use takt_mir::program::Program;
use takt_mir::stmt::{Block, Method, Observe, Place, Stmt, StmtKind};
use takt_mir::types::Type;

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
    /// `None` auf einer geteilten Ebene des Schritts: Dort entscheidet
    /// `leaf_reg` zur Laufzeit.
    pub leaf: Option<takt_mir::StateId>,
    /// Das Register mit der Nummer des aktiven Blatts im Schritt.
    pub leaf_reg: Option<Reg>,
    /// Die Blaetter unter der Ebene, die gerade entsteht.
    pub region: Vec<takt_mir::StateId>,
    /// Wie viele Meldungsstellen die Maschine schon hat.
    ///
    /// Der Index identifiziert die Stelle im Trace; die Reihenfolge ist
    /// die der Erzeugung und damit die des Quelltexts.
    sites: u32,
    /// Sprungziele der laufenden Schleifen; `break` nimmt das oberste.
    breaks: Vec<String>,
    /// Das Ende der Schrittfunktion; `->` als Anweisung springt dorthin
    /// (11.2). `None` heisst: Der Block laeuft im Entry-Modus oder in
    /// einer Funktion, wo ein `->` nicht wirkt (5.2 Regel 4).
    pub end: Option<String>,
}

impl<'a> Ctx<'a> {
    /// Ein Kontext fuer eine Maschine.
    pub fn new(machine: &'a Machine, state: &'a StateStruct, program: &'a Program) -> Ctx<'a> {
        let machine_index = program.machines.iter().position(|m| m.name == machine.name).unwrap_or(0) as u32;
        Ctx {
            machine,
            state,
            program,
            checks: 0,
            machine_index,
            leaf: None,
            leaf_reg: None,
            region: Vec::new(),
            sites: 0,
            breaks: Vec::new(),
            end: None,
        }
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
        StateVars {
            machine: self.machine,
            leaf: self.leaf,
            shared: self.leaf.is_none() && self.leaf_reg.is_some(),
            state: self.state,
            program: self.program,
            machine_index: self.machine_index,
        }
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
        match self.leaf {
            Some(leaf) => format!("fault_{}_{}", self.machine.name, leaf.index()),
            None => format!("fault_{}_any", self.machine.name),
        }
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
    /// Eine geteilte Ebene des Schritts: Der Fault-Trampolin verzweigt
    /// zur Laufzeit auf das Blatt.
    pub shared: bool,
    /// Ihr Zustands-Struct.
    pub state: &'a StateStruct,
    /// Das Programm, fuer die Typen.
    pub program: &'a Program,
    /// Die Nummer der Maschine im Programm (`Ctx::machine_index`).
    pub machine_index: u32,
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
        image_slot(self.program, channel, slot, m)
    }
}

/// Ein Feld des Abbild-Eintrags eines Channels (`crate::image`); `%1` ist
/// das Prozessabbild.
pub fn image_slot(
    program: &Program,
    channel: takt_mir::ChannelId,
    slot: crate::image::Slot,
    m: &mut Module,
) -> Option<Lowered> {
    let entry = crate::image::entry_type(channel, program)?;
    let LlvmType::Struct(fields) = &entry else { return None };
    let ty = fields.get(slot as usize)?.clone();
    let off = crate::image::offset_of(channel, program)?;
    // Byteweise adressiert, aber natuerlich ausgerichtet — `image` legt
    // die Eintraege so, und der Rahmen rechnet denselben Versatz.
    let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {}", off + entry.field_offset(slot as usize)));
    let v = m.inst(&format!("load {ty}, ptr {at}"));
    Some(Lowered { value: v.to_string(), ty })
}

impl Vars for StateVars<'_> {
    fn fault_label(&self) -> Option<String> {
        if self.shared {
            return Some(format!("fault_{}_any", self.machine.name));
        }
        Some(format!("fault_{}_{}", self.machine.name, self.leaf?.index()))
    }

    fn job(&self, handle: takt_mir::VarId, field: JobField, p: &Program, m: &mut Module) -> Option<Lowered> {
        let mi = p.machines.iter().position(|x| x.name == self.machine.name)?;
        let slot = self.machine.layout.job_slots.iter().position(|s| s.handle == handle)?;
        let off = crate::image::job_offset(takt_mir::MachineId(mi as u32), slot, p)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
        match field {
            JobField::Done => {
                let b = m.inst(&format!("load i8, ptr {at}"));
                let v = m.inst(&format!("icmp ne i8 {b}, 0"));
                Some(Lowered { value: v.to_string(), ty: LlvmType::Int(1) })
            }
            // `T!JobErr` wie `wrap` es baut: Wert, Diskriminante, Flag.
            JobField::Result => {
                let ret = p.natives.get(self.machine.layout.job_slots[slot].native.index())?.ret;
                let ok_ty = ty::lower(ret, p)?;
                let res_ty = LlvmType::Struct(vec![ok_ty, LlvmType::Int(32), LlvmType::Int(1)]);
                let dst = m.alloca(&res_ty);
                let okp = m.inst(&format!("getelementptr inbounds i8, ptr {at}, i64 1"));
                let ok8 = m.inst(&format!("load i8, ptr {okp}"));
                let ok = m.inst(&format!("icmp ne i8 {ok8}, 0"));
                let f2 = m.inst(&format!("getelementptr inbounds {res_ty}, ptr {dst}, i32 0, i32 2"));
                m.void_inst(&format!("store i1 {ok}, ptr {f2}"));
                let errp = m.inst(&format!("getelementptr inbounds i8, ptr {at}, i64 4"));
                let err = m.inst(&format!("load i32, ptr {errp}, align 1"));
                let f1 = m.inst(&format!("getelementptr inbounds {res_ty}, ptr {dst}, i32 0, i32 1"));
                m.void_inst(&format!("store i32 {err}, ptr {f1}"));
                let valp = m.inst(&format!("getelementptr inbounds i8, ptr {at}, i64 8"));
                let f0 = m.inst(&format!("getelementptr inbounds {res_ty}, ptr {dst}, i32 0, i32 0"));
                crate::persist::decode_canonical(p, ret, valp, f0, m).ok()?;
                let v = m.inst(&format!("load {res_ty}, ptr {dst}"));
                Some(Lowered { value: v.to_string(), ty: res_ty })
            }
        }
    }

    fn machine_index(&self) -> Option<u32> {
        Some(self.machine_index)
    }

    fn trigger_slots(&self, t: takt_mir::TriggerId, m: &mut Module) -> Option<(Reg, Reg)> {
        let nth = self.machine.layout.trigger_flags.iter().position(|x| *x == t)?;
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        let mut at = |role: Role| {
            let i = self.state.index_of(role, nth)?;
            Some(m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}")))
        };
        Some((at(Role::Armed)?, at(Role::TriggerCursor)?))
    }

    fn stream_slots(&self, stream: takt_mir::expr::StreamRef, m: &mut Module) -> Option<(Reg, Reg)> {
        let nth = self.machine.layout.cursors.iter().position(|c| *c == stream)?;
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        let mut at = |role: Role| {
            let i = self.state.index_of(role, nth)?;
            Some(m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}")))
        };
        Some((at(Role::Cursor)?, at(Role::Examined)?))
    }

    fn var(&self, id: takt_mir::VarId, m: &mut Module) -> Option<Lowered> {
        let (ptr, ty) = self.address(id, m)?;
        let v = m.inst(&format!("load {ty}, ptr {ptr}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    fn address(&self, id: takt_mir::VarId, m: &mut Module) -> Option<(Reg, LlvmType)> {
        let def = self.machine.vars.get(id.index())?;
        let ty = ty::lower(def.ty, self.program)?;
        let i = self.state.index_of(Role::Var, id.index())?;
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        Some((m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}")), ty))
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
                let cell = crate::machine::timer_cell(self.machine, self.state, self.machine.states.len(), m)?;
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

    /// Ψ einer anderen Maschine (7.2); `%1` ist das Abbild.
    fn published(&self, target: takt_mir::MachineId, field: crate::psi::Field, m: &mut Module) -> Option<Lowered> {
        crate::psi::load(self.machine, target, field, self.program, m)
    }

    fn published_at(
        &self,
        first: takt_mir::MachineId,
        field: crate::psi::Field,
        len: u32,
        index: &Lowered,
        m: &mut Module,
    ) -> Option<Lowered> {
        crate::psi::load_indexed(first, field, len, index, self.program, m)
    }
}

/// Senkt einen Block (11.2: Straight-Line-Code).
pub fn block(b: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    for s in &b.stmts {
        if m.instrument == crate::target::Instrument::Statements {
            mark(s.span.start, ctx, m);
        }
        let slots = m.slot_mark();
        stmt(s, ctx, m)?;
        m.end_slots(slots);
    }
    Ok(())
}

/// Instrumentierung (11.2): `pc` bekommt, wo die Maschine steht.
pub fn mark(at: u32, ctx: &mut Ctx<'_>, m: &mut Module) {
    if let Some(ptr) = ctx.field(Role::Pc, 0, m) {
        m.void_inst(&format!("store i32 {at}, ptr {ptr}"));
    }
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
        StmtKind::ForEach { vars, iter, body } => for_each(vars, iter, body, ctx, m),
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
        StmtKind::At { time, body } => at(time, body, ctx, m),
        // 11.2: „`->` → Setzen der Goto-Vormerkung + Sprung ans
        // Kettenende." Ohne bekanntes Ende laeuft der Block im
        // Entry-Modus, und dort ist ein `->` wirkungslos (5.2 Regel 4) —
        // genau das, was `exec` mit `Mode::Entry` tut.
        StmtKind::Goto(target) => match ctx.end.clone() {
            Some(end) => crate::step::goto(*target, ctx, m, &end),
            None => Ok(()),
        },
        StmtKind::Cancel(c) => {
            m.void_inst(&format!("call void @{}(i32 {})", Abi::CANCEL, c.0));
            Ok(())
        }
        StmtKind::Raise(s) => crate::psi::raise(takt_mir::MachineId(ctx.machine_index), *s, ctx.program, m)
            .ok_or(NotYet { what: "`raise`" }),
        StmtKind::Job { handle, native, args } => job_begin(*handle, *native, args, ctx, m),
        StmtKind::Arm { trigger, on } => {
            let (armed, _) = ctx.vars().trigger_slots(*trigger, m).ok_or(NotYet { what: "`arm` eines Triggers" })?;
            m.void_inst(&format!("store i1 {on}, ptr {armed}"));
            Ok(())
        }
        other => Err(NotYet { what: crate::scope::stmt_name(other) }),
    }
}

/// `job v = f(args)` (4.5): Die Argumente gehen als Folge kanonischer
/// Bloecke (je `u32` Laenge, dann die Bytes) an die Runtime, die den Job
/// fuehrt und den Slot im Abbild schreibt.
fn job_begin(
    handle: takt_mir::VarId,
    native: takt_mir::NativeId,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let p = ctx.program;
    let slot =
        ctx.machine.layout.job_slots.iter().position(|s| s.handle == handle).ok_or(NotYet { what: "Job-Slot" })?;
    let n = p.natives.get(native.index()).ok_or(NotYet { what: "native Funktion" })?;
    let mut cap = 0u64;
    for q in &n.params {
        let size = takt_mir::bytes::max_size(p, q.ty).map_err(|_| NotYet { what: "Job-Argument ohne Byteform" })?;
        cap += 4 + u64::from(size);
    }
    let buf = m.alloca(&format!("[{} x i8]", cap.max(1)));
    let vars = ctx.vars();
    let mut off = m.inst("add i64 0, 0");
    for a in args {
        let v = lower_expr(a, p, m, &vars)?;
        let tmp = m.alloca(&v.ty);
        m.write(&v.ty, &v.value, &tmp.to_string());
        let body = m.inst(&format!("add i64 {off}, 4"));
        let dst = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {body}"));
        let len = crate::persist::encode_canonical(p, a.ty, tmp, dst, m)?;
        let len32 = m.inst(&format!("trunc i64 {len} to i32"));
        let lenp = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {off}"));
        m.void_inst(&format!("store i32 {len32}, ptr {lenp}, align 1"));
        off = m.inst(&format!("add i64 {body}, {len}"));
    }
    let total = m.inst(&format!("trunc i64 {off} to i32"));
    m.void_inst(&format!(
        "call void @{}(i32 {}, i32 {slot}, i32 {}, ptr {buf}, i32 {total})",
        Abi::JOB_BEGIN,
        ctx.machine_index,
        native.index()
    ));
    Ok(())
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
    let buffer = m.alloca(&ty);
    let vars = ctx.vars();
    // Laenge und Bytes, wenn sie nicht im Puffer liegen.
    let mut source: Option<(Reg, Reg)> = None;
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
        // Ein fertiger Wert: Text und `bytes<N>` in ihrer Sammlungsform
        // (`frame.encode()` etwa, 8.8), alles andere in der kanonischen
        // Byteform — so liegt es im Ring, und so liest es der Empfaenger
        // (plan/m6.md 2.2).
        _ => {
            let vty = ty::lower(value.ty, ctx.program).ok_or(NotYet { what: "Elementtyp" })?;
            let textual = matches!(
                ctx.program.types.list.get(value.ty.index()),
                Some(Type::Bytes { .. } | Type::Str { .. } | Type::Line { .. })
            );
            // Eine Stelle wird gelesen, wo sie liegt; ein gerechneter Text
            // derselben Form entsteht gleich im Puffer (FB-214).
            let place = crate::expr::address_of(value, m, &vars);
            let at_place = place.is_some();
            if textual && vty == ty && place.is_none() {
                crate::expr::store(value, &buffer.to_string(), None, ctx.program, m, &vars)?;
            } else {
                let src = match place {
                    Some((at, _)) => at,
                    None => {
                        let slot = m.alloca(&vty);
                        crate::expr::store(value, &slot.to_string(), None, ctx.program, m, &vars)?;
                        slot
                    }
                };
                if textual && at_place {
                    // Die Stelle traegt `{ len, bytes }` selbst; der Ring
                    // liest sie unmittelbar.
                    let len_ptr = m.inst(&format!("getelementptr inbounds {vty}, ptr {src}, i32 0, i32 0"));
                    let bytes = m.inst(&format!("getelementptr inbounds {vty}, ptr {src}, i32 0, i32 1"));
                    source = Some((len_ptr, bytes));
                } else if textual {
                    // `line<N>` traegt hinter den Bytes noch `truncated`; das
                    // Praefix `{ len, bytes }` ist bei allen dreien gleich.
                    // Kopiert wird der kleinere Typ: Wert und Puffer koennen
                    // verschieden gross sein.
                    let prefix = if vty.aligned_size() < ty.aligned_size() { &vty } else { &ty };
                    m.copy(prefix, &src.to_string(), &buffer.to_string());
                } else {
                    // Feste Slot-Form (plan/m6.md 2.2): die kanonische Form,
                    // mit Nullen auf `len_max` (= `max_size`) gefuellt — so
                    // liegt das Element im Ring, und so trennt es der
                    // Empfaenger, auch mit einem `bytes<N>`-Feld (FB-189).
                    m.write(&ty, "zeroinitializer", &buffer.to_string());
                    let out = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 1"));
                    let _ = crate::persist::encode_canonical(ctx.program, value.ty, src, out, m)?;
                    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 0"));
                    m.void_inst(&format!("store i32 {len_max}, ptr {len_ptr}"));
                }
            }
        }
    }
    let (len_ptr, bytes) = match source {
        Some(at) => at,
        None => (
            m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 0")),
            m.inst(&format!("getelementptr inbounds {ty}, ptr {buffer}, i32 0, i32 1")),
        ),
    };
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
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
    let cell = crate::machine::timer_cell(ctx.machine, ctx.state, ctx.machine.states.len(), m)
        .ok_or(NotYet { what: "t_in_state im Zustand" })?;
    let ticks = m.inst(&format!("load i64, ptr {cell}"));
    let period_ns = i64::from(ctx.machine.period.max(1)).saturating_mul(ctx.program.config.tick);
    Ok(m.inst(&format!("mul i64 {ticks}, {period_ns}")).to_string())
}

/// `at T: o = v` (9.8): geplante Schreibvorgaenge.
///
/// **Was der Block enthaelt, ist eng begrenzt** — nur Zuweisungen an
/// Outputs (9.8, `eval_writes`). Die rechten Seiten werden *jetzt*
/// ausgewertet und der Wert zusammen mit `T` eingeplant; die Runtime
/// stellt ihn zum Zeitpunkt, nicht der Tickschritt.
///
/// **Zwei Faults koennen dabei entstehen** (9.8): `TimingFault`, wenn
/// `T` nicht in der Zukunft liegt, und `ScheduleOverflow`, wenn K_o
/// erreicht ist. Beide entscheidet die Runtime, weil nur sie die
/// Warteschlange kennt — der erzeugte Code prueft das Ergebnis und nimmt
/// bei `false` seinen Fault-Pfad. Eine Pruefung hier waere eine zweite
/// Meinung ueber eine Datenstruktur, die er nicht sieht.
fn at(time: &Expr, body: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let t = lower_expr(time, ctx.program, m, &vars)?;
    if t.ty != LlvmType::Int(64) {
        return Err(NotYet { what: "`at` mit einem Zeitpunkt, der keine Dauer ist" });
    }
    for stmt in &body.stmts {
        let StmtKind::Assign { target: Place::Output(c), value } = &stmt.kind else {
            // 9.8 laesst nur Output-Zuweisungen zu; das Sema hat es
            // geprueft, und alles andere waere hier ein Fehler im Lowering.
            return Err(NotYet { what: "`at`-Block mit mehr als Output-Zuweisungen" });
        };
        let vars = ctx.vars();
        let v = lower_expr(value, ctx.program, m, &vars)?;
        // Der Aufruf nimmt den Wert als `i64`. Ein `double` traegt
        // dieselben 64 Bit, ein schmalerer Ganzzahltyp wird erweitert —
        // die Runtime legt ihn im Latch ab, dessen Typ sie am Channel
        // kennt (11.2).
        let word = match &v.ty {
            LlvmType::Int(64) => v.value.clone(),
            LlvmType::F64 => m.inst(&format!("bitcast double {} to i64", v.value)).to_string(),
            LlvmType::F32 => {
                let wide = m.inst(&format!("fpext float {} to double", v.value));
                m.inst(&format!("bitcast double {wide} to i64")).to_string()
            }
            LlvmType::Int(1) => m.inst(&format!("zext i1 {} to i64", v.value)).to_string(),
            LlvmType::Int(n) => m.inst(&format!("sext i{n} {} to i64", v.value)).to_string(),
            _ => return Err(NotYet { what: "`at` mit einem zusammengesetzten Wert" }),
        };
        let ok = m.inst(&format!("call i1 @{}(i32 {}, i64 {}, i64 {word})", Abi::SCHEDULE, c.0, t.value));
        ctx.checks += 1;
        let go_on = format!("geplant{}_{}", ctx.checks, ctx.machine.name);
        m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{}", ctx.trampoline()));
        m.label(&go_on);
    }
    Ok(())
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

/// `for x in W` (9.2, 9.6): ueber ein Fenster laeuft die Schleife ueber
/// Elemente, ueber ein Array oder eine Sammlung ueber Werte.
fn for_each(
    vars: &takt_mir::stmt::ForVars,
    iter: &Expr,
    body: &Block,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let takt_mir::stmt::ForVars::One(var) = vars else {
        return Err(NotYet { what: "`for` mit zwei Variablen (`map`)" });
    };
    match &iter.kind {
        takt_mir::expr::ExprKind::Input { channel, .. } => {
            for_window(*var, takt_mir::expr::StreamRef::Channel(*channel), body, ctx, m)
        }
        takt_mir::expr::ExprKind::Stream(s) => for_window(*var, takt_mir::expr::StreamRef::Internal(*s), body, ctx, m),
        _ => for_items(*var, iter, body, ctx, m),
    }
}

/// `for x in s` ueber ein Fenster (8.7, 9.6): jedes besuchte Element gilt
/// als untersucht, ein `break` laesst die uebrigen im Fenster — wie
/// `for_window` im Interpreter.
fn for_window(
    var: takt_mir::VarId,
    stream: takt_mir::expr::StreamRef,
    body: &Block,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let (cur_ptr, ex_ptr) = ctx.vars().stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let elem = crate::stream::element(ctx.program, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let mi = ctx.machine_index;
    let k = ctx.next_label();
    let name = ctx.machine.name.clone();
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n = m.inst(&format!("call i32 @{}(i32 {sid}, i64 {cur})", crate::stream::Streams::COUNT));
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let buf = crate::stream::scratch(ctx.program, elem, m)?;
    let (head, loop_body, end_at) =
        (format!("fenster{k}_{name}"), format!("fenster{k}_{name}_rumpf"), format!("fenster{k}_{name}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {n}"));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    let seq = m.inst(&format!("call i64 @{}(i32 {sid}, i64 {cur}, i32 {i}, ptr {buf})", crate::stream::Streams::AT));
    crate::step::bind_element(var, buf, seq, elem, ctx, m)?;
    m.void_inst(&format!("call void @{}(i32 {sid}, i32 {mi}, i64 {seq})", crate::stream::Streams::EXAMINED));
    crate::stream::note_examined(ex_ptr, seq, m);
    ctx.breaks.push(end_at.clone());
    let result = block(body, ctx, m);
    ctx.breaks.pop();
    result?;
    let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let next = m.inst(&format!("add i32 {cur_i}, 1"));
    m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&end_at);
    Ok(())
}

/// `for x in a` ueber ein Array fester Laenge oder eine Sammlung mit
/// Laenge (`bytes<N>`, `vec<T, N>`): die Werte der Reihe nach in die
/// Schleifenvariable.
fn for_items(var: takt_mir::VarId, iter: &Expr, body: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let x = lower_expr(iter, ctx.program, m, &vars)?;
    let (elem_ty, len, data_index) = match &x.ty {
        LlvmType::Array(elem, n) => ((**elem).clone(), n.to_string(), None),
        LlvmType::Struct(fields) if fields.len() == 2 && fields[0] == LlvmType::Int(32) => {
            let LlvmType::Array(elem, _) = &fields[1] else {
                return Err(NotYet { what: "`for` ueber diese Sammlung" });
            };
            let n = m.inst(&format!("extractvalue {} {}, 0", x.ty, x.value));
            ((**elem).clone(), n.to_string(), Some(1))
        }
        _ => return Err(NotYet { what: "`for` ueber diese Sammlung" }),
    };
    let slot = m.alloca(&x.ty);
    m.write(&x.ty, &x.value, &slot.to_string());
    let (ptr, ty) = place(&Place::Var(var), ctx, m)?;
    let k = ctx.next_label();
    let name = ctx.machine.name.clone();
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let (head, loop_body, end_at) =
        (format!("elemente{k}_{name}"), format!("elemente{k}_{name}_rumpf"), format!("elemente{k}_{name}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {len}"));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    let at = match data_index {
        Some(d) => m.inst(&format!("getelementptr inbounds {}, ptr {slot}, i32 0, i32 {d}, i32 {i}", x.ty)),
        None => m.inst(&format!("getelementptr inbounds {}, ptr {slot}, i32 0, i32 {i}", x.ty)),
    };
    let v = m.inst(&format!("load {elem_ty}, ptr {at}"));
    if elem_ty != ty {
        return Err(NotYet { what: "`for` mit anderem Elementtyp" });
    }
    m.void_inst(&format!("store {ty} {v}, ptr {ptr}"));
    ctx.breaks.push(end_at.clone());
    let result = block(body, ctx, m);
    ctx.breaks.pop();
    result?;
    let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let next = m.inst(&format!("add i32 {cur_i}, 1"));
    m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
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
    // 12.10: Ein Portzugriff ist `volatile` — sofort und in
    // Programmreihenfolge, nicht umgeordnet oder zusammengefasst. `memset`,
    // `memmove` und `sret` tragen das nicht, also bleibt es beim `store`.
    if roots_in_port(target) {
        let v = lower_expr(value, ctx.program, m, &vars)?;
        let (ptr, _) = place(target, ctx, m)?;
        m.void_inst(&format!("store volatile {} {}, ptr {ptr}", v.ty, v.value));
        return Ok(());
    }
    // Ein Index im Ziel kann faulten; der Interpreter rechnet den Wert
    // davor, und die Reihenfolge der Faults ist Spur (Satz 9.4.4).
    if indexed(target) {
        let want = ty::lower(value.ty, ctx.program).ok_or(NotYet { what: "Typ" })?;
        if want.indirect() {
            let tmp = m.alloca(&want);
            crate::expr::store(value, &tmp.to_string(), None, ctx.program, m, &vars)?;
            let (dst, _) = place(target, ctx, m)?;
            m.copy(&want, &tmp.to_string(), &dst.to_string());
        } else {
            let v = lower_expr(value, ctx.program, m, &vars)?;
            let (dst, _) = place(target, ctx, m)?;
            m.write(&v.ty, &v.value, &dst.to_string());
        }
        return Ok(());
    }
    let (dst, _) = place(target, ctx, m)?;
    crate::expr::store(value, &dst.to_string(), Some(target), ctx.program, m, &vars)
}

/// Fuehrt die Stelle ueber einen Index? (3.9: er wird geprueft)
fn indexed(p: &Place) -> bool {
    match p {
        Place::Index(..) | Place::Index2(..) => true,
        Place::Field(b, _) => indexed(b),
        Place::Var(_) | Place::Output(_) | Place::Port(_) => false,
    }
}

/// Wurzelt die Stelle in einem Registerport? (12.10)
fn roots_in_port(p: &Place) -> bool {
    match p {
        Place::Port(_) => true,
        Place::Field(b, _) | Place::Index(b, _) | Place::Index2(b, _, _) => roots_in_port(b),
        Place::Var(_) | Place::Output(_) => false,
    }
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
/// Der MIR-Typ einer Stelle, soweit er statisch feststeht.
fn place_mir_type(target: &Place, ctx: &Ctx<'_>) -> Option<takt_mir::TypeId> {
    match target {
        Place::Var(id) => Some(ctx.machine.vars.get(id.index())?.ty),
        Place::Output(c) => Some(ctx.program.channels.get(c.index())?.ty),
        Place::Field(base, field) => {
            let ty = place_mir_type(base, ctx)?;
            let Type::Record(r) = ctx.program.types.list.get(ty.index())? else { return None };
            Some(ctx.program.records.get(r.index())?.fields.get(*field as usize)?.ty)
        }
        _ => None,
    }
}

/// `m.insert(k, v)`, `m.remove(k)` (3.9): Schluessel und Wert gehen in
/// kanonischer, auf K bzw. V Byte aufgefuellter Form an
/// `takt_native_map_*`, das ueber den Slots der Map sondiert — dieselbe
/// Logik wie `takt_native::map`, die der Interpreter ruft.
fn map_method_call(
    target: Option<&Place>,
    receiver: &Place,
    (key, value, cap): (takt_mir::TypeId, takt_mir::TypeId, u32),
    method: Method,
    args: &[Expr],
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let p = ctx.program;
    let (klen, vlen) = crate::persist::map_widths(p, key, value)?;
    let (slots, _) = place(receiver, ctx, m)?;
    let vars = ctx.vars();
    let k = args.first().ok_or(NotYet { what: "map-Methode ohne Schluessel" })?;
    let kbuf = crate::persist::encode_padded(k, klen, p, m, &vars)?;
    let hit = match method {
        Method::Insert => {
            let v = args.get(1).ok_or(NotYet { what: "`insert` ohne Wert" })?;
            let vbuf = crate::persist::encode_padded(v, vlen, p, m, &vars)?;
            m.needs_intrinsic("i1 @takt_native_map_insert(ptr, i32, i32, i32, ptr, ptr)");
            m.inst(&format!(
                "call i1 @takt_native_map_insert(ptr {slots}, i32 {cap}, i32 {klen}, i32 {vlen}, ptr {kbuf}, ptr {vbuf})"
            ))
        }
        Method::Remove => {
            m.needs_intrinsic("i1 @takt_native_map_remove(ptr, i32, i32, i32, ptr)");
            m.inst(&format!(
                "call i1 @takt_native_map_remove(ptr {slots}, i32 {cap}, i32 {klen}, i32 {vlen}, ptr {kbuf})"
            ))
        }
        other => return Err(collection::unsupported(other)),
    };
    if let Some(t) = target {
        let (dst, _) = place(t, ctx, m)?;
        m.void_inst(&format!("store i1 {hit}, ptr {dst}"));
    }
    Ok(())
}

fn place(target: &Place, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(Reg, LlvmType), NotYet> {
    match target {
        // 12.10: Die Adresse steht in der Deklaration; ob der Zugriff
        // `volatile` wird, entscheidet die Stelle, die ihn erzeugt.
        Place::Port(id) => {
            let port = ctx.program.ports.get(id.index()).ok_or(NotYet { what: "Port" })?;
            let ty = ty::lower(port.ty, ctx.program).ok_or(NotYet { what: "Portrecord" })?;
            let ptr = m.inst(&format!("inttoptr i64 {} to ptr", port.address));
            Ok((ptr, ty))
        }
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
        Place::Index2(base, row, col) => {
            let (ptr, ty) = place(base, ctx, m)?;
            let (_, _, elem) = crate::matrix::shape(&ty).ok_or(NotYet { what: "Index auf Nicht-Matrix" })?;
            let vars = ctx.vars();
            let i = lower_expr(row, ctx.program, m, &vars)?;
            let j = lower_expr(col, ctx.program, m, &vars)?;
            let at = m.inst(&format!(
                "getelementptr inbounds {ty}, ptr {ptr}, i32 0, {} {}, {} {}",
                i.ty, i.value, j.ty, j.value
            ));
            Ok((at, elem))
        }
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
    if let Some(Type::Map { key, value, cap }) =
        place_mir_type(receiver, ctx).and_then(|t| ctx.program.types.list.get(t.index()))
    {
        return map_method_call(target, receiver, (*key, *value, *cap), method, args, ctx, m);
    }
    if !collection::is_collection_method(method) {
        return Err(collection::unsupported(method));
    }
    let (recv, ty) = place(receiver, ctx, m)?;
    let layout = collection::layout_of(&ty).ok_or(NotYet { what: "Methode auf einer Nicht-Sammlung" })?;
    let label = m.next_label();
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
    /// Der `sret`-Platz, wenn die Rueckgabe indirekt geht (FB-214).
    pub sret: Option<Reg>,
    /// Der Rueckgabetyp, wie das Programm ihn nennt.
    pub ret: LlvmType,
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
        let slots = m.slot_mark();
        fn_stmt(s, ctx, m)?;
        m.end_slots(slots);
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
            match ctx.sret {
                Some(out) => {
                    crate::expr::store(e, &out.to_string(), None, ctx.program, m, &ctx.vars)?;
                    m.void_inst("ret void");
                }
                None => {
                    let v = lower_expr(e, ctx.program, m, &ctx.vars)?;
                    m.void_inst(&format!("ret {} {}", v.ty, v.value));
                }
            }
            Ok(())
        }
        StmtKind::Assign { target, value } => {
            if indexed(target) {
                let want = ty::lower(value.ty, ctx.program).ok_or(NotYet { what: "Typ" })?;
                if want.indirect() {
                    let tmp = m.alloca(&want);
                    crate::expr::store(value, &tmp.to_string(), None, ctx.program, m, &ctx.vars)?;
                    let (ptr, _) = fn_place(target, ctx, m)?;
                    m.copy(&want, &tmp.to_string(), &ptr.to_string());
                } else {
                    let v = lower_expr(value, ctx.program, m, &ctx.vars)?;
                    let (ptr, _) = fn_place(target, ctx, m)?;
                    m.write(&v.ty, &v.value, &ptr.to_string());
                }
                return Ok(());
            }
            let (ptr, _) = fn_place(target, ctx, m)?;
            crate::expr::store(value, &ptr.to_string(), Some(target), ctx.program, m, &ctx.vars)
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
                            let tmp = m.alloca(&v.ty);
                            m.write(&v.ty, &v.value, &tmp.to_string());
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
    let fid = match method {
        Method::Step => def.step.ok_or(NotYet { what: "Block ohne `step`" })?,
        // Jede weitere Methode (5.7): ein gewoehnlicher Aufruf mit der
        // Instanz als erstem Argument. Nur `step` ist auf einmal je
        // Aktivierung beschraenkt; `result()` oder `converged()` lesen
        // und duerfen so oft laufen, wie das Programm sie nennt.
        Method::Block(fid) => fid,
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
    // 5.7: `step` hoechstens einmal je Aktivierung. Der Zweig ueberspringt
    // den zweiten Aufruf, statt ihn zu wiederholen.
    let once = method == Method::Step;
    let label = m.next_label();
    let end_at = format!("step{label}_ende");
    if once {
        let flag = m.inst(&format!(
            "getelementptr inbounds {}, ptr {ptr}, i32 0, i32 {}",
            LlvmType::Struct(inst.fields.clone()),
            inst.stepped()
        ));
        let done = m.inst(&format!("load i1, ptr {flag}"));
        let go_on = format!("step{label}");
        m.void_inst(&format!("br i1 {done}, label %{end_at}, label %{go_on}"));
        m.label(&go_on);
        m.void_inst(&format!("store i1 true, ptr {flag}"));
    }
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
    if once {
        m.void_inst(&format!("br label %{end_at}"));
        m.label(&end_at);
    }
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
    let exit = ctx.vars().fault_label().ok_or(NotYet { what: "Fault-Marke" })?;
    crate::block::init_state(ptr, def, inst, exit, ctx.program, m)
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
        Place::Index2(base, row, col) => {
            let (ptr, ty) = fn_place(base, ctx, m)?;
            let (_, _, elem) = crate::matrix::shape(&ty).ok_or(NotYet { what: "Index auf Nicht-Matrix" })?;
            let i = lower_expr(row, ctx.program, m, &ctx.vars)?;
            let j = lower_expr(col, ctx.program, m, &ctx.vars)?;
            let at = m.inst(&format!(
                "getelementptr inbounds {ty}, ptr {ptr}, i32 0, {} {}, {} {}",
                i.ty, i.value, j.ty, j.value
            ));
            Ok((at, elem))
        }
        _ => Err(NotYet { what: "Zuweisungsziel in einer Funktion" }),
    }
}
