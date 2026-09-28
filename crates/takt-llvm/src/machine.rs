//! Maschinen nach LLVM-IR: Zustands-Struct und Tickschritt (11.2).
//!
//! 11.2 gibt die Form woertlich vor:
//!
//! ```text
//! struct hotfire_state { conf: [u8; DEPTH], t_in_state: [u64; DEPTH], vars…,
//!                        state_vars…, blocks…, every_next…, cur: [u64; STREAMS],
//!                        pending: Option<Fault>, last_fault: Fault, pc: u32 }
//! fn hotfire_step(s: &mut hotfire_state, i: &Inputs, psi: &Published, o: &mut OutLatch)
//! ```
//!
//! **Die Konfiguration ist ein Pfad-Array fester Tiefe**, kein Zeiger und
//! keine Rekursion: `conf[d]` ist der aktive Zustand auf Ebene `d`. Damit
//! ist der Zustandsraum statisch bekannt — die Bedingung dafuer, dass
//! `takt size` (11.5) rechnen kann und dass kein Heap noetig ist (13.4,
//! Power of Ten).
//!
//! **Der Fault-Trampolin ist ein eigener Block je Maschine.** Jeder
//! `Checked`-Knoten springt dorthin (4.1); 11.2 verlangt, dass die
//! Fault-Pfade `cold` sind und keine Spekulation ueber sie hinweg
//! stattfindet.

use core::fmt::Write as _;

use takt_mir::machine::{Machine, State};
use takt_mir::program::Program;
use takt_mir::{MachineId, StateId};

use crate::emit::Module;
use crate::ty::{self, LlvmType};

/// Der Bauplan des Zustands-Structs einer Maschine (11.2).
///
/// Er entsteht einmal und ist danach die einzige Quelle fuer die Indizes
/// im erzeugten Code — wie der `wire_size`-Plan eines Records im Sema.
/// Zwei Stellen, die die Reihenfolge unabhaengig voneinander ausrechnen,
/// waeren zwei Gelegenheiten, sie verschieden auszurechnen.
#[derive(Clone, Debug, PartialEq)]
pub struct StateStruct {
    /// Die Felder in ihrer Reihenfolge im Struct.
    pub fields: Vec<Field>,
    /// Tiefe des Zustandsbaums; die Laenge von `conf` und `t_in_state`.
    pub depth: u32,
    /// Je Variable ihr Versatz im Overlay (11.2) — `None` fuer ein eigenes Feld.
    pub overlay: Vec<Option<u64>>,
    /// Wie viele Zaehler und Flanken jede Stelle hat (5.6, 5.8).
    pub counters: Counters,
}

/// Ein Feld des Zustands-Structs.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    /// Name, wie er im Kommentar der IR steht.
    pub name: String,
    /// Sein LLVM-Typ.
    pub ty: LlvmType,
    /// Wofuer es steht.
    pub role: Role,
    /// Die Nummer innerhalb der Rolle; bei einer Variablen ihre `VarId`.
    pub nth: usize,
}

/// Wofuer ein Feld steht (11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Role {
    /// `conf[d]`: aktiver Zustand je Ebene.
    Conf,
    /// `t_in_state[d]`: Zeit in diesem Zustand, in Ticks.
    TimeInState,
    /// Eine Variable der Maschine oder eines Zustands.
    Var,
    /// `every_next[c]`: naechster Tick, an dem `every` feuert.
    EveryNext,
    /// `viol[site]`: Bestaetigungszaehler eines `check … for d` oder
    /// `alert … for d` in Nanosekunden, je Durchlauf der umgebenden
    /// Schleifen (5.6).
    Viol,
    /// `cur[s]`: Cursor eines gelesenen Stroms (9.6).
    Cursor,
    /// `examined[s] + 1`: hinter dem hoechsten untersuchten Element (9.6).
    Examined,
    /// `dropped[s, m]`: was die Maschine im `idle` verworfen hat (5.10, 9.6).
    Dropped,
    /// Stand die Maschine am Ende des vorigen Ticks im `idle`? Fuer den
    /// Alert `StreamPaused` beim Verlassen (5.10).
    WasIdle,
    /// `last_fault`.
    LastFault,
    /// Der Abort-Latch (5.4, 9.3): gesetzt mit einem Abort-Pfad, geloest
    /// mit dem naechsten normalen Uebergang.
    AbortLatch,
    /// Eine Zustellung fuer den naechsten Aufruf des Schritts (5.4): die
    /// Art und die Bits aus `step::DELIVER_PHASE`, null ohne.
    Deliver,
    /// `pc`: die Zeile der letzten Anweisung, fuer die Instrumentierung.
    Pc,
    /// `saved[i]`: zuletzt aktives Blatt eines `resume`-Zustands (5.12);
    /// -1 vor dem ersten Austritt.
    Saved,
    /// `armed[i]` eines Triggers dieser Maschine (7.5).
    Armed,
    /// Der Cursor eines Triggers auf seinem Quellstrom (7.5). Er liegt
    /// im Zustand des Besitzers, gehoert aber dem Trigger: Der liest mit
    /// Ereignisrate und unabhaengig davon, was die Maschine untersucht.
    TriggerCursor,
    /// Der Speicher der zustandslokalen und gehobenen Variablen:
    /// Geschwister teilen ihn (11.2, Overlay).
    Overlay,
}

