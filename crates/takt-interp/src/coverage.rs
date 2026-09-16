//! Coverage aus der Simulation (Referenz 13.2): Zustaende, Transitionen,
//! `check`-Auswertungen und Handler je Lauf; `takt test` vereinigt sie
//! ueber die Szenarien. Ein Bericht, keine Semantik — darum kein
//! Trace-Zeilentyp, sondern eine eigene, versionierte Datei.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use takt_mir::Program;
use takt_mir::machine::{Machine, MachineKind};
use takt_mir::stmt::{Block, Stmt, StmtKind};

use crate::env::CoverKind;

/// Formatversion der Coverage-Datei (11.3).
pub const COVERAGE_VERSION: u16 = 1;

/// Treffer je (Art, Maschine, Schluessel).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Zaehler.
    pub hits: BTreeMap<(CoverKind, String, String), u64>,
}

/// Was ein Programm insgesamt anbietet (der Nenner der Abdeckung).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Universe {
    /// Zustaende aller laufenden Maschinen.
    pub states: u64,
    /// Transitionen.
    pub transitions: u64,
    /// `check`- und `expect`-Stellen.
    pub checks: u64,
    /// Handler.
    pub handlers: u64,
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

    /// Wie viele verschiedene Schluessel einer Art getroffen wurden.
    pub fn distinct(&self, kind: CoverKind) -> u64 {
        self.hits.keys().filter(|(k, _, _)| *k == kind).count() as u64
    }

    /// Die Datei: `# takt-coverage 1`, dann `art,maschine,schluessel,zaehler`.
    pub fn render(&self) -> String {
        let mut out = format!("# takt-coverage {COVERAGE_VERSION}\nart,maschine,schluessel,zaehler\n");
        for ((kind, machine, name), n) in &self.hits {
            let _ = writeln!(out, "{},{machine},{name},{n}", kind.name());
        }
        out
    }

    /// Der Bericht in einer Zeile je Art, gegen den Nenner des Programms.
    pub fn summary(&self, u: Universe) -> String {
        let failed = self.distinct(CoverKind::CheckFailed);
        format!(
            "Zustaende {}/{}, Transitionen {}/{}, Checks {}/{} ({failed} verletzt), Handler {}/{}",
            self.distinct(CoverKind::State),
            u.states,
            self.distinct(CoverKind::Transition),
            u.transitions,
            self.distinct(CoverKind::Check),
            u.checks,
            self.distinct(CoverKind::Handler),
            u.handlers
        )
    }
}

/// Zaehlt, was die laufenden Maschinen anbieten; Szenarien zaehlen mit,
/// weil ihre Zustaende ebenso besucht werden.
pub fn universe(p: &Program) -> Universe {
    let mut u = Universe::default();
    for m in &p.machines {
        if m.kind == MachineKind::Template || m.states.is_empty() {
            continue;
        }
        u.states += m.states.len() as u64;
        u.transitions +=
            m.states.iter().map(|s| s.transitions.len() as u64).sum::<u64>() + m.faulted.transitions.len() as u64;
        u.handlers += m.handlers.len() as u64 + m.states.iter().map(|s| s.handlers.len() as u64).sum::<u64>();
        for b in blocks(m) {
            walk(b, &mut |s| {
                if matches!(s.kind, StmtKind::Check { .. }) {
                    u.checks += 1;
                }
            });
        }
    }
    u
}

/// Alle Bloecke einer Maschine.
fn blocks(m: &Machine) -> Vec<&Block> {
    let mut out = vec![&m.loop_block];
    out.extend(m.handlers.iter().map(|h| &h.body));
    out.extend(m.faulted.transitions.iter().map(|t| &t.actions));
    for s in &m.states {
        out.extend([&s.enter, &s.exit, &s.loop_block]);
        out.extend(s.handlers.iter().map(|h| &h.body));
        out.extend(s.transitions.iter().map(|t| &t.actions));
    }
    out
}

fn walk(b: &Block, f: &mut impl FnMut(&Stmt)) {
    for s in &b.stmts {
        f(s);
        match &s.kind {
            StmtKind::If { then, otherwise, .. } => {
                walk(then, f);
                walk(otherwise, f);
            }
            StmtKind::ForRange { body, .. }
            | StmtKind::ForEach { body, .. }
            | StmtKind::At { body, .. }
            | StmtKind::Every { body, .. } => walk(body, f),
            StmtKind::Match { arms, .. } => arms.iter().for_each(|a| walk(&a.body, f)),
            _ => {}
        }
    }
}
