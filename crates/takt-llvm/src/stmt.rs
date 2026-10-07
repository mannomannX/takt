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
    /// Anhang der Fault-Marken dieser Funktion: Eine Maschine hat mehrere
    /// Funktionen mit Trampolin (Schritt, Entry-Ticks), die Marken eines
    /// Moduls sind aber eindeutig.
    pub tag: String,
    /// Wie viele Meldungsstellen die Maschine schon hat.
    ///
    /// Der Index identifiziert die Stelle im Trace; die Reihenfolge ist
    /// die der Erzeugung und damit die des Quelltexts.
    sites: u32,
    /// Sprungziele der laufenden Schleifen; `break` nimmt das oberste.
    breaks: Vec<String>,
    /// Die laufenden `for`-Schleifen von aussen nach innen: Zeiger auf den
    /// Durchlaufzaehler, sein Typ und die statische Schranke (5.6).
    loops: Vec<(String, LlvmType, Option<u32>)>,
    /// Das Ende der Schrittfunktion; `->` als Anweisung springt dorthin
    /// (11.2). `None` heisst: Der Block laeuft im Entry-Modus oder in
    /// einer Funktion, wo ein `->` nicht wirkt (5.2 Regel 4).
    pub end: Option<String>,
    /// In einer `loop:`-Funktion: wahr im Entry-Tick, wo `->` nicht wirkt.
    pub entry_reg: Option<Reg>,
    /// Der Block laeuft im Modus ENTRY (9.3), auch wenn er im Schritt steht:
    /// die Aktionen eines Uebergangs, `exit:` und `enter:` eines Wechsels.
    pub entry_mode: bool,
    /// Die Fault-Pfade, die am Ende der Funktion entstehen: je Quelle einer
    /// (5.3, [`FaultFrom`]).
    pub fault_paths: Vec<FaultFrom>,
    /// Anhang ihrer Marken. In einer `loop:`-Funktion fuehrt die Marke
    /// eines Blatts zurueck in den Schritt; die Pfade eines Wechsels dort
    /// brauchen eigene.
    pub fault_suffix: &'static str,
    /// Ein Fault-Ziel ausserhalb des Schritts (Trigger-Phase, 7.5).
    pub fault: Option<String>,
}

impl<'a> Ctx<'a> {
    /// Ein Kontext fuer eine Maschine.
    pub fn new(machine: &'a Machine, state: &'a StateStruct, program: &'a Program) -> Ctx<'a> {
        let machine_index = program.machines.iter().position(|m| m.name == machine.name).unwrap_or(0) as u32;
        Ctx {
            machine,
            state,
            program,
            machine_index,
            leaf: None,
            leaf_reg: None,
            region: Vec::new(),
            tag: String::new(),
            sites: 0,
            breaks: Vec::new(),
            loops: Vec::new(),
            end: None,
            entry_reg: None,
            entry_mode: false,
            fault_paths: Vec::new(),
            fault_suffix: "",
            fault: None,
        }
    }

    /// Eine frische Nummer fuer eine Meldungsstelle (9.3).
    pub fn next_site(&mut self) -> u32 {
        let n = self.sites;
        self.sites += 1;
        n
    }

    /// Der laufende Durchlauf als Platz in den Zaehlern einer Stelle (5.6):
    /// die Indizes der umgebenden `for`-Schleifen, gemischt nach ihren
    /// Schranken, als `i64`-Operand.
    pub fn pass_index(&self, m: &mut Module) -> Result<String, NotYet> {
        let mut acc: Option<String> = None;
        for (ptr, ty, bound) in &self.loops {
            let bound = bound.ok_or(NotYet { what: "Zaehler in einer Schleife ohne statische Schranke" })?;
            let i = m.inst(&format!("load {ty}, ptr {ptr}"));
            // Ein Durchlaufzaehler ist nie negativ; `zext` gilt fuer jede
            // Breite, auch fuer eine verengte (3.4).
            let wide = match ty {
                LlvmType::Int(64) => i.to_string(),
                _ => m.inst(&format!("zext {ty} {i} to i64")).to_string(),
            };
            acc = Some(match acc {
                None => wide,
                Some(a) => {
                    let scaled = m.inst(&format!("mul i64 {a}, {bound}"));
                    m.inst(&format!("add i64 {scaled}, {wide}")).to_string()
                }
            });
        }
        Ok(acc.unwrap_or_else(|| "0".into()))
    }

    /// Der Zeiger auf den Zaehler einer Stelle im laufenden Durchlauf
    /// (5.6, 5.8).
    pub fn counter(&self, role: Role, nth: usize, pass: &str, m: &mut Module) -> Result<Reg, NotYet> {
        let i = self.state.index_of(role, nth).ok_or(NotYet { what: "Zaehler im Zustand" })?;
        let n = match &self.state.fields[i as usize].ty {
            LlvmType::Array(_, n) => *n,
            _ => return Err(NotYet { what: "Zaehler ohne Platz je Durchlauf" }),
        };
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        let field = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
        Ok(m.inst(&format!("getelementptr inbounds [{n} x i64], ptr {field}, i64 0, i64 {pass}")))
    }

    /// Die Periode der Maschine in Nanosekunden (`P_m`, 7.2).
    fn period_ns(&self) -> i64 {
        i64::from(self.machine.period.max(1)).saturating_mul(self.program.config.tick)
    }

    /// Eine frische Nummer fuer eine Marke.
    ///
    /// Marken muessen je erzeugter Verzweigung eindeutig sein, nicht je
    /// Zustand oder Anweisung: Derselbe Block kann mehrfach erzeugt
    /// werden, etwa der `loop:` einer Zwischenebene je Blatt darunter
    /// (5.2).
    pub fn next_label(&mut self, m: &mut Module) -> u32 {
        m.next_label()
    }

    /// Ob das Fenster der Stroeme hier leer ist: im Modus ENTRY (9.6). Fest
    /// in Eintritt, `exit:`, `enter:` und den Aktionen eines Uebergangs, in
    /// einer `loop:`-Funktion ihr Entry-Register; `None`, wo es offen ist.
    pub fn window_closed(&self) -> Option<String> {
        if self.entry_mode || self.end.is_none() {
            return Some("true".to_string());
        }
        self.entry_reg.map(|r| r.to_string())
    }

    /// Die Variablenabbildung dieser Maschine.
    pub fn vars(&self) -> StateVars<'a> {
        StateVars {
            machine: self.machine,
            leaf: self.leaf,
            shared: self.leaf.is_none() && self.leaf_reg.is_some(),
            tag: self.tag.clone(),
            state: self.state,
            program: self.program,
            machine_index: self.machine_index,
            fault: self.fault.clone(),
            window_closed: self.window_closed(),
        }
    }

    /// Der Zeiger auf das `n`-te Feld einer Rolle im Zustands-Struct.
    ///
    /// Das ist die einzige Stelle, an der ein Feldindex in Code wird —
    /// `StateStruct` ist die Quelle, und hier wird sie gelesen.
    pub fn field(&self, role: Role, nth: usize, m: &mut Module) -> Option<Reg> {
        self.state.field_ptr(&self.machine.name, role, nth, m)
    }

    /// Liest die Maschine `last_fault` (5.3)? Dann fuehrt ihr Zustand es.
    pub fn reads_last_fault(&self) -> bool {
        self.state.index_of(Role::LastFault, 0).is_some()
    }

