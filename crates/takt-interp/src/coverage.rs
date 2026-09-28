//! Coverage aus der Simulation (Referenz 13.2): Zustaende, Transitionen,
//! `check`-Auswertungen und Handler je Lauf; `takt test` vereinigt sie
//! ueber die Szenarien. Ein Bericht, keine Semantik — darum kein
//! Trace-Zeilentyp, sondern eine eigene, versionierte Datei.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use takt_diag::Span;
use takt_mir::Program;
use takt_mir::machine::{Machine, MachineKind, Target, Transition};
use takt_mir::stmt::{CheckKind, Observe, StmtKind};

use crate::env::CoverKind;

/// Formatversion der Coverage-Datei (11.3).
pub const COVERAGE_VERSION: u16 = 1;

/// Treffer je (Art, Maschine, Schluessel).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Zaehler.
    pub hits: BTreeMap<(CoverKind, String, String), u64>,
}

/// Eine Stelle, die ein Lauf erreichen kann; alle zusammen sind der
/// Nenner der Abdeckung (13.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// Art.
    pub kind: CoverKind,
    /// Maschine.
    pub machine: String,
    /// Schluessel, wie ihn der Treffer traegt.
    pub key: String,
    /// Position in der Quelle.
    pub span: Span,
}

impl Coverage {
    /// Ein Treffer.
    pub fn hit(&mut self, kind: CoverKind, machine: &str, name: &str) {
        *self.hits.entry((kind, machine.to_string(), name.to_string())).or_default() += 1;
    }

    /// Vereinigt die Treffer eines weiteren Laufs.
    pub fn merge(&mut self, other: &Coverage) {
        for (k, n) in &other.hits {
            *self.hits.entry(k.clone()).or_default() += n;
        }
    }

    /// Hat ein Lauf die Stelle als `kind` getroffen?
    pub fn has(&self, kind: CoverKind, item: &Item) -> bool {
        self.hits.contains_key(&(kind, item.machine.clone(), item.key.clone()))
    }

    /// Die Stellen, die kein Lauf erreicht hat.
    pub fn missing<'a>(&self, items: &'a [Item]) -> Vec<&'a Item> {
        items.iter().filter(|i| !self.has(i.kind, i)).collect()
    }

    /// Die Datei: `# takt-coverage 1`, dann `art,maschine,schluessel,zaehler`.
    pub fn render(&self) -> String {
        let mut out = format!("# takt-coverage {COVERAGE_VERSION}\nart,maschine,schluessel,zaehler\n");
        for ((kind, machine, name), n) in &self.hits {
            let _ = writeln!(out, "{},{machine},{name},{n}", kind.name());
        }
        out
    }

    /// Der Bericht in einer Zeile je Art, gegen die Stellen des Programms.
    pub fn summary(&self, items: &[Item]) -> String {
        let count = |kind: CoverKind, hit: CoverKind| {
            let all = items.iter().filter(|i| i.kind == kind);
            (all.clone().filter(|i| self.has(hit, i)).count(), all.count())
        };
        let (states, n_states) = count(CoverKind::State, CoverKind::State);
        let (transitions, n_transitions) = count(CoverKind::Transition, CoverKind::Transition);
        let (checks, n_checks) = count(CoverKind::Check, CoverKind::Check);
        let (failed, _) = count(CoverKind::Check, CoverKind::CheckFailed);
        let (handlers, n_handlers) = count(CoverKind::Handler, CoverKind::Handler);
        format!(
            "Zustaende {states}/{n_states}, Transitionen {transitions}/{n_transitions}, \
             Checks {checks}/{n_checks} ({failed} verletzt), Handler {handlers}/{n_handlers}"
        )
    }
}

/// Jede Stelle der laufenden Maschinen. Szenarien zaehlen nicht: Sie sind
/// der Test, und ihr Timeout-Pfad bleibt in jedem bestandenen Lauf liegen.
pub fn items(p: &Program) -> Vec<Item> {
    let mut out = Vec::new();
    let counted =
        |m: &&Machine| !matches!(m.kind, MachineKind::Template | MachineKind::Scenario) && !m.states.is_empty();
    for m in p.machines.iter().filter(counted) {
        let mut push = |kind, key, span| out.push(Item { kind, machine: m.name.clone(), key, span });
        for s in &m.states {
            push(CoverKind::State, s.name.clone(), s.span);
            for t in &s.transitions {
                push(CoverKind::Transition, transition_key(m, &s.name, t), t.span);
            }
        }
        for t in &m.faulted.transitions {
            push(CoverKind::Transition, transition_key(m, "FAULTED", t), t.span);
        }
        for h in m.handlers.iter().chain(m.states.iter().flat_map(|s| &s.handlers)) {
            push(CoverKind::Handler, format!("on @{}", h.span.start), h.span);
        }
        for b in m.blocks() {
            b.walk(&mut |s| {
                // 13.4: `verify` zaehlt mit, weil es dieselbe Coverage-Art traegt.
                let word = match &s.kind {
                    StmtKind::Check { kind: CheckKind::Check, .. } => "check",
                    StmtKind::Check { kind: CheckKind::Expect, .. } => "expect",
                    StmtKind::Observe(Observe::Verify { .. }) => "verify",
                    _ => return,
                };
                push(CoverKind::Check, format!("{word} @{}", s.span.start), s.span);
            });
        }
    }
    out
}

/// Schluessel einer Transition aus `from`.
pub fn transition_key(m: &Machine, from: &str, t: &Transition) -> String {
    format!("{from}->{} @{}", target_name(m, t.target), t.span.start)
}

/// Name eines Ziels in Schluessel und Trace.
pub fn target_name(m: &Machine, t: Target) -> String {
    match t {
        Target::Faulted => "FAULTED".to_string(),
        Target::State(s) => m.states[s.index()].name.clone(),
        Target::Fault(k) => format!("[Fault {k:?}]"),
    }
}
