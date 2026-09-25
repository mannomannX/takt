//! Typisiertes Kostenmodell (Referenz 9.4.3, Satz 9.4.3).
//!
//! „Die Kosten sind Vektoren ueber den Operationsklassen c ∈ {i32, i64, f32,
//! f64, mem, call, native}: `N(s) ∈ ℕ^7`, `cost(e)` zaehlt jede Operation in
//! ihrer Klasse (eine Integer-Operation nach ihrer gewaehlten Darstellung in
//! `i32` oder `i64` (3.4) …)."
//!
//! Deshalb laeuft `narrow` vor `cost`: Ohne die Darstellung waere jede
//! Integer-Operation `i64` und das Budget auf 32-Bit-Kernen zu pessimistisch.
//!
//! **Ein Aufruf kostet seinen Rumpf** (9.4.3: `N(f(args)) = Σ cost(args) +
//! N(body f)`), dazu die Operation `call` selbst. Das gilt fuer Funktionen,
//! fuer den `step` einer Blockinstanz und fuer ihre weiteren Methoden. Die
//! Rumpfkosten stehen danach in `Fn::cost`; gerechnet werden sie bis zum
//! Fixpunkt, weil der Aufrufgraph azyklisch ist (4.3) und eine Funktion erst
//! feststeht, wenn alle feststehen, die sie ruft.
//!
//! **Eine Aktivierung ist der Durchlauf einer Kette, nicht eines Zustands**
//! (9.3). `step_m` fuehrt die `loop:`-Bloecke und Handler aller Ebenen der
//! aktiven Kette aus, wertet die Waechter aller ihrer Uebergaenge aus und
//! nimmt hoechstens einen Wechsel: die Austritte der verlassenen Zustaende,
//! die Aktionen, dann Initialwerte, Eintritte und `loop:`-Bloecke der
//! betretenen im Modus ENTRY. `B_m` ist das Maximum ueber die Ketten und
//! `FAULTED`; `F_m` der teuerste Fault-Pfad ab einer Kette, entlang des
//! Fault-Waldes bis zu seinem Ende (Lemma 9.3.1).
//!
//! **Was der erzeugte Code selbst tut, zaehlt mit.** Die Gleichungen aus
//! 9.4.3 nennen fuer Muster, Senden, Dispatch und Tabellen die
//! Groessenordnung (`max_len`, `len_max`, `CAP`, `n`). Je Einheit — ein
//! Byte, eine Ziffer, ein Schleifendurchlauf — steht hier als fester
//! Vektor, was der Codegen dafuer ausfuehrt. Die Referenzkerne aus 13.8
//! pruefen die Vektoren auf dem Board: Liegt ein Kern ueber seiner
//! Schranke, ist hier ein Vektor zu klein.

use crate::Program;
use crate::TypeId;
use crate::expr::{Accessor, BinaryOp, CheckedKind, Expr, ExprKind, Intrinsic, MatOp, MatchKind, Repr, StreamRef};
use crate::fns::{BlockDef, CostClass, CostVec, Heavy};
use crate::ids::{FnId, StateId};
use crate::machine::{Budget, FaultTarget, Guard, Handler, Machine, Target, TransTrigger, VarDef};
use crate::pattern::{CaptureKind, Format, FormatPiece, Pattern, PatternPiece};
use crate::stmt::{Block, Method, Observe, Place, Stmt, StmtKind};
use crate::types::{Const, FloatWidth, HandleKind, RangeOrigin, Type};

/// Die Eins aus 9.4.3 (`check`, `->`, `for`, `at`, `every`, ein Waechter,
/// ein Wechsel): ein Vergleich und ein Sprung.
const STEP: CostVec = CostVec { i32: 1, ..CostVec::ZERO };

/// Je Aktivierung einer Maschine, bevor Nutzercode laeuft: `conf` laden
/// und ins Blatt verzweigen (11.2).
const DISPATCH: CostVec = CostVec { i32: 2, mem: 1, ..CostVec::ZERO };

/// Je Zaehler `t_in_state` und Aktivierung: laden, erhoehen, speichern.
/// Der erzeugte Code schreibt alle fort, einen je Zustand und einen fuer
/// das Blatt, nicht nur die der Kette (`advance_timers`).
const TIMER: CostVec = CostVec { i64: 1, mem: 1, ..CostVec::ZERO };

/// Ein `loop:`-Block und der Entry-Tick eines Wechsels laufen als eigene
/// Funktion (FB-224): der Aufruf und die Auswertung seines Ergebnisses.
const BLOCK_CALL: CostVec = CostVec { call: 1, i32: 1, ..CostVec::ZERO };

/// Ein Aufruf der Runtime: der Fault-Hook, der Abbruch eines Jobs (5.3).
const RUNTIME_CALL: CostVec = CostVec { call: 1, ..CostVec::ZERO };

/// Je Durchlauf einer Schleife: weiterzaehlen und vergleichen. 9.4.3
/// schreibt `1 + n·N(s)`; ohne diesen Anteil laege eine enge Schleife um
/// das Doppelte ueber ihrer Schranke.
const ITERATION: CostVec = CostVec { i32: 2, ..CostVec::ZERO };

/// Ein Byte, das kopiert wird: ein Element in den Scratch und in die
/// Bindung, ein Text in den Sendepuffer (`len_max` beim `send`), ein
/// Aggregat bei einer Zuweisung.
const BYTE: CostVec = CostVec { mem: 1, ..CostVec::ZERO };

/// Ein Byte eines Musterabgleichs (8.7). Der Automat laedt Zeichen,
/// Alphabetklasse und Tabelleneintrag und rechnet die Zeile
/// (`takt_llvm::dfa::run`); der Vorwaertsdurchlauf laedt und vergleicht
/// (`takt_llvm::captures::walk`). Der Vektor deckt beide.
const MATCH_BYTE: CostVec = CostVec { i32: 3, mem: 3, ..CostVec::ZERO };

/// Ein Zeichen eines Platzhalters beim Einlesen: noch einmal lesen und in
/// `i64` zusammenrechnen (`captures::parse_int`).
const CAPTURE_DIGIT: CostVec = CostVec { i32: 1, i64: 2, mem: 1, ..CostVec::ZERO };

/// Ein Zeichen Text in einen Formatpuffer (`takt_llvm::format::text`):
/// Rand pruefen, schreiben, weiterzaehlen.
const TEXT_BYTE: CostVec = CostVec { i32: 3, mem: 2, ..CostVec::ZERO };

/// Eine Dezimalziffer beim Formatieren (`takt_llvm::format::digits`): in
/// `i64` durch die Basis teilen, den Rest bilden, das Zeichen in den
/// Zwischenpuffer legen und es umgedreht in den Text schreiben.
const DECIMAL_DIGIT: CostVec = CostVec { i32: 6, i64: 3, i64_div: 1, mem: 3, ..CostVec::ZERO };

/// Eine Hexziffer: wie eine Dezimalziffer, aber die Basis 16 ist ein
/// Schieben, keine Division.
const HEX_DIGIT: CostVec = CostVec { i32: 6, i64: 3, mem: 3, ..CostVec::ZERO };

/// Ein Element aus dem Fenster holen (`takt_stream_at`, 9.6): der Aufruf,
/// der Zaehler der Schleife, der Vermerk `examined`. Die Bytes zaehlt
/// [`BYTE`] dazu.
const FETCH: CostVec = CostVec { i32: 2, mem: 1, call: 1, ..CostVec::ZERO };

/// Die Zeichen eines Platzhalters beim Einlesen hoechstens (8.7): `int`
/// mit Vorzeichen und 19 Ziffern, `hex` mit `0x` und 16, `word` 64.
const INT_CHARS: u64 = 20;
const HEX_CHARS: u64 = 18;
const WORD_CHARS: u64 = 64;

/// Die Zeichen eines Platzhalters beim Formatieren, wie `takt-sema` sie
/// fuer `len_max` ansetzt (`lower::format`): `bool` 5, `int` 20, `hex` 16.
const BOOL_WIDTH: u64 = 5;
const INT_WIDTH: u64 = 20;
const HEX_WIDTH: u64 = 16;

/// Rechnet die Rumpfkosten jeder Funktion in `Fn::cost`, dann je Maschine
/// `B_m` (Aktivierung) und `F_m` (Fault-Pfad) in `Machine::budget`.
pub fn budgets(program: &mut Program) {
    let fns = fn_costs(program);
    for (f, cost) in program.fns.iter_mut().zip(&fns) {
        f.cost = Some(*cost);
    }
    let table = longest_table(program);
    for i in 0..program.machines.len() {
        let budget = {
            let m = &program.machines[i];
            let tree = Tree::new(m, Ctx::of_machine(program, m, &fns, table));
            Budget { activation: tree.activation().total, fault_path: tree.fault_path() }
        };
        program.machines[i].budget = Some(budget);
    }
}