    /// Der Name des Fault-Trampolins des laufenden Blatts (5.3).
    ///
    /// Je Blatt einer, weil das Fault-Ziel am innersten Zustand haengt,
    /// der eines deklariert (Fault-Wald). Ein gesetztes `fault` geht vor,
    /// wie bei `StateVars::fault_label`: Im Wechsel gilt der Pfad des
    /// neuen Blatts, auch fuer `send` und `at` in seinem `enter:`.
    pub fn trampoline(&self) -> String {
        if let Some(f) = &self.fault {
            return f.clone();
        }
        match self.leaf {
            Some(leaf) => format!("fault_{}_{}{}", self.machine.name, leaf.index(), self.tag),
            None => format!("fault_{}_any{}", self.machine.name, self.tag),
        }
    }

    /// Der Trampolin fuer einen Fault der Art `kind` (5.3), ueber den
    /// Block, der die Art ablegt.
    pub fn trampoline_for(&self, kind: takt_mir::machine::FaultKind, m: &mut Module) -> String {
        m.fault_to(&self.trampoline(), crate::abi::fault_code(kind))
    }

    /// Die Marke des Fault-Pfads ab `from` in dieser Funktion (5.3). Der
    /// Pfad wird vorgemerkt und am Ende der Funktion geschrieben.
    pub fn fault_path(&mut self, from: FaultFrom) -> String {
        if !self.fault_paths.contains(&from) {
            self.fault_paths.push(from);
        }
        let at = match from {
            FaultFrom::State(s) => s.index().to_string(),
            FaultFrom::Root => "wurzel".to_string(),
            FaultFrom::Faulted => "faulted".to_string(),
            FaultFrom::Redirected(s, to) => {
                format!("{}_x{}", s.index(), to.map_or("f".to_string(), |t| t.index().to_string()))
            }
        };
        format!("fault_{}_{at}{}{}", self.machine.name, self.tag, self.fault_suffix)
    }
}

/// Woher ein Fault-Pfad ausgeht (5.3, 9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultFrom {
    /// Ein aktiver Zustand: das Blatt, nach einem Fault in `exit:` der
    /// kleinste gemeinsame Vorfahr des Wechsels (FB-289).
    State(takt_mir::StateId),
    /// Kein Zustand mehr aktiv: Ein `exit:` der obersten Ebene scheiterte,
    /// und der Fault faellt auf das Fault-Ziel der Maschine.
    Root,
    /// `FAULTED`, die Senke des Fault-Walds.
    Faulted,
    /// Ein `check … -> X` im Blatt: Der Fault fuehrt nach `X` statt zum
    /// Fault-Ziel des Zustands, mit `None` nach `FAULTED` (5.3, FB-412).
    Redirected(takt_mir::StateId, Option<takt_mir::StateId>),
}