/// Die Tiefe des Zustandsbaums einer Maschine.
///
/// Mindestens 1: Auch eine Maschine mit einer einzigen Ebene hat eine
/// Konfiguration.
pub fn depth(m: &Machine) -> u32 {
    fn from(m: &Machine, id: StateId, seen: u32) -> u32 {
        let s = &m.states[id.index()];
        s.children.iter().map(|c| from(m, *c, seen + 1)).max().unwrap_or(seen)
    }
    m.roots.iter().map(|r| from(m, *r, 1)).max().unwrap_or(1)
}

/// Baut den Zustands-Struct einer Maschine (11.2).
///
/// `None` heisst: Eine Variable hat einen Typ, den der Codegen noch nicht
/// abbildet. Die Auskunft ist ehrlicher als ein Struct, in dem ein Feld
/// fehlt — der Versatz aller folgenden waere falsch, und niemand saehe es.
pub fn state_struct(m: &Machine, p: &Program) -> Option<StateStruct> {
    let d = depth(m);
    let counters = counters(m, p);
    let field = |name: String, ty: LlvmType, role: Role, nth: usize| Field { name, ty, role, nth };
    let mut fields = Vec::new();
    // 11.2: `conf: [u8; DEPTH]`. Ein `u8` je Ebene reicht, solange eine
    // Ebene nicht mehr als 256 Geschwister hat; darueber waere die
    // Maschine ohnehin nicht mehr lesbar (Prinzip: Struktur sichtbar).
    fields.push(field("conf".into(), LlvmType::Array(Box::new(LlvmType::Int(8)), d), Role::Conf, 0));
    // 11.2, 5.2: `t_in_state` je Zustand — `after` an einem Vorfahren
    // misst dessen Eintritt, nicht den des Blatts (FB-208). Dahinter der
    // Zaehler des Blatts fuer `time_in_state`.
    let timers_ty = LlvmType::Array(Box::new(LlvmType::Int(64)), timers(m) as u32);
    fields.push(field("t_in_state".into(), timers_ty, Role::TimeInState, 0));
    // Eine Blockinstanz traegt ihren Zustand im Struct der Maschine
    // (5.7); ihr Typ steht nicht im Typsystem, sondern in
    // `Layout::block_instances`.
    let mut tys = Vec::with_capacity(m.vars.len());
    for (i, v) in m.vars.iter().enumerate() {
        tys.push(match instance_block(m, takt_mir::VarId(i as u32)) {
            Some(b) => crate::block::instance_of(p.blocks.get(b.index())?, p)?.llvm(),
            None => ty::storage(v.ty, p)?,
        });
    }
    // 11.2: Zustandslokale und gehobene Variablen von Geschwistern teilen
    // sich den Platz, rekursiv entlang des Baums — Bedarf eines Zustands
    // sind seine Variablen plus das Maximum seiner Kinder.
    let mut overlay = vec![None; m.vars.len()];
    let region = place(m, &tys, &m.roots, 0, &mut overlay);
    for (i, v) in m.vars.iter().enumerate() {
        if overlay[i].is_none() {
            fields.push(field(format!("var{i}_{}", v.name), tys[i].clone(), Role::Var, i));
        }
    }
    if region > 0 {
        let words = u32::try_from(region.div_ceil(8)).ok()?;
        fields.push(field("overlay".into(), LlvmType::Array(Box::new(LlvmType::Int(64)), words), Role::Overlay, 0));
    }
    // 5.6, 5.8: je Stelle ein Zaehler je Durchlauf der umgebenden Schleifen.
    let per_pass = |n: Option<u32>| LlvmType::Array(Box::new(LlvmType::Int(64)), n.unwrap_or(1));
    for (i, n) in counters.every.iter().enumerate() {
        fields.push(field(format!("every_next{i}"), per_pass(*n), Role::EveryNext, i));
    }
    for (i, n) in counters.viol.iter().enumerate() {
        fields.push(field(format!("viol{i}"), per_pass(*n), Role::Viol, i));
    }
    for (i, _) in m.layout.cursors.iter().enumerate() {
        fields.push(field(format!("cur{i}"), LlvmType::Int(64), Role::Cursor, i));
    }
    for (i, _) in m.layout.cursors.iter().enumerate() {
        fields.push(field(format!("examined{i}"), LlvmType::Int(64), Role::Examined, i));
    }
    for (i, _) in m.layout.cursors.iter().enumerate() {
        fields.push(field(format!("dropped{i}"), LlvmType::Int(32), Role::Dropped, i));
    }
    if crate::step::drops(m, p) {
        fields.push(field("was_idle".into(), LlvmType::Int(1), Role::WasIdle, 0));
    }
    for (i, _) in m.layout.trigger_flags.iter().enumerate() {
        fields.push(field(format!("armed{i}"), LlvmType::Int(1), Role::Armed, i));
        fields.push(field(format!("trig_cur{i}"), LlvmType::Int(64), Role::TriggerCursor, i));
    }
    // 5.3: `last_fault` fuehrt nur, wer es liest (`crate::fault`). Das
    // `pending` aus 11.2 ist `deliver`: die Art samt Phase, null ohne.
    if takt_mir::visit::reads_last_fault(m) {
        fields.push(field("last_fault".into(), crate::fault::field_type(), Role::LastFault, 0));
    }
    fields.push(field("abort_latched".into(), LlvmType::Int(1), Role::AbortLatch, 0));
    fields.push(field("deliver".into(), LlvmType::Int(32), Role::Deliver, 0));
    fields.push(field("pc".into(), LlvmType::Int(32), Role::Pc, 0));
    for (i, _) in m.layout.saved_paths.iter().enumerate() {
        fields.push(field(format!("saved{i}"), LlvmType::Int(32), Role::Saved, i));
    }
    // Hinter `conf` und `t_in_state` (11.2): Skalare vor den grossen
    // Puffern, damit ihre Versaetze klein bleiben (RISC-V: 12 Bit),
    // darin absteigend nach Ausrichtung ohne Fuellbytes.
    fields[2..].sort_by_key(|f| (f.ty.aligned_size() >= 256, std::cmp::Reverse(f.ty.align())));
    Some(StateStruct { fields, depth: d, overlay, counters })
}