/// `N(body f)` je Funktion, in der Reihenfolge von `Program::fns`.
///
/// Bis zum Fixpunkt: Anfangs kostet jede Funktion null, und jede Runde
/// rechnet alle Ruempfe mit den Werten der vorigen. Im azyklischen
/// Aufrufgraphen steht nach so vielen Runden, wie die laengste Aufrufkette
/// lang ist, alles fest; mehr als eine Runde je Funktion braucht es nie,
/// und so viele sind die Schranke.
fn fn_costs(p: &Program) -> Vec<CostVec> {
    let table = longest_table(p);
    let mut costs = vec![CostVec::ZERO; p.fns.len()];
    for _ in 0..=p.fns.len() {
        let next: Vec<CostVec> = p
            .fns
            .iter()
            .map(|f| {
                let ctx = Ctx { vars: &f.locals, ..Ctx::of_program(p, &costs, table) };
                let locals = f
                    .locals
                    .iter()
                    .filter_map(|v| v.init.as_ref())
                    .fold(CostVec::ZERO, |acc, e| acc + expr_cost(e, &ctx) + ctx.copy(e.ty));
                locals + block_cost(&f.body, &ctx)
            })
            .collect();
        if next == costs {
            break;
        }
        costs = next;
    }
    costs
}

/// Was die Rechnung ueber das Programm und den Rahmen wissen muss.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    p: &'a Program,
    /// `N(body f)` je Funktion.
    fns: &'a [CostVec],
    /// Die Variablen des Rahmens — der Maschine oder der Funktion —, fuer
    /// den Block hinter einem `step` und den Strom hinter einer Variable.
    vars: &'a [VarDef],
    /// Die Stuetzstellen der laengsten Tabelle des Programms (3.9).
    table: u64,
}

impl<'a> Ctx<'a> {
    fn of_program(p: &'a Program, fns: &'a [CostVec], table: u64) -> Ctx<'a> {
        Ctx { p, fns, vars: &[], table }
    }

    fn of_machine(p: &'a Program, m: &'a Machine, fns: &'a [CostVec], table: u64) -> Ctx<'a> {
        Ctx { vars: &m.vars, ..Ctx::of_program(p, fns, table) }
    }

    fn types(&self) -> &'a [Type] {
        &self.p.types.list
    }

    fn ty(&self, t: TypeId) -> Option<&'a Type> {
        self.p.types.list.get(t.index())
    }

    /// Byte eines Werts, wie `takt size` sie zaehlt (11.5).
    fn bytes(&self, t: TypeId) -> u64 {
        u64::from(super::size::type_bytes(self.p, t))
    }

    /// Was eine Zuweisung ueber den Ausdruck hinaus kostet: Ein Aggregat
    /// wird byteweise kopiert, ein Skalar geht mit der Operation, die ihn
    /// rechnet — sein Gewicht traegt das Speichern schon (13.8).
    fn copy(&self, t: TypeId) -> CostVec {
        if self.ty(t).is_some_and(|ty| aggregate(ty, self.types())) { BYTE.times(self.bytes(t)) } else { CostVec::ZERO }
    }

    /// Was das Setzen eines Werts beim Eintritt kostet: wie [`Ctx::copy`],
    /// ein Skalar mindestens ein Speichern.
    fn store(&self, t: TypeId) -> CostVec {
        let c = self.copy(t);
        if c.is_zero() { BYTE } else { c }
    }

    fn native(&self, n: crate::ids::NativeId) -> CostVec {
        self.p.natives.get(n.index()).map(|n| n.cost).unwrap_or_default()
    }

    fn function(&self, f: FnId) -> CostVec {
        self.fns.get(f.index()).copied().unwrap_or_default()
    }

    /// Der Block hinter einem Empfaenger, aus dem Typ seiner Variable.
    fn block_of(&self, receiver: &Place) -> Option<&'a BlockDef> {
        let Place::Var(v) = receiver else { return None };
        match self.ty(self.vars.get(v.index())?.ty)? {
            Type::Handle(HandleKind::Block(b)) => self.p.blocks.get(b.index()),
            _ => None,
        }
    }

    /// Der teuerste `step` aller Bloecke: die Schranke, wenn der Block
    /// eines Empfaengers nicht aus seinem Typ zu lesen ist.
    fn any_step(&self) -> CostVec {
        self.p.blocks.iter().filter_map(|b| b.step).fold(CostVec::ZERO, |acc, f| acc.max(self.function(f)))
    }

    /// `CAP` eines Stroms: so viele Elemente fasst sein Fenster (9.6).
    fn window(&self, s: StreamRef) -> u64 {
        let internal = |i: crate::ids::StreamId| self.p.streams.get(i.index()).map_or(0, |d| u64::from(d.capacity));
        match s {
            // Die Sema traegt jedem Eingabestrom seine Kapazitaet ein
            // (Pruefung 17); ohne sie gilt die des Interpreters.
            StreamRef::Channel(c) => {
                self.p.channels.get(c.index()).map_or(0, |d| u64::from(d.attrs.capacity.unwrap_or(16)))
            }
            StreamRef::Internal(i) => internal(i),
            StreamRef::Fired(t) => self.p.triggers.get(t.index()).map_or(0, |d| internal(d.fired)),
            // Ein Strom als Parameter: Der Codegen senkt ihn nicht; die
            // Schranke ist das groesste Fenster, das ihn binden kann.
            StreamRef::Var(_) => self.widest_window(),
        }
    }

    fn widest_window(&self) -> u64 {
        let channels = (0..self.p.channels.len())
            .filter(|i| matches!(self.ty(self.p.channels[*i].ty), Some(Type::Stream(_))))
            .map(|i| self.window(StreamRef::Channel(crate::ids::ChannelId(i as u32))));
        let internal = self.p.streams.iter().map(|s| u64::from(s.capacity));
        channels.chain(internal).max().unwrap_or(0)
    }

    /// Der Elementtyp eines Stroms.
    fn element(&self, s: StreamRef) -> Option<TypeId> {
        let of = |t: TypeId| match self.ty(t) {
            Some(Type::Stream(e)) => Some(*e),
            _ => None,
        };
        match s {
            StreamRef::Channel(c) => of(self.p.channels.get(c.index())?.ty),
            StreamRef::Internal(i) => Some(self.p.streams.get(i.index())?.elem),
            StreamRef::Fired(t) => Some(self.p.streams.get(self.p.triggers.get(t.index())?.fired.index())?.elem),
            StreamRef::Var(v) => of(self.vars.get(v.index())?.ty),
        }
    }

    /// Der Strom hinter einem Ausdruck, wenn er einer ist.
    fn stream_of(&self, e: &Expr) -> Option<StreamRef> {
        match &e.kind {
            ExprKind::Stream(s) => Some(StreamRef::Internal(*s)),
            ExprKind::Input { channel, .. } => Some(StreamRef::Channel(*channel)),
            ExprKind::Var(v) => Some(StreamRef::Var(*v)),
            _ => None,
        }
        .filter(|_| matches!(self.ty(e.ty), Some(Type::Stream(_))))
    }

    /// Ein Element holen und in eine Bindung legen: Aufruf, Zaehler und
    /// die Bytes des Elements samt Zeitstempel (8.7, Wrapper-Regel).
    fn fetch(&self, element: Option<TypeId>) -> CostVec {
        FETCH + BYTE.times(element.map_or(0, |t| self.bytes(t)) + 8)
    }

    /// Die Zeichen, die ein Muster hoechstens liest: die Kapazitaet eines
    /// Texts, sonst die Bytes des Werts.
    fn text_len(&self, t: Option<TypeId>) -> u64 {
        match t.and_then(|t| self.ty(t)) {
            Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => u64::from(*cap),
            _ => t.map_or(0, |t| self.bytes(t)),
        }
    }
}

/// Traegt ein Wert dieses Typs mehr als ein Maschinenwort, so dass eine
/// Zuweisung ihn Byte fuer Byte kopiert?
fn aggregate(t: &Type, types: &[Type]) -> bool {
    let inner = |id: TypeId| types.get(id.index()).is_some_and(|t| aggregate(t, types));
    match t {
        Type::Record(_)
        | Type::Array { .. }
        | Type::Bytes { .. }
        | Type::Vec { .. }
        | Type::Str { .. }
        | Type::Line { .. }
        | Type::Samples { .. }
        | Type::Map { .. }
        | Type::Mat { .. }
        | Type::Capture { .. } => true,
        Type::Optional(i) => inner(*i),
        Type::Result { ok, .. } => inner(*ok),
        _ => false,
    }
}

