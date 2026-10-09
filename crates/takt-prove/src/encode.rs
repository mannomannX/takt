//! Die Schrittfunktion als Transitionssystem (plan/m6.md 2.8): je Maschine
//! Blatt, `FAULTED`, Abort-Latch, Aktivierungszaehler, Timer je Zustand,
//! Variablen und Signale; je Output der Latch. Ein Tick ist die
//! symbolische Ausfuehrung des Interpreters (`takt-interp/src/machine.rs`):
//! `loop:`-Bloecke der Kette, Uebergaenge outer-first, `resolve_m` mit
//! Fault-Pfad und `switch`, Abort-Phase, Zaehler, Commit. Zweige werden
//! mit `ite` gemischt, jede Zuweisung unter der Bedingung, dass die
//! Ausfuehrung sie noch erreicht (`alive`).
//!
//! **Reichweite ehrlich.** Was die Kodierung nicht abbildet — Stroeme,
//! Handler, Jobs, `every`, `at`, Sammlungen, Zeitoperatoren in Formeln —
//! meldet sie beim Namen statt es zu naehern; was sie annimmt, steht als
//! Notiz im Modell und im Urteil.
//!
//! **Der Rand gehoert zum Modell** (3.5, 12.6, FB-372). Jeder Input traegt
//! je Tick eine freie Qualitaet: `Good` liegt in der Range und innerhalb
//! `max_slew` zum letzten guten Wert, `Suspect` haelt ihn und entsteht nur
//! durch eine echte Verletzung unter `debounce`, `Stale` und `Bad` sind
//! jederzeit moeglich. Ein ungueltiger Input faultet beim Lesen wie im
//! Interpreter (`SensorFault`); `.valid`, `.or`, `.suspect` und `.stale`
//! lesen die Qualitaet. Tunables sind je Tick frei in ihrer Range (8.4).
//!
//! **Jede implizite Pruefung ist ein Fault-Zweig** (4.1): Range, Division,
//! Endlichkeit, Ueberlauf in der Breite des Ergebnisses, Schiebebetrag,
//! Konversion und Definitionsbereich, wie der Interpreter sie ausloest.
//! Ganzzahlen rechnen in 64 Bit mit Vorzeichen; ein `u64` laege jenseits
//! davon und ist nicht kodiert.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Not;

use takt_diag::Span;
use takt_mir::expr::{
    BinaryOp, Builtin, CheckedKind, ConvertKind, Expr, ExprKind, Intrinsic, TProp, TemporalOp, UnaryOp,
};
use takt_mir::fns::BlockDef;
use takt_mir::machine::{ArithKind, FaultKind, FaultTarget, Machine, Target, TransTrigger};
use takt_mir::program::{Direction, Program, Property};
use takt_mir::stmt::{Block, ForVars, Method, Place, StmtKind};
use takt_mir::types::{Const, FloatWidth, HandleKind, IntWidth, Type};
use takt_mir::{BlockId, ChannelId, CommandId, MachineId, StateId, TypeId, VarId};

use crate::eval;
use crate::term::{Fun, Node, Op, Rounding, Sort, Term};

mod canon;
mod fault;
mod map;
mod monitor;
mod pattern;
mod sched;
mod stream;
mod text;
mod tx;
mod value;
mod wire;
use fault::Cause;
use monitor::Monitor;
use value::V;

/// Etwas, das die Kodierung nicht abbildet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    /// Was.
    pub what: String,
    /// Wo.
    pub span: Span,
}

type R<T> = Result<T, Unsupported>;

fn no<T>(what: impl Into<String>, span: Span) -> R<T> {
    Err(Unsupported { what: what.into(), span })
}

/// Eine Zustandsvariable des Systems.
#[derive(Clone, Debug)]
pub struct StateVar {
    /// Name `s.…`.
    pub name: String,
    /// Sorte.
    pub sort: Sort,
    /// Anfangswert ueber den Eingaben des Ticks 0.
    pub init: Term,
    /// Naechster Wert ueber dem Zustand und den Eingaben des naechsten Ticks.
    pub next: Term,
}

/// Eine Eigenschaft als Invariante ueber Zustand und Eingaben eines Ticks.
#[derive(Clone, Debug)]
pub struct Goal {
    /// Name.
    pub name: String,
    /// `assumption` statt `property`.
    pub assumption: bool,
    /// Die Formel.
    pub formula: Term,
}

/// Eine Pruefstelle (`check`/`expect`) als Beweisziel (plan/m6.md 2.8, B3):
/// „sie feuert nie".
#[derive(Clone, Debug)]
pub struct CheckSite {
    /// Anfang der Anweisung im Quelltext; so heisst die Stelle auch in der
    /// Coverage (`check @<start>`).
    pub start: u32,
    /// Position.
    pub span: Span,
    /// Die Maschine.
    pub machine: String,
    /// `check` oder `expect`.
    pub kind: String,
    /// Feuert im Tick 0, ueber den Eingaben des Ticks 0.
    pub init: Term,
    /// Feuert im Uebergang, ueber dem Zustand davor und den Eingaben danach.
    pub fires: Term,
}

/// Der Vertrag eines Blocks als Beweisziel (5.7, B2): aus jedem
/// typkonformen Zustand haelt ein Schritt unter `requires` sein `ensures`.
#[derive(Clone, Debug)]
pub struct ContractGoal {
    /// Der Block.
    pub block: String,
    /// Position des `step`.
    pub span: Span,
    /// Freie Variablen `c.…`: Parameter, Zustand, Schrittparameter.
    pub vars: Vec<(String, Sort)>,
    /// Erfuellbar genau dann, wenn der Vertrag verletzt sein kann.
    pub violation: Term,
}

/// Das Transitionssystem.
#[derive(Clone, Debug, Default)]
pub struct Model {
    /// Zustand, nach Namen sortiert.
    pub state: Vec<StateVar>,
    /// Eingaben je Tick: `i.<channel>`, `i.cmd.<command>`.
    pub inputs: Vec<(String, Sort)>,
    /// Bedingungen der freien Eingaben je Tick: der Rand der Inputs (3.5),
    /// Ψ, fremde Outputs und Tunables in ihrem Typ. Sie sind Modell, keine
    /// Annahme — der Rand erzwingt sie (12.6), und was frei ist, ist es
    /// hoechstens mehr als im Lauf.
    pub assumptions: Vec<Term>,
    /// Invarianten des Zustands aus den Typen (3.4): Ranges, Enums, Blaetter,
    /// Zaehler — sie gelten in jedem erreichbaren Zustand, weil ein Wert
    /// ausserhalb faultet statt gespeichert zu werden.
    pub invariants: Vec<Term>,
    /// Beweisziele. Eine `assumption` (13.3) ist zugleich Annahme der
    /// Eigenschaften: Sie beschraenkt deren Beweisverpflichtung, nicht die
    /// Typsicherheit (3.4) — eine Pruefstelle wird ohne sie klassifiziert,
    /// sonst liesse eine Beweisdatei eine Pruefung aus, die ein Lauf
    /// jenseits der Annahme braucht.
    pub properties: Vec<Goal>,
    /// Pruefstellen als Beweisziele (B3).
    pub checks: Vec<CheckSite>,
    /// Block-Vertraege als Beweisziele (B2).
    pub contracts: Vec<ContractGoal>,
    /// Was die Kodierung annimmt oder auslaesst.
    pub notes: Vec<String>,
    /// Je Maschine die Zustandscodes mit Namen.
    pub leaves: BTreeMap<String, Vec<(i64, String)>>,
    /// Die laengste Frist eines `after` und das laengste Fenster eines
    /// Monitors, in Ticks (FB-375): Eine k-Induktion mit kleinerem k sieht
    /// einen Zeitablauf nicht ganz.
    pub horizon: u32,
    /// Kandidaten fuer Hilfslemmata (FB-375): Beziehungen zwischen der Zeit
    /// eines aktiven Blatts und den Zaehlern, die mit ihr laufen. Welche
    /// gelten, entscheidet `solve::lemmas` nach Houdini.
    pub candidates: Vec<Term>,
    /// Die Funktionen aus `libtaktm`, die der Solver uninterpretiert sieht
    /// (4.2): Ein Pfad ueber sie kann an ihrem wahren Wert scheitern.
    pub uninterpreted: Vec<String>,
}

impl Model {
    /// Der Name eines Blattcodes.
    pub fn leaf_name(&self, machine: &str, code: i64) -> Option<&str> {
        self.leaves.get(machine)?.iter().find(|(c, _)| *c == code).map(|(_, n)| n.as_str())
    }

    /// Fuehrt das Modell konkret aus: `input(step, name)` liefert die
    /// Eingaben, fehlende sind null. Ergebnis: der Zustand je Schritt.
    pub fn simulate(&self, steps: u64, input: &dyn Fn(u64, &str) -> Option<eval::Val>) -> Vec<eval::Env> {
        let mut out = Vec::new();
        let mut env = eval::Env::new();
        for (name, sort) in &self.inputs {
            env.insert(name.clone(), input(0, name).unwrap_or(eval::Val::zero(*sort)));
        }
        let names = || self.state.iter().map(|v| v.name.clone());
        let mut state: eval::Env = names().zip(eval::eval_all(self.state.iter().map(|v| &v.init), &env)).collect();
        out.push(state.clone());
        for k in 1..=steps {
            let mut env = state.clone();
            for (name, sort) in &self.inputs {
                env.insert(name.clone(), input(k, name).unwrap_or(eval::Val::zero(*sort)));
            }
            state = names().zip(eval::eval_all(self.state.iter().map(|v| &v.next), &env)).collect();
            out.push(state.clone());
        }
        out
    }
}

/// Werte der Orte.
type Env = BTreeMap<String, Term>;

/// Ein Wert, der sich nach einer Bedingung waehlen laesst.
trait Choose {
    fn choose(c: &Term, a: Self, b: Self) -> Self;
}

impl Choose for Term {
    fn choose(c: &Term, a: Term, b: Term) -> Term {
        Term::ite(c.clone(), a, b)
    }
}

impl Choose for V {
    fn choose(c: &Term, a: V, b: V) -> V {
        V::ite(c, a, b)
    }
}

fn ite_env(c: &Term, a: &Env, b: &Env) -> Env {
    a.iter()
        .map(|(k, va)| {
            let vb = b.get(k).unwrap_or(va);
            (k.clone(), Term::ite(c.clone(), va.clone(), vb.clone()))
        })
        .collect()
}

/// Wie ein Block endet.
#[derive(Clone, Debug)]
enum ExitKind {
    /// Ein Fault mit seiner Ursache, mit explizitem Ziel (`check … -> X`)
    /// oder dem Fault-Ziel des Blatts.
    Fault(Option<Target>, Cause),
    /// `abort`.
    Abort(Cause),
    /// `-> q`.
    Goto(Target),
}

#[derive(Clone, Debug)]
struct Exit {
    cond: Term,
    kind: ExitKind,
}

/// Kontrollfluss eines Blocks: `alive` heisst, die Ausfuehrung erreicht
/// die naechste Anweisung; die Ausgaenge sind paarweise exklusiv.
#[derive(Clone, Debug)]
struct Flow {
    alive: Term,
    exits: Vec<Exit>,
}

impl Flow {
    fn new(alive: Term) -> Flow {
        Flow { alive, exits: Vec::new() }
    }
}

/// Modus eines Blocks (9.3).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Run,
    Entry,
}

/// Kontext einer Auswertung.
struct Cx<'a> {
    /// Die Maschine; `None` in einer Eigenschaft.
    m: Option<MachineId>,
    /// Das Blatt, dessen Zweig entsteht.
    leaf: Option<StateId>,
    mode: Mode,
    /// Zustand am Tick-Anfang (Ψ_k, committete Outputs).
    pre: &'a Env,
    /// Ist eine Maschine in diesem Tick aktiv (fuer frische Lesevorgaenge).
    active: &'a BTreeMap<MachineId, Term>,
    /// Lokale einer eingebetteten Funktion.
    locals: Option<BTreeMap<VarId, V>>,
}

impl Cx<'_> {
    /// Derselbe Kontext im Entry-Modus (9.3).
    fn entry(&self) -> Cx<'_> {
        Cx {
            m: self.m,
            leaf: self.leaf,
            mode: Mode::Entry,
            pre: self.pre,
            active: self.active,
            locals: self.locals.clone(),
        }
    }
}

/// Wie viele Schleifendurchlaeufe die Kodierung eines Pfads ausrollt: des
/// Starts einer Maschine oder ihres Ticks aus einem Blatt. Jeder Durchlauf
/// vertieft die Terme der Variablen, die er schreibt; nichts rekursiert
/// ueber diese Tiefe (`term::post_order`), die Grenze haelt das Modell in
/// der Groesse, die Auswertung und Solver tragen (FB-403).
pub const UNROLL_LIMIT: i64 = 4096;

/// Der Kodierer.
struct Enc<'p> {
    p: &'p Program,
    order: Vec<MachineId>,
    inputs: BTreeMap<String, Sort>,
    notes: Vec<String>,
    /// `abort` in diesem Tick, je Stelle.
    aborts: Vec<Term>,
    /// Feuerbedingungen der Pruefstellen der laufenden Phase, je Stelle.
    sites: BTreeMap<SiteKey, Vec<Term>>,
    /// Position und Maschine je Pruefstelle.
    site_info: BTreeMap<SiteKey, (Span, String)>,
    /// Nur diese Maschine (13.3): Ψ, fremde Outputs und ein fremder `abort`
    /// sind freie Eingaben je Tick.
    scope: Option<MachineId>,
    /// Was die Typen ueber freie Ψ-Eingaben sagen.
    psi_assumptions: Vec<Term>,
    psi_seen: BTreeSet<String>,
    /// Was die Typen ueber Tunables sagen; je Tunable einmal.
    tune_assumptions: Vec<Term>,
    tunes_seen: BTreeSet<String>,
    /// Ausgerollte Schleifendurchlaeufe des laufenden Pfads ([`UNROLL_LIMIT`]).
    unrolled: i64,
    /// Die Typen der Lokalen des Rumpfs, der gerade eingebettet wird.
    local_types: BTreeMap<VarId, TypeId>,
    /// Die Monitore der Eigenschaften mit Zeitoperatoren (13.3).
    monitors: Vec<Monitor>,
    /// Grenzen der Zaehler von `check … for`: Ort und Frist in Nanosekunden.
    confirms: Vec<(String, i64)>,
    /// `now` des Ticks, der gerade kodiert wird.
    now: Term,
    /// Die Stroeme des Modells (8.6).
    streams: Vec<stream::Stream>,
    /// Die Ausgabestroeme der kodierten Maschinen (8.8).
    txs: Vec<tx::Tx>,
    /// Der Zustand nach dem Zustellen des laufenden Ticks: Aus ihm stehen
    /// die Fenster fest (9.6).
    delivered: Env,
    /// Die Fenster des laufenden Ticks je Leser und Cursor.
    windows: BTreeMap<(MachineId, usize), stream::Window>,
    /// Was die Konstrukte des laufenden Ticks untersucht haben.
    marks: Vec<stream::Mark>,
    /// Die `send` des laufenden Ticks auf interne Stroeme, in ihrer Reihenfolge.
    queued: Vec<stream::Queued>,
    /// Je offene Schleife, wo ein `break` sie verlaesst.
    breaks: Vec<Vec<Term>>,
    /// Bindungen von `matches … as m` im laufenden Ausdruck: Ort, Typ, Wert
    /// und wo sie gelten; Lesevorgaenge sehen sie, bevor sie im Zustand stehen.
    binds: Vec<(String, TypeId, V, Term)>,
    /// Die uninterpretierten Funktionen des Modells.
    uninterpreted: BTreeSet<String>,
    /// Die Maschinen, die `last_fault` lesen (5.3).
    last_fault: BTreeSet<MachineId>,
    /// Die Indizes der ausgerollten Schleifen um die laufende Anweisung,
    /// aussen zuerst: Der Interpreter fuehrt die Zaehler von `every` und
    /// `check … for` je Durchlauf (`Counters::at`).
    loop_path: Vec<i64>,
    /// Je Maschine die Indexpfade ihrer Zaehler.
    counter_paths: BTreeMap<MachineId, CounterPaths>,
}

/// Unter welchen Indexpfaden die Zaehler einer Maschine laufen: je
/// `every`-Zaehler und je Stelle von `check … for`.
#[derive(Clone, Debug, Default)]
struct CounterPaths {
    every: Vec<Vec<Vec<i64>>>,
    viol: Vec<Vec<Vec<i64>>>,
}

/// Eine Pruefstelle: Anfang, Ende und Art. Zwei Pruefungen koennen denselben
/// Anfang haben, die Validitaet von `x` und die Endlichkeit von `x - y`
/// (FB-393).
type SiteKey = (u32, u32, String);

/// Was der Rand eines Inputs aus seinen Lieferungen macht (3.5, 12.6).
#[derive(Clone, Debug)]
pub struct Edge {
    /// Der Kanal.
    pub channel: ChannelId,
    /// Sein Name.
    pub name: String,
    sort: Sort,
    /// Die Range einer guten Lieferung.
    range: Option<(Term, Term)>,
    /// Laesst sich die Range mit einem darstellbaren Wert verletzen? Nur dann
    /// kann sie eine Lieferung `Suspect` machen.
    violable: bool,
    /// Ein solcher Wert, fuer den Stimulus.
    outside: Option<eval::Val>,
    /// `max_slew` mal Tickdauer, je Tick seit dem letzten guten Wert.
    slew: Option<f64>,
    /// `debounce`.
    debounce: u32,
}

impl Edge {
    /// Kann eine Lieferung `Suspect` werden? Nur durch eine Verletzung unter
    /// `debounce` (3.5).
    pub fn suspect(&self) -> bool {
        self.debounce > 0 && (self.violable || self.slew.is_some())
    }

    /// Braucht der Rand Zustand zwischen den Ticks: den letzten guten Wert?
    fn stateful(&self) -> bool {
        self.suspect() || self.slew.is_some()
    }

    /// Was `max_slew` je Tick an Aenderung zulaesst, um acht ulp geweitet:
    /// Der Spielraum waechst durch Addition, und gerundet darf er nie enger
    /// sein als der Rand.
    fn per_tick(&self) -> Option<f64> {
        self.slew.map(|s| s * (1.0 + 8.0 * f64::EPSILON))
    }

    /// Die Teile des Zustands: der letzte gute Wert, ob es ihn gibt, die
    /// Verletzungen in Folge und der Spielraum fuer `max_slew`.
    fn parts(&self) -> &'static [&'static str] {
        if self.slew.is_some() { &["has", "good", "strikes", "room"] } else { &["has", "good", "strikes"] }
    }

    fn loc(&self, part: &str) -> String {
        format!("s.q.{}.{part}", self.name)
    }

    /// Eine Lieferung, die der Rand als Verletzung zaehlt: ausserhalb der
    /// Range, sonst jenseits `max_slew` zum letzten guten Wert `last`, der
    /// `since` Ticks zurueckliegt. Daraus wird unter `debounce` `Suspect`.
    pub fn violation(&self, last: Option<&eval::Val>, since: i64) -> Option<eval::Val> {
        if let Some(v) = &self.outside {
            return Some(*v);
        }
        let jump = 2.0 * self.slew? * (since + 1) as f64 + 1.0;
        Some(match last? {
            eval::Val::Int(x) => eval::Val::Int(x.saturating_add(jump.ceil() as i64)),
            eval::Val::F64(x) => eval::Val::F64(x + jump),
            eval::Val::F32(x) => eval::Val::F32(x + jump as f32),
            eval::Val::Bool(_) => return None,
        })
    }
}

/// Die Qualitaet einer Lieferung als Code im Modell (3.5).
pub mod quality {
    /// `Good`.
    pub const GOOD: i64 = 0;
    /// `Suspect`.
    pub const SUSPECT: i64 = 1;
    /// `Stale`.
    pub const STALE: i64 = 2;
    /// `Bad`.
    pub const BAD: i64 = 3;
}

/// Kodiert ein Programm.
pub fn encode(p: &Program) -> R<Model> {
    encode_with(p, None)
}

/// Kodiert eine Maschine allein (13.3): Ψ-Lesevorgaenge, fremde Outputs
/// und ein fremder `abort` sind freie Eingaben unter den Annahmen ihrer
/// Typen — mehr Verhalten als das Ganze, darum bleibt „unerreichbar" wahr.
pub fn encode_machine(p: &Program, m: MachineId) -> R<Model> {
    encode_with(p, Some(m))
}

