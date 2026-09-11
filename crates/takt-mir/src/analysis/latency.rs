//! Safe-State-Latenz (Referenz 9.4.5, 5.2, 5.3, 5.4).
//!
//! Wie lange dauert es vom Eintreten einer Bedingung bis zu dem Tick, in
//! dem alle von ihr betroffenen Aktoren auf ihrem `safe`-Wert stehen? Fuer
//! IEC 61508 und ISO 26262 ist das die zentrale Zahl (FTTI beziehungsweise
//! Process Safety Time); sie wird sonst geschaetzt und gemessen.
//!
//! Die Rechnung nutzt nur Groessen, die die MIR ohnehin traegt:
//!
//! ```text
//! D_detect  = n_m                 Perioden-Ticks; schlimmster Fall ist,
//!                                 dass die Bedingung kurz nach der
//!                                 Aktivierung von m wahr wird (1.3)
//! D_confirm = ceil(d / P_m)       nur bei `check … for d` (5.6)
//! D_fault   = depth(q)            Ticks im Fault-Wald bis zu einem
//!                                 stabilen Ziel (5.3)
//! D_commit  = 0 bei `asap`        die Entry-Tick-Regel prueft vor dem
//!             1 bei `boundary`    Commit; der unsichere Wert wird nie
//!                                 committet (5.2, Punkt 4)
//! ```
//!
//! **Zwei Einheiten, zwei Gueltigkeiten.** Die Tickzahl folgt aus Perioden
//! und Fault-Wald und ist exakt. Die Zeit ist `Ticks · T0` und gilt unter
//! der Annahme, dass jeder Tick eingehalten wird — das prueft erst die
//! Schedulability (7.2) mit der kalibrierten Kostentabelle (13.8). Solange
//! die fehlt, traegt die Zeitspalte diesen Vorbehalt sichtbar mit, wie die
//! Posten in [`super::size`] ihre Herkunft tragen.
//!
//! Was die Rechnung nicht behauptet: Sie gilt fuer die Logik, nicht fuer
//! den Aktor — `guard` und `jitter` des Treibers kommen aus der
//! Konformitaetsmessung (7.5, 13.8). Und sie ist eine **obere** Schranke;
//! der Normalfall ist kuerzer.

use std::collections::BTreeMap;

use takt_diag::Span;

use crate::expr::ExprKind;
use crate::machine::{FaultTarget, Machine, MachineKind};
use crate::program::OutputTiming;
use crate::stmt::{Block, CheckKind, Stmt, StmtKind};
use crate::{ChannelId, MachineId, Program, StateId};

/// Eine Stelle, an der ein Fault entstehen kann, mit ihrer Latenz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Maschine, in der die Stelle steht.
    pub machine: MachineId,
    /// Zustand; `None` fuer den maschinenweiten `loop:`.
    pub state: Option<StateId>,
    /// Art der Stelle.
    pub kind: SiteKind,
    /// `n_m`: Ticks bis zur naechsten Auswertung (1.3).
    pub detect: u64,
    /// `ceil(d / P_m)`: Ticks der Bestaetigungszeit (5.6).
    pub confirm: u64,
    /// Ticks im Fault-Wald bis zu einem stabilen Ziel (5.3).
    pub fault: u64,
    /// Geforderte Latenz aus `within d` (9.4.5), in Nanosekunden.
    pub within: Option<i64>,
    /// Position im Quelltext.
    pub span: Span,
}

impl Site {
    /// Ticks von der Verletzung bis zum stabilen Fault-Ziel, ohne Commit.
    pub fn ticks(&self) -> u64 {
        self.detect + self.confirm + self.fault
    }
}

/// Art einer Fault-Stelle (5.6, 5.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteKind {
    /// `check c, "…"` — kontinuierlich.
    Check,
    /// `expect c, "…"` — einmalig in einer Sequenz.
    Expect,
    /// `abort "…"` — wirkt systemweit in der Abort-Phase (5.4).
    Abort,
}

impl SiteKind {
    /// Name im Report.
    pub fn name(self) -> &'static str {
        match self {
            SiteKind::Check => "check",
            SiteKind::Expect => "expect",
            SiteKind::Abort => "abort",
        }
    }
}

/// Latenz bis zum sicheren Zustand eines Outputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLatency {
    /// Der Output.
    pub channel: ChannelId,
    /// Ticks insgesamt, einschliesslich Commit.
    pub ticks: u64,
    /// Die Stelle, die die Schranke bestimmt.
    pub worst: Option<Site>,
}