/// `B_m` mit seiner Herkunft je Zustand.
///
/// **Warum die Aufschluesselung hier entsteht und nicht im Bericht.** Die
/// Kettenwerte fallen beim Rechnen von `B_m` ohnehin an — `max` wirft sie
/// nur weg. Sie im Bericht erneut zu rechnen waere eine zweite Stelle mit
/// derselben Formel, und sie in `Budget` zu legen hiesse, sie in jede
/// MIR-Datei zu schreiben (Feld 20 des Formats), damit ein Werkzeug sie
/// gelegentlich anzeigen kann. Eine Rechnung, zwei Verbraucher: Wer nur
/// die Summe will, nimmt `.total`.
#[derive(Clone, Debug, Default)]
pub struct Activation {
    /// `B_m`: das Maximum ueber alle Ketten und `FAULTED`.
    pub total: CostVec,
    /// Was jede Kette ausser `FAULTED` traegt: der maschinenweite `loop:`
    /// und die Handler der Maschinenebene.
    pub base: CostVec,
    /// Je Zustand der teuerste Tick, dessen innerster Zustand er ist —
    /// ohne `base`, indiziert wie `Machine::states`.
    pub states: Vec<CostVec>,
    /// Ein Tick in `FAULTED`: nur dessen Uebergaenge (5.3).
    pub faulted: CostVec,
}

impl Activation {
    /// Der Zustand, der eine Klasse bestimmt — je Klasse ein anderer.
    ///
    /// **Es gibt keinen „teuersten Zustand".** Das Maximum ist
    /// komponentenweise (9.4.3: Kosten sind Vektoren), und die Schranke
    /// darf das sein: Zwei Zustaende schliessen einander aus, also ist
    /// fuer *jede* Klasse einzeln der groesste Wert erreichbar. Wer den
    /// Bericht liest, will darum je Klasse wissen, wo die Zahl herkommt —
    /// und das koennen verschiedene Zustaende sein.
    ///
    /// Bei Gleichstand gewinnt der erste: willkuerlich, aber stabil, damit
    /// zwei Laeufe desselben Programms denselben Bericht ergeben.
    pub fn driver(&self, class: CostClass) -> Option<usize> {
        let peak = self.states.iter().map(|c| c.of(class)).max()?;
        self.states.iter().position(|c| c.of(class) == peak)
    }
}

/// `B_m`: was eine Aktivierung im schlimmsten Fall kostet, mit den
/// Rumpfkosten der Funktionen aus `Fn::cost` (nach [`budgets`]).
pub fn activation(p: &Program, m: &Machine) -> Activation {
    let fns: Vec<CostVec> = p.fns.iter().map(|f| f.cost.unwrap_or_default()).collect();
    Tree::new(m, Ctx::of_machine(p, m, &fns, longest_table(p))).activation()
}

/// Der Zustandsbaum einer Maschine mit dem, was jeder Zustand zu einer
/// Kette beitraegt.
struct Tree<'a> {
    m: &'a Machine,
    ctx: Ctx<'a>,
    /// Je Zustand: Initialwerte, Zaehler, Instanzen, `enter:` und `loop:`
    /// im Modus ENTRY (ohne Handler) — was sein eigenes Betreten kostet.
    own_entry: Vec<CostVec>,
    /// Je Zustand: sein Betreten samt den Kindern, in die es weitergeht —
    /// `initial`, mit `resume` das teuerste (5.12: der gespeicherte Pfad
    /// ist statisch unbekannt).
    entry: Vec<CostVec>,
    /// Je Zustand: `exit:` und das Abschalten seiner Instanzen.
    exit: Vec<CostVec>,
    /// Je Zustand: `loop:` und Handler im Modus RUN.
    run: Vec<CostVec>,
    /// Je Zustand: die Waechter seiner Uebergaenge.
    guards: Vec<CostVec>,
}