impl FaultFrom {
    /// Das Blatt `leaf` als Quelle, `None` als `FAULTED`.
    pub fn of(leaf: Option<takt_mir::StateId>) -> FaultFrom {
        leaf.map_or(FaultFrom::Faulted, FaultFrom::State)
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
    /// Anhang der Fault-Marken der Funktion (`Ctx::tag`).
    pub tag: String,
    /// Ihr Zustands-Struct.
    pub state: &'a StateStruct,
    /// Das Programm, fuer die Typen.
    pub program: &'a Program,
    /// Die Nummer der Maschine im Programm (`Ctx::machine_index`).
    pub machine_index: u32,
    /// Ein Fault-Ziel ausserhalb des Schritts (`Ctx::fault`).
    pub fault: Option<String>,
    /// Ob das Fenster der Stroeme leer ist (`Ctx::window_closed`).
    pub window_closed: Option<String>,
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
        if let Some(f) = &self.fault {
            return Some(f.clone());
        }
        if self.shared {
            return Some(format!("fault_{}_any{}", self.machine.name, self.tag));
        }
        Some(format!("fault_{}_{}{}", self.machine.name, self.leaf?.index(), self.tag))
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

    fn window_closed(&self) -> Option<String> {
        self.window_closed.clone()
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

    fn stream_dropped(&self, stream: takt_mir::expr::StreamRef, m: &mut Module) -> Option<String> {
        let Some(nth) = self.machine.layout.cursors.iter().position(|c| *c == stream) else {
            return Some("0".into());
        };
        let i = self.state.index_of(Role::Dropped, nth)?;
        let state_ty = format!("%{}_state", crate::fns::sanitized(&self.machine.name));
        let at = m.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {i}"));
        Some(m.inst(&format!("load i32, ptr {at}")).to_string())
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
        let want = ty::lower(self.machine.vars.get(id.index())?.ty, self.program)?;
        Some(crate::expr::fit(Lowered { value: v.to_string(), ty }, &want, m))
    }

    /// Die Adresse und die Speicherform (`ty::storage`).
    fn address(&self, id: takt_mir::VarId, m: &mut Module) -> Option<(Reg, LlvmType)> {
        let def = self.machine.vars.get(id.index())?;
        let ty = ty::storage(def.ty, self.program)?;
        Some((self.state.field_ptr(&self.machine.name, Role::Var, id.index(), m)?, ty))
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
                let v = m.inst(&format!("call i64 @{}(ptr %arena)", m.runtime(crate::abi::Abi::NOW)));
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
            B::LastFault => {
                let at = self.state.field_ptr(&self.machine.name, Role::LastFault, 0, m)?;
                crate::fault::value(&at, p, m).ok()
            }
            B::Event => None,
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

    /// Der Latch eines eigenen Outputs; `%3` ist der Latch (11.2). Ein
    /// fremder Output kommt aus der Ψ-Bank seines Besitzers (8.3).
    fn output(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        let c = self.program.channels.get(channel.index())?;
        let ty = ty::lower(c.ty, self.program)?;
        if let Some(owner) = c.owner.filter(|o| o.0 != self.machine_index) {
            let off = crate::psi::region_offset(owner, false, self.program)?
                + crate::psi::field_offset(owner, crate::psi::Field::Output(channel), self.program)?;
            let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
            let v = m.inst(&format!("load {ty}, ptr {at}"));
            return Some(Lowered { value: v.to_string(), ty });
        }
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

/// Senkt eine Anweisung. Eine Fault-Stelle darin nennt ihre Zeile (5.3).
pub fn stmt(s: &Stmt, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let outer = std::mem::replace(&mut m.at, s.span);
    let r = stmt_here(s, ctx, m);
    m.at = outer;
    r
}

#[deny(clippy::wildcard_enum_match_arm)]
fn stmt_here(s: &Stmt, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    match &s.kind {
        StmtKind::Assign { target, value } => assign(target, value, ctx, m),
        // `within` gilt der Latenzanalyse (9.4.5), `req` dem Bericht (13.4);
        // zur Laufzeit wirken beide nicht.
        StmtKind::Check { cond, kind, confirm, message, target, within: _, req: _ } => {
            check(cond, *kind, confirm.as_ref(), message.as_ref(), target.as_ref(), ctx, m)
        }
        StmtKind::If { cond, then, otherwise } => branch(cond, then, otherwise, ctx, m),
        StmtKind::Observe(o) => observe(o, s.span, ctx, m),
        StmtKind::Match { subject, arms } => match_stmt(subject, arms, ctx, m),
        StmtKind::MethodCall { target, receiver, method, args } => {
            method_call(target.as_ref(), receiver, *method, args, ctx, m)
        }
        StmtKind::Abort { message } => {
            // 5.4: `abort` faultet *alle* Maschinen im selben Tick. Die
            // anderen kennt der erzeugte Code nicht — die Runtime merkt
            // sie vor und stellt in der Abort-Phase zu (`<m>_deliver`).
            // Die eigene Maschine nimmt ihren Fault-Pfad sofort, es sei
            // denn, ihr Latch steht (9.3): Dann endet nur der Schritt.
            let site = ctx.next_site();
            m.void_inst(&format!(
                "call void @{}(ptr %arena, i32 {}, i32 {site})",
                m.runtime(Abi::ABORT),
                ctx.machine_index
            ));
            let latch = ctx.field(Role::AbortLatch, 0, m).ok_or(NotYet { what: "Abort-Latch im Zustand" })?;
            let held = m.inst(&format!("load i1, ptr {latch}"));
            let fault = ctx.trampoline_for(takt_mir::machine::FaultKind::Abort, m);
            let stop = format!("abgebrochen{}_{}", m.next_label(), ctx.machine.name);
            branch_or_fault(&held.to_string(), &stop, &fault, message.as_ref(), "abort", ctx, m)?;
            m.label(&stop);
            m.void_inst(if ctx.entry_reg.is_some() { "ret i8 3" } else { "ret void" });
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
        StmtKind::Send { stream, value, len_max } => send(*stream, value, *len_max, s.span, ctx, m),
        StmtKind::Every { period, counter, body } => every(period, *counter, body, ctx, m),
        StmtKind::At { time, body } => at(time, body, ctx, m),
        // 11.2: „`->` → Setzen der Goto-Vormerkung + Sprung ans
        // Kettenende." Ohne bekanntes Ende laeuft der Block im
        // Entry-Modus, und dort ist ein `->` wirkungslos (5.2 Regel 4) —
        // genau das, was `exec` mit `Mode::Entry` tut.
        StmtKind::Goto(target) => match (ctx.end.clone(), ctx.entry_reg) {
            (Some(end), Some(entry)) => {
                let k = ctx.next_label(m);
                let (go, stay) = (format!("gehe{k}_{}", ctx.machine.name), format!("bleibe{k}_{}", ctx.machine.name));
                m.void_inst(&format!("br i1 {entry}, label %{stay}, label %{go}"));
                m.label(&go);
                crate::step::goto(*target, ctx, m, &end)?;
                m.void_inst(&format!("br label %{stay}"));
                m.label(&stay);
                Ok(())
            }
            (Some(end), None) => crate::step::goto(*target, ctx, m, &end),
            (None, _) => Ok(()),
        },
        StmtKind::Cancel(c) => {
            m.void_inst(&format!("call void @{}(ptr %arena, i32 {})", m.runtime(Abi::CANCEL), c.0));
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
        StmtKind::Skip(stream) => skip(*stream, ctx, m),
        // `return` steht nur in Funktionen (`fns.rs`).
        StmtKind::Return(_) => Err(NotYet { what: "`return` in einer Maschine" }),
    }
}

/// `s.skip()` (8.6): untersucht das ganze Fenster und verwirft es — der
/// Cursor rueckt hinter dessen letztes Element, wie im Interpreter. Im
/// Modus ENTRY ist das Fenster leer (9.6).
fn skip(stream: takt_mir::expr::StreamRef, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    if ctx.entry_mode {
        return Ok(());
    }
    let p = ctx.program;
    let elem = crate::stream::element(p, stream).ok_or(NotYet { what: "Elementtyp eines Stroms" })?;
    let sid = crate::stream::number(stream).ok_or(NotYet { what: "Strom ohne feste Nummer" })?;
    let vars = ctx.vars();
    let (cur_ptr, ex_ptr) = vars.stream_slots(stream, m).ok_or(NotYet { what: "Cursor eines Stroms" })?;
    let buf = crate::stream::scratch(p, elem, m)?;
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n =
        m.inst(&format!("call i32 @{}(ptr %arena, i32 {sid}, i64 {cur})", m.runtime(crate::stream::Streams::COUNT)));
    let n = crate::stream::window_count(n, vars.window_closed(), m);
    let some = m.inst(&format!("icmp sgt i32 {n}, 0"));
    let k = m.next_label();
    let (read, done) = (format!("skip{k}_lesen"), format!("skip{k}_fertig"));
    m.void_inst(&format!("br i1 {some}, label %{read}, label %{done}"));
    m.label(&read);
    let last = m.inst(&format!("sub i32 {n}, 1"));
    let seq = m.inst(&format!(
        "call i64 @{}(ptr %arena, i32 {sid}, i64 {cur}, i32 {last}, ptr {buf})",
        m.runtime(crate::stream::Streams::AT)
    ));
    crate::stream::note_examined(ex_ptr, seq, m);
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    Ok(())
}

/// `job v = f(args)` (4.5): Die Argumente gehen als Folge kanonischer
/// Bloecke (je `u32` Laenge, dann die Bytes) in den Eingang des Slots, von
/// ihrer Stelle aus kodiert — ein Argument von 4 KiB kostete sonst dreimal
/// so viel Stack (FB-455). Die Runtime fuehrt den Job und schreibt den Slot
/// im Abbild.
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
    let buf =
        m.inst(&format!("call ptr @{}(ptr %arena, i32 {}, i32 {slot})", m.runtime(Abi::JOB_ARGS), ctx.machine_index));
    let vars = ctx.vars();
    let mut off = m.inst("add i64 0, 0");
    for a in args {
        let want = crate::ty::lower(a.ty, p).ok_or(NotYet { what: "Job-Argument" })?;
        let src = crate::expr::place_of(a, &want, p, m, &vars)?;
        let body = m.inst(&format!("add i64 {off}, 4"));
        let dst = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {body}"));
        let len = crate::persist::encode_canonical(p, a.ty, src, dst, m)?;
        let len32 = m.inst(&format!("trunc i64 {len} to i32"));
        let lenp = m.inst(&format!("getelementptr inbounds i8, ptr {buf}, i64 {off}"));
        m.void_inst(&format!("store i32 {len32}, ptr {lenp}, align 1"));
        off = m.inst(&format!("add i64 {body}, {len}"));
    }
    let total = m.inst(&format!("trunc i64 {off} to i32"));
    m.void_inst(&format!(
        "call void @{}(ptr %arena, i32 {}, i32 {slot}, i32 {}, i32 {total})",
        m.runtime(Abi::JOB_BEGIN),
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
    span: takt_diag::Span,
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
    let ok = m.inst(&format!(
        "call i1 @{}(ptr %arena, i32 {sid}, ptr {bytes}, i32 {len})",
        m.runtime(crate::stream::Streams::SEND)
    ));
    let go_on = format!("gesendet{}_{}", m.next_label(), ctx.machine.name);
    // 8.6, 8.8: Passt das Element nicht, faultet der Schreiber — ausser
    // mit `overflow = drop`: Dann verwirft er es und meldet einen Alert.
    if crate::machine::drops_when_full(stream, ctx.program) {
        // 5.6: Die Stelle ist ein Alert der Runtime, aktiv, wenn verworfen.
        let (slot, _) = ctx.state.counters.alert(span).ok_or(NotYet { what: "verwerfendes `send` ohne Platz" })?;
        let dropped = m.inst(&format!("xor i1 {ok}, true"));
        m.void_inst(&format!(
            "call void @{}(ptr %arena, i32 {}, i32 {slot}, i1 {dropped}, i1 0)",
            m.runtime(Abi::ALERT),
            ctx.machine_index
        ));
        m.void_inst(&format!("br label %{go_on}"));
    } else {
        let fault = ctx.trampoline_for(takt_mir::machine::FaultKind::StreamOverflow, m);
        m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{fault}"));
    }
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
/// **Der Zaehler steht im Zustand**, ein `i64` je Aufrufstelle und
/// Durchlauf der umgebenden Schleifen (`Role::EveryNext`, 11.2, 5.8):
/// Mit einem Zaehler je Stelle zoegen alle Durchlaeufe gemeinsam.
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
    let pass = ctx.pass_index(m)?;
    let slot = ctx.counter(Role::EveryNext, counter.index(), &pass, m)?;
    let vars = ctx.vars();
    let d = lower_expr(period, ctx.program, m, &vars)?;
    if d.ty != LlvmType::Int(64) {
        return Err(NotYet { what: "`every` mit einer Dauer, die keine Dauer ist" });
    }
    // Die Uhr: `t_in_state` in einem Zustand, sonst `now` (5.8).
    let clock = match site.state {
        Some(_) => time_in_state_ns(ctx, m)?,
        None => {
            // `P_now` steht im Modulkopf (`Abi::declare`).
            m.inst(&format!("call i64 @{}(ptr %arena)", m.runtime(crate::abi::Abi::NOW))).to_string()
        }
    };
    let next = m.inst(&format!("load i64, ptr {slot}"));
    // `-1` heisst „seit dem Eintritt noch nicht gesetzt": Dann gilt `d`
    // als naechster Zeitpunkt (5.8, Startwert `d`).
    let fresh = m.inst(&format!("icmp slt i64 {next}, 0"));
    let due_at = m.inst(&format!("select i1 {fresh}, i64 {}, i64 {next}", d.value));
    let due = m.inst(&format!("icmp sge i64 {clock}, {due_at}"));

    let k = ctx.next_label(m);
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
        m.at = stmt.span;
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
        let code = m.inst(&format!(
            "call i32 @{}(ptr %arena, i32 {}, i64 {}, i64 {word})",
            m.runtime(Abi::SCHEDULE),
            c.0,
            t.value
        ));
        let ok = m.inst(&format!("icmp eq i32 {code}, 0"));
        m.fault_code_at(&code.to_string());
        let here = m.at;
        m.fault_line_at(here);
        let go_on = format!("geplant{}_{}", m.next_label(), ctx.machine.name);
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
    let k = ctx.next_label(m);
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
    ctx.loops.push((ptr.to_string(), ty.clone(), crate::machine::range_bound(count)));
    let result = block(body, ctx, m);
    ctx.loops.pop();
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
    let var = match vars {
        takt_mir::stmt::ForVars::One(var) => var,
        takt_mir::stmt::ForVars::Pair(k, v) => {
            let vars = ctx.vars();
            let want = ty::lower(iter.ty, ctx.program).ok_or(NotYet { what: "`for` ueber diese `map`" })?;
            let slot = crate::expr::place_of(iter, &want, ctx.program, m, &vars)?.to_string();
            let (kp, _) = place(&Place::Var(*k), ctx, m)?;
            let (vp, _) = place(&Place::Var(*v), ctx, m)?;
            let n = ctx.next_label(m);
            let program = ctx.program;
            return pairs_loop(iter.ty, &slot, (kp, vp), n, program, m, &mut |m, end_at, pass| {
                ctx.breaks.push(end_at.to_string());
                ctx.loops.push((pass.to_string(), LlvmType::Int(32), map_cap(iter.ty, program)));
                let result = block(body, ctx, m);
                ctx.loops.pop();
                ctx.breaks.pop();
                result
            });
        }
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
    let k = ctx.next_label(m);
    let name = ctx.machine.name.clone();
    let cur = m.inst(&format!("load i64, ptr {cur_ptr}"));
    let n =
        m.inst(&format!("call i32 @{}(ptr %arena, i32 {sid}, i64 {cur})", m.runtime(crate::stream::Streams::COUNT)));
    let n = crate::stream::window_count(n, ctx.window_closed(), m);
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let direct = crate::stream::direct(ctx.program, elem);
    let buf = if direct { None } else { Some(crate::stream::scratch(ctx.program, elem, m)?) };
    let (head, loop_body, end_at) =
        (format!("fenster{k}_{name}"), format!("fenster{k}_{name}_rumpf"), format!("fenster{k}_{name}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {n}"));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    let seq = match buf {
        Some(buf) => {
            let seq = m.inst(&format!(
                "call i64 @{}(ptr %arena, i32 {sid}, i64 {cur}, i32 {i}, ptr {buf})",
                m.runtime(crate::stream::Streams::AT)
            ));
            crate::step::bind_element(var, buf, seq, elem, ctx, m)?;
            seq
        }
        None => crate::step::bind_direct(var, sid, &cur, &i, elem, ctx, m)?,
    };
    crate::stream::note_examined(ex_ptr, seq, m);
    ctx.breaks.push(end_at.clone());
    ctx.loops.push((i_ptr.to_string(), LlvmType::Int(32), crate::machine::window_bound(stream, ctx.program)));
    let result = block(body, ctx, m);
    ctx.loops.pop();
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
    let want = ty::lower(iter.ty, ctx.program).ok_or(NotYet { what: "`for` ueber diese Sammlung" })?;
    let slot = crate::expr::place_of(iter, &want, ctx.program, m, &vars)?.to_string();
    let (ptr, ty) = place(&Place::Var(var), ctx, m)?;
    let k = ctx.next_label(m);
    let bound = crate::machine::each_bound(iter, ctx.program);
    items_loop(&want, &slot, (&ptr.to_string(), &ty), k, m, &mut |m, end_at, pass| {
        ctx.breaks.push(end_at.to_string());
        ctx.loops.push((pass.to_string(), LlvmType::Int(32), bound));
        let result = block(body, ctx, m);
        ctx.loops.pop();
        ctx.breaks.pop();
        result
    })
}

/// Die Schleife ueber die Elemente einer Sammlung an ihrer Adresse; den
/// Rumpf senkt der Rufer und bekommt dafuer das Ende als Sprungziel und
/// den Zeiger auf den Durchlaufzaehler.
fn items_loop(
    want: &LlvmType,
    slot: &str,
    (ptr, ty): (&str, &LlvmType),
    k: u32,
    m: &mut Module,
    body: &mut dyn FnMut(&mut Module, &str, &str) -> Result<(), NotYet>,
) -> Result<(), NotYet> {
    let (elem_ty, len, data_index) = match want {
        LlvmType::Array(elem, n) => ((**elem).clone(), n.to_string(), None),
        LlvmType::Struct(fields) if fields.len() == 2 && fields[0] == LlvmType::Int(32) => {
            let LlvmType::Array(elem, _) = &fields[1] else {
                return Err(NotYet { what: "`for` ueber diese Sammlung" });
            };
            let at = m.inst(&format!("getelementptr inbounds {want}, ptr {slot}, i32 0, i32 0"));
            let n = m.inst(&format!("load i32, ptr {at}"));
            ((**elem).clone(), n.to_string(), Some(1))
        }
        _ => return Err(NotYet { what: "`for` ueber diese Sammlung" }),
    };
    if elem_ty != *ty {
        return Err(NotYet { what: "`for` mit anderem Elementtyp" });
    }
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let (head, loop_body, end_at) = (format!("elemente{k}"), format!("elemente{k}_rumpf"), format!("elemente{k}_ende"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {len}"));
    m.void_inst(&format!("br i1 {go_on}, label %{loop_body}, label %{end_at}"));
    m.label(&loop_body);
    let at = match data_index {
        Some(d) => m.inst(&format!("getelementptr inbounds {want}, ptr {slot}, i32 0, i32 {d}, i32 {i}")),
        None => m.inst(&format!("getelementptr inbounds {want}, ptr {slot}, i32 0, i32 {i}")),
    };
    let v = m.inst(&format!("load {elem_ty}, ptr {at}"));
    m.void_inst(&format!("store {ty} {v}, ptr {ptr}"));
    body(m, &end_at, &i_ptr.to_string())?;
    let cur_i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let next = m.inst(&format!("add i32 {cur_i}, 1"));
    m.void_inst(&format!("store i32 {next}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&end_at);
    Ok(())
}

/// `for (k, v) in m` (3.9): die belegten Slots einer `map` in ihrer
/// Reihenfolge, wie der Interpreter sie durchlaeuft (`Value::Map`, die
/// Sondierordnung von `takt_native::map`); Schluessel und Wert aus ihrer
/// kanonischen Form in die Schleifenvariablen. Den Rumpf senkt der Rufer.
fn pairs_loop(
    map: takt_mir::TypeId,
    slot: &str,
    (key_ptr, value_ptr): (Reg, Reg),
    k: u32,
    p: &takt_mir::program::Program,
    m: &mut Module,
    body: &mut dyn FnMut(&mut Module, &str, &str) -> Result<(), NotYet>,
) -> Result<(), NotYet> {
    let Some(takt_mir::types::Type::Map { key, value, cap }) = p.types.list.get(map.index()).cloned() else {
        return Err(NotYet { what: "`for (k, v)` ausserhalb einer `map`" });
    };
    let (kw, vw) = crate::persist::map_widths(p, key, value)?;
    let width = 1 + kw + vw;
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {i_ptr}"));
    let (head, used, fill, next, end_at) = (
        format!("paare{k}"),
        format!("paare{k}_slot"),
        format!("paare{k}_rumpf"),
        format!("paare{k}_weiter"),
        format!("paare{k}_ende"),
    );
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp slt i32 {i}, {cap}"));
    m.void_inst(&format!("br i1 {go_on}, label %{used}, label %{end_at}"));
    m.label(&used);
    let off = m.inst(&format!("mul i32 {i}, {width}"));
    let at = m.inst(&format!("getelementptr inbounds i8, ptr {slot}, i32 {off}"));
    let tag = m.inst(&format!("load i8, ptr {at}"));
    let full = m.inst(&format!("icmp eq i8 {tag}, 1"));
    m.void_inst(&format!("br i1 {full}, label %{fill}, label %{next}"));
    m.label(&fill);
    let kp = m.inst(&format!("getelementptr inbounds i8, ptr {at}, i32 1"));
    crate::persist::decode_canonical(p, key, kp, key_ptr, m)?;
    let vp = m.inst(&format!("getelementptr inbounds i8, ptr {at}, i32 {}", 1 + kw));
    crate::persist::decode_canonical(p, value, vp, value_ptr, m)?;
    body(m, &end_at, &i_ptr.to_string())?;
    m.void_inst(&format!("br label %{next}"));
    m.label(&next);
    let cur = m.inst(&format!("load i32, ptr {i_ptr}"));
    let n = m.inst(&format!("add i32 {cur}, 1"));
    m.void_inst(&format!("store i32 {n}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&end_at);
    Ok(())
}

/// Die Zahl der Slots einer `map`: die Schranke ihrer Schleife.
fn map_cap(map: takt_mir::TypeId, p: &takt_mir::program::Program) -> Option<u32> {
    match p.types.list.get(map.index()) {
        Some(takt_mir::types::Type::Map { cap, .. }) => Some(*cap),
        _ => None,
    }
}

/// `for x in a` in einer Funktion (4.4).
fn fn_for_each<V: Slots>(
    vars: &takt_mir::stmt::ForVars,
    iter: &Expr,
    body: &Block,
    ctx: &mut FnCtx<'_, V>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let var = match vars {
        takt_mir::stmt::ForVars::One(var) => var,
        takt_mir::stmt::ForVars::Pair(k, v) => {
            let want = ty::lower(iter.ty, ctx.program).ok_or(NotYet { what: "`for` ueber diese `map`" })?;
            let slot = crate::expr::place_of(iter, &want, ctx.program, m, &ctx.vars)?.to_string();
            let (kp, _) = ctx.vars.slot(*k, m).ok_or(NotYet { what: "Schleifenvariable" })?;
            let (vp, _) = ctx.vars.slot(*v, m).ok_or(NotYet { what: "Schleifenvariable" })?;
            let n = ctx.next_label(m);
            let program = ctx.program;
            return pairs_loop(iter.ty, &slot, (kp, vp), n, program, m, &mut |m, end_at, _| {
                ctx.breaks.push(end_at.to_string());
                let result = fn_block(body, ctx, m);
                ctx.breaks.pop();
                result
            });
        }
    };
    let want = ty::lower(iter.ty, ctx.program).ok_or(NotYet { what: "`for` ueber diese Sammlung" })?;
    let slot = crate::expr::place_of(iter, &want, ctx.program, m, &ctx.vars)?.to_string();
    let (ptr, ty) = ctx.vars.slot(*var, m).ok_or(NotYet { what: "Schleifenvariable" })?;
    let k = ctx.next_label(m);
    items_loop(&want, &slot, (&ptr.to_string(), &ty), k, m, &mut |m, end_at, _| {
        ctx.breaks.push(end_at.to_string());
        let result = fn_block(body, ctx, m);
        ctx.breaks.pop();
        result
    })
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
#[deny(clippy::wildcard_enum_match_arm)]
fn assign(target: &Place, value: &Expr, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    // 12.10: Ein Portzugriff geschieht sofort und in Programmreihenfolge,
    // nicht umgeordnet oder zusammengefasst. `memset`, `memmove` und `sret`
    // tragen das nicht, also bleibt es beim einzelnen Zugriff ueber den
    // Helfer ([`crate::mmio`]).
    if roots_in_port(target) {
        let v = lower_expr(value, ctx.program, m, &vars)?;
        let (ptr, _) = place(target, ctx, m)?;
        m.mmio_write(&v.ty, &ptr, &v.value);
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
    let (dst, ty) = place(target, ctx, m)?;
    // Eine Bereichsganzzahl liegt schmaler, als gerechnet wird (3.4).
    if let LlvmType::Int(_) = &ty
        && ty::lower(value.ty, ctx.program).is_some_and(|want| want != ty)
    {
        let v = lower_expr(value, ctx.program, m, &vars)?;
        let v = crate::expr::fit(v, &ty, m);
        m.write(&ty, &v.value, &dst.to_string());
        return Ok(());
    }
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
#[allow(clippy::too_many_arguments)]
fn check(
    cond: &Expr,
    kind: takt_mir::stmt::CheckKind,
    confirm: Option<&takt_mir::stmt::Confirm>,
    message: Option<&takt_mir::pattern::Format>,
    target: Option<&takt_mir::machine::Target>,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let c = lower_expr(cond, ctx.program, m, &vars)?;
    if c.ty != LlvmType::Int(1) {
        return Err(NotYet { what: "Bedingung ist kein `bool`" });
    }
    // 5.6: Mit `for d` scheitert der Check erst, wenn die Bedingung `d`
    // lang ununterbrochen verletzt ist. Der Zaehler waechst je verletzter
    // Auswertung um die Periode; eine erfuellte setzt ihn zurueck, und
    // der Fault auch — wie `exec` im Interpreter.
    let holds = match confirm {
        None => c.value,
        Some(k) => {
            let d = duration_of(&k.duration, ctx, m)?;
            let pass = ctx.pass_index(m)?;
            let viol = ctx.counter(Role::Viol, k.site.index(), &pass, m)?;
            let old = m.inst(&format!("load i64, ptr {viol}"));
            let up = m.inst(&format!("add i64 {old}, {}", ctx.period_ns()));
            let due = m.inst(&format!("icmp sge i64 {up}, {d}"));
            let kept = m.inst(&format!("select i1 {due}, i64 0, i64 {up}"));
            let new = m.inst(&format!("select i1 {}, i64 0, i64 {kept}", c.value));
            m.void_inst(&format!("store i64 {new}, ptr {viol}"));
            let calm = m.inst(&format!("xor i1 {due}, true"));
            m.inst(&format!("or i1 {}, {calm}", c.value)).to_string()
        }
    };
    let go_on = format!("weiter{}_{}", m.next_label(), ctx.machine.name);
    let fault_kind = match kind {
        takt_mir::stmt::CheckKind::Check => takt_mir::machine::FaultKind::CheckFailed,
        takt_mir::stmt::CheckKind::Expect => takt_mir::machine::FaultKind::Expect,
    };
    let code = crate::abi::fault_code(fault_kind);
    // 5.3: `check e -> X` fuehrt nach `X` statt zum Fault-Ziel; ueber
    // mehreren Blaettern entscheidet das Blatt zur Laufzeit, wie bei `->`.
    let to = match target {
        None => None,
        Some(takt_mir::machine::Target::State(s)) => Some(Some(*s)),
        Some(takt_mir::machine::Target::Faulted) => Some(None),
        Some(takt_mir::machine::Target::Fault(_)) => return Err(NotYet { what: "Fault-Art als Ziel eines `check`" }),
    };
    let (fault, shared) = match (to, ctx.leaf) {
        (None, _) => (ctx.trampoline_for(fault_kind, m), None),
        (Some(to), Some(leaf)) => {
            let path = ctx.fault_path(FaultFrom::Redirected(leaf, to));
            (m.fault_to(&path, code), None)
        }
        (Some(to), None) => {
            let shared = format!("abweichung{}_{}", m.next_label(), ctx.machine.name);
            (m.fault_to(&shared, code), Some((shared, to)))
        }
    };
    branch_or_fault(&holds, &go_on, &fault, message, "check verletzt", ctx, m)?;
    if let Some((shared, to)) = shared {
        let leaf_reg = ctx.leaf_reg.ok_or(NotYet { what: "`check -> X` ausserhalb eines Blattzweigs" })?;
        let leaves = crate::machine::leaves(ctx.machine);
        let mut arms = Vec::new();
        for leaf in ctx.region.clone() {
            let i = leaves.iter().position(|l| *l == leaf).ok_or(NotYet { what: "Blatt" })?;
            arms.push(format!("i8 {i}, label %{}", ctx.fault_path(FaultFrom::Redirected(leaf, to))));
        }
        m.label(&shared);
        let none = format!("{shared}_kein_blatt");
        m.void_inst(&format!("switch i8 {leaf_reg}, label %{none} [ {} ]", arms.join(" ")));
        m.label(&none);
        m.void_inst("unreachable");
    }
    m.label(&go_on);
    Ok(())
}

/// Verzweigt bei `cond` nach `ok`, sonst in den Fault-Pfad `fault`. Liest
/// die Maschine `last_fault`, legt der Fault-Zweig vorher die Nachricht der
/// Anweisung ab — ihre Meldung, ohne sie `default` (5.3, `crate::fault`).
fn branch_or_fault(
    cond: &str,
    ok: &str,
    fault: &str,
    message: Option<&takt_mir::pattern::Format>,
    default: &str,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    if !ctx.reads_last_fault() {
        m.void_inst(&format!("br i1 {cond}, label %{ok}, label %{fault}"));
        return Ok(());
    }
    let say = format!("meldung{}_{}", m.next_label(), ctx.machine.name);
    m.void_inst(&format!("br i1 {cond}, label %{ok}, label %{say}"));
    m.label(&say);
    let vars = ctx.vars();
    crate::fault::state(message, default, ctx.program, m, &vars)?;
    m.void_inst(&format!("br label %{fault}"));
    Ok(())
}

/// Eine Bestaetigungszeit in Nanosekunden (5.6).
fn duration_of(d: &Expr, ctx: &Ctx<'_>, m: &mut Module) -> Result<String, NotYet> {
    let vars = ctx.vars();
    let d = lower_expr(d, ctx.program, m, &vars)?;
    if d.ty != LlvmType::Int(64) {
        return Err(NotYet { what: "Bestaetigungszeit, die keine Dauer ist" });
    }
    Ok(d.value)
}

/// `if c: … else: …`
fn branch(cond: &Expr, then: &Block, otherwise: &Block, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let vars = ctx.vars();
    let c = lower_expr(cond, ctx.program, m, &vars)?;
    let n = m.next_label();
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
fn observe(o: &Observe, span: takt_diag::Span, ctx: &mut Ctx<'_>, m: &mut Module) -> Result<(), NotYet> {
    let machine = ctx.machine_index;
    // Ohne Diagnose entfallen die Aufrufe und mit ihnen die Ausdruecke:
    // Eine Beobachtung faultet nicht, ihre Ausdruecke wirken also nicht.
    // `alert` ist ein Betriebssignal (5.6) und bleibt.
    let silent = m.diagnostics == crate::target::Diagnostics::None && !matches!(o, Observe::Alert { .. });
    if silent {
        ctx.next_site();
        return Ok(());
    }
    match o {
        Observe::Alert { cond, confirm, .. } => {
            // 3.5: Ein ungueltiger Wert laesst den Alert feuern, mit Zusatz.
            let (c, invalid) = observed(cond, "true", ctx, m, |v, _| Ok(v))?;
            let pass = ctx.pass_index(m)?;
            // 5.6: Mit `for d` wird der Alert erst aktiv, wenn die
            // Bedingung `d` lang zutrifft; der Zaehler steht bei `d` still.
            let active = match confirm {
                None => c.value,
                Some(k) => {
                    let d = duration_of(&k.duration, ctx, m)?;
                    let viol = ctx.counter(Role::Viol, k.site.index(), &pass, m)?;
                    let old = m.inst(&format!("load i64, ptr {viol}"));
                    let up = m.inst(&format!("add i64 {old}, {}", ctx.period_ns()));
                    let below = m.inst(&format!("icmp slt i64 {up}, {d}"));
                    let capped = m.inst(&format!("select i1 {below}, i64 {up}, i64 {d}"));
                    let new = m.inst(&format!("select i1 {}, i64 {capped}, i64 0", c.value));
                    m.void_inst(&format!("store i64 {new}, ptr {viol}"));
                    let reached = m.inst(&format!("icmp sge i64 {new}, {d}"));
                    m.inst(&format!("and i1 {}, {reached}", c.value)).to_string()
                }
            };
            // Die Flanke bildet die Runtime (5.6), je Stelle und Durchlauf:
            // Der Platz ist der erste der Stelle plus der Durchlauf.
            let (base, _) = ctx.state.counters.alert(span).ok_or(NotYet { what: "Alert-Stelle ohne Platz" })?;
            let at = m.inst(&format!("add i64 {pass}, {base}"));
            let slot = m.inst(&format!("trunc i64 {at} to i32"));
            m.void_inst(&format!(
                "call void @{}(ptr %arena, i32 {machine}, i32 {slot}, i1 {active}, i1 {invalid})",
                m.runtime(Abi::ALERT)
            ));
            Ok(())
        }
        Observe::Log(_) => {
            let site = ctx.next_site();
            m.void_inst(&format!("call void @{}(ptr %arena, i32 {machine}, i32 {site})", m.runtime(Abi::LOG)));
            Ok(())
        }
        Observe::Measure { value, .. } => {
            // Der Report rechnet in `double`, unabhaengig von der Breite
            // des Programms (13.2): Ein Messwert ist eine Zahl fuer
            // Menschen, keine, mit der weitergerechnet wird.
            let (v, invalid) = observed(value, "0.0", ctx, m, |v, m| {
                let value = match v.ty {
                    LlvmType::F64 => v.value,
                    LlvmType::F32 => m.inst(&format!("fpext float {} to double", v.value)).to_string(),
                    LlvmType::Int(_) => m.inst(&format!("sitofp {} {} to double", v.ty, v.value)).to_string(),
                    _ => return Err(NotYet { what: "`measure` auf diesem Typ" }),
                };
                Ok(Lowered { value, ty: LlvmType::F64 })
            })?;
            let site = ctx.next_site();
            m.void_inst(&format!(
                "call void @{}(ptr %arena, i32 {machine}, i32 {site}, double {}, i1 {invalid})",
                m.runtime(Abi::MEASURE),
                v.value
            ));
            Ok(())
        }
        Observe::Verify { cond, .. } => {
            // 3.5: Ein ungueltiger Wert zaehlt als Verletzung.
            let (c, _) = observed(cond, "false", ctx, m, |v, _| Ok(v))?;
            let site = ctx.next_site();
            m.void_inst(&format!(
                "call void @{}(ptr %arena, i32 {machine}, i32 {site}, i1 {})",
                m.runtime(Abi::VERIFY),
                c.value
            ));
            Ok(())
        }
        // `verdict pass | fail` (13.2): das Urteil eines Tests. Wie
        // `verify` eine reine Beobachtung — sie loest nie einen Fault aus
        // (Leitentscheidung 14), also nur ein Aufruf.
        Observe::Verdict { pass, .. } => {
            let site = ctx.next_site();
            let v = u8::from(*pass);
            m.void_inst(&format!(
                "call void @{}(ptr %arena, i32 {machine}, i32 {site}, i1 {v})",
                m.runtime(Abi::VERDICT)
            ));
            Ok(())
        }
    }
}

/// Der Ausdruck einer Beobachtung (3.5, 5.6): Eine Beobachtung faultet nie.
///
/// Jede Pruefung darin springt an eine eigene Marke statt in den Fault-Pfad
/// der Maschine, wie der Interpreter den Fault dort abfaengt; dann gilt
/// `fallback`. `finish` formt den gueltigen Wert, bevor die Pfade
/// zusammenlaufen. Das zweite Ergebnis ist das Flag „ungueltig“.
fn observed(
    e: &Expr,
    fallback: &str,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
    finish: impl FnOnce(Lowered, &mut Module) -> Result<Lowered, NotYet>,
) -> Result<(Lowered, String), NotYet> {
    let n = ctx.next_label(m);
    let name = ctx.machine.name.clone();
    let invalid_at = format!("beob{n}_ungueltig_{name}");
    let done_at = format!("beob{n}_{name}");
    let mut vars = ctx.vars();
    vars.fault = Some(invalid_at.clone());
    let v = lower_expr(e, ctx.program, m, &vars)?;
    let v = finish(v, m)?;
    let valid_from = m.block().to_string();
    m.void_inst(&format!("br label %{done_at}"));
    m.label(&invalid_at);
    m.label(&done_at);
    let value = m.inst(&format!("phi {} [ {}, %{valid_from} ], [ {fallback}, %{invalid_at} ]", v.ty, v.value));
    let invalid = m.inst(&format!("phi i1 [ false, %{valid_from} ], [ true, %{invalid_at} ]"));
    Ok((Lowered { value: value.to_string(), ty: v.ty }, invalid.to_string()))
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

/// Die Indexpruefung einer Zuweisungsstelle (4.1): nur unter einem
/// `Checked{Index}`-Knoten; die Grenze ist die Laenge, bei Arrays die Zahl.
fn index_guard(
    index: &Expr,
    i: &Lowered,
    ty: &LlvmType,
    ptr: &Reg,
    vars: &dyn Vars,
    m: &mut Module,
) -> Result<(), NotYet> {
    if !matches!(index.kind, ExprKind::Checked { kind: takt_mir::expr::CheckedKind::Index { .. }, .. }) {
        return Ok(());
    }
    let bound = match ty {
        LlvmType::Array(_, n) => n.to_string(),
        LlvmType::Struct(_) => {
            let at = m.inst(&format!("getelementptr inbounds {ty}, ptr {ptr}, i32 0, i32 0"));
            let n = m.inst(&format!("load i32, ptr {at}"));
            m.inst(&format!("sext i32 {n} to {}", i.ty)).to_string()
        }
        _ => return Err(NotYet { what: "Index auf diesem Typ" }),
    };
    let target = vars
        .fault_to(takt_mir::machine::FaultKind::Range, m)
        .ok_or(NotYet { what: "Indexpruefung ohne Fault-Pfad" })?;
    let ok = m.inst(&format!("icmp ult {} {}, {bound}", i.ty, i.value));
    let go_on = format!("index_ok{}", m.next_label());
    m.void_inst(&format!("br i1 {ok}, label %{go_on}, label %{target}"));
    m.label(&go_on);
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
            let ty = ty::storage(def.ty, ctx.program).ok_or(NotYet { what: "Variablentyp" })?;
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
            let vars = ctx.vars();
            let i = lower_expr(index, ctx.program, m, &vars)?;
            index_guard(index, &i, &ty, &ptr, &vars, m)?;
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
            let src_ptr = match src.kind {
                ExprKind::Var(id) => place(&Place::Var(id), ctx, m)?.0,
                _ => {
                    let vars = ctx.vars();
                    let want = ty::lower(src.ty, ctx.program).ok_or(NotYet { what: "Quelle von `append`" })?;
                    let tmp = m.alloca(&want);
                    crate::expr::store(src, &tmp.to_string(), None, ctx.program, m, &vars)?;
                    tmp
                }
            };
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
        StmtKind::ForEach { vars, iter, body } => fn_for_each(vars, iter, body, ctx, m),
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

/// `i.step(...)`, `i.reset()` oder eine weitere Methode einer Blockinstanz
/// (5.7).
///
/// Eine Instanz hat je Aktivierung genau ein Ergebnis: Der erste `step`
/// schreitet und legt Wert und Fault-Art in der Instanz ab, jeder weitere
/// liefert sie, ohne zu schreiten (FB-423), wie `exec` im Interpreter. Ein
/// Fault der Methode steht danach im Flag der Arena und nimmt den
/// Fault-Pfad der Maschine (4.1, FB-424).
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
    let symbol = crate::block::method_symbol(def, &f.name);
    ops.push(format!("ptr {}", crate::arena::PARAM));
    let call = format!("call {ret} @{symbol}({})", ops.join(", "));
    if method != Method::Step {
        let value = if ret == LlvmType::Void {
            m.void_inst(&call);
            None
        } else {
            Some(m.inst(&call))
        };
        crate::expr::propagate_fault(m, &vars)?;
        if let (Some(t), Some(v)) = (target, value) {
            let (dst, _) = place(t, ctx, m)?;
            m.write(&ret, &v.to_string(), &dst.to_string());
        }
        return Ok(());
    }
    let (result, fault) = inst.memo().ok_or(NotYet { what: "Ergebnis von `step`" })?;
    let struct_ty = inst.llvm();
    let field =
        |i: u32, m: &mut Module| m.inst(&format!("getelementptr inbounds {struct_ty}, ptr {ptr}, i32 0, i32 {i}"));
    let (flag, result_at, fault_at) = (field(inst.stepped(), m), field(result, m), field(fault, m));
    let flag_at = crate::arena::at(crate::arena::fault::FLAG, m);
    let done = m.inst(&format!("load i1, ptr {flag}"));
    let label = m.next_label();
    let (go_on, end_at) = (format!("step{label}"), format!("step{label}_ende"));
    m.void_inst(&format!("br i1 {done}, label %{end_at}, label %{go_on}"));
    m.label(&go_on);
    m.void_inst(&format!("store i1 true, ptr {flag}"));
    let value = m.inst(&call);
    m.write(&ret, &value.to_string(), &result_at.to_string());
    let code = m.inst(&format!("load i32, ptr {flag_at}"));
    m.void_inst(&format!("store i32 {code}, ptr {fault_at}"));
    m.void_inst(&format!("br label %{end_at}"));
    // Der erste wie jeder weitere Aufruf liest das Ergebnis der Aktivierung.
    m.label(&end_at);
    let code = m.inst(&format!("load i32, ptr {fault_at}"));
    m.void_inst(&format!("store i32 {code}, ptr {flag_at}"));
    crate::expr::propagate_fault(m, &vars)?;
    if let Some(t) = target {
        let (dst, _) = place(t, ctx, m)?;
        m.copy(&ret, &result_at.to_string(), &dst.to_string());
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
    let n = ctx.next_label(m);
    let name = ctx.machine.name.clone();
    let end_at = format!("match{n}_{name}");
    // Verglichen wird, was die Variante unterscheidet: bei `T!E` das Flag
    // (3.8: Wert, Fehler, Flag), bei einem Summentyp mit Feldern die
    // Diskriminante im Feld 0. Ein feldloses Enum *ist* seine Diskriminante.
    let disc = match (&value.ty, ctx.program.types.list.get(subject.ty.index())) {
        (LlvmType::Struct(_), Some(takt_mir::types::Type::Result { .. })) => {
            let d = m.inst(&format!("extractvalue {} {}, 2", value.ty, value.value));
            Lowered { value: d.to_string(), ty: LlvmType::Int(1) }
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
                let d = tag_of(subject.ty, *variant, ctx.program).ok_or(NotYet { what: "Variante des `match`" })?;
                let ok = m.inst(&format!("icmp eq {} {}, {d}", disc.ty, disc.value));
                m.void_inst(&format!("br i1 {ok}, label %{hit}, label %{go_on}"));
                m.label(&hit);
                // 6.1: Die Felder der Variante werden an gehobene
                // Variablen gebunden, bevor der Rumpf laeuft. Sie liegen
                // im Wert hinter der Diskriminante.
                bind_fields(&value, fields, subject.ty, *variant, ctx, m)?;
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
                        // `case a..b`: einschliesslich beider Grenzen,
                        // verglichen nach der Vorzeichenart des Subjekts.
                        Some(hi) => {
                            let h = lower_expr(hi, ctx.program, m, &vars)?;
                            let (ge, le) = if crate::expr::int_is_signed(subject.ty, ctx.program) {
                                ("sge", "sle")
                            } else {
                                ("uge", "ule")
                            };
                            let a = m.inst(&format!("icmp {ge} {} {}, {}", disc.ty, disc.value, lo.value));
                            let b = m.inst(&format!("icmp {le} {} {}, {}", disc.ty, disc.value, h.value));
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

/// Woran eine Variante des `match`-Subjekts zu erkennen ist: bei einem
/// Enum die Diskriminante aus der MIR, bei `T!E` das Flag (`OK` ist die
/// Variante 0, `ERR` die Variante 1, 3.8).
fn tag_of(ty: takt_mir::TypeId, variant: u32, p: &Program) -> Option<String> {
    match p.types.list.get(ty.index())? {
        takt_mir::types::Type::Enum(e) => {
            Some(p.enums.get(e.index())?.variants.get(variant as usize)?.discriminant.to_string())
        }
        takt_mir::types::Type::Result { .. } => Some((variant == 0).to_string()),
        _ => None,
    }
}

/// Bindet die Felder einer Variante an ihre gehobenen Variablen (6.1).
///
/// Die Felder stehen im Wert hinter der Diskriminante; bei einem `T!E`
/// ist das Feld 0 der Wert und Feld 1 der Fehler (3.8). Mehr als ein Feld
/// braucht den Aufbau der Variante im Wert, den erst die Summentypen mit
/// Feldern mitbringen.
fn bind_fields(
    value: &Lowered,
    fields: &[takt_mir::VarId],
    subject: takt_mir::TypeId,
    variant: u32,
    ctx: &mut Ctx<'_>,
    m: &mut Module,
) -> Result<(), NotYet> {
    if fields.is_empty() {
        return Ok(());
    }
    let LlvmType::Struct(parts) = &value.ty else { return Err(NotYet { what: "Variante ohne Felder im Wert" }) };
    // Ein Enum mit Feldern: die Faecher hinter der Diskriminante, je Feld
    // eines (11.2).
    if let (takt_mir::types::Type::Enum(_), Some(arr @ LlvmType::Array(..))) =
        (ctx.program.types.get(subject), parts.get(1))
    {
        let arr = arr.clone();
        let payload = m.inst(&format!("extractvalue {} {}, 1", value.ty, value.value));
        for (k, var) in fields.iter().enumerate() {
            let def = ctx.machine.vars.get(var.index()).ok_or(NotYet { what: "Bindung" })?;
            let want = ty::storage(def.ty, ctx.program).ok_or(NotYet { what: "Typ der Bindung" })?;
            let slot = m.inst(&format!("extractvalue {arr} {payload}, {k}"));
            let v = crate::expr::from_slot(&slot.to_string(), &want, m);
            let ptr = ctx.field(Role::Var, var.index(), m).ok_or(NotYet { what: "Bindung im Zustand" })?;
            m.void_inst(&format!("store {want} {v}, ptr {ptr}"));
        }
        return Ok(());
    }
    if fields.len() > 1 {
        return Err(NotYet { what: "`case` mit mehreren Feldbindungen" });
    }
    // Bei `T!E` traegt Feld 0 den Wert von `OK` und Feld 1 den Fehler von
    // `ERR` — die Variante sagt, welches; der Typ nicht, wenn beide gleich
    // breit sind.
    let var = fields[0];
    let def = ctx.machine.vars.get(var.index()).ok_or(NotYet { what: "Bindung" })?;
    let want = ty::lower(def.ty, ctx.program).ok_or(NotYet { what: "Typ der Bindung" })?;
    if parts.get(variant as usize) != Some(&want) {
        return Err(NotYet { what: "Feld der Variante" });
    }
    let v = m.inst(&format!("extractvalue {} {}, {variant}", value.ty, value.value));
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
            index_guard(index, &i, &ty, &ptr, &ctx.vars, m)?;
            // Bei einer Sammlung liegt das Element im `data`-Feld (3.9),
            // bei einem Array unmittelbar.
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