/// Die Zaehler einer Maschine je Stelle (5.6, 5.8).
///
/// Steht eine Stelle in `for`-Schleifen, hat sie einen Zaehler je
/// Durchlauf — Schluessel ist die Stelle samt den Indizes der umgebenden
/// Schleifen —, und ihre Zahl ist das Produkt der Schleifenschranken, die
/// statisch sind (4.3). `None` heisst: Eine Schranke kennt der Codegen
/// nicht, und die Stelle wird nicht gesenkt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Je Bestaetigungsstelle (`check … for`, `alert … for`).
    pub viol: Vec<Option<u32>>,
    /// Je `every`.
    pub every: Vec<Option<u32>>,
    /// Je Alert-Stelle ihre Position, ihr erster Platz in der
    /// Flankentabelle der Runtime und die Zahl ihrer Plaetze. Dazu kommen
    /// die Alerts der Runtime: ein `send`, das bei vollem Puffer verwirft
    /// (8.6, 8.8), `StreamPaused` (5.10) und `PersistReset` je
    /// `persist`-Variable (5.9) — je einer, ohne Schleifenindex, wie im
    /// Interpreter.
    pub alerts: Vec<(takt_diag::Span, u32, Option<u32>)>,
}

impl Counters {
    /// Wie viele Plaetze die Flankentabelle der Runtime fuer die Maschine
    /// braucht (5.6: „die Runtime protokolliert Flanken").
    pub fn alert_slots(&self) -> u32 {
        self.alerts.iter().map(|(_, base, n)| base + n.unwrap_or(0)).max().unwrap_or(0)
    }