impl<'a> Tree<'a> {
    fn new(m: &'a Machine, ctx: Ctx<'a>) -> Tree<'a> {
        let own_entry: Vec<CostVec> =
            m.states.iter().enumerate().map(|(i, s)| own_entry(m, StateId(i as u32), s, &ctx)).collect();
        let exit = m.states.iter().map(|s| block_cost(&s.exit, &ctx) + BYTE.times(s.instances.len() as u64)).collect();
        let run = m
            .states
            .iter()
            .map(|s| {
                let call = if s.loop_block.stmts.is_empty() { CostVec::ZERO } else { BLOCK_CALL };
                call + BYTE.times(s.vars.len() as u64) + block_cost(&s.loop_block, &ctx) + dispatch(&s.handlers, &ctx)
            })
            .collect();
        let guards = m
            .states
            .iter()
            .map(|s| s.transitions.iter().fold(CostVec::ZERO, |acc, t| acc + trigger_cost(&t.trigger, &ctx)))
            .collect();
        let mut tree = Tree { m, ctx, own_entry, entry: Vec::new(), exit, run, guards };
        tree.entry = (0..m.states.len()).map(|i| tree.entry_of(StateId(i as u32), false, 0)).collect();
        tree
    }

    /// Das Betreten von `s` und seinen Kindern; `any` nimmt das teuerste
    /// Kind statt `initial` (tiefe History, 5.12).
    fn entry_of(&self, s: StateId, any: bool, depth: usize) -> CostVec {
        let Some(state) = self.m.states.get(s.index()) else { return CostVec::ZERO };
        let own = self.own_entry[s.index()];
        if state.children.is_empty() || depth > self.m.states.len() {
            return own;
        }
        let any = any || state.resume;
        let below = match state.initial.filter(|_| !any) {
            Some(i) => self.entry_of(i, false, depth + 1),
            None => state.children.iter().fold(CostVec::ZERO, |acc, c| acc.max(self.entry_of(*c, any, depth + 1))),
        };
        own + below
    }

    /// Die innersten Zustaende, in denen ein Betreten von `s` enden kann.
    fn entered_leaves(&self, s: StateId) -> Vec<StateId> {
        let mut out = Vec::new();
        let mut open = vec![(s, false)];
        while let Some((at, any)) = open.pop() {
            let Some(state) = self.m.states.get(at.index()) else { continue };
            if state.children.is_empty() || out.len() + open.len() > self.m.states.len() {
                out.push(at);
                continue;
            }
            let any = any || state.resume;
            match state.initial.filter(|_| !any) {
                Some(i) => open.push((i, false)),
                None => open.extend(state.children.iter().map(|c| (*c, any))),
            }
        }
        out
    }

    /// Die Kette eines Zustands: seine Vorfahren und er, von aussen nach
    /// innen (5.1).
    fn chain(&self, s: StateId) -> Vec<StateId> {
        let mut out = vec![s];
        let mut at = self.m.states.get(s.index()).and_then(|st| st.parent);
        while let Some(p) = at {
            // Ein Baum hat hoechstens so viele Ebenen wie Zustaende.
            if out.len() > self.m.states.len() {
                break;
            }
            out.push(p);
            at = self.m.states.get(p.index()).and_then(|st| st.parent);
        }
        out.reverse();
        out
    }

    /// Die Austritte der Kette ab Ebene `keep`, innen nach aussen.
    fn exits(&self, from: &[StateId], keep: usize) -> CostVec {
        from.iter().skip(keep).fold(CostVec::ZERO, |acc, s| acc + self.exit[s.index()])
    }

    /// Ein Wechsel von der Kette `from` nach `q` (9.3 `switch`, dann
    /// `exec_chain(entered, ENTRY)`).
    ///
    /// Der kleinste gemeinsame Vorfahr bleibt aktiv (5.2). `q` selbst wird
    /// verlassen und neu betreten, auch wenn es in der Kette liegt: die
    /// teurere Lesart eines Uebergangs auf sich selbst, also eine Schranke
    /// fuer beide.
    fn switch(&self, from: &[StateId], q: StateId) -> CostVec {
        let to = self.chain(q);
        let above = &to[..to.len() - 1];
        let shared = from.iter().zip(above).take_while(|(a, b)| a == b).count();
        let between = above[shared..].iter().fold(CostVec::ZERO, |acc, s| acc + self.own_entry[s.index()]);
        // `conf`, `pc` und je betretenem Zustand sein Zaehler, dazu der des
        // Blatts; der Entry-Tick ist ein Aufruf (9.3, 11.2).
        let entered = to.len().saturating_sub(shared) as u64;
        let machinery = BYTE.times(entered + 3) + BLOCK_CALL;
        STEP + machinery + self.exits(from, shared) + between + self.entry.get(q.index()).copied().unwrap_or_default()
    }

    /// Der Weg nach `FAULTED`: `conf` und die `safe`-Werte der eigenen
    /// Outputs (5.3).
    fn faulted_switch(&self) -> CostVec {
        let own = self.ctx.p.machines.iter().position(|x| std::ptr::eq(x, self.m));
        let safe = self
            .ctx
            .p
            .channels
            .iter()
            .filter(|c| own.is_some_and(|i| c.owner == Some(crate::ids::MachineId(i as u32))) && c.attrs.safe.is_some())
            .count();
        STEP + BYTE.times(1 + safe as u64)
    }

    /// Was jede Aktivierung vor und nach dem Nutzercode tut: verteilen, die
    /// Zaehler fortschreiben, die Variablen der Maschine laden und
    /// speichern (LLVM haelt sie im Aufruf in Registern).
    fn frame(&self) -> CostVec {
        let own = self.m.vars.iter().filter(|v| v.scope == crate::machine::VarScope::Machine).count();
        DISPATCH + TIMER.times(self.m.states.len() as u64 + 1) + BYTE.times(own as u64)
    }

    /// Was ein Ziel von der Kette `from` aus kostet. Ein Fault-Ziel geht
    /// ueber den Fault-Pfad, und der steht in `F_m`.
    fn target(&self, from: &[StateId], t: Target) -> CostVec {
        match t {
            Target::State(q) => self.switch(from, q),
            Target::Faulted => self.faulted_switch() + self.exits(from, 0),
            Target::Fault(_) => STEP,
        }
    }

    /// `B_m` je Kette (9.3, 9.4.3).
    fn activation(&self) -> Activation {
        let ctx = &self.ctx;
        let machine_loop = if self.m.loop_block.stmts.is_empty() { CostVec::ZERO } else { BLOCK_CALL };
        let base = self.frame() + machine_loop + block_cost(&self.m.loop_block, ctx) + dispatch(&self.m.handlers, ctx);
        let mut machine_gotos = gotos(&self.m.loop_block);
        for h in &self.m.handlers {
            machine_gotos.extend(gotos(&h.body));
        }
        let states: Vec<CostVec> = (0..self.m.states.len())
            .map(|i| {
                let from = self.chain(StateId(i as u32));
                let mut body = CostVec::ZERO;
                let mut fire = CostVec::ZERO;
                let mut targets = machine_gotos.clone();
                for s in &from {
                    let state = &self.m.states[s.index()];
                    body = body + self.run[s.index()] + self.guards[s.index()];
                    for t in &state.transitions {
                        fire = fire.max(block_cost(&t.actions, ctx) + self.target(&from, t.target));
                    }
                    targets.extend(gotos(&state.loop_block));
                    for h in &state.handlers {
                        targets.extend(gotos(&h.body));
                    }
                }
                // Ein `->` in einem Rumpf kostet seine Eins dort; hier nur
                // der Wechsel.
                for t in targets {
                    fire = fire.max(self.target(&from, t));
                }
                body + fire
            })
            .collect();
        let faulted = self.faulted();
        let peak = states.iter().fold(CostVec::ZERO, |acc, c| acc.max(*c));
        Activation { total: (base + peak).max(faulted), base, states, faulted }
    }

    /// Ein Tick in `FAULTED`: kein Nutzercode, nur die Waechter und der
    /// teuerste Uebergang hinaus (5.3). Von dort wird jede Ebene betreten.
    fn faulted(&self) -> CostVec {
        let ctx = &self.ctx;
        let ts = &self.m.faulted.transitions;
        let guards = ts.iter().fold(CostVec::ZERO, |acc, t| acc + trigger_cost(&t.trigger, ctx));
        let fire =
            ts.iter().fold(CostVec::ZERO, |acc, t| acc.max(block_cost(&t.actions, ctx) + self.target(&[], t.target)));
        self.frame() + guards + fire
    }

    /// `F_m`: der teuerste Fault-Pfad ab einer Kette (9.4.3).
    fn fault_path(&self) -> CostVec {
        let mut memo = vec![None; self.m.states.len()];
        let mut open = vec![false; self.m.states.len()];
        (0..self.m.states.len())
            .fold(CostVec::ZERO, |acc, i| acc.max(self.fault_from(StateId(i as u32), &mut memo, &mut open)))
    }

    /// Der Fault-Pfad, wenn `s` der innerste Zustand ist: Vormerken und
    /// Verwerfen (5.3), der Wechsel zum Fault-Ziel, dort die Bloecke im Modus
    /// ENTRY — und wenn die scheitern, der naechste Fault ab der neuen
    /// Kette, bis `FAULTED` oder das Ende des Fault-Waldes.
    fn fault_from(&self, s: StateId, memo: &mut Vec<Option<CostVec>>, open: &mut Vec<bool>) -> CostVec {
        if let Some(c) = memo[s.index()] {
            return c;
        }
        // Der Fault-Wald ist azyklisch (5.3); ein Kreis kaeme nur aus einer
        // MIR, die an der Sema vorbeiging, und endet hier.
        if open[s.index()] {
            return CostVec::ZERO;
        }
        open[s.index()] = true;
        let from = self.chain(s);
        let layout = &self.m.layout;
        // `last_fault` setzen, geplante Ausgaben verwerfen, Jobs abbrechen,
        // Trigger entschaerfen (5.3).
        let book = STEP
            + BYTE.times(1 + (layout.output_queues.len() + layout.trigger_flags.len()) as u64)
            + RUNTIME_CALL.times(1 + layout.job_slots.len() as u64);
        let mut worst = CostVec::ZERO;
        for target in self.fault_targets(&from) {
            let path = match target {
                FaultTarget::Faulted => self.exits(&from, 0) + self.faulted_switch(),
                FaultTarget::State(q) => {
                    let next = self
                        .entered_leaves(q)
                        .into_iter()
                        .fold(CostVec::ZERO, |acc, l| acc.max(self.fault_from(l, memo, open)));
                    self.switch(&from, q) + next
                }
            };
            worst = worst.max(book + path);
        }
        open[s.index()] = false;
        memo[s.index()] = Some(worst);
        worst
    }

    /// Wohin ein Fault aus der Kette `from` fuehren kann: das Ziel des
    /// innersten Zustands, der eines deklariert, sonst das der Maschine
    /// (5.2 Regel 5), dazu die eigenen Ziele der `check`s der Kette.
    fn fault_targets(&self, from: &[StateId]) -> Vec<FaultTarget> {
        // 5.3: Der als Fault-Ziel der Maschine deklarierte Zustand erbt
        // nicht von ihr, sein Ziel ist `FAULTED` — sonst fuehrte er auf
        // sich selbst.
        let inherited = from.last().map_or(self.m.fault_target, |s| self.m.fault_target_of(*s));
        let mut out = vec![inherited];
        let mut blocks: Vec<&Block> = vec![&self.m.loop_block];
        blocks.extend(self.m.handlers.iter().map(|h| &h.body));
        for s in from {
            let st = &self.m.states[s.index()];
            blocks.extend([&st.enter, &st.exit, &st.loop_block]);
            blocks.extend(st.handlers.iter().map(|h| &h.body));
            blocks.extend(st.transitions.iter().map(|t| &t.actions));
        }
        for b in blocks {
            b.walk(&mut |stmt| {
                if let StmtKind::Check { target: Some(t), .. } = &stmt.kind {
                    let target = match t {
                        Target::State(q) => FaultTarget::State(*q),
                        Target::Faulted => FaultTarget::Faulted,
                        Target::Fault(_) => inherited,
                    };
                    if !out.contains(&target) {
                        out.push(target);
                    }
                }
            });
        }
        out
    }
}

/// Was das eigene Betreten eines Zustands kostet (9.3 Schritte 3 und 4,
/// dann `exec_chain` im Modus ENTRY): Initialwerte der zustandslokalen
/// Variablen, Timer und Zaehler, die gescopten Instanzen, `enter:` und
/// `loop:` ohne Handler.
fn own_entry(m: &Machine, id: StateId, s: &crate::machine::State, ctx: &Ctx<'_>) -> CostVec {
    let mut c = block_cost(&s.enter, ctx) + block_cost(&s.loop_block, ctx);
    for v in s.vars.iter().filter_map(|v| m.vars.get(v.index())) {
        c = c + v.init.as_ref().map_or(CostVec::ZERO, |e| expr_cost(e, ctx)) + ctx.store(v.ty);
    }
    let l = &m.layout;
    let counters = l.timers.iter().filter(|t| t.state == id).count()
        + l.every_counters.iter().filter(|c| c.state == Some(id)).count()
        + l.viol_sites.iter().filter(|c| c.state == Some(id)).count();
    c = c + BYTE.times(counters as u64);
    for si in &s.instances {
        let Some(inst) = ctx.p.machines.get(si.machine.index()) else { continue };
        let inst_ctx = Ctx { vars: &inst.vars, ..*ctx };
        c = c + BYTE;
        for v in inst.vars.iter().filter(|v| v.scope == crate::machine::VarScope::Machine) {
            c = c + v.init.as_ref().map_or(CostVec::ZERO, |e| expr_cost(e, &inst_ctx)) + inst_ctx.store(v.ty);
        }
    }
    c
}

/// Die Ziele der `->` eines Blocks, auch in Zweigen.
fn gotos(b: &Block) -> Vec<Target> {
    let mut out = Vec::new();
    b.walk(&mut |s| {
        if let StmtKind::Goto(t) = &s.kind {
            out.push(*t);
        }
    });
    out
}