fn encode_with(p: &Program, scope: Option<MachineId>) -> R<Model> {
    let order = match scope {
        Some(m) => vec![m],
        None => takt_mir::analysis::schedule::order(p).unwrap_or_else(|_| takt_mir::analysis::schedule::runnable(p)),
    };
    let mut enc = Enc::new(p, order, scope);
    enc.check_reach()?;
    enc.last_fault = enc.last_fault_readers().into_iter().collect();
    enc.txs = enc.tx_defs()?;
    enc.streams = enc.stream_defs()?;
    // Eigenschaften gehoeren zum Ganzen (13.3); eine Maschine allein hat keine.
    if scope.is_none() {
        enc.monitors = enc.monitors()?;
    }
    let init = enc.init()?;
    let init_sites = std::mem::take(&mut enc.sites);
    let pre: Env = init.keys().map(|k| (k.clone(), Term::var(k.clone(), init[k].sort()))).collect();
    let next = enc.tick(&pre)?;
    let tick_sites = std::mem::take(&mut enc.sites);
    let invariants = enc.state_invariants(&pre);
    let candidates = enc.candidates(&pre);
    let contracts = enc.contract_goals()?;
    let checks: Vec<CheckSite> = enc
        .site_info
        .iter()
        .map(|(key, (span, machine))| CheckSite {
            start: span.start,
            span: *span,
            machine: machine.clone(),
            kind: key.2.clone(),
            init: Term::or(init_sites.get(key).cloned().unwrap_or_default()),
            fires: Term::or(tick_sites.get(key).cloned().unwrap_or_default()),
        })
        .collect();
    let mut properties = Vec::new();
    let mut assumptions = enc.channel_assumptions()?;
    assumptions.extend(enc.psi_assumptions.clone());
    assumptions.extend(enc.tune_assumptions.clone());
    assumptions.extend(enc.stream_assumptions()?);
    let scoped = enc.scope.is_some();
    for prop in p.properties.iter().filter(|_| !scoped) {
        match enc.goal(prop, &pre)? {
            Some(goal) => properties.push(goal),
            None => enc
                .notes
                .push(format!("`{}` nicht kodiert: nur `always(…)`/`never(…)` ohne Zeitoperatoren (2.8)", prop.name)),
        }
    }
    let state: Vec<StateVar> = init
        .iter()
        .map(|(name, i)| StateVar { name: name.clone(), sort: i.sort(), init: i.clone(), next: next[name].clone() })
        .collect();
    let mut leaves = BTreeMap::new();
    for &id in &enc.order {
        let m = &p.machines[id.index()];
        let mut codes: Vec<(i64, String)> =
            m.states.iter().enumerate().map(|(i, s)| (enc.code(id, StateId(i as u32)), s.name.clone())).collect();
        if let Some(c) = enc.faulted_code(id) {
            codes.push((c, "FAULTED".to_string()));
        }
        leaves.insert(m.name.clone(), codes);
    }
    let horizon = enc.horizon()?;
    let mut notes = enc.notes;
    notes.sort();
    notes.dedup();
    Ok(Model {
        state,
        inputs: enc.inputs.into_iter().collect(),
        assumptions,
        invariants,
        properties,
        checks,
        contracts,
        notes,
        leaves,
        horizon,
        candidates,
        uninterpreted: enc.uninterpreted.into_iter().collect(),
    })
}

/// Die Raender der Inputs eines Programms, wie das Modell sie kodiert: Der
/// Stimulus eines Gegenbeispiels braucht sie, um eine Qualitaet so zu
/// liefern, wie der Rand des Interpreters sie entstehen laesst.
pub fn input_edges(p: &Program) -> R<Vec<Edge>> {
    Enc::new(p, Vec::new(), None).edges()
}

impl<'p> Enc<'p> {
    fn new(p: &'p Program, order: Vec<MachineId>, scope: Option<MachineId>) -> Enc<'p> {
        Enc {
            p,
            order,
            inputs: BTreeMap::new(),
            notes: Vec::new(),
            aborts: Vec::new(),
            sites: BTreeMap::new(),
            site_info: BTreeMap::new(),
            scope,
            psi_assumptions: Vec::new(),
            psi_seen: BTreeSet::new(),
            tune_assumptions: Vec::new(),
            tunes_seen: BTreeSet::new(),
            unrolled: 0,
            local_types: BTreeMap::new(),
            monitors: Vec::new(),
            confirms: Vec::new(),
            now: Term::int(0),
            streams: Vec::new(),
            txs: Vec::new(),
            delivered: Env::new(),
            windows: BTreeMap::new(),
            marks: Vec::new(),
            queued: Vec::new(),
            breaks: Vec::new(),
            binds: Vec::new(),
            uninterpreted: BTreeSet::new(),
            last_fault: BTreeSet::new(),
            loop_path: Vec::new(),
            counter_paths: BTreeMap::new(),
        }
    }
}

impl Enc<'_> {
    fn machine(&self, m: MachineId) -> &Machine {
        &self.p.machines[m.index()]
    }

    fn note(&mut self, text: &str) {
        if !self.notes.iter().any(|n| n == text) {
            self.notes.push(text.to_string());
        }
    }

    /// Was das Modell grundsaetzlich nicht abbildet.
    fn check_reach(&self) -> R<()> {
        for &id in &self.order {
            let m = self.machine(id);
            if !m.layout.job_slots.is_empty() {
                return no("Jobs", m.span);
            }
            if !m.faulted.transitions.is_empty() {
                return no("Uebergaenge aus FAULTED", m.span);
            }
            if m.states.iter().any(|s| !s.instances.is_empty()) {
                return no("gescopte Instanzen", m.span);
            }
        }
        Ok(())
    }

    /// Eine freie Ψ-Eingabe (13.3); `true`, wenn sie neu ist.
    fn free_psi(&mut self, target: MachineId, loc: &str, sort: Sort) -> (Term, bool) {
        let name = format!("i.psi.{}", loc.trim_start_matches("s."));
        let fresh = self.psi_seen.insert(name.clone());
        let owner = self.machine(target).name.clone();
        self.note(&format!("Ψ aus `{owner}` ist eine freie Eingabe je Tick (13.3)"));
        (self.input(name, sort), fresh)
    }

    fn sort_of(&self, ty: TypeId, span: Span) -> R<Sort> {
        match self.p.types.get(ty) {
            Type::Bool => Ok(Sort::Bool),
            Type::Int { width: IntWidth::U64, .. } => no(U64, span),
            Type::Int { .. } | Type::Duration { .. } | Type::Enum(_) => Ok(Sort::Int),
            Type::Float { width: FloatWidth::F32, .. } => Ok(Sort::F32),
            Type::Float { width: FloatWidth::F64, .. } => Ok(Sort::F64),
            other => no(format!("Typ {other:?}"), span),
        }
    }

    /// Die Breite eines ganzzahligen Typs; eine Dauer rechnet in `i64` (3.3).
    fn int_width(&self, ty: TypeId, span: Span) -> R<Option<IntWidth>> {
        match self.p.types.get(ty) {
            Type::Int { width: IntWidth::U64, .. } => no(U64, span),
            Type::Int { width, .. } => Ok(Some(*width)),
            Type::Duration { .. } => Ok(Some(IntWidth::I64)),
            _ => Ok(None),
        }
    }

    fn zero(sort: Sort) -> Term {
        match sort {
            Sort::Bool => Term::bool(false),
            Sort::Int => Term::int(0),
            s => Term::float(0.0, s),
        }
    }

    // ------------------------------------------------------------ Orte

    fn loc_leaf(&self, m: MachineId) -> String {
        format!("s.{}.leaf", self.machine(m).name)
    }
    fn loc_faulted(&self, m: MachineId) -> String {
        format!("s.{}.faulted", self.machine(m).name)
    }
    fn loc_latched(&self, m: MachineId) -> String {
        format!("s.{}.latched", self.machine(m).name)
    }
    fn loc_countdown(&self, m: MachineId) -> String {
        format!("s.{}.countdown", self.machine(m).name)
    }
    fn loc_timer(&self, m: MachineId, s: StateId) -> String {
        format!("s.{}.t.{}", self.machine(m).name, self.machine(m).states[s.index()].name)
    }
    fn loc_var(&self, m: MachineId, v: VarId) -> String {
        format!("s.{}.v.{}", self.machine(m).name, self.machine(m).vars[v.index()].name)
    }
    fn loc_sig(&self, m: MachineId, s: usize) -> String {
        format!("s.{}.sig.{}", self.machine(m).name, self.machine(m).signals[s].name)
    }
    /// Der Bestaetigungszaehler einer Stelle von `check … for` (5.6) im
    /// Durchlauf `path`, in ns.
    fn loc_viol(&self, m: MachineId, site: usize, path: &[i64]) -> String {
        at_path(format!("s.{}.viol.{site}", self.machine(m).name), path)
    }
    /// Der naechste Zeitpunkt eines `every` (5.8) im Durchlauf `path`, in
    /// ns; `-1`, bis er zum ersten Mal gelesen wird.
    fn loc_every(&self, m: MachineId, counter: usize, path: &[i64]) -> String {
        at_path(format!("s.{}.every.{counter}", self.machine(m).name), path)
    }

    /// Die Indexpfade der Zaehler einer Maschine: die Durchlaeufe aller
    /// Schleifen um ihre Stelle (Schleifen sind ausgerollt, ihre Laenge steht
    /// fest).
    fn paths_of(&mut self, m: MachineId) -> R<CounterPaths> {
        if let Some(p) = self.counter_paths.get(&m) {
            return Ok(p.clone());
        }
        let machine = self.machine(m).clone();
        let mut out = CounterPaths {
            every: vec![Vec::new(); machine.layout.every_counters.len()],
            viol: vec![Vec::new(); machine.layout.viol_sites.len()],
        };
        let mut blocks: Vec<&Block> = vec![&machine.loop_block];
        blocks.extend(machine.handlers.iter().map(|h| &h.body));
        for s in &machine.states {
            blocks.extend([&s.enter, &s.exit, &s.loop_block]);
            blocks.extend(s.handlers.iter().map(|h| &h.body));
            blocks.extend(s.transitions.iter().map(|t| &t.actions));
        }
        for b in blocks {
            self.walk_counters(b, &mut Vec::new(), &mut out)?;
        }
        self.counter_paths.insert(m, out.clone());
        Ok(out)
    }