    /// Haengt eine Alert-Stelle an, wenn es sie noch nicht gibt.
    fn push_alert(&mut self, span: takt_diag::Span, n: Option<u32>) {
        if self.alert(span).is_none() {
            let base = self.alert_slots();
            self.alerts.push((span, base, n));
        }
    }

    /// Erster Platz und Zahl der Plaetze einer Alert-Stelle.
    pub fn alert(&self, span: takt_diag::Span) -> Option<(u32, Option<u32>)> {
        self.alerts.iter().find(|(s, ..)| *s == span).map(|(_, base, n)| (*base, *n))
    }
}

/// Zaehlt die Zaehler einer Maschine je Stelle, in der Reihenfolge ihrer
/// Bloecke (`Machine::blocks`).
pub fn counters(m: &Machine, p: &Program) -> Counters {
    let mut c = Counters {
        viol: vec![Some(1); m.layout.viol_sites.len()],
        every: vec![Some(1); m.layout.every_counters.len()],
        alerts: Vec::new(),
    };
    for b in m.blocks() {
        count_in(b, Some(1), p, &mut c);
    }
    if crate::step::drops(m, p) {
        c.push_alert(paused_span(m), Some(1));
    }
    // 5.9: `PersistReset` je Variable, unter ihrer Position.
    for pv in &m.persist {
        if let Some(v) = m.vars.get(pv.var.index()) {
            c.push_alert(v.span, Some(1));
        }
    }
    c
}

/// Die Position, unter der der Interpreter `StreamPaused` meldet: die des
/// ersten Zustands.
pub fn paused_span(m: &Machine) -> takt_diag::Span {
    m.states.first().map(|s| s.span).unwrap_or_default()
}

/// Verwirft ein `send` bei vollem Puffer, statt zu faulten (8.6, 8.8)?
pub fn drops_when_full(stream: takt_mir::expr::StreamRef, p: &Program) -> bool {
    use takt_mir::program::Overflow;
    match stream {
        takt_mir::expr::StreamRef::Internal(s) => {
            p.streams.get(s.index()).is_some_and(|d| matches!(d.overflow, Overflow::Drop))
        }
        takt_mir::expr::StreamRef::Channel(c) => {
            p.channels.get(c.index()).is_some_and(|c| matches!(c.attrs.overflow, Some(Overflow::Drop)))
        }
        _ => false,
    }
}

fn count_in(b: &takt_mir::stmt::Block, n: Option<u32>, p: &Program, c: &mut Counters) {
    use takt_mir::stmt::{Observe, StmtKind};
    let set = |v: &mut Vec<Option<u32>>, i: usize| {
        if let Some(x) = v.get_mut(i) {
            *x = n;
        }
    };
    let times = |bound: Option<u32>| n.zip(bound).and_then(|(a, b)| a.checked_mul(b));
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Check { confirm: Some(k), .. } => set(&mut c.viol, k.site.index()),
            StmtKind::Observe(Observe::Alert { confirm, .. }) => {
                if let Some(k) = confirm {
                    set(&mut c.viol, k.site.index());
                }
                c.push_alert(s.span, n);
            }
            StmtKind::Send { stream, .. } if drops_when_full(*stream, p) => c.push_alert(s.span, Some(1)),
            StmtKind::Every { counter, body, .. } => {
                set(&mut c.every, counter.index());
                count_in(body, n, p, c);
            }
            StmtKind::ForRange { count, body, .. } => count_in(body, times(range_bound(count)), p, c),
            StmtKind::ForEach { iter, body, .. } => count_in(body, times(each_bound(iter, p)), p, c),
            StmtKind::If { then, otherwise, .. } => {
                count_in(then, n, p, c);
                count_in(otherwise, n, p, c);
            }
            StmtKind::Match { arms, .. } => arms.iter().for_each(|a| count_in(&a.body, n, p, c)),
            StmtKind::At { body, .. } => count_in(body, n, p, c),
            _ => {}
        }
    }
}

/// Die Schranke von `for i in range(n)`: `n` ist nach dem Lowering eine
/// Konstante (4.3).
pub fn range_bound(count: &takt_mir::expr::Expr) -> Option<u32> {
    match count.kind {
        takt_mir::expr::ExprKind::Int(n) => u32::try_from(n.max(0)).ok(),
        _ => None,
    }
}