/// Der Waechter eines Uebergangs (5.2, 8.7): ausgewertet wird jeder der
/// Kette, bis einer zutrifft — im schlimmsten Fall alle.
fn trigger_cost(t: &TransTrigger, ctx: &Ctx<'_>) -> CostVec {
    STEP + match t {
        TransTrigger::After(d) => expr_cost(d, ctx),
        TransTrigger::When(Guard::Expr(e)) => expr_cost(e, ctx),
        TransTrigger::When(Guard::Match { subject, kind, pattern, binding }) => {
            let element = match ctx.ty(subject.ty) {
                Some(Type::Stream(e)) => Some(*e),
                _ => None,
            };
            match element {
                // Ein Strom-Guard sucht das erste passende Element im
                // Fenster (8.7).
                Some(e) => {
                    let n = ctx.stream_of(subject).map_or_else(|| ctx.widest_window(), |s| ctx.window(s));
                    let bind = if binding.is_some() { ctx.copy_always(e) } else { CostVec::ZERO };
                    (ctx.fetch(Some(e)) + pattern_cost(*kind, pattern, Some(e), ctx) + bind + STEP).times(n)
                }
                None => expr_cost(subject, ctx) + pattern_cost(*kind, pattern, Some(subject.ty), ctx),
            }
        }
        TransTrigger::When(Guard::Next { stream, .. }) => {
            let e = ctx.element(*stream);
            CostVec { call: 1, ..CostVec::ZERO } + ctx.fetch(e)
        }
    }
}

impl Ctx<'_> {
    /// Eine Bindung bekommt die Bytes eines Elements immer, auch einen
    /// Skalar: Sie ist ein Record mit `t`, `seq` und dem Inhalt (8.7).
    fn copy_always(&self, t: TypeId) -> CostVec {
        BYTE.times(self.bytes(t) + 16)
    }
}

/// Der Dispatch einer Ebene (9.7): je Strom das Fenster, je Element die
/// Handler des Stroms bis zum treffenden und der teuerste Rumpf.
///
/// 9.4.3 rechnet `CAP · (max_len + max_h N(body h))` und setzt dabei einen
/// Durchlauf je Element voraus, den Produkt-DFA aus 11.2. Der Codegen
/// prueft die Handler nacheinander (`takt_llvm::step::handler_chain`); die
/// Schranke zaehlt darum je Handler seinen Abgleich — das, was laeuft.
fn dispatch(handlers: &[Handler], ctx: &Ctx<'_>) -> CostVec {
    let mut streams: Vec<StreamRef> = Vec::new();
    for h in handlers {
        if !streams.contains(&h.stream) {
            streams.push(h.stream);
        }
    }
    streams.into_iter().fold(CostVec::ZERO, |acc, s| {
        let element = ctx.element(s);
        let hs = handlers.iter().filter(|h| h.stream == s);
        let tries = hs.clone().fold(CostVec::ZERO, |acc, h| acc + handler_try(h, element, ctx));
        let body = hs.fold(CostVec::ZERO, |acc, h| acc.max(block_cost(&h.body, ctx)));
        // Die Zahl der Elemente im Fenster (`takt_stream_count`), dann je
        // Element holen, pruefen, einen Rumpf.
        let count = CostVec { call: 1, ..CostVec::ZERO };
        acc + count + (ctx.fetch(element) + tries + body).times(ctx.window(s))
    })
}

/// Was ein Handler an einem Element prueft, bevor sein Rumpf laeuft oder
/// der naechste dran ist: Bindung, Muster, Guard (8.7, FB-14).
fn handler_try(h: &Handler, element: Option<TypeId>, ctx: &Ctx<'_>) -> CostVec {
    let bind = match (h.binding, element) {
        (Some(_), Some(e)) => ctx.copy_always(e),
        _ => CostVec::ZERO,
    };
    let pattern = h.pattern.as_ref().map_or(CostVec::ZERO, |(kind, p)| pattern_cost(*kind, p, element, ctx));
    let guard = h.guard.as_ref().map_or(CostVec::ZERO, |g| expr_cost(g, ctx) + STEP);
    STEP + bind + pattern + guard
}

/// Ein Musterabgleich ueber einem Text (8.7; 9.4.3 `N(x matches P) =
/// max_len(x)`).
///
/// `matches` laeuft einmal ueber den Text. `has` sucht ein Vorkommen: Der
/// Codegen setzt den Durchlauf an jeder Stelle neu an
/// (`takt_llvm::step::text_has`), und jeder Ansatz liest hoechstens so weit,
/// wie das Muster reicht — ohne offenes Ende also die Laenge des Musters,
/// mit `{_}` den Rest des Texts.
fn pattern_cost(kind: MatchKind, p: &Pattern, subject: Option<TypeId>, ctx: &Ctx<'_>) -> CostVec {
    match p {
        Pattern::Text { pieces, .. } => {
            let n = ctx.text_len(subject);
            match kind {
                MatchKind::Matches => MATCH_BYTE.times(n) + captures(pieces, n),
                MatchKind::Has => {
                    let reach = span(pieces, n);
                    (MATCH_BYTE.times(reach) + captures(pieces, reach) + STEP).times(n + 1)
                }
            }
        }
        // Eine Konjunktion von Feldgleichheiten ueber dem dekodierten
        // Element.
        Pattern::Record { fields, .. } => {
            BYTE.times(subject.map_or(0, |t| ctx.bytes(t)))
                + (STEP + CostVec::op(CostClass::I64)).times(fields.len() as u64)
        }
    }
}

/// Wie weit ein Ansatz des Musters hoechstens liest, gekappt bei `n`.
fn span(pieces: &[PatternPiece], n: u64) -> u64 {
    let reach: u64 = pieces
        .iter()
        .map(|p| match p {
            PatternPiece::Text(t) => t.len() as u64,
            PatternPiece::Capture { kind: CaptureKind::Int, .. } => INT_CHARS,
            PatternPiece::Capture { kind: CaptureKind::Hex, .. } => HEX_CHARS,
            PatternPiece::Capture { kind: CaptureKind::Word, .. } => WORD_CHARS,
            PatternPiece::Capture { kind: CaptureKind::Str(k), .. } => u64::from(*k),
            PatternPiece::Capture { kind: CaptureKind::Float, .. } | PatternPiece::Any => n,
        })
        .fold(0u64, u64::saturating_add);
    reach.min(n)
}

/// Die Werte der Platzhalter: Ziffern werden ein zweites Mal gelesen und
/// zusammengerechnet, Woerter und Texte in die Bindung kopiert.
fn captures(pieces: &[PatternPiece], n: u64) -> CostVec {
    pieces.iter().fold(CostVec::ZERO, |acc, p| {
        acc + match p {
            PatternPiece::Capture { kind: CaptureKind::Int, .. } => CAPTURE_DIGIT.times(INT_CHARS.min(n)),
            PatternPiece::Capture { kind: CaptureKind::Hex, .. } => CAPTURE_DIGIT.times(HEX_CHARS.min(n)),
            PatternPiece::Capture { kind: CaptureKind::Float, .. } => CAPTURE_DIGIT.times(n),
            PatternPiece::Capture { kind: CaptureKind::Word, .. } => BYTE.times(WORD_CHARS.min(n)),
            PatternPiece::Capture { kind: CaptureKind::Str(k), .. } => BYTE.times(u64::from(*k).min(n)),
            PatternPiece::Text(_) | PatternPiece::Any => CostVec::ZERO,
        }
    })
}

/// Ein Formatstring (3.9, 8.8): Text Zeichen fuer Zeichen, Zahlen Ziffer
/// fuer Ziffer (`takt_llvm::format`).
///
/// Wie viele Ziffern eine Zahl hoechstens hat, sagt [`max_abs`].
/// Platzhalter anderer Typen (Fliesskomma, Dauer, Enum, Record, Feld)
/// senkt der Codegen nicht; fuer sie zaehlt je Zeichen ihres Anteils an
/// `len_max` eine Ziffer.
fn format_cost(f: &Format, ctx: &Ctx<'_>) -> CostVec {
    let mut c = CostVec::ZERO;
    let mut known = 0u64;
    for piece in &f.pieces {
        match piece {
            FormatPiece::Text(t) => {
                known += t.len() as u64;
                c = c + TEXT_BYTE.times(t.len() as u64);
            }
            FormatPiece::Expr { expr, spec } => match ctx.ty(expr.ty) {
                Some(Type::Bool) => {
                    known += BOOL_WIDTH;
                    c = c + STEP + TEXT_BYTE.times(BOOL_WIDTH);
                }
                Some(Type::Int { .. }) => {
                    let (magnitude, negative) = max_abs(expr, ctx);
                    match spec.as_deref() {
                        Some("hex") => {
                            known += HEX_WIDTH;
                            // Ohne Vorzeichen zaehlt das Bitmuster: eine
                            // negative Zahl hat alle 16 Stellen.
                            let digits = if negative { 16 } else { hex_digits(magnitude) };
                            c = c + HEX_DIGIT.times(digits);
                        }
                        Some(s) if s.starts_with('0') => {
                            let width: u64 = s.parse().unwrap_or(0);
                            known += INT_WIDTH.max(width);
                            let digits = decimal_digits(magnitude);
                            // Erst zaehlen, wie viele Stellen es sind, dann
                            // mit Nullen fuellen, dann schreiben.
                            let count = (CostVec::heavy_op(Heavy::Div, CostClass::I64) + ITERATION).times(digits);
                            c = c + count + TEXT_BYTE.times(width.max(1) + 1) + DECIMAL_DIGIT.times(digits);
                        }
                        _ => {
                            known += INT_WIDTH;
                            let sign = if negative { TEXT_BYTE } else { CostVec::ZERO };
                            c = c + sign + DECIMAL_DIGIT.times(decimal_digits(magnitude));
                        }
                    }
                }
                _ => {}
            },
        }
    }
    c + DECIMAL_DIGIT.times(u64::from(f.len_max).saturating_sub(known))
}