    fn walk_counters(&mut self, b: &Block, counts: &mut Vec<i64>, out: &mut CounterPaths) -> R<()> {
        let record = |list: &mut Vec<Vec<i64>>, counts: &[i64]| {
            let mut paths: Vec<Vec<i64>> = vec![Vec::new()];
            for &n in counts {
                paths =
                    paths.into_iter().flat_map(|p| (0..n.max(0)).map(move |i| [p.clone(), vec![i]].concat())).collect();
            }
            for p in paths {
                if !list.contains(&p) {
                    list.push(p);
                }
            }
        };
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Every { counter, body, .. } => {
                    record(&mut out.every[counter.index()], counts);
                    self.walk_counters(body, counts, out)?;
                }
                StmtKind::Check { confirm: Some(c), .. } => record(&mut out.viol[c.site.0 as usize], counts),
                StmtKind::If { then, otherwise, .. } => {
                    self.walk_counters(then, counts, out)?;
                    self.walk_counters(otherwise, counts, out)?;
                }
                StmtKind::Match { arms, .. } => {
                    for a in arms {
                        self.walk_counters(&a.body, counts, out)?;
                    }
                }
                StmtKind::At { body, .. } => self.walk_counters(body, counts, out)?,
                StmtKind::ForRange { count, body, .. } => {
                    counts.push(self.const_int(count)?);
                    self.walk_counters(body, counts, out)?;
                    counts.pop();
                }
                StmtKind::ForEach { iter, body, .. } => {
                    let n = match self.p.types.get(iter.ty) {
                        Type::Array { len, .. } => i64::from(*len),
                        Type::Bytes { cap } | Type::Vec { cap, .. } => i64::from(*cap),
                        // Der Index eines Durchlaufs ist der Rang unter den
                        // belegten Slots, kein fester Platz.
                        Type::Map { .. } => {
                            let mut inner = CounterPaths {
                                every: vec![Vec::new(); out.every.len()],
                                viol: vec![Vec::new(); out.viol.len()],
                            };
                            self.walk_counters(body, &mut Vec::new(), &mut inner)?;
                            if inner.every.iter().chain(&inner.viol).any(|l| !l.is_empty()) {
                                return no("`every` oder `check … for` in einer Schleife ueber eine map", s.span);
                            }
                            continue;
                        }
                        Type::Stream(_) => match stream::stream_of(iter) {
                            Some(key) => self.window_slots(key, s.span)?,
                            None => return no("`for` ueber diesen Strom", s.span),
                        },
                        _ => return no("`for` ueber diesen Wert", s.span),
                    };
                    counts.push(n);
                    self.walk_counters(body, counts, out)?;
                    counts.pop();
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn loc_out(&self, c: ChannelId) -> String {
        format!("s.out.{}", self.p.channels[c.index()].name)
    }

    fn input(&mut self, name: String, sort: Sort) -> Term {
        self.inputs.insert(name.clone(), sort);
        Term::var(name, sort)
    }

    /// Der Code eines Zustands: seine Variante in `<m>.State`, sonst sein Index.
    /// Die Varianten von `<m>.State`.
    fn state_variants(&self, m: MachineId) -> &[takt_mir::types::VariantDef] {
        let name = format!("{}.State", self.machine(m).name);
        self.p.enums.iter().find(|e| e.name == name).map_or(&[], |e| e.variants.as_slice())
    }

    /// Der Code eines Blatts: seine Variante in `<m>.State`; ein Blatt ohne
    /// eigene, ein Segment einer Sequenz, steht hinter allen Varianten,
    /// damit kein Code zweimal vorkommt.
    fn code(&self, m: MachineId, s: StateId) -> i64 {
        let name = &self.machine(m).states[s.index()].name;
        let variants = self.state_variants(m);
        variants.iter().position(|v| v.name == *name).map_or((variants.len() + s.index()) as i64, |i| i as i64)
    }

    /// `m.state` zum Blattcode `leaf`: die Variante zum letzten Teil des
    /// Namens, sonst die erste (`state_value`, `publish_function`).
    fn state_value(&self, m: MachineId, leaf: Term) -> Term {
        let variants = self.state_variants(m);
        let mut out = leaf.clone();
        for l in self.leaves(m) {
            let name = &self.machine(m).states[l.index()].name;
            let last = name.rsplit('.').next().unwrap_or(name);
            let value = variants.iter().position(|v| v.name == last).unwrap_or(0) as i64;
            let code = self.code(m, l);
            if value != code {
                out = Term::ite(Term::eq(leaf.clone(), Term::int(code)), Term::int(value), out);
            }
        }
        out
    }

    fn faulted_code(&self, m: MachineId) -> Option<i64> {
        let machine = self.machine(m);
        let e = self.p.enums.iter().find(|e| e.name == format!("{}.State", machine.name))?;
        e.variants.iter().position(|v| v.name == "FAULTED").map(|i| i as i64)
    }

    fn chain_to(&self, m: MachineId, s: StateId) -> Vec<StateId> {
        let machine = self.machine(m);
        let mut out = Vec::new();
        let mut cur = Some(s);
        while let Some(id) = cur {
            out.push(id);
            cur = machine.states[id.index()].parent;
        }
        out.reverse();
        out
    }

    /// Die Kette bis zum Anfangsblatt unter `s`.
    fn descend(&self, m: MachineId, s: StateId) -> Vec<StateId> {
        let machine = self.machine(m);
        let mut out = self.chain_to(m, s);
        let mut cur = s;
        while let Some(i) = machine.states[cur.index()].initial {
            out.push(i);
            cur = i;
        }
        out
    }

    fn leaves(&self, m: MachineId) -> Vec<StateId> {
        self.machine(m)
            .states
            .iter()
            .enumerate()
            .filter(|(_, s)| s.children.is_empty())
            .map(|(i, _)| StateId(i as u32))
            .collect()
    }

    fn fault_target(&self, m: MachineId, leaf: StateId) -> Target {
        match self.machine(m).fault_target_of(leaf) {
            FaultTarget::State(s) => Target::State(s),
            FaultTarget::Faulted => Target::Faulted,
        }
    }

    // ------------------------------------------------------------ Ausdruecke

    /// Ein konstanter Wert (Parameter-Defaults, `safe`, `after`).
    fn const_value(&mut self, e: &Expr) -> R<V> {
        let pre = Env::new();
        let active = BTreeMap::new();
        let cx = Cx { m: None, leaf: None, mode: Mode::Entry, pre: &pre, active: &active, locals: None };
        let mut flow = Flow::new(Term::bool(true));
        let v = self.value(e, &cx, &pre, &mut flow)?;
        let mut leaves = Vec::new();
        v.leaves(&mut leaves);
        if !flow.exits.is_empty() || leaves.iter().any(|t| has_var(t)) {
            return no("kein konstanter Ausdruck", e.span);
        }
        Ok(v)
    }

    fn const_expr(&mut self, e: &Expr) -> R<Term> {
        self.const_value(e)?.leaf(e.span)
    }

    fn const_int(&mut self, e: &Expr) -> R<i64> {
        let t = self.const_expr(e)?;
        match eval::eval(&t, &eval::Env::new()) {
            eval::Val::Int(i) => Ok(i),
            _ => no("Ganzzahl erwartet", e.span),
        }
    }

    #[deny(clippy::wildcard_enum_match_arm)]
    fn expr(&mut self, e: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Term> {
        let span = e.span;
        if self.composite(e.ty) {
            return no("zusammengesetzter Wert an dieser Stelle", span);
        }
        let sort = self.sort_of(e.ty, span);
        Ok(match &e.kind {
            ExprKind::Bool(b) => Term::bool(*b),
            ExprKind::Int(i) | ExprKind::Duration(i) => Term::int(*i),
            ExprKind::Float(f) => Term::float(*f, sort?),
            ExprKind::Default => Enc::zero(sort?),
            ExprKind::Variant { variant, fields, .. } => {
                if !fields.is_empty() {
                    return no("Variante mit Feldern", span);
                }
                Term::int(i64::from(*variant))
            }
            ExprKind::Var(v) => {
                if let Some(local) = cx.locals.as_ref().and_then(|l| l.get(v)) {
                    return local.clone().leaf(span);
                }
                let Some(m) = cx.m else { return no("Variable ausserhalb einer Maschine", span) };
                env.get(&self.loc_var(m, *v)).cloned().ok_or_else(|| Unsupported { what: "Variable".into(), span })?
            }
            ExprKind::Param(id) => {
                let param = &self.p.params[id.index()];
                if param.tunable {
                    // 8.4: zur Laufzeit gestellt, je Tick ein Wert in seiner Range.
                    let (name, ty) = (format!("i.tune.{}", param.name), param.ty);
                    let x = self.input(name.clone(), self.sort_of(ty, span)?);
                    if self.tunes_seen.insert(name)
                        && let Some(t) = self.type_invariant(x.clone(), ty)
                    {
                        self.tune_assumptions.push(t);
                    }
                    return Ok(x);
                }
                let default = param.default.clone();
                self.const_expr(&default)?
            }
            ExprKind::Command(c) => self.command(*c),
            ExprKind::Input { channel, .. } => self.read_input(*channel, cx, flow, span)?,
            ExprKind::Accessor { base, accessor, .. } if matches!(self.p.types.get(base.ty), Type::Stream(_)) => {
                self.stream_accessor(base, *accessor, cx, env, span)?
            }
            ExprKind::Accessor { base, .. } if matches!(base.kind, ExprKind::Input { .. }) => {
                self.input_accessor(e, cx, env, flow)?
            }
            ExprKind::Accessor { base, accessor, args } => self.accessor(base, *accessor, args, cx, env, flow, span)?,
            ExprKind::Field { base, field } => self.field(base, *field as usize, cx, env, flow)?.leaf(span)?,
            ExprKind::Index { base, index } => self.element(base, index, cx, env, flow, span)?.leaf(span)?,
            ExprKind::Output(c) => {
                let ch = &self.p.channels[c.index()];
                if let Some(owner) = ch.owner.filter(|o| !self.order.contains(o)) {
                    let (ty, name) = (ch.ty, ch.name.clone());
                    let (x, fresh) = self.free_psi(owner, &format!("s.out.{name}"), self.sort_of(ty, span)?);
                    if fresh && let Some(t) = self.type_invariant(x.clone(), ty) {
                        self.psi_assumptions.push(t);
                    }
                    return Ok(x);
                }
                let own = cx.m.is_some() && ch.owner == cx.m;
                let loc = self.loc_out(*c);
                let src = if own { env } else { cx.pre };
                src.get(&loc).cloned().ok_or_else(|| Unsupported { what: "Output".into(), span })?
            }
            ExprKind::Published { machine, var } => {
                let var = *var;
                self.per_instance(machine, cx, env, flow, &mut |enc, m| enc.published(m, var, cx, env, span))?
            }
            ExprKind::StateOf(machine) => {
                self.per_instance(machine, cx, env, flow, &mut |enc, m| enc.state_of(m, cx, env, span))?
            }
            ExprKind::Signal { machine, signal } => {
                let loc = |enc: &Enc<'_>, m| enc.loc_sig(m, signal.index());
                self.per_instance(machine, cx, env, flow, &mut |enc, m| {
                    let at = loc(enc, m);
                    if !enc.order.contains(&m) {
                        return Ok(enc.free_psi(m, &at, Sort::Bool).0);
                    }
                    enc.psi(cx, env, m, &at, span)
                })?
            }
            ExprKind::Builtin(b) => match b {
                Builtin::Tick => Term::int(self.p.config.tick),
                Builtin::Now => self.now.clone(),
                Builtin::TimeInState => {
                    let (Some(m), Some(leaf)) = (cx.m, cx.leaf) else { return no("`time_in_state`", span) };
                    let period = i64::from(self.machine(m).period.max(1)).saturating_mul(self.p.config.tick);
                    let timer = env.get(&self.loc_timer(m, leaf)).cloned().expect("Timer");
                    Term::bin(Op::Mul, timer, Term::int(period))
                }
                other @ (Builtin::LastFault | Builtin::Event) => return no(format!("`{other:?}`"), span),
            },
            ExprKind::Unary { op, expr } => {
                let x = self.expr(expr, cx, env, flow)?;
                match (op, x.sort()) {
                    (UnaryOp::Not, _) => x.not(),
                    (UnaryOp::Neg, Sort::Int) => Term::app(Op::Neg, vec![x]),
                    (UnaryOp::Neg, _) => Term::app(Op::FNeg, vec![x]),
                    (UnaryOp::BitNot, _) => return no("`~`", span),
                }
            }
            ExprKind::Binary { op: op @ (BinaryOp::And | BinaryOp::Or), lhs, rhs } => {
                let a = self.expr(lhs, cx, env, flow)?;
                let guard = if *op == BinaryOp::And { a.clone() } else { a.clone().not() };
                let b = self.guarded(&guard, flow, |enc, flow| enc.expr(rhs, cx, env, flow))?;
                self.binary(*op, a, b, None, span)?
            }
            // Text gleicht als Text, auch ueber verschiedene Kapazitaeten (`value::same`).
            ExprKind::Binary { op: op @ (BinaryOp::Eq | BinaryOp::Ne), lhs, rhs }
                if self.text_type(lhs.ty).is_some() =>
            {
                let a = text::Text::of(self.value(lhs, cx, env, flow)?, span)?;
                let b = text::Text::of(self.value(rhs, cx, env, flow)?, span)?;
                let eq = a.equal(&b);
                if *op == BinaryOp::Eq { eq } else { eq.not() }
            }
            ExprKind::Binary { op: op @ (BinaryOp::Eq | BinaryOp::Ne), lhs, rhs } if self.composite(lhs.ty) => {
                let a = self.value(lhs, cx, env, flow)?;
                let b = self.value(rhs, cx, env, flow)?;
                let eq = Enc::equal(&a, &b);
                if *op == BinaryOp::Eq { eq } else { eq.not() }
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.expr(lhs, cx, env, flow)?;
                let b = self.expr(rhs, cx, env, flow)?;
                let width = self.int_width(lhs.ty, span)?;
                self.binary(*op, a, b, width, span)?
            }
            ExprKind::Cond { cond, then, otherwise } => {
                let c = self.expr(cond, cx, env, flow)?;
                let a = self.guarded(&c, flow, |enc, flow| enc.expr(then, cx, env, flow))?;
                let b = self.guarded(&c.clone().not(), flow, |enc, flow| enc.expr(otherwise, cx, env, flow))?;
                Term::ite(c, a, b)
            }
            ExprKind::Checked { expr: inner, kind } => self.checked(kind, inner, e, cx, env, flow)?,
            ExprKind::Convert { expr, kind, unit } => {
                let x = self.expr(expr, cx, env, flow)?;
                self.convert(x, *kind, *unit, expr.ty, sort?, span)?
            }
            ExprKind::Cast { expr, .. } => {
                let x = self.expr(expr, cx, env, flow)?;
                match (x.sort(), sort?) {
                    (Sort::Int, Sort::Int) => x,
                    (Sort::Int, Sort::F32) => Term::app(Op::ToF32, vec![x]),
                    (Sort::Int, Sort::F64) => Term::app(Op::ToF64, vec![x]),
                    (a, b) if a == b => x,
                    _ => return no("Konversion", span),
                }
            }
            ExprKind::Intrinsic { op: Intrinsic::Interp, args } => self.interp(args, cx, env, flow, span)?,
            ExprKind::Intrinsic { op, args } => {
                let mut xs = Vec::new();
                for a in args {
                    xs.push(self.expr(a, cx, env, flow)?);
                }
                let arg = args.first().map_or(e.ty, |a| a.ty);
                self.intrinsic(*op, xs, (e.ty, arg), flow, span)?
            }
            ExprKind::Call { callee, args } => {
                let mut xs = Vec::new();
                for a in args {
                    xs.push(self.value(a, cx, env, flow)?);
                }
                self.call(*callee, xs, cx, env, flow, span)?.leaf(span)?
            }
            ExprKind::Matches { subject, kind, pattern, binding } => {
                self.value_match(subject, *kind, pattern, *binding, cx, env, flow, span)?
            }
            ExprKind::NativeCall { native, args } => {
                self.native_call(*native, args, cx, env, flow, span)?.leaf(span)?
            }
            other @ (ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Record { .. }
            | ExprKind::Array(_)
            | ExprKind::Tuple(..)
            | ExprKind::BlockInit { .. }
            | ExprKind::Armed(_)
            | ExprKind::PortRead(_)
            | ExprKind::Index2 { .. }
            | ExprKind::Slice { .. }
            | ExprKind::Format(_)
            | ExprKind::JobState { .. }
            | ExprKind::Stream(_)
            | ExprKind::MatOp { .. }
            | ExprKind::Decode { .. }
            | ExprKind::Lift(_)
            | ExprKind::Ok(_)
            | ExprKind::Err(_)) => return no(format!("Ausdruck {}", node_name(other)), span),
        })
    }

    fn command(&mut self, c: CommandId) -> Term {
        let name = format!("i.cmd.{}", self.p.commands[c.index()].name);
        self.input(name, Sort::Bool)
    }

    /// Ψ eines anderen Maschinenwerts: Follower lesen frisch, was in diesem
    /// Tick schon lief; sonst gilt Ψ_k (7.2).
    fn psi(&mut self, cx: &Cx<'_>, env: &Env, target: MachineId, loc: &str, span: Span) -> R<Term> {
        let old = cx.pre.get(loc).cloned().ok_or_else(|| Unsupported { what: "Ψ".into(), span })?;
        let fresh = cx.m.is_some_and(|m| self.machine(m).follows.contains(&target));
        if !fresh {
            return Ok(old);
        }
        let new = env.get(loc).cloned().unwrap_or_else(|| old.clone());
        let active = cx.active.get(&target).cloned().unwrap_or_else(|| Term::bool(false));
        Ok(Term::ite(active, new, old))
    }

    /// Ein Bezug auf eine Instanz (`machine_index`): ohne Index die Maschine,
    /// sonst je Element des Instanz-Arrays `read`, ausgewaehlt nach dem
    /// Index; ausserhalb des Arrays ein `RangeFault`.
    fn per_instance<T>(
        &mut self,
        mref: &takt_mir::expr::MachineRef,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        read: &mut dyn FnMut(&mut Self, MachineId) -> R<T>,
    ) -> R<T>
    where
        T: Choose,
    {
        let Some(index) = &mref.index else { return read(self, mref.machine) };
        let i = self.expr(index, cx, env, flow)?;
        let n = match &self.machine(mref.machine).kind {
            takt_mir::machine::MachineKind::Instance(info) => info.array.map_or(1, |(_, n)| n),
            _ => 1,
        };
        let outside = Term::or(vec![
            Term::bin(Op::Lt, i.clone(), Term::int(0)),
            Term::bin(Op::Ge, i.clone(), Term::int(i64::from(n))),
        ]);
        flow.exits.push(Exit {
            cond: Term::and(vec![flow.alive.clone(), outside.clone()]),
            kind: ExitKind::Fault(None, self.cause(FaultKind::Range, index.span)),
        });
        flow.alive = Term::and(vec![flow.alive.clone(), outside.not()]);
        let mut out = read(self, MachineId(mref.machine.0 + n.max(1) - 1))?;
        for k in (0..n.saturating_sub(1)).rev() {
            let v = read(self, MachineId(mref.machine.0 + k))?;
            out = T::choose(&Term::eq(i.clone(), Term::int(i64::from(k))), v, out);
        }
        Ok(out)
    }

    /// Eine `pub var` einer Maschine: frisch nach `follows`, sonst Ψ.
    fn published(&mut self, m: MachineId, var: VarId, cx: &Cx<'_>, env: &Env, span: Span) -> R<Term> {
        let loc = self.loc_var(m, var);
        if !self.order.contains(&m) {
            let ty = self.machine(m).vars[var.index()].ty;
            let (x, fresh) = self.free_psi(m, &loc, self.sort_of(ty, span)?);
            if fresh && let Some(t) = self.type_invariant(x.clone(), ty) {
                self.psi_assumptions.push(t);
            }
            return Ok(x);
        }
        self.psi(cx, env, m, &loc, span)
    }

    /// Der Zustand einer Maschine als Variante ihres Zustandstyps.
    fn state_of(&mut self, m: MachineId, cx: &Cx<'_>, env: &Env, span: Span) -> R<Term> {
        let loc = self.loc_leaf(m);
        if !self.order.contains(&m) {
            let (x, fresh) = self.free_psi(m, &loc, Sort::Int);
            if fresh {
                let mut codes: Vec<Term> =
                    self.leaves(m).into_iter().map(|l| Term::eq(x.clone(), Term::int(self.code(m, l)))).collect();
                if let Some(c) = self.faulted_code(m) {
                    codes.push(Term::eq(x.clone(), Term::int(c)));
                }
                self.psi_assumptions.push(Term::or(codes));
            }
            return Ok(self.state_value(m, x));
        }
        let leaf = self.psi(cx, env, m, &loc, span)?;
        Ok(self.state_value(m, leaf))
    }

    /// Ein zweistelliger Operator; `width` ist die Breite eines ganzzahligen
    /// linken Operanden und damit des Ergebnisses einer Rechnung (3.10).
    #[deny(clippy::wildcard_enum_match_arm)]
    fn binary(&mut self, op: BinaryOp, a: Term, b: Term, width: Option<IntWidth>, span: Span) -> R<Term> {
        let float = matches!(a.sort(), Sort::F32 | Sort::F64);
        Ok(match op {
            BinaryOp::And => Term::and(vec![a, b]),
            BinaryOp::Or => Term::or(vec![a, b]),
            BinaryOp::Eq if float => Term::bin(Op::FEq, a, b),
            BinaryOp::Eq => Term::eq(a, b),
            BinaryOp::Ne if float => Term::bin(Op::FEq, a, b).not(),
            BinaryOp::Ne => Term::eq(a, b).not(),
            BinaryOp::Lt => Term::bin(if float { Op::FLt } else { Op::Lt }, a, b),
            BinaryOp::Le => Term::bin(if float { Op::FLe } else { Op::Le }, a, b),
            BinaryOp::Gt => Term::bin(if float { Op::FGt } else { Op::Gt }, a, b),
            BinaryOp::Ge => Term::bin(if float { Op::FGe } else { Op::Ge }, a, b),
            BinaryOp::Add => Term::bin(if float { Op::FAdd } else { Op::Add }, a, b),
            BinaryOp::Sub => Term::bin(if float { Op::FSub } else { Op::Sub }, a, b),
            BinaryOp::Mul => Term::bin(if float { Op::FMul } else { Op::Mul }, a, b),
            BinaryOp::Div => Term::bin(if float { Op::FDiv } else { Op::Div }, a, b),
            BinaryOp::Rem if !float => Term::bin(Op::Rem, a, b),
            BinaryOp::BitAnd => Term::bin(Op::BitAnd, a, b),
            BinaryOp::BitOr => Term::bin(Op::BitOr, a, b),
            BinaryOp::BitXor => Term::bin(Op::BitXor, a, b),
            // `<<` wickelt in die Breite (3.10); `>>` bleibt in ihr.
            BinaryOp::Shl => {
                let width = width.unwrap_or(IntWidth::I64);
                Term::app(Op::Wrap { bits: width.bits(), signed: width.signed() }, vec![Term::bin(Op::Shl, a, b)])
            }
            BinaryOp::Shr => Term::bin(Op::Shr, a, b),
            other => return no(format!("Operator `{other:?}`"), span),
        })
    }

    #[deny(clippy::wildcard_enum_match_arm)]
    /// `tys`: der Typ des Ergebnisses und der des ersten Arguments, fuer
    /// die Breite der Ganzzahl-Primitive.
    fn intrinsic(
        &mut self,
        op: Intrinsic,
        xs: Vec<Term>,
        tys: (TypeId, TypeId),
        flow: &mut Flow,
        span: Span,
    ) -> R<Term> {
        let float = xs.first().is_some_and(|x| matches!(x.sort(), Sort::F32 | Sort::F64));
        // Ein Fault der Primitive selbst (`call.rs`): Definitionsbereich,
        // nicht endliches Ergebnis, Bereich einer Rundung; ohne Pruefknoten.
        let fault = |fail: Term, cause: Cause, flow: &mut Flow| {
            flow.exits.push(Exit {
                cond: Term::and(vec![flow.alive.clone(), fail.clone()]),
                kind: ExitKind::Fault(None, cause),
            });
            flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        };
        let lit = |x: f64, like: &Term| Term::float(x, like.sort());
        let math = |f: Fun, args: Vec<Term>| Term::app(Op::Math(f), args);
        Ok(match (op, xs.as_slice()) {
            (Intrinsic::Abs, [x]) if float => Term::app(Op::FAbs, vec![x.clone()]),
            (Intrinsic::Abs, [x]) => {
                let neg = Term::bin(Op::Lt, x.clone(), Term::int(0));
                Term::ite(neg, Term::app(Op::Neg, vec![x.clone()]), x.clone())
            }
            (Intrinsic::Min | Intrinsic::Max, [a, b]) => {
                let lt = Term::bin(if float { Op::FLt } else { Op::Lt }, a.clone(), b.clone());
                if op == Intrinsic::Min {
                    Term::ite(lt, a.clone(), b.clone())
                } else {
                    Term::ite(lt, b.clone(), a.clone())
                }
            }
            (Intrinsic::Sqrt, [x]) if float => Term::app(Op::FSqrt, vec![x.clone()]),
            (Intrinsic::Fma, [a, b, c]) if float => Term::app(Op::FFma, vec![a.clone(), b.clone(), c.clone()]),
            (
                Intrinsic::Sin
                | Intrinsic::Cos
                | Intrinsic::Tan
                | Intrinsic::Asin
                | Intrinsic::Acos
                | Intrinsic::Atan
                | Intrinsic::Exp
                | Intrinsic::Log,
                [x],
            ) if float => {
                let Some(f) = libm(op) else { return no(format!("Primitive `{op:?}`"), span) };
                let domain = match f {
                    Fun::Asin | Fun::Acos => Term::and(vec![
                        Term::bin(Op::FGe, x.clone(), lit(-1.0, x)),
                        Term::bin(Op::FLe, x.clone(), lit(1.0, x)),
                    ]),
                    Fun::Log => Term::bin(Op::FGt, x.clone(), lit(0.0, x)),
                    Fun::Sin | Fun::Cos | Fun::Tan | Fun::Atan | Fun::Atan2 | Fun::Exp | Fun::Pow => Term::bool(true),
                };
                fault(domain.not(), self.cause(FaultKind::Arithmetic(ArithKind::Domain), span), flow);
                self.uninterpreted.insert(f.name().to_string());
                let r = math(f, vec![x.clone()]);
                fault(
                    Term::app(Op::IsFinite, vec![r.clone()]).not(),
                    self.cause(FaultKind::Arithmetic(ArithKind::NonFinite), span),
                    flow,
                );
                r
            }
            // 4.1: `pow` ist definiert fuer y = 0, x > 0, x = 0 mit y > 0 und
            // x < 0 mit ganzem y.
            (Intrinsic::Atan2 | Intrinsic::Pow, [x, y]) if float => {
                if op == Intrinsic::Pow {
                    let zero = lit(0.0, x);
                    let whole =
                        Term::bin(Op::FEq, Term::app(Op::Round(Rounding::TowardZero), vec![y.clone()]), y.clone());
                    let defined = Term::or(vec![
                        Term::bin(Op::FEq, y.clone(), zero.clone()),
                        Term::bin(Op::FGt, x.clone(), zero.clone()),
                        Term::and(vec![
                            Term::bin(Op::FEq, x.clone(), zero.clone()),
                            Term::bin(Op::FGt, y.clone(), zero.clone()),
                        ]),
                        Term::and(vec![Term::bin(Op::FLt, x.clone(), zero), whole]),
                    ]);
                    fault(defined.not(), self.cause(FaultKind::Arithmetic(ArithKind::Domain), span), flow);
                }
                let f = if op == Intrinsic::Pow { Fun::Pow } else { Fun::Atan2 };
                self.uninterpreted.insert(f.name().to_string());
                let r = math(f, vec![x.clone(), y.clone()]);
                fault(
                    Term::app(Op::IsFinite, vec![r.clone()]).not(),
                    self.cause(FaultKind::Arithmetic(ArithKind::NonFinite), span),
                    flow,
                );
                r
            }
            // In der Breite des Ergebnisses gewickelt (`call.rs`).
            (Intrinsic::WrappingAdd | Intrinsic::WrappingSub | Intrinsic::WrappingMul, [a, b]) if !float => {
                let w = self.int_width(tys.0, span)?.unwrap_or(IntWidth::I64);
                let r = if op == Intrinsic::WrappingAdd {
                    Term::bin(Op::Add, a.clone(), b.clone())
                } else if op == Intrinsic::WrappingSub {
                    Term::bin(Op::Sub, a.clone(), b.clone())
                } else {
                    Term::bin(Op::Mul, a.clone(), b.clone())
                };
                Term::app(Op::Wrap { bits: w.bits(), signed: w.signed() }, vec![r])
            }
            // Auf die Grenzen der Breite geklemmt; in 64 Bit entscheidet der
            // Ueberlauf die Richtung.
            (Intrinsic::SaturatingAdd | Intrinsic::SaturatingSub, [a, b]) if !float => {
                let w = self.int_width(tys.0, span)?.unwrap_or(IntWidth::I64);
                let (lo, hi) = width_bounds(w);
                let (lo, hi) = (Term::int(lo as i64), Term::int(hi as i64));
                let add = op == Intrinsic::SaturatingAdd;
                let r = Term::bin(if add { Op::Add } else { Op::Sub }, a.clone(), b.clone());
                let wraps = Term::bin(if add { Op::AddOverflows } else { Op::SubOverflows }, a.clone(), b.clone());
                let clamped = Term::ite(
                    Term::bin(Op::Lt, r.clone(), lo.clone()),
                    lo.clone(),
                    Term::ite(Term::bin(Op::Gt, r.clone(), hi.clone()), hi.clone(), r),
                );
                // Eine Summe ueber `i64` hat das Vorzeichen des ersten Summanden.
                let up = Term::bin(Op::Ge, a.clone(), Term::int(0));
                Term::ite(wraps, Term::ite(up, hi, lo), clamped)
            }
            // Rotation in der Breite des Arguments, der Betrag modulo der Breite.
            (Intrinsic::Rotl | Intrinsic::Rotr, [x, n]) if !float => {
                let w = self.int_width(tys.1, span)?.unwrap_or(IntWidth::I64);
                let bits = i64::from(w.bits());
                let mask = if bits >= 64 { Term::int(-1) } else { Term::int((1i64 << bits) - 1) };
                let r = Term::bin(Op::Rem, n.clone(), Term::int(bits));
                let n = Term::ite(
                    Term::bin(Op::Lt, r.clone(), Term::int(0)),
                    Term::bin(Op::Add, r.clone(), Term::int(bits)),
                    r,
                );
                let u = Term::bin(Op::BitAnd, x.clone(), mask.clone());
                let back = Term::bin(Op::Sub, Term::int(bits), n.clone());
                let (first, second) = if op == Intrinsic::Rotl { (Op::Shl, Op::Shr) } else { (Op::Shr, Op::Shl) };
                let rot = Term::bin(Op::BitOr, Term::bin(first, u.clone(), n), Term::bin(second, u, back));
                Term::app(Op::Wrap { bits: w.bits(), signed: w.signed() }, vec![Term::bin(Op::BitAnd, rot, mask)])
            }
            // Gerundet, dann als Ganzzahl; ausserhalb von `i64` ein Range-Fault.
            (Intrinsic::Round | Intrinsic::Floor | Intrinsic::Ceil, [x]) if float => {
                let mode = if op == Intrinsic::Round {
                    Rounding::HalfAway
                } else if op == Intrinsic::Floor {
                    Rounding::Down
                } else {
                    Rounding::Up
                };
                let r = Term::app(Op::Round(mode), vec![x.clone()]);
                let inside = Term::and(vec![
                    Term::bin(Op::FGe, r.clone(), lit(i64::MIN as f64, x)),
                    Term::bin(Op::FLt, r.clone(), lit(i64::MAX as f64, x)),
                ]);
                fault(inside.not(), self.cause(FaultKind::Range, span), flow);
                Term::app(Op::FloatToInt, vec![r])
            }
            _ => return no(format!("Primitive `{op:?}`"), span),
        })
    }

    /// Ein Aufruf einer Native der kuratierten Menge (4.5, `call_native`):
    /// die Argumente in der Form der Grenze, `bytes<N>` roh, alles andere in
    /// kanonischer Byteform; eine Pruefsumme ist eine Zahl, ein Digest 32
    /// Byte. Der Solver sieht die Native uninterpretiert, die Auswertung
    /// rechnet sie genau.
    fn native_call(
        &mut self,
        id: takt_mir::NativeId,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        use takt_native::Native;
        let n = self.p.natives[id.index()].clone();
        let Some(f) = Native::by_name(&n.name) else { return no(format!("Projekt-Native `{}`", n.name), span) };
        let mut blocks = Vec::new();
        for (a, param) in args.iter().zip(&n.params) {
            let v = self.value(a, cx, env, flow)?;
            let (len, bytes) = match self.p.types.get(param.ty) {
                Type::Bytes { .. } => {
                    let V::Node(parts) = v else { return no("Bytes", span) };
                    let mut it = parts.into_iter();
                    let len = it.next().map_or_else(|| no("Bytes", span), |x| x.leaf(span))?;
                    (len, it.map(|x| x.leaf(span)).collect::<R<Vec<_>>>()?)
                }
                _ => {
                    let form = self.canonical(param.ty, v, span)?;
                    (form.len, form.bytes)
                }
            };
            blocks.push(Term::int(bytes.len() as i64));
            blocks.push(len);
            blocks.extend(bytes);
        }
        let part = |part: u16, bits: u8| V::Leaf(Term::app(Op::Native { f, part, bits }, blocks.clone()));
        let out = match f {
            Native::Crc32 | Native::Crc32c => part(0, 32),
            Native::Crc16 => part(0, 16),
            Native::Sum8 => part(0, 8),
            Native::Sha256 | Native::HmacSha256 => {
                V::Node(std::iter::once(V::Leaf(Term::int(32))).chain((0..32).map(|k| part(k, 8))).collect())
            }
            Native::Sha256Init
            | Native::Sha256Update
            | Native::Sha256Final
            | Native::EcdsaP256Verify
            | Native::Fft256
            | Native::Rsa3072Verify
            | Native::AesGcmDecrypt => return no(format!("Native `{}`", n.name), span),
        };
        self.uninterpreted.insert(n.name.clone());
        Ok(out)
    }

    /// `interp(t, x)` ueber einer konstanten Tabelle (`call.rs`): bis zum
    /// ersten Punkt sein Wert, ab dem letzten dessen, dazwischen linear im
    /// ersten Abschnitt, der `x` enthaelt, jede Operation gerundet; ein
    /// nicht endliches Ergebnis faultet.
    fn interp(&mut self, args: &[Expr], cx: &Cx<'_>, env: &Env, flow: &mut Flow, span: Span) -> R<Term> {
        let [table, x] = args else { return no("`interp` ohne Tabelle und Stelle", span) };
        let ExprKind::Array(items) = &table.kind else { return no("`interp` ueber einer berechneten Tabelle", span) };
        let mut points = Vec::new();
        for item in items {
            let ExprKind::Tuple(px, py) = &item.kind else { return no("Punkt einer Tabelle", item.span) };
            points.push((self.expr(px, cx, env, flow)?, self.expr(py, cx, env, flow)?));
        }
        let x = self.expr(x, cx, env, flow)?;
        let (Some((x0, y0)), Some((xn, yn))) = (points.first().cloned(), points.last().cloned()) else {
            return no("leere Tabelle", span);
        };
        let mut inner = yn.clone();
        for pair in points.windows(2).rev() {
            let ((a, fa), (b, fb)) = (&pair[0], &pair[1]);
            let dy = Term::bin(Op::FSub, fb.clone(), fa.clone());
            let dx = Term::bin(Op::FSub, x.clone(), a.clone());
            let w = Term::bin(Op::FSub, b.clone(), a.clone());
            let q = Term::bin(Op::FDiv, Term::bin(Op::FMul, dy, dx), w);
            let r = Term::bin(Op::FAdd, fa.clone(), q);
            inner = Term::ite(Term::bin(Op::FLe, x.clone(), b.clone()), r, inner);
        }
        let below = Term::bin(Op::FLe, x.clone(), x0);
        let above = Term::bin(Op::FGe, x.clone(), xn);
        let middle = Term::and(vec![below.clone().not(), above.clone().not()]);
        let fail = Term::and(vec![flow.alive.clone(), middle, Term::app(Op::IsFinite, vec![inner.clone()]).not()]);
        let cause = self.cause(FaultKind::Arithmetic(ArithKind::NonFinite), span);
        flow.exits.push(Exit { cond: fail.clone(), kind: ExitKind::Fault(None, cause) });
        flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        Ok(Term::ite(below, y0, Term::ite(above, yn, inner)))
    }

    /// Eine Einheitenumrechnung (3.2, `eval::convert`): `as(U)` teilt die
    /// Nanosekunden durch die der Einheit, `to(U)` und `to_float(U)` gehen
    /// ueber die Basiseinheit — Versatz der Quelle, der exakte Bruch beider
    /// Faktoren einmal gerundet, Versatz des Ziels.
    fn convert(
        &self,
        x: Term,
        kind: ConvertKind,
        unit: takt_mir::UnitId,
        from: TypeId,
        sort: Sort,
        span: Span,
    ) -> R<Term> {
        let dst = &self.p.units[unit.index()];
        let constant = |num: i64, den: u64| {
            let (mag, den) = (u128::from(num.unsigned_abs()), u128::from(den));
            let sign = if num < 0 { -1.0 } else { 1.0 };
            match sort {
                Sort::F32 => Term::float(f64::from(libtaktm::scale_f32(sign as f32, mag, den)), Sort::F32),
                _ => Term::float(libtaktm::scale_f64(sign, mag, den), Sort::F64),
            }
        };
        let to_float = |x: Term| Term::app(if sort == Sort::F32 { Op::ToF32 } else { Op::ToF64 }, vec![x]);
        if kind == ConvertKind::As {
            let per = i128::from(dst.factor.num) * 1_000_000_000 / i128::from(dst.factor.den);
            return Ok(Term::bin(Op::FDiv, to_float(x), Term::float(per as f64, sort)));
        }
        let (factor, offset) = match (kind, self.p.types.get(from)) {
            (_, Type::Float { unit: Some(u), .. }) | (ConvertKind::ToFloat, Type::Int { unit: Some(u), .. }) => {
                let src = &self.p.units[u.index()];
                (src.factor, src.affine_offset)
            }
            (ConvertKind::ToFloat, Type::Int { unit: None, .. }) => (takt_mir::types::Rational::int(1), None),
            _ => return no("`to(U)` ohne Quelleinheit", span),
        };
        let mut x = if kind == ConvertKind::ToFloat { to_float(x) } else { x };
        if let Some(off) = offset {
            x = Term::bin(Op::FAdd, x, constant(off.num, off.den));
        }
        let num = i128::from(factor.num) * i128::from(dst.factor.den);
        let den = i128::from(factor.den) * i128::from(dst.factor.num);
        let (Ok(num), Ok(den)) = (u128::try_from(num), u128::try_from(den)) else {
            return no("Einheitenfaktor nicht positiv", span);
        };
        x = Term::app(Op::Scale { num, den }, vec![x]);
        if let Some(off) = dst.affine_offset {
            x = Term::bin(Op::FSub, x, constant(off.num, off.den));
        }
        Ok(x)
    }

    /// Eine implizite Pruefung (4.1) als Fault-Zweig: Die Stelle feuert, wo
    /// die Operation im Interpreter faultet. `node` ist der Pruefknoten,
    /// `inner` was er umschliesst.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn checked(
        &mut self,
        kind: &CheckedKind,
        inner: &Expr,
        node: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<Term> {
        let span = node.span;
        let (value, fail) = match kind {
            // Die Intervallanalyse hat sie bewiesen (3.4).
            CheckedKind::Range(r) if r.origin == takt_mir::types::RangeOrigin::Proven => {
                return self.expr(inner, cx, env, flow);
            }
            CheckedKind::Range(r) => {
                let x = self.expr(inner, cx, env, flow)?;
                let (lo, hi) = (self.bound(&r.lo, x.sort()), self.bound(&r.hi, x.sort()));
                let (ge, le) = if x.sort() == Sort::Int { (Op::Ge, Op::Le) } else { (Op::FGe, Op::FLe) };
                let fail = Term::and(vec![Term::bin(ge, x.clone(), lo), Term::bin(le, x.clone(), hi)]).not();
                (x, fail)
            }
            CheckedKind::DivZero => {
                let x = self.expr(inner, cx, env, flow)?;
                let fail = Term::eq(x.clone(), Term::int(0));
                (x, fail)
            }
            CheckedKind::NonFinite => {
                let x = self.expr(inner, cx, env, flow)?;
                let fail = Term::app(Op::IsFinite, vec![x.clone()]).not();
                (x, fail)
            }
            CheckedKind::Overflow => self.overflow(inner, node.ty, cx, env, flow)?,
            // Der Betrag liegt in `0..Breite-1`, sonst ein Range-Fault (3.10).
            CheckedKind::Shift => {
                let ExprKind::Binary { op, lhs, rhs } = &inner.kind else {
                    return no("Schiebepruefung ohne Operator", span);
                };
                let width = self.int_width(node.ty, span)?.unwrap_or(IntWidth::I64);
                let a = self.expr(lhs, cx, env, flow)?;
                let b = self.expr(rhs, cx, env, flow)?;
                let fail = Term::or(vec![
                    Term::bin(Op::Lt, b.clone(), Term::int(0)),
                    Term::bin(Op::Ge, b.clone(), Term::int(i64::from(width.bits()))),
                ]);
                (self.binary(*op, a, b, Some(width), span)?, fail)
            }
            // `as` in eine engere Breite: Der Wert muss hineinpassen (3.10).
            CheckedKind::Convert => {
                let ExprKind::Cast { expr: x, to } = &inner.kind else {
                    return no("Konversionspruefung ohne `as`", span);
                };
                let (Some(_), Some(width)) = (self.int_width(x.ty, span)?, self.int_width(*to, span)?) else {
                    return no("Konversion", span);
                };
                let v = self.expr(x, cx, env, flow)?;
                let fail = outside(&v, width);
                (v, fail)
            }
            // `sqrt` unter null (4.2); der Knoten umschliesst das Argument.
            CheckedKind::Domain => {
                let x = self.expr(inner, cx, env, flow)?;
                if x.sort() == Sort::Int {
                    return no("Definitionsbereich einer Ganzzahl", span);
                }
                let fail = Term::bin(Op::FLt, x.clone(), Enc::zero(x.sort()));
                (x, fail)
            }
            // Der Lesevorgang faultet selbst (3.5, `read_input`); der Knoten
            // ist die Stelle, an der Codegen und Beweisdatei ihn fuehren (11.3).
            CheckedKind::Valid => {
                let first = flow.exits.len();
                let x = self.expr(inner, cx, env, flow)?;
                let fires = Term::or(flow.exits[first..].iter().map(|x| x.cond.clone()).collect());
                self.site(kind, span, fires, cx);
                return Ok(x);
            }
            CheckedKind::Index { len } => return self.index_access(inner, *len, span, kind, cx, env, flow)?.leaf(span),
            CheckedKind::Missing => return self.unwrap(inner, span, kind, cx, env, flow)?.leaf(span),
        };
        let cond = Term::and(vec![flow.alive.clone(), fail.clone()]);
        self.site(kind, span, cond.clone(), cx);
        flow.exits.push(Exit { cond, kind: ExitKind::Fault(None, self.cause(kind.fault(), span)) });
        flow.alive = Term::and(vec![flow.alive.clone(), fail.not()]);
        Ok(value)
    }

    /// Der Wert einer Rechnung unter `Checked{Overflow}` und wann sie in der
    /// Breite ihres Ergebnisses ueberlaeuft (4.1, 3.10). Operanden schmaler
    /// Breiten liegen in ihr und rechnen in 64 Bit exakt; ein Produkt zweier
    /// `u32` ueber 2^63 erscheint negativ und liegt ebenso ausserhalb. In 64
    /// Bit entscheiden die Vorzeichen.
    fn overflow(&mut self, inner: &Expr, ty: TypeId, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<(Term, Term)> {
        let span = inner.span;
        let Some(width) = self.int_width(ty, span)? else { return no("Ueberlauf ohne Ganzzahl", span) };
        let narrow = width.bits() < 64;
        let min = |x: &Term| Term::eq(x.clone(), Term::int(i64::MIN));
        Ok(match &inner.kind {
            ExprKind::Binary { op, lhs, rhs } => {
                let a = self.expr(lhs, cx, env, flow)?;
                let b = self.expr(rhs, cx, env, flow)?;
                let r = self.binary(*op, a.clone(), b.clone(), Some(width), span)?;
                let fail = match op {
                    _ if narrow => outside(&r, width),
                    BinaryOp::Add => Term::bin(Op::AddOverflows, a, b),
                    BinaryOp::Sub => Term::bin(Op::SubOverflows, a, b),
                    BinaryOp::Mul => Term::bin(Op::MulOverflows, a, b),
                    BinaryOp::Div => Term::and(vec![min(&a), Term::eq(b, Term::int(-1))]),
                    // `MIN % -1` ist null.
                    BinaryOp::Rem => Term::bool(false),
                    _ => return no(format!("Ueberlauf bei `{op:?}`"), span),
                };
                (r, fail)
            }
            // `-x` und `abs(x)`: Ueberlauf genau bei `MIN`.
            ExprKind::Unary { op: UnaryOp::Neg, expr: x } => {
                let x = self.expr(x, cx, env, flow)?;
                let r = Term::app(Op::Neg, vec![x.clone()]);
                let fail = if narrow { outside(&r, width) } else { min(&x) };
                (r, fail)
            }
            ExprKind::Intrinsic { op: Intrinsic::Abs, args } if args.len() == 1 => {
                let x = self.expr(&args[0], cx, env, flow)?;
                let r = self.intrinsic(Intrinsic::Abs, vec![x.clone()], (args[0].ty, args[0].ty), flow, span)?;
                let fail = if narrow { outside(&r, width) } else { min(&x) };
                (r, fail)
            }
            _ => return no("Ueberlaufpruefung ohne Operator", span),
        })
    }

    /// Eine Pruefstelle (11.3): `takt prove` zeigt, ob sie je faultet.
    fn site(&mut self, kind: &CheckedKind, span: Span, fires: Term, cx: &Cx<'_>) {
        let key = (span.start, span.end, takt_mir::analysis::walk::name(kind).to_string());
        self.sites.entry(key.clone()).or_default().push(fires);
        if let Some(m) = cx.m {
            self.site_info.insert(key, (span, self.machine(m).name.clone()));
        }
    }

    fn bound(&self, c: &Const, sort: Sort) -> Term {
        match (c, sort) {
            (Const::Int(i) | Const::Duration(i), Sort::Int) => Term::int(*i),
            (Const::Int(i) | Const::Duration(i), s) => Term::float(*i as f64, s),
            (Const::Float(f), Sort::Int) => Term::int(*f as i64),
            (Const::Float(f), s) => Term::float(*f, s),
            (Const::Bool(b), _) => Term::bool(*b),
        }
    }

    /// Ein Aufruf einer reinen Funktion (4.4), eingebettet.
    fn call(
        &mut self,
        callee: takt_mir::FnId,
        args: Vec<V>,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        let f = self.p.fns[callee.index()].clone();
        // Ein `inout`-Parameter ist die Rueckgabe (3.9); die Sema schreibt
        // ihn an der Aufrufstelle zurueck.
        let Some(ret) = f.ret else { return no("Funktion ohne Rueckgabe", span) };
        let mut locals: BTreeMap<VarId, V> = BTreeMap::new();
        for (i, local) in f.locals.iter().enumerate() {
            let value = match args.get(i) {
                Some(a) => a.clone(),
                None => self.zero_of(local.ty, local.span)?,
            };
            locals.insert(VarId(i as u32), value);
        }
        let inner =
            Cx { m: cx.m, leaf: cx.leaf, mode: Mode::Entry, pre: cx.pre, active: cx.active, locals: Some(locals) };
        let mut sub = Flow::new(flow.alive.clone());
        let mut ret_val = self.zero_of(ret, span)?;
        let mut env = env.clone();
        let mut inner = inner;
        let types = f.locals.iter().enumerate().map(|(i, l)| (VarId(i as u32), l.ty)).collect();
        let outer = std::mem::replace(&mut self.local_types, types);
        self.fn_block(&f.body, &mut inner, &mut env, &mut sub, &mut ret_val)?;
        self.local_types = outer;
        // Die Funktion kehrt zurueck; nur ihre Faults beenden den Aufrufer.
        let faults = Term::or(sub.exits.iter().map(|x| x.cond.clone()).collect());
        flow.exits.extend(sub.exits);
        flow.alive = Term::and(vec![flow.alive.clone(), !faults]);
        Ok(ret_val)
    }

    /// Der Rumpf einer Funktion: Zuweisungen an Lokale, `if`, `return`,
    /// ausgerollte Schleifen mit `break`.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn fn_block(&mut self, b: &Block, cx: &mut Cx<'_>, env: &mut Env, flow: &mut Flow, ret: &mut V) -> R<()> {
        for s in &b.stmts {
            if flow.alive.is_bool(false) {
                break;
            }
            match &s.kind {
                StmtKind::Return(e) => {
                    let v = self.value(e, cx, env, flow)?;
                    *ret = V::ite(&flow.alive, v, ret.clone());
                    flow.alive = Term::bool(false);
                }
                StmtKind::Assign { target, value } => {
                    let val = self.value(value, cx, env, flow)?;
                    self.assign_local(target, val, cx, env, flow, s.span)?;
                }
                StmtKind::If { cond, then, otherwise } => {
                    let c = self.expr(cond, cx, env, flow)?;
                    let saved = cx.locals.clone();
                    let mut ft = Flow::new(Term::and(vec![flow.alive.clone(), c.clone()]));
                    let mut rt = ret.clone();
                    self.fn_block(then, cx, env, &mut ft, &mut rt)?;
                    let then_locals = cx.locals.replace(saved.clone().expect("Lokale"));
                    let mut fe = Flow::new(Term::and(vec![flow.alive.clone(), c.clone().not()]));
                    let mut re = ret.clone();
                    self.fn_block(otherwise, cx, env, &mut fe, &mut re)?;
                    let else_locals = cx.locals.take().expect("Lokale");
                    let then_locals = then_locals.expect("Lokale");
                    let merged: BTreeMap<VarId, V> = then_locals
                        .iter()
                        .map(|(k, a)| {
                            (*k, V::ite(&c, a.clone(), else_locals.get(k).cloned().unwrap_or_else(|| a.clone())))
                        })
                        .collect();
                    cx.locals = Some(merged);
                    *ret = V::ite(&c, rt, re);
                    flow.exits.extend(ft.exits);
                    flow.exits.extend(fe.exits);
                    flow.alive = Term::or(vec![ft.alive, fe.alive]);
                }
                StmtKind::ForRange { var, count, body } => {
                    let n = self.const_int(count)?.max(0);
                    self.unroll_steps(n, s.span)?;
                    self.breaks.push(Vec::new());
                    for i in 0..n {
                        self.set_local(cx, *var, V::Leaf(Term::int(i)), &flow.alive.clone(), s.span)?;
                        self.fn_block(body, cx, env, flow, ret)?;
                    }
                    self.left_loop(flow);
                }
                // Ueber ein Array, Bytes oder einen Vektor die Variable je
                // Platz, hinter der Laenge einer Sammlung nichts; ueber eine
                // map Schluessel und Wert je belegtem Slot.
                StmtKind::ForEach { vars, iter, body }
                    if matches!(
                        (vars, self.p.types.get(iter.ty)),
                        (ForVars::One(_), Type::Array { .. } | Type::Bytes { .. } | Type::Vec { .. })
                            | (ForVars::Pair(..), Type::Map { .. })
                    ) =>
                {
                    let v = self.value(iter, cx, env, flow)?;
                    let rounds: Vec<(Term, Vec<(VarId, V)>)> = match vars {
                        ForVars::One(var) => {
                            let (items, len) = self.places(iter.ty, v, s.span)?;
                            items
                                .into_iter()
                                .enumerate()
                                .map(|(k, item)| {
                                    let inside = match &len {
                                        Some(len) => Term::bin(Op::Lt, Term::int(k as i64), len.clone()),
                                        None => Term::bool(true),
                                    };
                                    (inside, vec![(*var, item)])
                                })
                                .collect()
                        }
                        ForVars::Pair(kv, vv) => self
                            .map_entries(iter.ty, v, s.span)?
                            .into_iter()
                            .map(|(has, key, value)| (has, vec![(*kv, key), (*vv, value)]))
                            .collect(),
                    };
                    self.unroll_steps(rounds.len() as i64, s.span)?;
                    self.breaks.push(Vec::new());
                    for (inside, bindings) in rounds {
                        let saved = cx.locals.clone();
                        let mut fk = Flow::new(Term::and(vec![flow.alive.clone(), inside.clone()]));
                        for (var, item) in bindings {
                            self.set_local(cx, var, item, &fk.alive.clone(), s.span)?;
                        }
                        let mut rk = ret.clone();
                        self.fn_block(body, cx, env, &mut fk, &mut rk)?;
                        let after = cx.locals.take().expect("Lokale");
                        let before = saved.expect("Lokale");
                        cx.locals = Some(
                            after
                                .into_iter()
                                .map(|(id, a)| {
                                    let b = before.get(&id).cloned().unwrap_or_else(|| a.clone());
                                    (id, V::ite(&inside, a, b))
                                })
                                .collect(),
                        );
                        *ret = V::ite(&inside, rk, ret.clone());
                        flow.exits.extend(fk.exits);
                        flow.alive = Term::or(vec![Term::and(vec![flow.alive.clone(), inside.not()]), fk.alive]);
                    }
                    self.left_loop(flow);
                }
                StmtKind::Break => match self.breaks.last_mut() {
                    Some(frame) => {
                        frame.push(flow.alive.clone());
                        flow.alive = Term::bool(false);
                    }
                    None => return no("`break` ausserhalb einer Schleife", s.span),
                },
                StmtKind::MethodCall { target, receiver, method, args }
                    if matches!(
                        method,
                        Method::Push | Method::Append | Method::Clear | Method::Insert | Method::Remove
                    ) =>
                {
                    self.local_collection_method(target.as_ref(), receiver, *method, args, cx, env, flow, s.span)?;
                }
                StmtKind::Pass | StmtKind::Observe(_) => {}
                other @ (StmtKind::Check { .. }
                | StmtKind::Goto(_)
                | StmtKind::Abort { .. }
                | StmtKind::ForEach { .. }
                | StmtKind::Match { .. }
                | StmtKind::Send { .. }
                | StmtKind::At { .. }
                | StmtKind::Cancel(_)
                | StmtKind::Skip(_)
                | StmtKind::Raise(_)
                | StmtKind::Job { .. }
                | StmtKind::Every { .. }
                | StmtKind::Arm { .. }
                | StmtKind::MethodCall { .. }) => {
                    return no(format!("Anweisung {} in einer Funktion", stmt_name(other)), s.span);
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ Anweisungen

    #[deny(clippy::wildcard_enum_match_arm)]
    fn block(&mut self, b: &Block, cx: &Cx<'_>, env: &mut Env, flow: &mut Flow) -> R<()> {
        let m = cx.m.expect("Maschine");
        for s in &b.stmts {
            if flow.alive.is_bool(false) {
                break;
            }
            let span = s.span;
            match &s.kind {
                StmtKind::Assign { target, value } => {
                    let v = self.value(value, cx, env, flow)?;
                    self.assign(target, v, cx, env, flow, span)?;
                }
                // `within d` ist eine Latenzforderung ohne Wirkung im Lauf
                // (9.4.5, Pruefung 61).
                StmtKind::Check { cond, message, confirm, target, kind, .. } => {
                    let c = self.expr(cond, cx, env, flow)?;
                    // `for d` (5.6): erst eine Verletzung ueber die ganze Frist faultet.
                    let failed = match confirm {
                        None => c.clone().not(),
                        Some(confirm) => {
                            let d = self.const_int(&confirm.duration)?;
                            let period = i64::from(machine_period(self.machine(m))).saturating_mul(self.p.config.tick);
                            let loc = self.loc_viol(m, confirm.site.0 as usize, &self.loop_path.clone());
                            let viol = env[&loc].clone();
                            let grown = Term::bin(Op::Add, viol.clone(), Term::int(period));
                            let due = Term::bin(Op::Ge, grown.clone(), Term::int(d));
                            let next = Term::ite(c.clone(), Term::int(0), Term::ite(due.clone(), Term::int(0), grown));
                            env.insert(loc.clone(), Term::ite(flow.alive.clone(), next, viol));
                            if !self.confirms.iter().any(|(l, _)| *l == loc) {
                                self.confirms.push((loc, d));
                            }
                            Term::and(vec![c.clone().not(), due])
                        }
                    };
                    let fail = Term::and(vec![flow.alive.clone(), failed.clone()]);
                    let word = if *kind == takt_mir::stmt::CheckKind::Check { "check" } else { "expect" };
                    let key = (span.start, span.end, word.to_string());
                    self.sites.entry(key.clone()).or_default().push(fail.clone());
                    self.site_info.insert(key, (span, self.machine(m).name.clone()));
                    let fault = if *kind == takt_mir::stmt::CheckKind::Check {
                        FaultKind::CheckFailed
                    } else {
                        FaultKind::Expect
                    };
                    let cause = self.stated(fault, message.as_ref(), "check verletzt", cx, env, flow, span)?;
                    flow.exits.push(Exit { cond: fail, kind: ExitKind::Fault(*target, cause) });
                    flow.alive = Term::and(vec![flow.alive.clone(), failed.not()]);
                }
                // `every d` (5.8): der Rumpf, sobald die Uhr den naechsten
                // Zeitpunkt erreicht; der rueckt dann um `d` weiter.
                StmtKind::Every { period, counter, body } => {
                    let d = self.const_int(period)?;
                    let site = self.machine(m).layout.every_counters[counter.index()];
                    let clock = match (site.state, cx.leaf) {
                        (Some(_), Some(leaf)) => {
                            let ticks = i64::from(machine_period(self.machine(m))).saturating_mul(self.p.config.tick);
                            Term::bin(Op::Mul, env[&self.loc_timer(m, leaf)].clone(), Term::int(ticks))
                        }
                        (Some(_), None) => return no("`every` ohne Zustand", span),
                        (None, _) => self.now.clone(),
                    };
                    let loc = self.loc_every(m, counter.index(), &self.loop_path.clone());
                    let stored = env[&loc].clone();
                    let next = Term::ite(Term::bin(Op::Lt, stored.clone(), Term::int(0)), Term::int(d), stored.clone());
                    let fire = Term::bin(Op::Ge, clock, next.clone());
                    let moved = Term::ite(fire.clone(), Term::bin(Op::Add, next.clone(), Term::int(d)), next);
                    env.insert(loc, Term::ite(flow.alive.clone(), moved, stored));
                    let mut env_b = env.clone();
                    let mut fb = Flow::new(Term::and(vec![flow.alive.clone(), fire.clone()]));
                    self.block(body, cx, &mut env_b, &mut fb)?;
                    *env = ite_env(&fire, &env_b, env);
                    flow.exits.extend(fb.exits);
                    flow.alive = Term::or(vec![Term::and(vec![flow.alive.clone(), fire.not()]), fb.alive]);
                }
                StmtKind::Goto(t) => {
                    if cx.mode == Mode::Run {
                        flow.exits.push(Exit { cond: flow.alive.clone(), kind: ExitKind::Goto(*t) });
                        flow.alive = Term::bool(false);
                    }
                }
                StmtKind::Abort { message } => {
                    let cause = self.stated(FaultKind::Abort, message.as_ref(), "abort", cx, env, flow, span)?;
                    self.aborts.push(flow.alive.clone());
                    flow.exits.push(Exit { cond: flow.alive.clone(), kind: ExitKind::Abort(cause) });
                    flow.alive = Term::bool(false);
                }
                StmtKind::If { cond, then, otherwise } => {
                    let c = self.expr(cond, cx, env, flow)?;
                    self.flush_binds(env)?;
                    let mut env_t = env.clone();
                    let mut ft = Flow::new(Term::and(vec![flow.alive.clone(), c.clone()]));
                    self.block(then, cx, &mut env_t, &mut ft)?;
                    let mut env_e = env.clone();
                    let mut fe = Flow::new(Term::and(vec![flow.alive.clone(), c.clone().not()]));
                    self.block(otherwise, cx, &mut env_e, &mut fe)?;
                    *env = ite_env(&c, &env_t, &env_e);
                    flow.exits.extend(ft.exits);
                    flow.exits.extend(fe.exits);
                    flow.alive = Term::or(vec![ft.alive, fe.alive]);
                }
                StmtKind::Match { subject, arms } => {
                    let subj = self.value(subject, cx, env, flow)?;
                    let mut remaining = flow.alive.clone();
                    let mut merged = env.clone();
                    let mut alive_out = Term::bool(false);
                    for arm in arms {
                        let (cond, payload) = self.arm(&arm.pattern, &subj, subject.ty, cx, env, flow, arm.span)?;
                        let take = Term::and(vec![remaining.clone(), cond.clone()]);
                        let mut env_a = env.clone();
                        // Die gebundenen Felder sind gehobene Variablen der Maschine.
                        for (slot, v) in arm.pattern.bound().iter().zip(payload) {
                            let ty = self.machine(m).vars[slot.index()].ty;
                            self.put(&mut env_a, &self.loc_var(m, *slot), ty, v, &take, arm.span)?;
                        }
                        let mut fa = Flow::new(take.clone());
                        self.block(&arm.body, cx, &mut env_a, &mut fa)?;
                        merged = ite_env(&take, &env_a, &merged);
                        alive_out = Term::or(vec![alive_out, fa.alive]);
                        flow.exits.extend(fa.exits);
                        remaining = Term::and(vec![remaining, cond.not()]);
                    }
                    *env = merged;
                    flow.alive = Term::or(vec![alive_out, remaining]);
                }
                StmtKind::ForRange { var, count, body } => {
                    let n = self.const_int(count)?.max(0);
                    self.unrolled = self.unrolled.saturating_add(n);
                    if self.unrolled > UNROLL_LIMIT {
                        return no(format!("mehr als {UNROLL_LIMIT} Durchlaeufe von Schleifen auf einem Pfad"), span);
                    }
                    let loc = self.loc_var(m, *var);
                    self.breaks.push(Vec::new());
                    for i in 0..n {
                        env.insert(loc.clone(), Term::ite(flow.alive.clone(), Term::int(i), env[&loc].clone()));
                        self.loop_path.push(i);
                        self.block(body, cx, env, flow)?;
                        self.loop_path.pop();
                    }
                    self.left_loop(flow);
                }
                // Ueber ein Array: die Schleifenvariable traegt je Durchlauf ein Element.
                StmtKind::ForEach { vars: ForVars::One(var), iter, body }
                    if matches!(self.p.types.get(iter.ty), Type::Array { .. }) =>
                {
                    let V::Node(items) = self.value(iter, cx, env, flow)? else { return no("`for … in`", span) };
                    self.unrolled = self.unrolled.saturating_add(items.len() as i64);
                    if self.unrolled > UNROLL_LIMIT {
                        return no(format!("mehr als {UNROLL_LIMIT} Durchlaeufe von Schleifen auf einem Pfad"), span);
                    }
                    let (loc, ty) = (self.loc_var(m, *var), self.machine(m).vars[var.index()].ty);
                    self.breaks.push(Vec::new());
                    for (k, item) in items.into_iter().enumerate() {
                        self.put(env, &loc, ty, item, &flow.alive.clone(), span)?;
                        self.loop_path.push(k as i64);
                        self.block(body, cx, env, flow)?;
                        self.loop_path.pop();
                    }
                    self.left_loop(flow);
                }
                StmtKind::ForEach { vars: ForVars::One(var), iter, body }
                    if matches!(self.p.types.get(iter.ty), Type::Bytes { .. } | Type::Vec { .. }) =>
                {
                    let (loc, ty) = (self.loc_var(m, *var), self.machine(m).vars[var.index()].ty);
                    self.for_collection(&loc, ty, iter, body, cx, env, flow, span)?;
                }
                StmtKind::ForEach { vars: ForVars::Pair(k, v), iter, body }
                    if matches!(self.p.types.get(iter.ty), Type::Map { .. }) =>
                {
                    let vars = &self.machine(m).vars;
                    let (k_ty, v_ty) = (vars[k.index()].ty, vars[v.index()].ty);
                    let (k_loc, v_loc) = (self.loc_var(m, *k), self.loc_var(m, *v));
                    self.for_map((&k_loc, k_ty, &v_loc, v_ty), iter, body, cx, env, flow, span)?;
                }
                // Ueber das Fenster eines Stroms (8.7).
                StmtKind::ForEach { vars: ForVars::One(var), iter, body }
                    if matches!(self.p.types.get(iter.ty), Type::Stream(_)) =>
                {
                    let Some(key) = stream::stream_of(iter) else { return no("`for` ueber diesen Strom", span) };
                    self.for_window(*var, key, body, cx, env, flow, span)?;
                }
                StmtKind::Break => match self.breaks.last_mut() {
                    Some(frame) => {
                        frame.push(flow.alive.clone());
                        flow.alive = Term::bool(false);
                    }
                    None => return no("`break` ausserhalb einer Schleife", span),
                },
                StmtKind::Send { stream, value, .. } => self.send(*stream, value, cx, env, flow, span)?,
                StmtKind::Skip(key) => self.skip(*key, cx, flow, span)?,
                StmtKind::Raise(sig) => {
                    let loc = self.loc_sig(m, sig.index());
                    let old = env.get(&loc).cloned().unwrap_or_else(|| Term::bool(false));
                    env.insert(loc, Term::ite(flow.alive.clone(), Term::bool(true), old));
                }
                StmtKind::MethodCall { target, receiver, method, args } => {
                    self.method_call(target.as_ref(), receiver, *method, args, cx, env, flow, span)?;
                }
                // 9.8: Die rechten Seiten jetzt, die Schreibvorgaenge nach `T`.
                StmtKind::At { time, body } => {
                    let t = self.expr(time, cx, env, flow)?;
                    for a in &body.stmts {
                        let StmtKind::Assign { target: Place::Output(c), value } = &a.kind else {
                            return no("`at` mit anderem als Output-Zuweisungen", a.span);
                        };
                        let v = self.expr(value, cx, env, flow)?;
                        self.schedule(*c, &t, v, env, flow, a.span)?;
                    }
                }
                StmtKind::Cancel(c) => self.cancel_schedule(*c, &flow.alive, env)?,
                StmtKind::Observe(_) | StmtKind::Pass => {}
                other @ (StmtKind::ForEach { .. }
                | StmtKind::Return(_)
                | StmtKind::Job { .. }
                | StmtKind::Arm { .. }) => {
                    return no(format!("Anweisung {}", stmt_name(other)), span);
                }
            }
            self.flush_binds(env)?;
        }
        Ok(())
    }

    /// Setzt eine Lokale, wo `alive` gilt.
    fn set_local(&self, cx: &mut Cx<'_>, var: VarId, v: V, alive: &Term, span: Span) -> R<()> {
        let locals = cx.locals.as_mut().ok_or_else(|| Unsupported { what: "Lokale".into(), span })?;
        let Some(old) = locals.get(&var).cloned() else { return no("Lokale", span) };
        locals.insert(var, V::ite(alive, v, old));
        Ok(())
    }

    /// Zaehlt ausgerollte Durchlaeufe gegen [`UNROLL_LIMIT`].
    fn unroll_steps(&mut self, n: i64, span: Span) -> R<()> {
        self.unrolled = self.unrolled.saturating_add(n);
        if self.unrolled > UNROLL_LIMIT {
            return no(format!("mehr als {UNROLL_LIMIT} Durchlaeufe von Schleifen auf einem Pfad"), span);
        }
        Ok(())
    }

    /// Nach einer ausgerollten Schleife: Wer sie mit `break` verliess, geht
    /// hinter ihr weiter.
    fn left_loop(&mut self, flow: &mut Flow) {
        let broke = self.breaks.pop().unwrap_or_default();
        flow.alive = Term::or(std::iter::once(flow.alive.clone()).chain(broke).collect());
    }

    // ------------------------------------------------------------ Bloecke (5.7)

    /// Block und Felder (`s.<m>.v.<instanz>.<feld>`: Parameter, dann
    /// Zustand) einer Instanz; keine bei Instanz-Arrays.
    fn block_fields(&self, m: MachineId, inst: VarId) -> Option<(BlockId, Vec<(String, TypeId)>)> {
        let machine = self.machine(m);
        let bi = machine.layout.block_instances.iter().find(|b| b.var == inst)?;
        if bi.count > 1 {
            return None;
        }
        let def = &self.p.blocks[bi.block.index()];
        let base = format!("s.{}.v.{}", machine.name, machine.vars[inst.index()].name);
        let fields = def
            .params
            .iter()
            .map(|p| (format!("{base}.{}", p.name), p.ty))
            .chain(def.state_vars.iter().map(|v| (format!("{base}.{}", v.name), v.ty)))
            .collect();
        Some((bi.block, fields))
    }

    /// Setzt den Zustand einer Instanz aus ihren Parametern (`reset`, 5.7).
    fn block_reset(
        &mut self,
        def: &BlockDef,
        fields: &[(String, TypeId)],
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
    ) -> R<()> {
        let mut locals: BTreeMap<VarId, V> = BTreeMap::new();
        for (i, (loc, ty)) in fields.iter().enumerate().take(def.params.len()) {
            let shape = self.shape(*ty, def.span)?;
            locals.insert(VarId(i as u32), self.load(env, loc, &shape, def.span)?);
        }
        let types = fields.iter().enumerate().map(|(i, (_, ty))| (VarId(i as u32), *ty)).collect();
        let outer = std::mem::replace(&mut self.local_types, types);
        for (j, sv) in def.state_vars.iter().enumerate() {
            let icx = Cx {
                m: cx.m,
                leaf: cx.leaf,
                mode: cx.mode,
                pre: cx.pre,
                active: cx.active,
                locals: Some(locals.clone()),
            };
            let v = match &sv.init {
                Some(e) => self.value(e, &icx, env, flow)?,
                None => self.zero_of(sv.ty, sv.span)?,
            };
            let id = VarId((def.params.len() + j) as u32);
            locals.insert(id, v.clone());
            let loc = fields[def.params.len() + j].0.clone();
            self.put(env, &loc, sv.ty, v, &flow.alive.clone(), sv.span)?;
        }
        self.local_types = outer;
        Ok(())
    }

    /// `b.step(args)`, `b.reset()`, `b.<methode>(args)`: der Rumpf eingebettet,
    /// der Zustand der Instanz in den Feldern; `requires` des `step` ist eine
    /// Pruefstelle ohne Laufzeitpruefung (5.7).
    #[allow(clippy::too_many_arguments)]
    #[deny(clippy::wildcard_enum_match_arm)]
    fn method_call(
        &mut self,
        target: Option<&Place>,
        receiver: &Place,
        method: Method,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &mut Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<()> {
        let m = cx.m.expect("Maschine");
        if matches!(method, Method::Push | Method::Append | Method::Clear | Method::Insert | Method::Remove) {
            return self.collection_method(target, receiver, method, args, cx, env, flow, span);
        }
        let Place::Var(inst) = receiver else { return no("Methodenaufruf auf diesem Ziel", span) };
        let Some((bid, fields)) = self.block_fields(m, *inst) else { return no("Methodenaufruf", span) };
        let def = self.p.blocks[bid.index()].clone();
        let mut arg_terms = Vec::new();
        for a in args {
            arg_terms.push(self.value(a, cx, env, flow)?);
        }
        let fid = match method {
            Method::Reset => return self.block_reset(&def, &fields, cx, env, flow),
            Method::Step => def.step.ok_or_else(|| Unsupported { what: "`step`".into(), span })?,
            Method::Block(f) => f,
            Method::Push | Method::Append | Method::Insert | Method::Remove | Method::Clear => {
                return no("Sammlungsmethode", span);
            }
        };
        let f = self.p.fns[fid.index()].clone();
        let base = fields.len() as u32;
        let mut locals: BTreeMap<VarId, V> = BTreeMap::new();
        for (i, (loc, ty)) in fields.iter().enumerate() {
            let shape = self.shape(*ty, span)?;
            locals.insert(VarId(i as u32), self.load(env, loc, &shape, span)?);
        }
        for (i, local) in f.locals.iter().enumerate() {
            let value = match arg_terms.get(i) {
                Some(a) => a.clone(),
                None => self.zero_of(local.ty, local.span)?,
            };
            locals.insert(VarId(base + i as u32), value);
        }
        let call_alive = flow.alive.clone();
        if method == Method::Step {
            let rcx = Cx {
                m: cx.m,
                leaf: cx.leaf,
                mode: cx.mode,
                pre: cx.pre,
                active: cx.active,
                locals: Some(locals.clone()),
            };
            for r in &def.requires {
                let mut rflow = Flow::new(call_alive.clone());
                let c = self.expr(r, &rcx, env, &mut rflow)?;
                let fail = Term::and(vec![call_alive.clone(), !c]);
                let key = (span.start, span.end, "requires".to_string());
                self.sites.entry(key.clone()).or_default().push(fail);
                self.site_info.insert(key, (span, self.machine(m).name.clone()));
            }
        }
        let mut icx =
            Cx { m: cx.m, leaf: cx.leaf, mode: Mode::Entry, pre: cx.pre, active: cx.active, locals: Some(locals) };
        let mut sub = Flow::new(call_alive.clone());
        let mut ret = match f.ret {
            Some(t) => self.zero_of(t, span)?,
            None => V::Leaf(Term::bool(false)),
        };
        let types = fields
            .iter()
            .map(|(_, ty)| *ty)
            .chain(f.locals.iter().map(|l| l.ty))
            .enumerate()
            .map(|(i, ty)| (VarId(i as u32), ty))
            .collect();
        let outer = std::mem::replace(&mut self.local_types, types);
        self.fn_block(&f.body, &mut icx, env, &mut sub, &mut ret)?;
        self.local_types = outer;
        let faults = Term::or(sub.exits.iter().map(|x| x.cond.clone()).collect());
        flow.exits.extend(sub.exits);
        // Der Zustand bleibt, was der Rumpf bis zu einem Fault schrieb.
        let locals = icx.locals.expect("Lokale");
        for (i, (loc, ty)) in fields.iter().enumerate().skip(def.params.len()) {
            let v = locals[&VarId(i as u32)].clone();
            self.put(env, loc, *ty, v, &call_alive, span)?;
        }
        flow.alive = Term::and(vec![call_alive, !faults]);
        if let Some(t) = target {
            self.assign(t, ret, cx, env, flow, span)?;
        }
        Ok(())
    }

    /// Die Vertraege aller Bloecke als Ziele (5.7, B2).
    fn contract_goals(&mut self) -> R<Vec<ContractGoal>> {
        let mut out = Vec::new();
        for def in self.p.blocks.clone() {
            let (Some(fid), false) = (def.step, def.ensures.is_empty()) else { continue };
            let f = self.p.fns[fid.index()].clone();
            let mut vars: Vec<(String, Sort)> = Vec::new();
            let mut locals: BTreeMap<VarId, V> = BTreeMap::new();
            let mut assume = Vec::new();
            let mut typed = |this: &mut Self, name: String, ty: TypeId, id: VarId| -> R<()> {
                let sort = this.sort_of(ty, def.span)?;
                let t = Term::var(name.clone(), sort);
                if let Some(inv) = this.type_invariant(t.clone(), ty) {
                    assume.push(inv);
                }
                vars.push((name, sort));
                locals.insert(id, V::Leaf(t));
                Ok(())
            };
            let mut ok = true;
            for (i, p) in def.params.iter().enumerate() {
                ok &= typed(self, format!("c.{}.{}", def.name, p.name), p.ty, VarId(i as u32)).is_ok();
            }
            for (j, v) in def.state_vars.iter().enumerate() {
                ok &= typed(self, format!("c.{}.{}", def.name, v.name), v.ty, VarId((def.params.len() + j) as u32))
                    .is_ok();
            }
            let base = (def.params.len() + def.state_vars.len()) as u32;
            for (i, p) in f.params.iter().enumerate() {
                ok &= typed(self, format!("c.{}.step.{}", def.name, p.name), p.ty, VarId(base + i as u32)).is_ok();
            }
            if !ok {
                self.note(&format!("Vertrag von `{}` nicht kodiert: ein Typ ausserhalb der Reichweite", def.name));
                continue;
            }
            for (i, local) in f.locals.iter().enumerate().skip(f.params.len()) {
                let Ok(zero) = self.zero_of(local.ty, local.span) else { continue };
                locals.insert(VarId(base + i as u32), zero);
            }
            let pre = Env::new();
            let actives = BTreeMap::new();
            let mut cx =
                Cx { m: None, leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: Some(locals) };
            let mut flow = Flow::new(Term::bool(true));
            let mut env = Env::new();
            for r in &def.requires {
                let t = self.expr(r, &cx, &env, &mut flow)?;
                assume.push(t);
            }
            let Some(ret_ty) = f.ret else { continue };
            let Ok(mut ret) = self.zero_of(ret_ty, f.span) else { continue };
            let types = def
                .params
                .iter()
                .map(|p| p.ty)
                .chain(def.state_vars.iter().map(|v| v.ty))
                .chain(f.locals.iter().map(|l| l.ty))
                .enumerate()
                .map(|(i, ty)| (VarId(i as u32), ty))
                .collect();
            let outer = std::mem::replace(&mut self.local_types, types);
            self.fn_block(&f.body, &mut cx, &mut env, &mut flow, &mut ret)?;
            self.local_types = outer;
            let faults = Term::or(flow.exits.iter().map(|x| x.cond.clone()).collect());
            // `result` ist die Lokale hinter den Schrittparametern.
            cx.locals.as_mut().expect("Lokale").insert(VarId(base + f.params.len() as u32), ret);
            let mut ensures = Vec::new();
            for e in &def.ensures {
                let mut eflow = Flow::new(Term::bool(true));
                ensures.push(self.expr(e, &cx, &env, &mut eflow)?);
            }
            let violation = Term::and(vec![Term::and(assume), !faults, !Term::and(ensures)]);
            out.push(ContractGoal { block: def.name.clone(), span: def.span, vars, violation });
        }
        Ok(out)
    }

    // ------------------------------------------------------------ Zustandswechsel

    /// `switch` (9.3): Austritte innen nach aussen, Eintritte aussen nach
    /// innen, Timer und zustandslokale Variablen frisch, Entry-Schleifen.
    /// Ein Fault in einem dieser Bloecke wird vom neuen Blatt aus
    /// aufgeloest, wie `resolve_m` es tut (Lemma 9.3.1 begrenzt die Tiefe).
    /// `under`: die Bedingung, unter der der Wechsel stattfindet — sie
    /// gehoert in jede Feuerbedingung, die seine Bloecke aufzeichnen.
    fn switch(
        &mut self,
        cx: &Cx<'_>,
        from: Option<StateId>,
        target: Target,
        env: &mut Env,
        depth: u32,
        under: &Term,
    ) -> R<()> {
        let m = cx.m.expect("Maschine");
        let machine = self.machine(m).clone();
        if depth > machine.states.len() as u32 + 2 {
            return no("Fault-Pfade tiefer als der Fault-Wald (Lemma 9.3.1)", machine.span);
        }
        let old = from.map(|s| self.chain_to(m, s)).unwrap_or_default();
        let new = match target {
            Target::Faulted => Vec::new(),
            Target::State(s) => self.descend(m, s),
            Target::Fault(_) => return no("Timeout-Fault einer Sequenz", machine.span),
        };
        let mut common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        if let Target::State(s) = target {
            common = common.min(self.chain_to(m, s).len() - 1);
        }
        // (1) Die neue Konfiguration steht, bevor ein Block laeuft.
        let new_leaf = new.last().copied();
        env.insert(self.loc_faulted(m), Term::bool(target == Target::Faulted));
        match new_leaf {
            Some(leaf) => {
                env.insert(self.loc_leaf(m), Term::int(self.code(m, leaf)));
            }
            None => {
                if let Some(code) = self.faulted_code(m) {
                    env.insert(self.loc_leaf(m), Term::int(code));
                }
            }
        }
        let entry = cx.entry();
        let mut flow = Flow::new(under.clone());
        for s in old[common..].iter().rev() {
            self.block(&machine.states[s.index()].exit, &entry, env, &mut flow)?;
        }
        // Ein verlassener Zustand vergisst seine Zeit: Gelesen wird sie nur,
        // solange er aktiv ist (`time_in_state`, `after`), und beim Eintritt
        // beginnt sie neu. So ist jeder inaktive Timer null, ein Lemma fuer
        // den Induktionsschritt.
        for s in &old[common..] {
            env.insert(self.loc_timer(m, *s), Term::int(0));
        }
        if target == Target::Faulted {
            for (i, c) in self.p.channels.iter().enumerate() {
                if c.owner != Some(m) || c.dir != Direction::Output {
                    continue;
                }
                if let Some(safe) = c.attrs.safe.clone() {
                    let v = self.const_value(&safe)?;
                    let (loc, ty) = (self.loc_out(ChannelId(i as u32)), c.ty);
                    self.put(env, &loc, ty, v, &flow.alive.clone(), safe.span)?;
                }
            }
        } else {
            // Was betreten wird, liest `time_in_state` des neuen Blatts; die
            // exit-Bloecke davor lasen noch das verlassene.
            let entry = Cx { leaf: new_leaf, ..cx.entry() };
            let entered: Vec<StateId> = new[common..].to_vec();
            for s in &entered {
                let loc = self.loc_timer(m, *s);
                let old = env[&loc].clone();
                env.insert(loc, Term::ite(flow.alive.clone(), Term::int(0), old));
                // Bestaetigungs- und `every`-Zaehler des Zustands (9.3, Schritt 3).
                let paths = self.paths_of(m)?;
                for (i, _) in machine.layout.viol_sites.iter().enumerate().filter(|(_, c)| c.state == Some(*s)) {
                    for p in &paths.viol[i] {
                        let loc = self.loc_viol(m, i, p);
                        let old = env[&loc].clone();
                        env.insert(loc, Term::ite(flow.alive.clone(), Term::int(0), old));
                    }
                }
                for (i, _) in machine.layout.every_counters.iter().enumerate().filter(|(_, c)| c.state == Some(*s)) {
                    for p in &paths.every[i] {
                        let loc = self.loc_every(m, i, p);
                        let old = env[&loc].clone();
                        env.insert(loc, Term::ite(flow.alive.clone(), Term::int(-1), old));
                    }
                }
                for v in machine.states[s.index()].vars.clone() {
                    let def = machine.vars[v.index()].clone();
                    let value = match &def.init {
                        Some(e) => self.value(e, &entry, env, &mut flow)?,
                        None => self.zero_of(def.ty, def.span)?,
                    };
                    let loc = self.loc_var(m, v);
                    self.put(env, &loc, def.ty, value, &flow.alive.clone(), def.span)?;
                }
            }
            for s in &entered {
                self.block(&machine.states[s.index()].enter, &entry, env, &mut flow)?;
            }
            if from.is_none() {
                self.block(&machine.loop_block, &entry, env, &mut flow)?;
            }
            for s in &entered {
                self.block(&machine.states[s.index()].loop_block, &entry, env, &mut flow)?;
            }
        }
        if flow.exits.is_empty() {
            return Ok(());
        }
        let base = env.clone();
        let mut merged = base.clone();
        for exit in &flow.exits {
            if exit.cond.is_bool(false) {
                continue;
            }
            let applied = self.resolve(cx, new_leaf, exit, &base, depth + 1)?;
            merged = ite_env(&exit.cond, &applied, &merged);
        }
        *env = merged;
        Ok(())
    }

    /// Wendet einen Ausgang an (`resolve_m`); `leaf` ist das Blatt, von dem
    /// aus gewechselt wird — keines in `FAULTED`.
    fn resolve(&mut self, cx: &Cx<'_>, leaf: Option<StateId>, exit: &Exit, env: &Env, depth: u32) -> R<Env> {
        let m = cx.m.expect("Maschine");
        // `FAULTED` ist die Senke des Fault-Walds (5.3): Ein Fault auf dem
        // Weg hinein fuehrt dorthin zurueck.
        let fault_target = match leaf {
            Some(l) => self.fault_target(m, l),
            None => Target::Faulted,
        };
        let mut out = env.clone();
        match &exit.kind {
            ExitKind::Goto(t) => {
                out.insert(self.loc_latched(m), Term::bool(false));
                // Der Timeout einer Sequenz nimmt den Fault-Pfad des Zustands (6.2).
                let t = match t {
                    Target::Fault(kind) => {
                        self.record_fault(m, &self.cause(*kind, Span::default()), &mut out)?;
                        self.clear_schedules(m, &mut out)?;
                        fault_target
                    }
                    other => *other,
                };
                self.switch(cx, leaf, t, &mut out, depth, &exit.cond)?;
            }
            ExitKind::Fault(explicit, cause) => {
                self.record_fault(m, cause, &mut out)?;
                self.clear_schedules(m, &mut out)?;
                let t = explicit.unwrap_or(fault_target);
                self.switch(cx, leaf, t, &mut out, depth, &exit.cond)?;
            }
            ExitKind::Abort(cause) => {
                let latched = env[&self.loc_latched(m)].clone();
                let mut sw = env.clone();
                sw.insert(self.loc_latched(m), Term::bool(true));
                self.record_fault(m, cause, &mut sw)?;
                self.clear_schedules(m, &mut sw)?;
                let under = Term::and(vec![exit.cond.clone(), !latched.clone()]);
                self.switch(cx, leaf, fault_target, &mut sw, depth, &under)?;
                out = ite_env(&latched, env, &sw);
            }
        }
        Ok(out)
    }

    /// Ein Tick einer Maschine (`step_m`), je Blatt ein Zweig.
    fn step_machine(
        &mut self,
        m: MachineId,
        active: &Term,
        pre: &Env,
        actives: &BTreeMap<MachineId, Term>,
        cur: &mut Env,
    ) -> R<()> {
        let machine = self.machine(m).clone();
        let base = cur.clone();
        let faulted = base.get(&self.loc_faulted(m)).cloned().expect("faulted");
        let leaf_now = base.get(&self.loc_leaf(m)).cloned().expect("leaf");
        let alive0 = Term::and(vec![active.clone(), faulted.not()]);
        let mut merged = base.clone();
        let period = i64::from(machine.period.max(1)).saturating_mul(self.p.config.tick);
        for leaf in self.leaves(m) {
            let is = Term::and(vec![alive0.clone(), Term::eq(leaf_now.clone(), Term::int(self.code(m, leaf)))]);
            let cx = Cx { m: Some(m), leaf: Some(leaf), mode: Mode::Run, pre, active: actives, locals: None };
            self.unrolled = 0;
            let mut env = base.clone();
            let mut flow = Flow::new(is.clone());
            // Ein vorgemerkter `StreamOverflow` kommt vor allem anderen (9.6).
            if self.has_pending(m) {
                let at = self.loc_pending(m);
                let pending = env[&at].clone();
                // Zugestellt hat der Ueberlauf keine Stelle (5.3).
                let cause = self.cause(FaultKind::StreamOverflow, Span::default());
                flow.exits.push(Exit {
                    cond: Term::and(vec![is.clone(), pending.clone()]),
                    kind: ExitKind::Fault(None, cause),
                });
                flow.alive = Term::and(vec![is.clone(), pending.not()]);
                env.insert(at, Term::bool(false));
            }
            // Handler laufen unmittelbar nach dem `loop:` ihrer Ebene (8.7).
            self.block(&machine.loop_block, &cx, &mut env, &mut flow)?;
            self.dispatch(&machine.handlers, &cx, &mut env, &mut flow)?;
            let chain = self.chain_to(m, leaf);
            for s in &chain {
                self.block(&machine.states[s.index()].loop_block, &cx, &mut env, &mut flow)?;
                self.dispatch(&machine.states[s.index()].handlers, &cx, &mut env, &mut flow)?;
            }
            for s in &chain {
                for t in &machine.states[s.index()].transitions {
                    let mut stream_hit = None;
                    let fired = match &t.trigger {
                        TransTrigger::When(takt_mir::machine::Guard::Expr(e)) => self.expr(e, &cx, &env, &mut flow)?,
                        TransTrigger::When(g) => {
                            let hit = self.stream_guard(g, &cx, &env, &mut flow)?;
                            let fired = hit.fired.clone();
                            stream_hit = Some(hit);
                            fired
                        }
                        TransTrigger::After(d) => {
                            let ns = self.const_int(d)?;
                            let needed = ((ns + period - 1) / period).max(1);
                            let timer = env.get(&self.loc_timer(m, *s)).cloned().expect("Timer");
                            Term::bin(Op::Ge, timer, Term::int(needed))
                        }
                    };
                    self.flush_binds(&mut env)?;
                    let take = Term::and(vec![flow.alive.clone(), fired]);
                    if let Some(hit) = stream_hit {
                        self.take_guard(hit, m, &take, &mut env)?;
                    }
                    let entry = cx.entry();
                    let mut sub = Flow::new(take.clone());
                    self.block(&t.actions, &entry, &mut env, &mut sub)?;
                    flow.exits.extend(sub.exits);
                    flow.exits.push(Exit { cond: sub.alive, kind: ExitKind::Goto(t.target) });
                    flow.alive = Term::and(vec![flow.alive.clone(), take.not()]);
                }
            }
            let mut out = env.clone();
            for exit in &flow.exits {
                if exit.cond.is_bool(false) {
                    continue;
                }
                let applied = self.resolve(&cx, Some(leaf), exit, &env, 0)?;
                out = ite_env(&exit.cond, &applied, &out);
            }
            merged = ite_env(&is, &out, &merged);
        }
        *cur = merged;
        Ok(())
    }

    /// Die Abort-Phase (5.4): jede nicht gelatchte Maschine nimmt ihren
    /// Fault-Pfad, wenn in diesem Tick ein `abort` lief.
    fn abort_phase(&mut self, raised: &Term, pre: &Env, actives: &BTreeMap<MachineId, Term>, cur: &mut Env) -> R<()> {
        for &m in &self.order.clone() {
            let faulted = cur.get(&self.loc_faulted(m)).cloned().expect("faulted");
            let latched = cur.get(&self.loc_latched(m)).cloned().expect("latched");
            let deliver = Term::and(vec![raised.clone(), faulted.not(), latched.not()]);
            if deliver.is_bool(false) {
                continue;
            }
            let leaf_now = cur.get(&self.loc_leaf(m)).cloned().expect("leaf");
            let base = cur.clone();
            let mut merged = base.clone();
            for leaf in self.leaves(m) {
                let is = Term::and(vec![deliver.clone(), Term::eq(leaf_now.clone(), Term::int(self.code(m, leaf)))]);
                let cx = Cx { m: Some(m), leaf: Some(leaf), mode: Mode::Entry, pre, active: actives, locals: None };
                let mut env = base.clone();
                env.insert(self.loc_latched(m), Term::bool(true));
                self.record_fault(m, &self.cause(FaultKind::Abort, Span::default()), &mut env)?;
                self.clear_schedules(m, &mut env)?;
                let t = self.fault_target(m, leaf);
                self.switch(&cx, Some(leaf), t, &mut env, 0, &is)?;
                merged = ite_env(&is, &env, &merged);
            }
            *cur = merged;
        }
        Ok(())
    }

    /// `advance_counters` (7.2): Timer entlang der neuen Kette, Zaehler.
    fn advance(&mut self, actives: &BTreeMap<MachineId, Term>, cur: &mut Env) {
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let active = actives[&m].clone();
            if machine.period > 1 {
                let loc = self.loc_countdown(m);
                let c = cur[&loc].clone();
                let next = Term::ite(
                    active.clone(),
                    Term::int(i64::from(machine.period) - 1),
                    Term::bin(Op::Sub, c.clone(), Term::int(1)),
                );
                cur.insert(loc, next);
            }
            let faulted = cur[&self.loc_faulted(m)].clone();
            let leaf_now = cur[&self.loc_leaf(m)].clone();
            for (i, _) in machine.states.iter().enumerate() {
                let s = StateId(i as u32);
                let in_chain: Vec<Term> = self
                    .leaves(m)
                    .into_iter()
                    .filter(|l| self.chain_to(m, *l).contains(&s))
                    .map(|l| Term::eq(leaf_now.clone(), Term::int(self.code(m, l))))
                    .collect();
                let cond = Term::and(vec![active.clone(), faulted.clone().not(), Term::or(in_chain)]);
                let loc = self.loc_timer(m, s);
                let t = cur[&loc].clone();
                cur.insert(loc, Term::ite(cond, Term::bin(Op::Add, t.clone(), Term::int(1)), t));
            }
        }
    }

    fn actives(&self, pre: &Env) -> BTreeMap<MachineId, Term> {
        self.order
            .iter()
            .map(|&m| {
                let active = if self.machine(m).period > 1 {
                    Term::eq(pre[&self.loc_countdown(m)].clone(), Term::int(0))
                } else {
                    Term::bool(true)
                };
                (m, active)
            })
            .collect()
    }

    /// Kandidaten fuer Hilfslemmata (Houdini, FB-375): Ist ein Blatt aktiv,
    /// laeuft ein Zaehler mit seiner Zeit — hoechstens eins davor oder
    /// dahinter —, und der Zaehler einer Antwort hat keine offene Pflicht.
    /// Gezaehlt werden die Zaehler der Monitore und die ganzzahligen
    /// Variablen der Maschine.
    fn candidates(&self, pre: &Env) -> Vec<Term> {
        let mut counters: Vec<(Term, bool)> = self
            .monitors
            .iter()
            .flat_map(Monitor::counters)
            .filter_map(|(loc, wait)| pre.get(&loc).map(|t| (t.clone(), wait)))
            .collect();
        let mut out = Vec::new();
        for &m in &self.order {
            let machine = self.machine(m);
            let mine: Vec<(Term, bool)> = machine
                .vars
                .iter()
                .enumerate()
                .filter(|(_, v)| matches!(self.p.types.get(v.ty), Type::Int { .. } | Type::Duration { .. }))
                .filter_map(|(i, _)| pre.get(&self.loc_var(m, VarId(i as u32))).map(|t| (t.clone(), false)))
                .collect();
            counters.extend(mine);
            let faulted = pre[&self.loc_faulted(m)].clone();
            let leaf = pre[&self.loc_leaf(m)].clone();
            for l in self.leaves(m) {
                let active = Term::and(vec![faulted.clone().not(), Term::eq(leaf.clone(), Term::int(self.code(m, l)))]);
                let time = pre[&self.loc_timer(m, l)].clone();
                for (c, wait) in &counters {
                    let mut add = |t: Term| out.push(Term::or(vec![active.clone().not(), t]));
                    for d in [-1, 0, 1] {
                        add(Term::bin(Op::Le, c.clone(), Term::bin(Op::Add, time.clone(), Term::int(d))));
                    }
                    if *wait {
                        add(Term::eq(c.clone(), Term::int(-1)));
                    }
                }
            }
        }
        out
    }

    /// Die Frist, bis zu der der Timer eines aktiven Blatts hoechstens
    /// laeuft: das kleinste `after` des Blatts. Erreicht der Timer sie, feuert
    /// es am Anfang des naechsten Ticks, oder ein Sprung davor verlaesst das
    /// Blatt oder betritt es neu — ein Blatt hat keinen Unterzustand, der es
    /// aktiv hielte. Nur ein `abort` hinter gesetztem Latch beendet den Tick
    /// ohne Wechsel; steht eines in einer Schleife der Kette, gilt die
    /// Frist nicht.
    fn deadline(&mut self, m: MachineId, s: StateId) -> Option<i64> {
        let machine = self.machine(m).clone();
        let state = &machine.states[s.index()];
        if !state.children.is_empty() {
            return None;
        }
        let aborts = |b: &Block| {
            let mut hit = false;
            b.walk(&mut |st| hit |= matches!(st.kind, StmtKind::Abort { .. }));
            hit
        };
        let chain = self.chain_to(m, s);
        let handled = |hs: &[takt_mir::machine::Handler]| hs.iter().any(|h| aborts(&h.body));
        if aborts(&machine.loop_block)
            || handled(&machine.handlers)
            || chain.iter().any(|c| {
                let st = &machine.states[c.index()];
                aborts(&st.loop_block) || handled(&st.handlers)
            })
        {
            return None;
        }
        let period = i64::from(machine.period.max(1)).saturating_mul(self.p.config.tick);
        let mut out: Option<i64> = None;
        for t in &state.transitions {
            if let TransTrigger::After(d) = &t.trigger {
                let ns = self.const_int(d).ok()?;
                let needed = ((ns + period - 1) / period).max(1);
                out = Some(out.map_or(needed, |o| o.min(needed)));
            }
        }
        out
    }

    /// Die laengste Frist eines `after` der kodierten Maschinen und das
    /// laengste Fenster eines Monitors, in Ticks.
    fn horizon(&mut self) -> R<u32> {
        let mut out = self.monitors.iter().map(Monitor::window).max().unwrap_or(0);
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let period = i64::from(machine.period.max(1)).saturating_mul(self.p.config.tick);
            for s in &machine.states {
                for t in &s.transitions {
                    if let TransTrigger::After(d) = &t.trigger {
                        let ns = self.const_int(d)?;
                        out = out.max(((ns + period - 1) / period).max(1));
                    }
                }
            }
        }
        Ok(u32::try_from(out).unwrap_or(u32::MAX))
    }

    /// Wo `sys/next_run` steht und welche Varianten den Lauf beenden (12.7):
    /// der Ort der Variante und ihre Indizes. Nur, wenn eine kodierte
    /// Maschine den Output schreibt.
    fn run_end(&self) -> Option<(String, Vec<i64>)> {
        let (index, variants) = takt_mir::sys::next_run(self.p)?;
        let c = &self.p.channels[index];
        if c.owner.is_some_and(|o| !self.order.contains(&o)) {
            return None;
        }
        let base = self.loc_out(ChannelId(index as u32));
        let loc = if self.composite(c.ty) { format!("{base}.tag") } else { base };
        let ending = variants.iter().enumerate().filter(|(_, (_, end))| end.is_some()).map(|(i, _)| i as i64);
        Some((loc, ending.collect()))
    }

    /// Beendet der committete Zustand `env` den Lauf?
    fn ends(env: &Env, (loc, ending): &(String, Vec<i64>)) -> Term {
        Term::or(ending.iter().map(|i| Term::eq(env[loc].clone(), Term::int(*i))).collect())
    }

    /// Die Outputs, wie sie zu Beginn und nach dem Ende eines Laufs stehen:
    /// `safe`, sonst der Standardwert (`eval_safe_outputs`).
    fn safe_outputs(&mut self) -> R<Vec<(String, TypeId, V)>> {
        let mut out = Vec::new();
        for (i, c) in self.p.channels.clone().iter().enumerate() {
            // Ein Ausgabestrom hat keinen Latch; sein Puffer gehoert dem Treiber (8.8).
            let stream = matches!(self.p.types.get(c.ty), Type::Stream(_));
            if c.dir != Direction::Output || stream || c.owner.is_some_and(|o| !self.order.contains(&o)) {
                continue;
            }
            let value = match &c.attrs.safe {
                Some(e) => self.const_value(e)?,
                None => self.zero_of(c.ty, c.span)?,
            };
            out.push((self.loc_out(ChannelId(i as u32)), c.ty, value));
        }
        Ok(out)
    }

    /// Ein Tick des Systems ueber `pre`. Nach dem Ende eines Laufs (12.7)
    /// laeuft keine Maschine mehr, und die Outputs stehen auf `safe`.
    fn tick(&mut self, pre: &Env) -> R<Env> {
        let mut cur = pre.clone();
        self.now = Term::bin(Op::Add, pre[NOW].clone(), Term::int(self.p.config.tick));
        cur.insert(NOW.into(), self.now.clone());
        for &m in &self.order.clone() {
            for i in 0..self.machine(m).signals.len() {
                cur.insert(self.loc_sig(m, i), Term::bool(false));
            }
        }
        let end = self.run_end();
        let ended = end.as_ref().map(|_| pre[ENDED].clone());
        let running = ended.clone().map_or_else(|| Term::bool(true), Term::not);
        let actives: BTreeMap<MachineId, Term> =
            self.actives(pre).into_iter().map(|(m, a)| (m, Term::and(vec![a, running.clone()]))).collect();
        self.aborts.clear();
        self.deliver(Some(pre), &mut cur)?;
        self.delivered = cur.clone();
        self.windows.clear();
        for &m in &self.order.clone() {
            let active = actives[&m].clone();
            self.step_machine(m, &active, pre, &actives, &mut cur)?;
        }
        if self.scope.is_some() {
            let foreign = self.input("i.abort.foreign".into(), Sort::Bool);
            self.aborts.push(foreign);
        }
        let raised = Term::and(vec![running, Term::or(std::mem::take(&mut self.aborts))]);
        if !raised.is_bool(false) {
            self.abort_phase(&raised, pre, &actives, &mut cur)?;
        }
        self.advance_streams(&mut cur)?;
        self.advance(&actives, &mut cur);
        self.apply_scheduled(&mut cur)?;
        self.drain_tx(&mut cur);
        self.edges_next(pre, &mut cur)?;
        if let (Some(end), Some(ended)) = (end, ended) {
            let now = Enc::ends(&cur, &end);
            for (loc, ty, safe) in self.safe_outputs()? {
                self.put(&mut cur, &loc, ty, safe, &ended, Span::default())?;
            }
            cur.insert(OVER.into(), ended.clone());
            cur.insert(ENDED.into(), Term::or(vec![ended, now]));
        }
        let monitors = self.monitors.clone();
        self.monitors_next(&monitors, Some(pre), &mut cur)?;
        Ok(cur)
    }

    /// Der Anfangszustand: Defaults, `safe`-Outputs, `init_vars`, dann der
    /// erste Eintritt jeder Maschine in Schrittordnung (`Sim::init`).
    fn init(&mut self) -> R<Env> {
        let mut env = Env::new();
        // `now` ist Tick mal Tickdauer, im Tick 0 also null.
        self.now = Term::int(0);
        env.insert(NOW.into(), self.now.clone());
        self.edges_initial(&mut env)?;
        self.streams_initial(&mut env)?;
        self.tx_initial(&mut env);
        self.sched_initial(&mut env)?;
        self.deliver(None, &mut env)?;
        let before = env.clone();
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            env.insert(self.loc_leaf(m), Term::int(self.code(m, machine.initial)));
            env.insert(self.loc_faulted(m), Term::bool(false));
            env.insert(self.loc_latched(m), Term::bool(false));
            if machine.period > 1 {
                env.insert(self.loc_countdown(m), Term::int(0));
            }
            for i in 0..machine.states.len() {
                env.insert(self.loc_timer(m, StateId(i as u32)), Term::int(0));
            }
            for (i, v) in machine.vars.iter().enumerate() {
                if matches!(self.p.types.get(v.ty), Type::Handle(HandleKind::Block(_))) {
                    let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) else {
                        return no("Instanz-Array eines Blocks", v.span);
                    };
                    for (loc, ty) in fields {
                        let zero = self.zero_of(ty, v.span)?;
                        self.init_loc(&mut env, &loc, ty, zero, v.span)?;
                    }
                    continue;
                }
                let zero = self.zero_of(v.ty, v.span)?;
                self.init_loc(&mut env, &self.loc_var(m, VarId(i as u32)), v.ty, zero, v.span)?;
            }
            for i in 0..machine.signals.len() {
                env.insert(self.loc_sig(m, i), Term::bool(false));
            }
            if self.reads_last_fault(m) {
                self.last_fault_initial(m, &mut env)?;
            }
            let paths = self.paths_of(m)?;
            for (i, list) in paths.viol.iter().enumerate() {
                for p in list {
                    env.insert(self.loc_viol(m, i, p), Term::int(0));
                }
            }
            for (i, list) in paths.every.iter().enumerate() {
                for p in list {
                    env.insert(self.loc_every(m, i, p), Term::int(-1));
                }
            }
        }
        for (loc, ty, value) in self.safe_outputs()? {
            self.init_loc(&mut env, &loc, ty, value, Span::default())?;
        }
        // `init_vars` liest Ψ mit den Anfangswerten; die Eintritte lesen
        // frisch (7.2), also gilt jede Maschine als aktiv.
        let actives: BTreeMap<MachineId, Term> = self.order.iter().map(|&m| (m, Term::bool(true))).collect();
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let pre = env.clone();
            let cx = Cx { m: Some(m), leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: None };
            for (i, v) in machine.vars.iter().enumerate() {
                if let Some(Expr { kind: ExprKind::BlockInit { block, args, .. }, .. }) = &v.init {
                    // Eine Instanz (5.7): Parameter aus den Argumenten, Zustand aus
                    // seinen Initialwerten — wie `instantiate_block`.
                    let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) else { continue };
                    let def = self.p.blocks[block.index()].clone();
                    let mut flow = Flow::new(Term::bool(true));
                    for (k, a) in args.iter().enumerate() {
                        let v = self.value(a, &cx, &env, &mut flow)?;
                        self.init_loc(&mut env, &fields[k].0, fields[k].1, v, a.span)?;
                    }
                    self.block_reset(&def, &fields, &cx, &mut env, &mut flow)?;
                    continue;
                }
                if let Some(e) = &v.init {
                    let mut flow = Flow::new(Term::bool(true));
                    let value = self.value(e, &cx, &env, &mut flow)?;
                    if !flow.exits.is_empty() {
                        self.note(
                            "ein Fault in einem Anfangswert ist nicht modelliert (der Interpreter bricht den Lauf ab)",
                        );
                    }
                    self.init_loc(&mut env, &self.loc_var(m, VarId(i as u32)), v.ty, value, e.span)?;
                }
            }
        }
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let pre = env.clone();
            let cx = Cx { m: Some(m), leaf: None, mode: Mode::Entry, pre: &pre, active: &actives, locals: None };
            self.unrolled = 0;
            self.switch(&cx, None, Target::State(machine.initial), &mut env, 0, &Term::bool(true))?;
        }
        self.flush_sends(&mut env)?;
        self.advance(&actives, &mut env);
        self.drain_tx(&mut env);
        self.edges_next(&before, &mut env)?;
        if let Some(end) = self.run_end() {
            env.insert(OVER.into(), Term::bool(false));
            env.insert(ENDED.into(), Enc::ends(&env, &end));
        }
        let monitors = self.monitors.clone();
        self.monitors_next(&monitors, None, &mut env)?;
        Ok(env)
    }

    // ------------------------------------------------------------ Eigenschaften

    /// Die Invariante eines typisierten Orts (3.4): Range, Endlichkeit, Enum.
    fn type_invariant(&self, x: Term, ty: TypeId) -> Option<Term> {
        Some(match self.p.types.get(ty) {
            Type::Int { range: Some(r), .. } | Type::Duration { range: Some(r) } => {
                let (lo, hi) = (self.bound(&r.lo, Sort::Int), self.bound(&r.hi, Sort::Int));
                Term::and(vec![Term::bin(Op::Ge, x.clone(), lo), Term::bin(Op::Le, x, hi)])
            }
            // Ein schmaler Wert liegt in seiner Breite (3.10).
            Type::Int { range: None, width, .. } if width.bits() < 64 => outside(&x, *width).not(),
            Type::Float { range: Some(r), .. } => {
                let s = x.sort();
                let (lo, hi) = (self.bound(&r.lo, s), self.bound(&r.hi, s));
                Term::and(vec![Term::bin(Op::FGe, x.clone(), lo), Term::bin(Op::FLe, x, hi)])
            }
            Type::Float { .. } => Term::app(Op::IsFinite, vec![x]),
            Type::Enum(e) => {
                let n = self.p.enums[e.index()].variants.len() as i64;
                Term::and(vec![Term::bin(Op::Ge, x.clone(), Term::int(0)), Term::bin(Op::Lt, x, Term::int(n))])
            }
            _ => return None,
        })
    }

    /// Invarianten des Zustands aus den Typen (3.4): Blaetter, Zaehler,
    /// Variablen und Outputs in ihren Ranges.
    fn state_invariants(&mut self, pre: &Env) -> Vec<Term> {
        let mut out = Vec::new();
        self.stream_invariants(pre, &mut out);
        self.tx_invariants(pre, &mut out);
        for &m in &self.order.clone() {
            let machine = self.machine(m).clone();
            let leaf = pre[&self.loc_leaf(m)].clone();
            let mut codes: Vec<Term> =
                self.leaves(m).into_iter().map(|l| Term::eq(leaf.clone(), Term::int(self.code(m, l)))).collect();
            if let Some(c) = self.faulted_code(m) {
                codes.push(Term::eq(leaf.clone(), Term::int(c)));
                // Nur der Wechsel nach `FAULTED` setzt diesen Code, und er
                // setzt das Flag mit (`switch`).
                let faulted = pre[&self.loc_faulted(m)].clone();
                out.push(Term::eq(faulted, Term::eq(leaf.clone(), Term::int(c))));
            }
            out.push(Term::or(codes));
            let faulted = pre[&self.loc_faulted(m)].clone();
            for i in 0..machine.states.len() {
                let s = StateId(i as u32);
                let t = pre[&self.loc_timer(m, s)].clone();
                out.push(Term::bin(Op::Ge, t.clone(), Term::int(0)));
                // Nur ein Zustand der aktiven Kette hat eine Zeit.
                let active: Vec<Term> = self
                    .leaves(m)
                    .into_iter()
                    .filter(|l| self.chain_to(m, *l).contains(&s))
                    .map(|l| Term::eq(leaf.clone(), Term::int(self.code(m, l))))
                    .collect();
                let active = Term::and(vec![faulted.clone().not(), Term::or(active)]);
                out.push(Term::or(vec![active.clone(), Term::eq(t.clone(), Term::int(0))]));
                if let Some(n) = self.deadline(m, s) {
                    out.push(Term::or(vec![active.not(), Term::bin(Op::Le, t, Term::int(n))]));
                }
            }
            if machine.period > 1 {
                let c = pre[&self.loc_countdown(m)].clone();
                out.push(Term::and(vec![
                    Term::bin(Op::Ge, c.clone(), Term::int(0)),
                    Term::bin(Op::Lt, c, Term::int(i64::from(machine.period))),
                ]));
            }
            for (i, v) in machine.vars.iter().enumerate() {
                if let Some((_, fields)) = self.block_fields(m, VarId(i as u32)) {
                    for (loc, ty) in fields {
                        self.typed_loc(pre, &loc, ty, &mut out);
                    }
                    continue;
                }
                self.typed_loc(pre, &self.loc_var(m, VarId(i as u32)), v.ty, &mut out);
            }
        }
        for (i, c) in self.p.channels.iter().enumerate() {
            if c.dir != Direction::Output {
                continue;
            }
            self.typed_loc(pre, &self.loc_out(ChannelId(i as u32)), c.ty, &mut out);
        }
        for m in &self.monitors {
            Enc::monitor_invariants(m, pre, &mut out);
        }
        // Ein Bestaetigungszaehler erreicht seine Frist nie: Dort faultet er
        // und beginnt neu.
        for (loc, d) in &self.confirms {
            let v = pre[loc].clone();
            out.push(Term::and(vec![
                Term::bin(Op::Ge, v.clone(), Term::int(0)),
                Term::bin(Op::Lt, v, Term::int((*d).max(1))),
            ]));
        }
        // `FAULTED` ist die Senke (5.3): Der Eintritt setzt die eigenen
        // Outputs auf `safe`, danach schreibt die Maschine nichts mehr. Ohne
        // das Lemma bliebe ein `FAULTED` mit offenem Ventil fuer jede Tiefe
        // ein Gegenbeispiel des Induktionsschritts.
        for &m in &self.order.clone() {
            let faulted = pre[&self.loc_faulted(m)].clone();
            for (i, c) in self.p.channels.clone().iter().enumerate() {
                let Some(safe) = c.attrs.safe.as_ref().filter(|_| c.dir == Direction::Output && c.owner == Some(m))
                else {
                    continue;
                };
                let (Ok(safe), Ok(shape)) = (self.const_value(safe), self.shape(c.ty, c.span)) else { continue };
                let Ok(now) = self.load(pre, &self.loc_out(ChannelId(i as u32)), &shape, c.span) else { continue };
                out.push(Term::or(vec![faulted.clone().not(), Enc::equal(&now, &safe)]));
            }
        }
        out
    }

    /// Die Raender der Inputs (3.5): jeder Input-Kanal mit skalarem Wert.
    fn edges(&self) -> R<Vec<Edge>> {
        let mut out = Vec::new();
        for (i, c) in self.p.channels.iter().enumerate() {
            if c.dir != Direction::Input || matches!(self.p.types.get(c.ty), Type::Stream(_)) {
                continue;
            }
            let sort = self.sort_of(c.ty, c.span)?;
            let (range, outside) = match self.p.types.get(c.ty) {
                Type::Int { range: Some(r), width, .. } => {
                    let (lo, hi) = (int_bound(&r.lo), int_bound(&r.hi));
                    let (min, max) = width_bounds(*width);
                    let outside =
                        if i128::from(hi) < max { Some(hi + 1) } else { (i128::from(lo) > min).then(|| lo - 1) };
                    (Some((Term::int(lo), Term::int(hi))), outside.map(eval::Val::Int))
                }
                Type::Duration { range: Some(r) } => {
                    let (lo, hi) = (int_bound(&r.lo), int_bound(&r.hi));
                    let outside = if hi < i64::MAX { Some(hi + 1) } else { (lo > i64::MIN).then(|| lo - 1) };
                    (Some((Term::int(lo), Term::int(hi))), outside.map(eval::Val::Int))
                }
                // Ohne Range liefert der Rand einen Wert seiner Breite, und
                // keinen anderen: Eine Verletzung gibt es nicht (3.10).
                Type::Int { range: None, width, .. } if width.bits() < 64 => {
                    let (lo, hi) = width_bounds(*width);
                    (Some((Term::int(lo as i64), Term::int(hi as i64))), None)
                }
                Type::Float { range: Some(r), width, .. } => {
                    let hi = match r.hi {
                        Const::Float(f) => f,
                        Const::Int(i) | Const::Duration(i) => i as f64,
                        Const::Bool(_) => 0.0,
                    };
                    let beyond = hi + hi.abs().max(1.0);
                    let outside = match width {
                        FloatWidth::F32 => eval::Val::F32(beyond as f32),
                        FloatWidth::F64 => eval::Val::F64(beyond),
                    };
                    (Some((self.bound(&r.lo, sort), self.bound(&r.hi, sort))), Some(outside))
                }
                _ => (None, None),
            };
            let violable = outside.is_some();
            let tick_s = self.p.config.tick as f64 / 1e9;
            let slew = match c.attrs.max_slew.as_ref().map(|e| &e.kind) {
                Some(ExprKind::Float(f)) => Some(*f * tick_s),
                Some(ExprKind::Int(n)) => Some(*n as f64 * tick_s),
                _ => None,
            };
            out.push(Edge {
                channel: ChannelId(i as u32),
                name: c.name.clone(),
                sort,
                range,
                violable,
                outside,
                slew,
                debounce: c.attrs.debounce.unwrap_or(0),
            });
        }
        Ok(out)
    }

    fn edge_of(&self, c: ChannelId) -> R<Edge> {
        let span = self.p.channels[c.index()].span;
        self.edges()?.into_iter().find(|e| e.channel == c).ok_or_else(|| Unsupported { what: "Input".into(), span })
    }

    /// Die Qualitaet des Inputs in diesem Tick, eine freie Eingabe.
    fn quality(&mut self, edge: &Edge) -> Term {
        self.input(format!("i.{}.q", edge.name), Sort::Int)
    }

    /// Ist der Input lesbar (3.5: `Good`, oder `Suspect` mit gehaltenem
    /// Wert)? `pre` traegt den Rand vor diesem Tick.
    fn readable(&mut self, edge: &Edge, pre: &Env) -> Term {
        let q = self.quality(edge);
        let good = Term::eq(q.clone(), Term::int(quality::GOOD));
        if !edge.suspect() {
            return good;
        }
        let held = pre.get(&edge.loc("has")).cloned().unwrap_or_else(|| Term::bool(false));
        Term::or(vec![good, Term::and(vec![Term::eq(q, Term::int(quality::SUSPECT)), held])])
    }

    /// Liest einen Input (3.5): Ist er ungueltig, faultet der Lesevorgang
    /// mit `SensorFault`, wie im Interpreter, auch ohne Pruefknoten.
    fn read_input(&mut self, c: ChannelId, cx: &Cx<'_>, flow: &mut Flow, span: Span) -> R<Term> {
        let edge = self.edge_of(c)?;
        let x = self.input(format!("i.{}", edge.name), edge.sort);
        let ok = self.readable(&edge, cx.pre);
        flow.exits.push(Exit {
            cond: Term::and(vec![flow.alive.clone(), ok.clone().not()]),
            kind: ExitKind::Fault(None, self.cause(FaultKind::SensorFault, span)),
        });
        flow.alive = Term::and(vec![flow.alive.clone(), ok]);
        Ok(x)
    }

    /// Die Zugriffe auf die Qualitaet eines Inputs (3.5).
    fn input_accessor(&mut self, e: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Term> {
        let span = e.span;
        let ExprKind::Accessor { base, accessor, args } = &e.kind else { return no("Zugriff", span) };
        let ExprKind::Input { channel: c, .. } = base.kind else { return no("Zugriff", span) };
        let accessor = *accessor;
        use takt_mir::expr::Accessor as A;
        let edge = self.edge_of(c)?;
        let q = self.quality(&edge);
        Ok(match accessor {
            A::Valid => self.readable(&edge, cx.pre),
            A::Suspect => Term::eq(q, Term::int(quality::SUSPECT)),
            A::Stale => Term::eq(q, Term::int(quality::STALE)),
            A::Or => {
                let [default] = args.as_slice() else { return no("`.or` ohne Ersatz", span) };
                let ok = self.readable(&edge, cx.pre);
                let x = self.input(format!("i.{}", edge.name), edge.sort);
                // Der Ersatz wird nur ausgewertet, wenn der Input ungueltig ist.
                let d = self.guarded(&ok.clone().not(), flow, |enc, flow| enc.expr(default, cx, env, flow))?;
                Term::ite(ok, x, d)
            }
            other => return no(format!("Zugriff `.{}` auf einen Input", other.name()), span),
        })
    }

    /// Wertet `f` nur dort aus, wo `guard` gilt (4.1: `and`, `or`, `?:`
    /// werten kurz aus): Was darin faultet, faultet nur, wenn die
    /// Auswertung es erreicht.
    fn guarded<T>(&mut self, guard: &Term, flow: &mut Flow, f: impl FnOnce(&mut Self, &mut Flow) -> R<T>) -> R<T> {
        let (before, first) = (flow.alive.clone(), flow.exits.len());
        flow.alive = Term::and(vec![before.clone(), guard.clone()]);
        let out = f(self, flow)?;
        // Ohne neuen Ausgang geht es weiter wie davor; der Term haengt dann
        // auch nicht am Waechter, und der Kegel einer Anfrage bleibt eng.
        flow.alive = if flow.exits.len() == first {
            before
        } else {
            Term::or(vec![Term::and(vec![before, guard.clone().not()]), flow.alive.clone()])
        };
        Ok(out)
    }

    /// Der Rand vor dem ersten Tick: kein guter Wert, keine Verletzung.
    fn edges_initial(&self, env: &mut Env) -> R<()> {
        for edge in self.edges()?.into_iter().filter(Edge::stateful) {
            env.insert(edge.loc("has"), Term::bool(false));
            env.insert(edge.loc("good"), Enc::zero(edge.sort));
            env.insert(edge.loc("strikes"), Term::int(0));
            if let Some(step) = edge.per_tick() {
                env.insert(edge.loc("room"), Term::float(step, Sort::F64));
            }
            for &part in edge.parts() {
                let v = env[&edge.loc(part)].clone();
                env.insert(edge.loc(&format!("{part}.prev")), v);
            }
        }
        Ok(())
    }

    /// Der Rand nach den Lieferungen dieses Ticks (3.5, `takt_hal::quality::Gate`):
    /// Eine gute Lieferung wird der letzte gute Wert, eine `Suspect` zaehlt als
    /// Verletzung, `Bad` loescht den Bezugspunkt, `Stale` aendert nichts.
    /// `pre` traegt den Stand davor; die Kopien `.prev` halten ihn fuer die
    /// Annahmen dieses Ticks.
    fn edges_next(&mut self, pre: &Env, cur: &mut Env) -> R<()> {
        for edge in self.edges()?.into_iter().filter(Edge::stateful) {
            let q = self.quality(&edge);
            let x = self.input(format!("i.{}", edge.name), edge.sort);
            let is = |code| Term::eq(q.clone(), Term::int(code));
            let get = |part: &str| pre[&edge.loc(part)].clone();
            let has = Term::ite(
                is(quality::GOOD),
                Term::bool(true),
                Term::ite(is(quality::BAD), Term::bool(false), get("has")),
            );
            let good = Term::ite(is(quality::GOOD), x, get("good"));
            let strikes = Term::ite(
                is(quality::SUSPECT),
                Term::bin(Op::Add, get("strikes"), Term::int(1)),
                Term::ite(Term::or(vec![is(quality::GOOD), is(quality::BAD)]), Term::int(0), get("strikes")),
            );
            let mut next = vec![("has", has), ("good", good), ("strikes", strikes)];
            // Nach einer guten Lieferung ein Tick Spielraum, sonst einer mehr.
            if let Some(step) = edge.per_tick() {
                let step = Term::float(step, Sort::F64);
                next.push(("room", Term::ite(is(quality::GOOD), step.clone(), Term::bin(Op::FAdd, get("room"), step))));
            }
            for (part, value) in next {
                cur.insert(edge.loc(&format!("{part}.prev")), get(part));
                cur.insert(edge.loc(part), value);
            }
        }
        Ok(())
    }

    /// Was der Rand ueber die Lieferungen eines Ticks zusichert (3.5, 12.6):
    /// Die Qualitaet ist frei; eine gute Lieferung ist endlich, liegt in der
    /// Range und innerhalb `max_slew` zum letzten guten Wert, gemessen ueber
    /// die Ticks seit ihm; `Suspect` entsteht nur durch eine Verletzung unter
    /// `debounce` und haelt den letzten guten Wert.
    fn channel_assumptions(&mut self) -> R<Vec<Term>> {
        let mut out = Vec::new();
        for edge in self.edges()? {
            let q = self.quality(&edge);
            let x = self.input(format!("i.{}", edge.name), edge.sort);
            let is = |code| Term::eq(q.clone(), Term::int(code));
            let implies = |a: Term, b: Term| Term::or(vec![a.not(), b]);
            out.push(Term::and(vec![
                Term::bin(Op::Ge, q.clone(), Term::int(quality::GOOD)),
                Term::bin(Op::Le, q.clone(), Term::int(quality::BAD)),
            ]));
            // Ein ungueltiger Wert ist unbeobachtbar: Lesen faultet, `.or`
            // nimmt den Ersatz, ein Atom ist falsch, der Rand verbucht ihn
            // nicht. Er steht darum fest, statt dem Solver jedes Bitmuster
            // zur Wahl zu lassen.
            let held = if edge.suspect() {
                Term::and(vec![is(quality::SUSPECT), Term::var(edge.loc("has.prev"), Sort::Bool)])
            } else {
                Term::bool(false)
            };
            let zero = match edge.sort {
                Sort::F32 | Sort::F64 => Term::bin(Op::FEq, x.clone(), Enc::zero(edge.sort)),
                _ => Term::eq(x.clone(), Enc::zero(edge.sort)),
            };
            out.push(implies(Term::or(vec![is(quality::GOOD), held]).not(), zero));
            // 4.1: NaN und Unendlich gibt es in der Sprache nicht.
            if matches!(edge.sort, Sort::F32 | Sort::F64) {
                out.push(implies(is(quality::GOOD), Term::app(Op::IsFinite, vec![x.clone()])));
            }
            if let Some((lo, hi)) = &edge.range {
                let (ge, le) = if edge.sort == Sort::Int { (Op::Ge, Op::Le) } else { (Op::FGe, Op::FLe) };
                let inside =
                    Term::and(vec![Term::bin(ge, x.clone(), lo.clone()), Term::bin(le, x.clone(), hi.clone())]);
                out.push(implies(is(quality::GOOD), inside));
            }
            let prev = |part: &str, sort| Term::var(edge.loc(&format!("{part}.prev")), sort);
            if edge.slew.is_some() {
                let wide = |t: Term| if t.sort() == Sort::F64 { t } else { Term::app(Op::ToF64, vec![t]) };
                let diff =
                    Term::app(Op::FAbs, vec![Term::bin(Op::FSub, wide(x.clone()), wide(prev("good", edge.sort)))]);
                let within = Term::bin(Op::FLe, diff, prev("room", Sort::F64));
                out.push(implies(Term::and(vec![is(quality::GOOD), prev("has", Sort::Bool)]), within));
            }
            if edge.suspect() {
                let room = Term::bin(Op::Lt, prev("strikes", Sort::Int), Term::int(i64::from(edge.debounce)));
                let cause = if edge.violable { Term::bool(true) } else { prev("has", Sort::Bool) };
                out.push(implies(is(quality::SUSPECT), Term::and(vec![room, cause])));
                let held = Term::eq(x.clone(), prev("good", edge.sort));
                let held = if edge.sort == Sort::Int || edge.sort == Sort::Bool {
                    held
                } else {
                    Term::bin(Op::FEq, x.clone(), prev("good", edge.sort))
                };
                out.push(implies(Term::and(vec![is(quality::SUSPECT), prev("has", Sort::Bool)]), held));
            } else {
                out.push(is(quality::SUSPECT).not());
            }
        }
        Ok(out)
    }

    /// `always(φ)`/`never(φ)` ohne Zeitoperatoren als Invariante ueber den
    /// Zustand nach dem Commit und die Eingaben des Ticks.
    fn goal(&mut self, prop: &Property, state: &Env) -> R<Option<Goal>> {
        self.now = state[NOW].clone();
        if let Some(m) = self.monitors.iter().find(|m| m.name() == prop.name).cloned() {
            let t = self.monitor_goal(&m, state)?;
            if m.future() > 0 && state.contains_key(OVER) {
                self.note(&format!(
                    "`{}`: die Positionen der letzten {} Ticks vor dem Ende eines Laufs entscheidet das Modell nicht",
                    prop.name,
                    m.future()
                ));
            }
            return Ok(Some(self.goal_after_end(prop, t, state)));
        }
        let (inner, negate) = match &prop.formula {
            TProp::Temporal { op: TemporalOp::Always, inner, .. } => (inner.as_ref(), false),
            TProp::Temporal { op: TemporalOp::Never, inner, .. } => (inner.as_ref(), true),
            _ => return Ok(None),
        };
        let actives = BTreeMap::new();
        // Der Rand vor diesem Tick steht in den Kopien `.prev`: Der Zustand
        // nach dem Commit hat die Lieferungen schon verbucht.
        let mut before = state.clone();
        for edge in self.edges()?.into_iter().filter(Edge::stateful) {
            before.insert(edge.loc("has"), Term::var(edge.loc("has.prev"), Sort::Bool));
        }
        let cx = Cx { m: None, leaf: None, mode: Mode::Entry, pre: &before, active: &actives, locals: None };
        let Some(t) = self.tprop(inner, &cx, state)? else { return Ok(None) };
        let t = if negate { t.not() } else { t };
        Ok(Some(self.goal_after_end(prop, t, state)))
    }

    /// Nach dem Ende eines Laufs (12.7) gibt es keinen Tick, an dem die
    /// Eigenschaft gelten muesste; der Tick des Endes zaehlt noch.
    fn goal_after_end(&self, prop: &Property, t: Term, state: &Env) -> Goal {
        let formula = match state.get(OVER) {
            Some(over) => Term::or(vec![over.clone(), t]),
            None => t,
        };
        Goal { name: prop.name.clone(), assumption: prop.assumption, formula }
    }

    #[deny(clippy::wildcard_enum_match_arm)]
    fn tprop(&mut self, f: &TProp, cx: &Cx<'_>, env: &Env) -> R<Option<Term>> {
        Ok(Some(match f {
            TProp::Atom(e) => {
                let mut flow = Flow::new(Term::bool(true));
                let t = self.expr(e, cx, env, &mut flow)?;
                // Ein Atom, das faultet, ist falsch (13.5).
                let faults = Term::or(flow.exits.iter().map(|x| x.cond.clone()).collect());
                Term::ite(faults, Term::bool(false), t)
            }
            TProp::Not(a) => match self.tprop(a, cx, env)? {
                Some(t) => t.not(),
                None => return Ok(None),
            },
            TProp::And(a, b) => {
                let (Some(x), Some(y)) = (self.tprop(a, cx, env)?, self.tprop(b, cx, env)?) else { return Ok(None) };
                Term::and(vec![x, y])
            }
            TProp::Or(a, b) => {
                let (Some(x), Some(y)) = (self.tprop(a, cx, env)?, self.tprop(b, cx, env)?) else { return Ok(None) };
                Term::or(vec![x, y])
            }
            TProp::Implies(a, b) => {
                let (Some(x), Some(y)) = (self.tprop(a, cx, env)?, self.tprop(b, cx, env)?) else { return Ok(None) };
                Term::or(vec![x.not(), y])
            }
            TProp::Temporal { .. } => return Ok(None),
        }))
    }
}

/// Der Wertebereich einer Ganzzahlbreite (3.10), wie `takt_interp::arith::bounds`.
pub fn width_bounds(width: takt_mir::types::IntWidth) -> (i128, i128) {
    let bits = width.bits();
    if width.signed() { (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1) } else { (0, (1i128 << bits) - 1) }
}

/// Liegt `x` ausserhalb der Breite?
fn outside(x: &Term, width: IntWidth) -> Term {
    let (lo, hi) = width_bounds(width);
    Term::or(vec![
        Term::bin(Op::Lt, x.clone(), Term::int(lo as i64)),
        Term::bin(Op::Gt, x.clone(), Term::int(hi as i64)),
    ])
}

/// Warum ein `u64` nicht kodiert ist.
const U64: &str = "`u64`: Die Kodierung rechnet in 64 Bit mit Vorzeichen";

/// Die Funktion aus `libtaktm` hinter einer Primitive (4.2).
#[deny(clippy::wildcard_enum_match_arm)]
fn libm(op: Intrinsic) -> Option<Fun> {
    Some(match op {
        Intrinsic::Sin => Fun::Sin,
        Intrinsic::Cos => Fun::Cos,
        Intrinsic::Tan => Fun::Tan,
        Intrinsic::Asin => Fun::Asin,
        Intrinsic::Acos => Fun::Acos,
        Intrinsic::Atan => Fun::Atan,
        Intrinsic::Atan2 => Fun::Atan2,
        Intrinsic::Exp => Fun::Exp,
        Intrinsic::Log => Fun::Log,
        Intrinsic::Pow => Fun::Pow,
        Intrinsic::Abs
        | Intrinsic::Min
        | Intrinsic::Max
        | Intrinsic::Sqrt
        | Intrinsic::Fma
        | Intrinsic::Round
        | Intrinsic::Floor
        | Intrinsic::Ceil
        | Intrinsic::Rotl
        | Intrinsic::Rotr
        | Intrinsic::WrappingAdd
        | Intrinsic::WrappingSub
        | Intrinsic::WrappingMul
        | Intrinsic::SaturatingAdd
        | Intrinsic::SaturatingSub
        | Intrinsic::Interp => return None,
    })
}

/// Ein Ort im Durchlauf `path` einer Schleife (`.every.0[1,2]`).
fn at_path(base: String, path: &[i64]) -> String {
    if path.is_empty() {
        return base;
    }
    let parts: Vec<String> = path.iter().map(i64::to_string).collect();
    format!("{base}[{}]", parts.join(","))
}

/// Die Periode einer Maschine in Ticks (`every`, 5.2).
fn machine_period(m: &Machine) -> u32 {
    m.period.max(1)
}

/// `now` des Ticks, der den Zustand ergab, in Nanosekunden.
const NOW: &str = "s.now";

/// Hat der Lauf geendet (12.7), in diesem Tick oder davor?
const ENDED: &str = "s.run.ended";

/// Hatte er schon vor diesem Tick geendet? Dann ist der Tick keiner des Laufs.
const OVER: &str = "s.run.over";

/// Eine ganzzahlige Grenze einer Range.
fn int_bound(c: &Const) -> i64 {
    match c {
        Const::Int(i) | Const::Duration(i) => *i,
        Const::Float(f) => *f as i64,
        Const::Bool(b) => i64::from(*b),
    }
}

fn has_var(t: &Term) -> bool {
    let leaf = |t: &Term| matches!(&*t.0, Node::Var(..));
    leaf(t)
        || crate::term::post_order(t, |_| false)
            .iter()
            .any(|n| matches!(&*n.0, Node::App(_, args) if args.iter().any(leaf)))
}

fn node_name(e: &ExprKind) -> &'static str {
    match e {
        ExprKind::Str(_) | ExprKind::Format(_) => "Text",
        ExprKind::Record { .. } | ExprKind::Field { .. } => "Record",
        ExprKind::Array(_) | ExprKind::Index { .. } | ExprKind::Slice { .. } => "Sammlung",
        ExprKind::Accessor { .. } => "Zugriff",
        ExprKind::Matches { .. } => "`matches`",
        ExprKind::NativeCall { .. } => "native Funktion",
        ExprKind::MatOp { .. } | ExprKind::Index2 { .. } => "Matrix",
        ExprKind::Decode { .. } => "`decode`",
        ExprKind::JobState { .. } => "Job",
        ExprKind::Stream(_) => "Strom",
        ExprKind::Lift(_) | ExprKind::Ok(_) | ExprKind::Err(_) | ExprKind::None => "Wrapper",
        _ => "dieser Art",
    }
}

fn stmt_name(s: &StmtKind) -> &'static str {
    match s {
        StmtKind::ForEach { .. } => "`for … in`",
        StmtKind::Send { .. } => "`send`",
        StmtKind::At { .. } => "`at`",
        StmtKind::Every { .. } => "`every`",
        StmtKind::Job { .. } => "`job`",
        StmtKind::MethodCall { .. } => "Methodenaufruf",
        StmtKind::Break => "`break`",
        StmtKind::Cancel(_) | StmtKind::Skip(_) | StmtKind::Arm { .. } => "Strom oder Trigger",
        StmtKind::Return(_) => "`return`",
        _ => "dieser Art",
    }
}