/// Die Schranke von `for x in s` ueber ein Fenster: die Kapazitaet des
/// Stroms (Lemma 9.6.1).
pub fn window_bound(stream: takt_mir::expr::StreamRef, p: &Program) -> Option<u32> {
    match stream {
        takt_mir::expr::StreamRef::Channel(c) => p.channels.get(c.index())?.attrs.capacity,
        takt_mir::expr::StreamRef::Internal(s) => Some(p.streams.get(s.index())?.capacity),
        _ => None,
    }
}

/// Die Schranke von `for x in a`: die Kapazitaet eines Fensters, die
/// Laenge eines Arrays oder die einer Sammlung, wie sie
/// `stmt::for_each` durchlaeuft.
pub fn each_bound(iter: &takt_mir::expr::Expr, p: &Program) -> Option<u32> {
    use takt_mir::expr::{ExprKind, StreamRef};
    match &iter.kind {
        ExprKind::Input { channel, .. } => window_bound(StreamRef::Channel(*channel), p),
        ExprKind::Stream(s) => window_bound(StreamRef::Internal(*s), p),
        _ => match ty::lower(iter.ty, p)? {
            LlvmType::Array(_, n) => Some(n),
            LlvmType::Struct(f) if f.len() == 2 && f[0] == LlvmType::Int(32) => match &f[1] {
                LlvmType::Array(_, n) => Some(*n),
                _ => None,
            },
            _ => None,
        },
    }
}

/// Der Zustand, dessen Overlay eine Variable gehoert (11.2): zustandslokal
/// oder gehoben, nicht veroeffentlicht, keine Blockinstanz.
fn overlaid(m: &Machine, i: usize) -> Option<StateId> {
    let v = m.vars.get(i)?;
    if v.public || instance_block(m, takt_mir::VarId(i as u32)).is_some() {
        return None;
    }
    match v.scope {
        takt_mir::machine::VarScope::State(s) | takt_mir::machine::VarScope::Lifted(s) => Some(s),
        _ => None,
    }
}

/// Legt die Variablen der Geschwister ab `base` an dieselbe Stelle, die
/// Kinder dahinter; das Ergebnis ist das Ende des groessten Bedarfs.
fn place(m: &Machine, tys: &[LlvmType], siblings: &[StateId], base: u64, overlay: &mut [Option<u64>]) -> u64 {
    let mut end = base;
    for id in siblings {
        let mut at = base;
        for (i, ty) in tys.iter().enumerate() {
            if overlaid(m, i) != Some(*id) {
                continue;
            }
            at = at.div_ceil(ty.align()) * ty.align();
            overlay[i] = Some(at);
            at += ty.aligned_size();
        }
        end = end.max(place(m, tys, &m.states[id.index()].children, at.div_ceil(8) * 8, overlay));
    }
    end
}

impl StateStruct {
    /// Der Struct als LLVM-Typ.
    pub fn llvm(&self) -> LlvmType {
        LlvmType::Struct(self.fields.iter().map(|f| f.ty.clone()).collect())
    }

    /// Der Index eines Felds, fuer `getelementptr`.
    pub fn index_of(&self, role: Role, nth: usize) -> Option<u32> {
        self.fields.iter().position(|f| f.role == role && f.nth == nth).and_then(|i| u32::try_from(i).ok())
    }

    /// Der Zeiger auf ein Feld des Zustands `%0`; eine ueberlagerte
    /// Variable liegt im Overlay (11.2). Die einzige Stelle, an der ein
    /// Feldindex in Code wird.
    pub fn field_ptr(&self, machine: &str, role: Role, nth: usize, m: &mut Module) -> Option<crate::emit::Reg> {
        let ty = format!("%{}_state", crate::fns::sanitized(machine));
        if role == Role::Var
            && let Some(off) = self.overlay.get(nth).copied().flatten()
        {
            let region = self.index_of(Role::Overlay, 0)?;
            let base = m.inst(&format!("getelementptr inbounds {ty}, ptr %0, i32 0, i32 {region}"));
            return Some(m.inst(&format!("getelementptr inbounds i8, ptr {base}, i64 {off}")));
        }
        let i = self.index_of(role, nth)?;
        Some(m.inst(&format!("getelementptr inbounds {ty}, ptr %0, i32 0, i32 {i}")))
    }