/// Der hoechste Betrag einer Ganzzahl und ob sie negativ sein kann — aus
/// dem, was ihren Ausdruck sicher begrenzt: ein Literal, der Rest durch ein
/// Literal (`|a % d| < |d|`), eine Maske mit nicht negativem Literal, der
/// Range ihres Typs, sonst die gewaehlte Darstellung (3.4).
fn max_abs(e: &Expr, ctx: &Ctx<'_>) -> (u64, bool) {
    let literal = |x: &Expr| match x.kind {
        ExprKind::Int(v) => Some(v),
        _ => None,
    };
    match &e.kind {
        ExprKind::Int(v) => return (v.unsigned_abs(), *v < 0),
        ExprKind::Checked { expr, .. } => return max_abs(expr, ctx),
        ExprKind::Binary { op: BinaryOp::Rem, lhs, rhs } => {
            if let Some(d) = literal(rhs).filter(|d| *d != 0) {
                // Der Rest traegt das Vorzeichen des Dividenden.
                let (_, negative) = max_abs(lhs, ctx);
                return (d.unsigned_abs() - 1, negative);
            }
        }
        ExprKind::Binary { op: BinaryOp::BitAnd, lhs, rhs } => {
            if let Some(m) = literal(rhs).or_else(|| literal(lhs)).filter(|m| *m >= 0) {
                return (m.unsigned_abs(), false);
            }
        }
        _ => {}
    }
    let ty = ctx.ty(e.ty);
    if let Some(Type::Int { range: Some(r), .. }) = ty
        && let (Const::Int(lo), Const::Int(hi)) = (&r.lo, &r.hi)
    {
        return (lo.unsigned_abs().max(hi.unsigned_abs()), *lo < 0);
    }
    let bits = match (e.repr, ty.and_then(crate::types::storage_width)) {
        (Some(Repr::I32), _) => 32,
        (_, Some(w)) => w.bits(),
        _ => 64,
    };
    (1u64 << (bits - 1), true)
}

fn decimal_digits(v: u64) -> u64 {
    u64::from(v.checked_ilog10().unwrap_or(0)) + 1
}

fn hex_digits(v: u64) -> u64 {
    u64::from(v.checked_ilog2().unwrap_or(0) / 4) + 1
}

fn block_cost(b: &Block, ctx: &Ctx<'_>) -> CostVec {
    b.stmts.iter().fold(CostVec::ZERO, |acc, s| acc + stmt_cost(s, ctx))
}

fn stmt_cost(s: &Stmt, ctx: &Ctx<'_>) -> CostVec {
    let one_mem = BYTE;
    match &s.kind {
        StmtKind::Assign { target, value } => place_cost(target, ctx) + expr_cost(value, ctx) + ctx.copy(value.ty),
        // `N(check e) = cost(e) + 1`; ein `for d` zaehlt seine Bestaetigung
        // mit (5.5).
        StmtKind::Check { cond, confirm, .. } => {
            let counter = confirm.as_ref().map_or(CostVec::ZERO, |c| expr_cost(&c.duration, ctx) + one_mem + STEP);
            expr_cost(cond, ctx) + STEP + counter
        }
        StmtKind::Goto(_) | StmtKind::Abort { .. } | StmtKind::Break => STEP,
        StmtKind::If { cond, then, otherwise } => {
            // Nur ein Zweig laeuft.
            expr_cost(cond, ctx) + block_cost(then, ctx).max(block_cost(otherwise, ctx))
        }
        StmtKind::ForRange { count, body, .. } => {
            let n = match count.kind {
                ExprKind::Int(v) => v.max(0) as u64,
                _ => 1,
            };
            STEP + expr_cost(count, ctx) + (block_cost(body, ctx) + ITERATION).times(n)
        }
        StmtKind::ForEach { iter, body, .. } => {
            // `1 + CAP · N(b)`: die Kapazitaet des Behaelters, bei einem
            // Strom sein Fenster, je Element geholt (9.6).
            let each = match ctx.ty(iter.ty) {
                Some(Type::Stream(e)) => ctx.fetch(Some(*e)) + ctx.copy_always(*e),
                Some(Type::Map { .. }) => ITERATION + one_mem.times(2),
                _ => ITERATION + one_mem,
            };
            let n = match ctx.stream_of(iter) {
                Some(s) => ctx.window(s),
                None => capacity(iter, ctx.types()),
            };
            STEP + expr_cost(iter, ctx) + (block_cost(body, ctx) + each).times(n)
        }
        StmtKind::Match { subject, arms } => {
            let worst = arms.iter().fold(CostVec::ZERO, |acc, a| acc.max(block_cost(&a.body, ctx)));
            expr_cost(subject, ctx) + worst
        }
        StmtKind::Every { period, body, .. } => STEP + expr_cost(period, ctx) + block_cost(body, ctx),
        StmtKind::At { time, body } => STEP + expr_cost(time, ctx) + block_cost(body, ctx),
        // `N(send o, e) = cost(e) + len_max(e)`: der Text entsteht im
        // Puffer, dann geht er in den Sendepuffer (`takt_stream_send`).
        StmtKind::Send { value, len_max, .. } => {
            expr_cost(value, ctx) + BYTE.times(u64::from(*len_max)) + STEP + CostVec { call: 1, ..CostVec::ZERO }
        }
        StmtKind::Return(value) => expr_cost(value, ctx) + ctx.copy(value.ty),
        StmtKind::MethodCall { target, receiver, method, args } => {
            let a = args.iter().fold(CostVec::ZERO, |acc, x| acc + expr_cost(x, ctx));
            let t = target.as_ref().map_or(CostVec::ZERO, |p| place_cost(p, ctx));
            let body = match (method, args.first()) {
                // `append` kopiert; die obere Schranke ist die Kapazitaet
                // der Quelle (3.9), nicht ihre aktuelle Laenge.
                (Method::Append, Some(src)) => CostVec { mem: capacity(src, ctx.types()), ..CostVec::ZERO },
                // Der Schritt einer Blockinstanz kostet seinen Rumpf (5.7).
                (Method::Step, _) => match ctx.block_of(receiver) {
                    Some(def) => def.step.map_or(CostVec::ZERO, |f| ctx.function(f)),
                    None => ctx.any_step(),
                },
                (Method::Block(f), _) => ctx.function(*f),
                // `reset` setzt den Zustand des Blocks auf seine
                // Initialwerte.
                (Method::Reset, _) => ctx.block_of(receiver).map_or(CostVec::ZERO, |def| {
                    def.state_vars.iter().fold(CostVec::ZERO, |acc, v| {
                        acc + v.init.as_ref().map_or(CostVec::ZERO, |e| expr_cost(e, ctx)) + ctx.store(v.ty)
                    })
                }),
                // 3.9: Eine Operation auf einer `map` kostet im schlimmsten
                // Fall O(N).
                (Method::Insert | Method::Remove | Method::Clear, _) => match receiver_type(receiver, ctx) {
                    Some(Type::Map { key, value, cap }) => match method {
                        Method::Clear => one_mem.times(u64::from(*cap)),
                        _ => map_cost(*key, *value, *cap, *method == Method::Remove, ctx),
                    },
                    _ => one_mem,
                },
                (Method::Push, first) => one_mem + first.map_or(CostVec::ZERO, |x| ctx.store(x.ty)),
                _ => CostVec::ZERO,
            };
            a + t + body + CostVec { call: 1, ..CostVec::ZERO }
        }
        StmtKind::Job { args, native, .. } => {
            args.iter().fold(CostVec::ZERO, |acc, x| acc + expr_cost(x, ctx)) + ctx.native(*native)
        }
        StmtKind::Observe(o) => observe_cost(o, ctx),
        StmtKind::Cancel(_) | StmtKind::Skip(_) | StmtKind::Raise(_) | StmtKind::Arm { .. } => one_mem,
        StmtKind::Pass => CostVec::ZERO,
    }
}

/// Eine Beobachtung ist ein Aufruf der Runtime mit dem Index ihres Texts
/// (`takt_llvm::stmt::observe`); formatiert wird am Host.
fn observe_cost(o: &Observe, ctx: &Ctx<'_>) -> CostVec {
    let call = CostVec { call: 1, ..CostVec::ZERO };
    match o {
        Observe::Alert { cond, .. } | Observe::Verify { cond, .. } => expr_cost(cond, ctx) + call,
        Observe::Measure { value, .. } => expr_cost(value, ctx) + CostVec::op(CostClass::F64) + call,
        Observe::Log(_) | Observe::Verdict { .. } => call,
    }
}