/// Ergebnis der Latenzanalyse.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Latency {
    /// Alle Fault-Stellen, nach Maschine und Position geordnet.
    pub sites: Vec<Site>,
    /// Je Output die Schranke bis zum sicheren Zustand.
    pub outputs: Vec<OutputLatency>,
    /// Basis-Tick in Nanosekunden (`T0`).
    pub tick_ns: i64,
    /// Ticks, die der Commit kostet: 0 bei `asap`, 1 bei `boundary`.
    pub commit: u64,
    /// Ob die Zeitspalte belastbar ist — das verlangt eine gepruefte
    /// Schedulability (7.2), also die Kalibrierung aus 13.8.
    pub time_is_proven: bool,
}

impl Latency {
    /// Ticks in Nanosekunden.
    pub fn ns(&self, ticks: u64) -> i128 {
        i128::from(ticks) * i128::from(self.tick_ns)
    }

    /// Die groesste Schranke ueber alle Outputs.
    pub fn worst_case(&self) -> u64 {
        self.outputs.iter().map(|o| o.ticks).max().unwrap_or(0)
    }

    /// Der Bericht je Output, mit Aufschluesselung (9.4.5).
    ///
    /// Die Tickspalte ist exakt; die Zeitspalte traegt ihren Vorbehalt, bis
    /// die Schedulability geprueft ist (7.2, 13.8). Ein Bericht, dem man
    /// nicht ansieht, welchem Teil er trauen kann, ist schlechter als
    /// keiner — dieselbe Linie wie in [`super::size`].
    pub fn lines(&self, p: &Program) -> Vec<String> {
        if self.outputs.is_empty() {
            return vec!["  (kein Output mit einer Fault-Stelle)".into()];
        }
        let name = |c: ChannelId| p.channels[c.index()].name.clone();
        let w = self.outputs.iter().map(|o| name(o.channel).len()).max().unwrap_or(0).max(6);
        let mut out = Vec::new();
        for o in &self.outputs {
            let time = crate::dump::duration(i64::try_from(self.ns(o.ticks)).unwrap_or(i64::MAX));
            out.push(format!("  {:w$}  {:>4} Ticks  {:>10}", name(o.channel), o.ticks, time));
            if let Some(s) = &o.worst {
                let m = &p.machines[s.machine.index()];
                let place = match s.state {
                    Some(q) => format!("{}.{}", m.name, m.states[q.index()].name),
                    None => format!("{}.loop", m.name),
                };
                out.push(format!(
                    "  {:w$}    {} in {place}: {} erkennen + {} bestaetigen + {} Fault-Pfad + {} Commit",
                    "",
                    s.kind.name(),
                    s.detect,
                    s.confirm,
                    s.fault,
                    self.commit
                ));
            }
        }
        out.push(format!("  {:w$}  {:>4} Ticks  (Schranke ueber alle Outputs)", "Summe", self.worst_case()));
        if !self.time_is_proven {
            out.push("  (die Zeitspalte gilt unter eingehaltenem Tick; das prueft die Schedulability mit 13.8)".into());
        }
        out
    }
}

/// Rechnet die Safe-State-Latenz eines Programms aus (9.4.5).
pub fn latency(p: &Program) -> Latency {
    // 5.2 Punkt 4: Bei `asap` laufen die `check`s des neu betretenen
    // Zustands im Entry-Modus, also *vor* dem Commit — der Output nimmt den
    // unsicheren Wert nie an. Bei `boundary` (LET) steht er einen Tick.
    let commit = match p.config.output_timing {
        OutputTiming::Asap => 0,
        OutputTiming::Boundary => 1,
    };

    // Templates werden nie aktiviert; ihre Instanzen stehen als eigene
    // Maschinen in der Tabelle (5.11).
    let mut sites = Vec::new();
    for (i, m) in p.machines.iter().enumerate() {
        if matches!(m.kind, MachineKind::Template) {
            continue;
        }
        collect(p, MachineId(i as u32), m, &mut sites);
    }

    // Single-Writer (Pruefung 7): Jeder Output gehoert genau einer
    // Maschine, `Channel::owner` ist die vollstaendige Zuordnung.
    let mut worst: BTreeMap<u32, Site> = BTreeMap::new();
    for s in &sites {
        for (c, ch) in p.channels.iter().enumerate() {
            if ch.owner != Some(s.machine) {
                continue;
            }
            let slot = worst.entry(c as u32);
            slot.and_modify(|w| {
                if s.ticks() > w.ticks() {
                    *w = s.clone();
                }
            })
            .or_insert_with(|| s.clone());
        }
    }

    let mut outputs: Vec<OutputLatency> = worst
        .into_iter()
        .map(|(c, s)| OutputLatency { channel: ChannelId(c), ticks: s.ticks() + commit, worst: Some(s) })
        .collect();
    outputs.sort_by_key(|o| (std::cmp::Reverse(o.ticks), o.channel.0));

    Latency {
        sites,
        outputs,
        tick_ns: p.config.tick,
        commit,
        // Ohne kalibrierte Kostentabelle ist die Tick-Treue eine Annahme
        // (7.2, 13.8). `tick_source` allein genuegt nicht.
        time_is_proven: false,
    }
}