    /// Der Byteversatz eines Felds, wie das Datenlayout es legt — fuer
    /// Rahmen, die den Zustand als Bytepuffer halten (12.1).
    pub fn byte_offset(&self, role: Role, nth: usize) -> Option<u64> {
        let index = self.index_of(role, nth)? as usize;
        let mut at = 0u64;
        for f in &self.fields[..index] {
            at = at.div_ceil(f.ty.align()) * f.ty.align() + f.ty.aligned_size();
        }
        let align = self.fields[index].ty.align();
        Some(at.div_ceil(align) * align)
    }

    /// Die Groesse in Bytes, ohne Ausrichtung (11.5 rechnet genauer).
    pub fn size(&self) -> u64 {
        self.fields.iter().map(|f| f.ty.size()).sum()
    }

    /// Die Groesse mit Ausrichtung: so viel belegt der Struct wirklich,
    /// und so viel muss ein Rahmen reservieren (FB-177).
    pub fn aligned_size(&self) -> u64 {
        self.llvm().aligned_size()
    }
}

/// Schreibt die Typdefinition des Zustands-Structs als benannten Typ.
///
/// Ein benannter Typ (`%maschine_state = type { … }`) statt eines
/// anonymen: Die IR bleibt lesbar, und das ist nach 13.4 kein
/// Schoenheitsargument — sie ist das Artefakt der Qualifikation.
pub fn declare_state(m: &Machine, st: &StateStruct, module: &mut Module) {
    let mut text = String::new();
    let _ = write!(text, "\n; Zustand der Maschine `{}` (11.2)", m.name);
    for f in &st.fields {
        let _ = write!(text, "\n;   {} : {}", f.name, f.ty);
    }
    let _ = write!(text, "\n%{}_state = type {}", m.name, st.llvm());
    module.declare(&text);
}

/// Der Name der Schrittfunktion einer Maschine (11.2: `hotfire_step`).
pub fn step_name(m: &Machine) -> String {
    format!("{}_step", crate::fns::sanitized(&m.name))
}

/// Beginnt die Schrittfunktion einer Maschine.
///
/// Die Signatur folgt 11.2: Zustand, Inputs, Ψ und der Output-Latch, alle
/// als Zeiger. Der Latch wird von der Runtime committet (12.1), nicht von
/// der Maschine — darum ist er ein Ausgabeparameter und kein Rueckgabewert.
pub fn begin_step(m: &Machine, module: &mut Module) -> Vec<crate::emit::Reg> {
    let ptr = LlvmType::Ptr;
    // `noalias` auf Zustand, Parametern und Latch: Ohne die Zusage
    // entwertet jeder Latch-Store alle Ladungen aus dem Zustand. Das
    // Abbild nicht — `takt_job_begin` schreibt es ueber die Runtime.
    module.begin_with(
        "",
        &step_name(m),
        &LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr],
        MACHINE_ATTRS,
        "",
    )
}

/// Die Parameterattribute der Maschinenfunktionen `(st, in, par, out)`.
pub const MACHINE_ATTRS: &[&str] = &["noalias", "", "noalias", "noalias"];

/// Der Name eines Zustands, wie er als Marke in der IR erscheint.
pub fn label_of(m: &Machine, id: StateId) -> String {
    format!("{}_{}", crate::fns::sanitized(&m.name), crate::fns::sanitized(&m.states[id.index()].name))
}

/// Die Blattzustaende einer Maschine in der Reihenfolge ihrer Ids.
///
/// 11.2: „Blattzustand = Enum-Diskriminante; `switch` ueber die Blaetter."
/// Nur Blaetter, weil ein zusammengesetzter Zustand nie allein aktiv ist —
/// aktiv ist immer ein vollstaendiger Pfad bis zu einem Blatt.
pub fn leaves(m: &Machine) -> Vec<StateId> {
    m.states.iter().enumerate().filter(|(_, s)| s.children.is_empty()).map(|(i, _)| StateId(i as u32)).collect()
}

/// Der Pfad von der Wurzel zu einem Zustand (11.2: `conf`).
pub fn path_to(m: &Machine, id: StateId) -> Vec<StateId> {
    let mut out = Vec::new();
    let mut cur = Some(id);
    while let Some(s) = cur {
        out.push(s);
        cur = m.states[s.index()].parent;
    }
    out.reverse();
    out
}

/// Die Zaehler `t_in_state`: einer je Zustand und einer fuer das Blatt.
pub fn timers(m: &Machine) -> usize {
    m.states.len() + 1
}