/// Der Typ hinter einem Empfaenger einer Methode.
fn receiver_type<'a>(p: &Place, ctx: &Ctx<'a>) -> Option<&'a Type> {
    match p {
        Place::Var(v) => ctx.ty(ctx.vars.get(v.index())?.ty),
        Place::Output(c) => ctx.ty(ctx.p.channels.get(c.index())?.ty),
        _ => None,
    }
}

/// Eine Operation auf einer `map<K, V, N>` (3.9): FNV-1a ueber den
/// Schluessel, lineare Sondierung ueber alle Plaetze mit Schluesselvergleich,
/// den Wert kopieren; `remove` verschiebt dahinter liegende Eintraege
/// zurueck.
fn map_cost(key: TypeId, value: TypeId, cap: u32, remove: bool, ctx: &Ctx<'_>) -> CostVec {
    let (k, v, n) = (ctx.bytes(key), ctx.bytes(value), u64::from(cap));
    let hash = (CostVec::op(CostClass::I32).times(2) + BYTE).times(k);
    let probe = (ITERATION + BYTE + (STEP + BYTE).times(k)).times(n);
    let shift = if remove { BYTE.times((k + v + 1).saturating_mul(n)) } else { CostVec::ZERO };
    hash + probe + BYTE.times(k + v) + shift
}

fn place_cost(p: &Place, ctx: &Ctx<'_>) -> CostVec {
    match p {
        Place::Var(_) | Place::Output(_) => CostVec::ZERO,
        // 12.10: Ein Portzugriff ist ein Lade- oder Speichervorgang.
        Place::Port(_) => BYTE,
        Place::Field(b, _) => place_cost(b, ctx),
        Place::Index(b, i) => place_cost(b, ctx) + expr_cost(i, ctx) + BYTE,
        Place::Index2(b, r, c) => place_cost(b, ctx) + expr_cost(r, ctx) + expr_cost(c, ctx) + BYTE,
    }
}

/// Kosten eines Ausdrucks: die Operation selbst plus ihre Kinder.
fn expr_cost(e: &Expr, ctx: &Ctx<'_>) -> CostVec {
    let types = ctx.types();
    let own = mat_cost(e, ctx).unwrap_or_else(|| match &e.kind {
        // 7.2: Division hat eigene Gewichte; der Rest geht ueber denselben
        // Dividierer.
        ExprKind::Binary { op: BinaryOp::Div | BinaryOp::Rem, .. } => CostVec::heavy_op(Heavy::Div, class(e, types)),
        // Ein Vergleich rechnet in der Klasse seiner Operanden, nicht in der
        // seines Ergebnisses: `a < b` auf `f64` ist auf einem Kern ohne
        // Doppel-FPU ein Aufruf der Bibliothek, kein `i32`-Vergleich.
        ExprKind::Binary {
            op: BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge | BinaryOp::Eq | BinaryOp::Ne,
            lhs,
            ..
        } => class_of(lhs, types),
        // Eine Umwandlung kostet auch in der Klasse ihrer Quelle: `f64 as
        // int` ist so teuer wie das Gleitkomma, aus dem sie rechnet.
        ExprKind::Cast { expr, .. } | ExprKind::Convert { expr, .. } => class_of(e, types).max(class_of(expr, types)),
        ExprKind::Binary { .. } | ExprKind::Unary { .. } => class_of(e, types),
        ExprKind::Index { .. } | ExprKind::Index2 { .. } | ExprKind::PortRead(_) => BYTE,
        ExprKind::Slice { .. } => BYTE + ctx.copy(e.ty),
        ExprKind::Cond { .. } => STEP,
        // 9.4.3: `N(f(args)) = Σ cost(args) + N(body f)`; die Argumente
        // zaehlen unten als Kinder.
        ExprKind::Call { callee, .. } => ctx.function(*callee) + CostVec { call: 1, ..CostVec::ZERO },
        ExprKind::NativeCall { native, .. } => ctx.native(*native),
        ExprKind::Intrinsic { op: Intrinsic::Interp, args } => interp_cost(e, args, ctx),
        // 7.2: `fma` und `sqrt` haben eigene Gewichte. Ohne Befehl dafuer
        // ruft der Kern eine Bibliotheksfunktion; der Aufruf steckt im
        // gemessenen Gewicht.
        ExprKind::Intrinsic { op: Intrinsic::Fma, .. } => CostVec::heavy_op(Heavy::Fma, class(e, types)),
        ExprKind::Intrinsic { op: Intrinsic::Sqrt, .. } => CostVec::heavy_op(Heavy::Sqrt, class(e, types)),
        ExprKind::Intrinsic { .. } => class_of(e, types) + CostVec { call: 1, ..CostVec::ZERO },
        ExprKind::Checked { kind: CheckedKind::Range(r), .. } if r.origin == RangeOrigin::Proven => CostVec::ZERO,
        // Eine implizite Pruefung ist ein Vergleich und ein Sprung; die
        // Endlichkeit einer Matrix vergleicht jedes Element.
        ExprKind::Checked { .. } => match types.get(e.ty.index()) {
            Some(Type::Mat { rows, cols, .. }) => {
                CostVec::op(float_class(ctx)).times(u64::from(*rows) * u64::from(*cols))
            }
            _ => class_of(e, types),
        },
        ExprKind::Format(f) => format_cost(f, ctx),
        ExprKind::Matches { subject, kind, pattern, binding } => {
            let bind = if binding.is_some() { ctx.copy_always(subject.ty) } else { CostVec::ZERO };
            pattern_cost(*kind, pattern, Some(subject.ty), ctx) + bind
        }
        ExprKind::Decode { bytes, .. } => BYTE.times(ctx.text_len(Some(bytes.ty))) + ctx.copy(e.ty),
        ExprKind::Accessor { base, accessor, args } => accessor_cost(e, base, *accessor, args, ctx),
        _ => CostVec::ZERO,
    });
    // `children_mut` braucht `&mut`; hier reicht die lesende Entsprechung.
    e.children().iter().fold(own, |c, child| c + expr_cost(child, ctx))
}

/// `interp(t, x)` (3.9; 9.4.3 `O(n)` mit statischem `n`): Der Codegen
/// rechnet jedes Segment und waehlt ohne Sprung (`takt_llvm::expr::interp`)
/// — je Segment drei Differenzen, ein Produkt, eine Division, eine Summe
/// und ein Vergleich. Eine Tabelle, die nicht als Literal im Aufruf steht,
/// entstand aus einem; die laengste des Programms bindet sie.
fn interp_cost(e: &Expr, args: &[Expr], ctx: &Ctx<'_>) -> CostVec {
    let points = match args.first().map(|a| &a.kind) {
        Some(ExprKind::Array(points)) => points.len() as u64,
        Some(ExprKind::Param(p)) => match ctx.p.params.get(p.index()).map(|d| &d.default.kind) {
            Some(ExprKind::Array(points)) => points.len() as u64,
            _ => ctx.table,
        },
        _ => ctx.table,
    };
    let class = class(e, ctx.types());
    let segment = CostVec::op(class).times(5) + CostVec::heavy_op(Heavy::Div, class) + CostVec::op(CostClass::I32);
    segment.times(points.saturating_sub(1)) + CostVec::op(class).times(2)
}

/// Die Stuetzstellen der laengsten Tabelle im Programm (3.9).
fn longest_table(p: &Program) -> u64 {
    fn expr(e: &Expr, p: &Program, n: &mut u64) {
        if let ExprKind::Array(points) = &e.kind
            && matches!(p.types.list.get(e.ty.index()), Some(Type::Table { .. }))
        {
            *n = (*n).max(points.len() as u64);
        }
        for c in e.children() {
            expr(c, p, n);
        }
    }
    fn block(b: &Block, p: &Program, n: &mut u64) {
        b.walk(&mut |s| {
            for e in super::stmt_exprs(s) {
                expr(e, p, n);
            }
        });
    }
    let mut n = 0;
    for m in &p.machines {
        for b in m.blocks() {
            block(b, p, &mut n);
        }
        for v in m.vars.iter().filter_map(|v| v.init.as_ref()) {
            expr(v, p, &mut n);
        }
    }
    for f in &p.fns {
        block(&f.body, p, &mut n);
        for v in f.locals.iter().filter_map(|v| v.init.as_ref()) {
            expr(v, p, &mut n);
        }
    }
    for d in &p.params {
        expr(&d.default, p, &mut n);
    }
    n
}