/// Alle Fault-Stellen einer Maschine, mit ihrer Latenz.
fn collect(p: &Program, id: MachineId, m: &Machine, out: &mut Vec<Site>) {
    let period = u64::from(m.period.max(1));
    let period_ns = i128::from(period) * i128::from(p.config.tick);

    // Der maschinenweite `loop:` gehoert keinem Zustand; sein Fault-Ziel
    // ist das der Maschine.
    let machine_depth = depth(m, m.fault_target);
    walk(&m.loop_block, &mut |kind, confirm, within, span| {
        out.push(Site {
            machine: id,
            state: None,
            kind,
            detect: period,
            confirm: confirm_ticks(confirm, period_ns),
            fault: machine_depth,
            within,
            span,
        });
    });

    for (s, state) in m.states.iter().enumerate() {
        let sid = StateId(s as u32);
        let d = depth(m, m.fault_target_of(sid));
        let mut push = |kind, confirm, within, span| {
            out.push(Site {
                machine: id,
                state: Some(sid),
                kind,
                detect: period,
                confirm: confirm_ticks(confirm, period_ns),
                fault: d,
                within,
                span,
            });
        };
        for b in [&state.enter, &state.loop_block, &state.exit] {
            walk(b, &mut push);
        }
        for h in &state.handlers {
            walk(&h.body, &mut push);
        }
        for t in &state.transitions {
            walk(&t.actions, &mut push);
        }
    }
}

/// Ticks im Fault-Wald von einem Ziel bis zu einem stabilen Zustand (5.3).
///
/// Jeder Schritt ist ein Tick: Das Fault-Ziel wird im Entry-Modus
/// ausgefuehrt, und scheitert sein Koerper erneut, folgt der naechste
/// Fault — „spaetestens `FAULTED`". Pruefung 9 haelt den Wald azyklisch,
/// der Durchlauf terminiert also; die Schranke `states.len()` faengt eine
/// MIR ab, die das nicht erfuellt.
fn depth(m: &Machine, from: FaultTarget) -> u64 {
    let mut cur = from;
    let mut steps = 0;
    for _ in 0..=m.states.len() {
        match cur {
            FaultTarget::Faulted => return steps + 1,
            FaultTarget::State(s) => {
                steps += 1;
                let next = m.fault_target_of(s);
                if next == FaultTarget::State(s) {
                    return steps;
                }
                cur = next;
            }
        }
    }
    steps
}

/// `ceil(d / P_m)` in Ticks. Eine Bestaetigungszeit, die kein Literal ist,
/// zaehlt als 0 — sie steht dann nicht in der MIR, und die Schranke waere
/// geraten statt gerechnet.
fn confirm_ticks(confirm: Option<i64>, period_ns: i128) -> u64 {
    match confirm {
        Some(d) if d > 0 && period_ns > 0 => {
            let d = i128::from(d);
            u64::try_from((d + period_ns - 1) / period_ns).unwrap_or(u64::MAX)
        }
        _ => 0,
    }
}

/// Ruft `f` fuer jede Fault-Stelle eines Blocks, auch in geschachtelten
/// Anweisungen. `alert` ist keine: Es kann nie einen Fault ausloesen (5.6).
fn walk(b: &Block, f: &mut impl FnMut(SiteKind, Option<i64>, Option<i64>, Span)) {
    for s in &b.stmts {
        walk_stmt(s, f);
    }
}

fn walk_stmt(s: &Stmt, f: &mut impl FnMut(SiteKind, Option<i64>, Option<i64>, Span)) {
    match &s.kind {
        StmtKind::Check { confirm, within, kind, .. } => {
            let ns = |e: &crate::expr::Expr| match e.kind {
                ExprKind::Duration(ns) => Some(ns),
                _ => None,
            };
            let d = confirm.as_ref().and_then(|c| ns(&c.duration));
            let w = within.as_ref().and_then(ns);
            let k = match kind {
                CheckKind::Check => SiteKind::Check,
                CheckKind::Expect => SiteKind::Expect,
            };
            f(k, d, w, s.span);
        }
        StmtKind::Abort { .. } => f(SiteKind::Abort, None, None, s.span),
        StmtKind::If { then, otherwise, .. } => {
            walk(then, f);
            walk(otherwise, f);
        }
        StmtKind::ForRange { body, .. } | StmtKind::ForEach { body, .. } => walk(body, f),
        StmtKind::Every { body, .. } | StmtKind::At { body, .. } => walk(body, f),
        StmtKind::Match { arms, .. } => {
            for a in arms {
                walk(&a.body, f);
            }
        }
        _ => {}
    }
}