/// Der Zaehler `t_in_state[i]` im Zustands-Struct.
pub fn timer_cell(m: &Machine, st: &StateStruct, i: usize, module: &mut Module) -> Option<crate::emit::Reg> {
    let t_i = st.index_of(Role::TimeInState, 0)?;
    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let base = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {t_i}"));
    Some(module.inst(&format!("getelementptr inbounds [{} x i64], ptr {base}, i32 0, i32 {i}", timers(m))))
}

/// Zaehlt alle `t_in_state` um eine Aktivierung weiter (9.4).
///
/// Auch die der inaktiven Zustaende: Sie werden beim Eintritt auf null
/// gesetzt und davor nie gelesen — so braucht das Schrittende das Blatt
/// nicht zu kennen.
pub fn advance_timers(m: &Machine, st: &StateStruct, module: &mut Module) {
    for i in 0..timers(m) {
        let Some(cell) = timer_cell(m, st, i, module) else { return };
        let now = module.inst(&format!("load i64, ptr {cell}"));
        let next = module.inst(&format!("add i64 {now}, 1"));
        module.void_inst(&format!("store i64 {next}, ptr {cell}"));
    }
}

/// Der Zustand zu einer Id.
pub fn state_of(m: &Machine, id: StateId) -> &State {
    &m.states[id.index()]
}

/// Die Maschine zu einer Id.
pub fn machine_of(p: &Program, id: MachineId) -> &Machine {
    &p.machines[id.index()]
}

/// Das Blatt, das beim Betreten eines Zustands aktiv wird (5.2).
///
/// Ein zusammengesetzter Zustand ist nie allein aktiv: Sein `initial`-Kind
/// wird mitbetreten, und dessen `initial`-Kind, bis ein Blatt erreicht
/// ist. `None` heisst, dass ein zusammengesetzter Zustand kein `initial`
/// hat — das faengt Pruefung 10 ab, aber der Codegen verlaesst sich nicht
/// darauf.
pub fn initial_leaf(m: &Machine, from: StateId) -> Option<StateId> {
    let mut cur = from;
    // Eine Vorlage hat keine Zustaende (5.8); `from` zeigt dann ins
    // Leere, und ein Index waere ein Absturz statt einer Meldung.
    if m.states.is_empty() {
        return None;
    }
    // Die Schranke ist die Zahl der Zustaende: Ein Zyklus im `initial`-Pfad
    // waere ein Fehler im Sema, aber eine Endlosschleife im Codegen waere
    // schlimmer als eine Meldung (4.1: beschraenkte Schleifen).
    for _ in 0..=m.states.len() {
        let s = &m.states[cur.index()];
        if s.children.is_empty() {
            return Some(cur);
        }
        cur = s.initial?;
    }
    None
}

/// Die Zustaende, die ein Wechsel verlaesst und betritt (9.3, `switch`):
/// verlassen innen nach aussen, betreten aussen nach innen.
///
/// Der kleinste gemeinsame Vorfahr liegt echt oberhalb des Zielzustands,
/// nicht nur oberhalb des Blatts, in dem das Betreten endet — wie im
/// Interpreter. Ein Uebergang auf einen Vorfahren verlaesst ihn also und
/// betritt ihn neu, ebenso einer auf sich selbst; zwischen zwei
/// Geschwistern bleibt der Elternzustand aktiv. `from` ist das verlassene
/// Blatt, `to` der Zielzustand mit dem Blatt darunter; `None` ist auf
/// beiden Seiten `FAULTED`, die leere Konfiguration (5.3).
pub fn crossing(m: &Machine, from: Option<StateId>, to: Option<(StateId, StateId)>) -> (Vec<StateId>, Vec<StateId>) {
    let old = from.map(|s| path_to(m, s)).unwrap_or_default();
    let new = to.map(|(_, leaf)| path_to(m, leaf)).unwrap_or_default();
    let mut common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    if let Some((goal, _)) = to {
        common = common.min(path_to(m, goal).len().saturating_sub(1));
    }
    (old[common..].iter().rev().copied().collect(), new[common..].to_vec())
}

/// Der Block, dessen Instanz in dieser Variablen steht (5.7).
///
/// Die Identitaet steht in `Layout::block_instances`, nicht im Typ: Das
/// Sema gibt jeder Instanz denselben Platzhaltertyp (FB-77).
pub fn instance_block(m: &Machine, var: takt_mir::VarId) -> Option<takt_mir::BlockId> {
    m.layout.block_instances.iter().find(|b| b.var == var).map(|b| b.block)
}