/// Ein Zugriff, der rechnet: die Kennzahlen der Abtastwerte eines Ticks
/// (8.9), `get` auf einer `map` (3.9), Bitoperationen (3.10), Kodieren und
/// Textvergleiche. Die uebrigen lesen ein Feld.
fn accessor_cost(e: &Expr, base: &Expr, accessor: Accessor, args: &[Expr], ctx: &Ctx<'_>) -> CostVec {
    let class = class(e, ctx.types());
    match accessor {
        Accessor::Min | Accessor::Max | Accessor::Mean | Accessor::Rms => {
            let n = match ctx.ty(base.ty) {
                Some(Type::Samples { len, .. }) => u64::from(*len),
                _ => 1,
            };
            let each =
                CostVec::op(class) + BYTE + if accessor == Accessor::Rms { CostVec::op(class) } else { CostVec::ZERO };
            let divide = CostVec::heavy_op(Heavy::Div, class);
            let finish = match accessor {
                Accessor::Mean => divide,
                Accessor::Rms => divide + CostVec::heavy_op(Heavy::Sqrt, class),
                _ => CostVec::ZERO,
            };
            each.times(n) + finish
        }
        Accessor::Get => match ctx.ty(base.ty) {
            Some(Type::Map { key, value, cap }) => map_cost(*key, *value, *cap, false, ctx),
            _ => BYTE + STEP,
        },
        // 3.10: Umrechnen modulo 2^n und ein Bit lesen sind je eine
        // Operation, ein Bitfeld lesen oder setzen Schieben und Maske.
        Accessor::Wrap(_) | Accessor::Bit => CostVec::op(class),
        Accessor::Bits | Accessor::WithBit => CostVec::op(class).times(2),
        Accessor::Encode => BYTE.times(ctx.bytes(base.ty)),
        Accessor::StartsWith => MATCH_BYTE.times(args.first().map_or(0, |a| ctx.text_len(Some(a.ty)))),
        Accessor::Contains => {
            let (n, k) = (ctx.text_len(Some(base.ty)), args.first().map_or(0, |a| ctx.text_len(Some(a.ty))));
            MATCH_BYTE.times(k.min(n).saturating_mul(n + 1))
        }
        _ => CostVec::ZERO,
    }
}

/// Kosten einer Matrixoperation (3.11, 9.4.3) in der Breite von `float`,
/// gezaehlt, wie der Codegen sie ausfuehrt (`takt_llvm::matrix`):
/// elementweise R·C Operationen, ein Produkt R·C `fma`-Ketten ueber K, die
/// Verfahren ueber die LU und Cholesky wie unten. Die Pruefung auf
/// Endlichkeit steht als eigener Knoten darum.
fn mat_cost(e: &Expr, ctx: &Ctx<'_>) -> Option<CostVec> {
    let types = ctx.types();
    let dims = |ty: TypeId| match types.get(ty.index()) {
        Some(Type::Mat { rows, cols, .. }) => Some((u64::from(*rows), u64::from(*cols))),
        _ => None,
    };
    let class = float_class(ctx);
    let op = CostVec::op(class);
    Some(match &e.kind {
        ExprKind::Binary { op: BinaryOp::Mul, lhs, rhs } => match (dims(lhs.ty), dims(rhs.ty)) {
            (Some((r, k)), Some((_, c))) => CostVec::heavy_op(Heavy::Fma, class).times(r * c * k),
            (Some((r, c)), None) | (None, Some((r, c))) => op.times(r * c),
            _ => return None,
        },
        ExprKind::Binary { op: BinaryOp::Div, .. } => {
            let (r, c) = dims(e.ty)?;
            CostVec::heavy_op(Heavy::Div, class).times(r * c)
        }
        ExprKind::Binary { .. } => {
            let (r, c) = dims(e.ty)?;
            op.times(r * c)
        }
        ExprKind::MatOp { op: mat, args } => {
            let (n, k) = dims(args.first()?.ty)?;
            match mat {
                MatOp::Transpose => BYTE.times(n * k),
                // Das Vorzeichen aus den Vertauschungen, dann das Produkt
                // der Diagonale.
                MatOp::Det => lu_cost(n, class) + CostVec::op(CostClass::I32).times(3) + (BYTE + op).times(n),
                // Die Einheitsmatrix Spalte fuer Spalte eingesetzt, das
                // Ergebnis aus dem Scratch geladen.
                MatOp::Inv => lu_cost(n, class) + lu_solve_cost(n, n, class) + BYTE.times(n * n),
                MatOp::Solve => {
                    let (_, c) = dims(args.get(1)?.ty)?;
                    lu_cost(n, class) + lu_solve_cost(n, c, class) + BYTE.times(2 * n * c)
                }
                MatOp::Cholesky => cholesky_cost(n, class),
            }
        }
        _ => return None,
    })
}

/// Die LU-Zerlegung wie `takt_llvm::matrix::lu`: Die Matrix und die
/// Permutation in den Scratch; je Spalte die Pivotsuche (je Kandidat
/// laden, Betrag, Vergleich, zwei `select`), der Test auf null, der Tausch
/// zweier Zeilen und ihrer Permutation samt Zaehler, dann je Zeile darunter
/// eine Division und eine Negation und je Spalte rechts davon ein `fma`
/// mit zwei Lese- und einem Schreibzugriff.
fn lu_cost(n: u64, class: CostClass) -> CostVec {
    let (op, int) = (CostVec::op(class), CostVec::op(CostClass::I32));
    let (fma, div) = (CostVec::heavy_op(Heavy::Fma, class), CostVec::heavy_op(Heavy::Div, class));
    let mut c = BYTE.times(n * n + n);
    for k in 0..n {
        let below = n - k - 1;
        let search = BYTE + op + (BYTE + op.times(2) + int.times(2)).times(below);
        let swap = op + int.times(2) + BYTE.times(4 * n + 4) + int.times(3);
        let eliminate = BYTE + (BYTE.times(2) + div + op + (BYTE.times(3) + fma).times(below)).times(below);
        c = c + search + swap + eliminate;
    }
    c
}

/// Vorwaerts- und Rueckwaertseinsetzen wie `takt_llvm::matrix::lu_solve`,
/// je Loesungsspalte: je Zeile die rechte Seite ueber die Permutation, je
/// Eintrag neben der Diagonale zwei Lesezugriffe, eine Negation und ein
/// `fma`, rueckwaerts dazu die Division durch die Diagonale.
fn lu_solve_cost(n: u64, cols: u64, class: CostClass) -> CostVec {
    let entry = BYTE.times(2) + CostVec::op(class) + CostVec::heavy_op(Heavy::Fma, class);
    let triangle = n * n.saturating_sub(1) / 2;
    let forward = (BYTE.times(3) + CostVec::op(CostClass::I32).times(2)).times(n) + entry.times(triangle);
    let backward = (BYTE.times(3) + CostVec::heavy_op(Heavy::Div, class)).times(n) + entry.times(triangle);
    (forward + backward).times(cols)
}

/// Cholesky wie `takt_llvm::matrix::cholesky`: Nullen in den Scratch; je
/// Spalte die Diagonale als `fma`-Kette ueber die Eintraege links davon,
/// ihr Vorzeichen und die Wurzel, dann je Zeile darunter eine Kette und
/// eine Division; zuletzt das Ergebnis laden.
fn cholesky_cost(n: u64, class: CostClass) -> CostVec {
    let op = CostVec::op(class);
    let fma = CostVec::heavy_op(Heavy::Fma, class);
    let mut c = BYTE.times(2 * n * n);
    for j in 0..n {
        let diagonal = (BYTE + op + fma).times(j)
            + op
            + CostVec::op(CostClass::I32)
            + CostVec::heavy_op(Heavy::Sqrt, class)
            + BYTE;
        let column = (BYTE.times(2) + op + fma).times(j) + CostVec::heavy_op(Heavy::Div, class) + BYTE;
        c = c + diagonal + column.times(n - j - 1);
    }
    c
}

/// Die Operationsklasse eines Ausdrucks nach Typ und gewaehlter Darstellung.
fn class(e: &Expr, types: &[Type]) -> CostClass {
    match types.get(e.ty.index()) {
        Some(Type::Float { width: FloatWidth::F32, .. }) => CostClass::F32,
        Some(Type::Float { width: FloatWidth::F64, .. }) => CostClass::F64,
        Some(Type::Int { .. } | Type::Duration { .. }) => match e.repr {
            // 9.4.3: „eine Integer-Operation nach ihrer gewaehlten
            // Darstellung in `i32` oder `i64` (3.4)".
            Some(Repr::I32) => CostClass::I32,
            _ => CostClass::I64,
        },
        _ => CostClass::I32,
    }
}

/// Die Klasse der Breite von `float` (4.2): Matrizen rechnen in ihr (3.11).
fn float_class(ctx: &Ctx<'_>) -> CostClass {
    match ctx.p.config.float_width {
        FloatWidth::F32 => CostClass::F32,
        FloatWidth::F64 => CostClass::F64,
    }
}

/// Eine Operation in der Klasse eines Ausdrucks.
fn class_of(e: &Expr, types: &[Type]) -> CostVec {
    CostVec::op(class(e, types))
}

/// Statische Schranke einer `for`-Schleife ueber einen Behaelter.
fn capacity(iter: &Expr, types: &[Type]) -> u64 {
    match types.get(iter.ty.index()) {
        Some(Type::Array { len, .. } | Type::Samples { len, .. }) => u64::from(*len),
        Some(Type::Vec { cap, .. } | Type::Bytes { cap } | Type::Map { cap, .. }) => u64::from(*cap),
        _ => 1,
    }
}
