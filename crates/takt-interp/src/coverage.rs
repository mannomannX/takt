//! Coverage aus der Simulation (Referenz 13.2): Zustaende, Transitionen,
//! `check`-Auswertungen, implizite Pruefstellen mit beiden Ausgaengen und
//! Handler je Lauf; `takt test` vereinigt sie ueber die Szenarien. Ein
//! Bericht, keine Semantik — darum kein Trace-Zeilentyp, sondern eine
//! eigene, versionierte Datei.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use takt_diag::Span;
use takt_mir::Program;
use takt_mir::analysis::walk::name_of_tag;
use takt_mir::expr::{CheckedKind, ExprKind};
use takt_mir::machine::{ArithKind, FaultKind, Machine, MachineKind, Target, Transition};
use takt_mir::stmt::{CheckKind, Observe, StmtKind};

use crate::env::CoverKind;
use crate::run::FaultSite;

/// Formatversion der Coverage-Datei (11.3): 2 mit den impliziten
/// Pruefstellen (`site`, `site_failed`).
pub const COVERAGE_VERSION: u16 = 2;

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

    /// Die Datei: `# takt-coverage 2`, dann `art,maschine,schluessel,zaehler`.
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
        let (sites, n_sites) = count(CoverKind::Site, CoverKind::Site);
        let (sites_failed, _) = count(CoverKind::Site, CoverKind::SiteFailed);
        let (handlers, n_handlers) = count(CoverKind::Handler, CoverKind::Handler);
        format!(
            "Zustaende {states}/{n_states}, Transitionen {transitions}/{n_transitions}, \
             Checks {checks}/{n_checks} ({failed} verletzt), \
             Pruefstellen {sites}/{n_sites} ({sites_failed} verletzt), Handler {handlers}/{n_handlers}"
        )
    }

    /// Traegt die impliziten Pruefstellen des Laufs ein (4.1): bestanden, wo
    /// eine Stelle oefter ausgewertet wurde, als ihr Faults zufallen;
    /// verletzt, wo ihr ein Fault zufaellt. Die Stellen gehoeren der Quelle,
    /// nicht einer Maschine — eine Funktion teilen sich ihre Aufrufer —, und
    /// stehen darum ohne Maschine.
    pub fn sites(&mut self, p: &Program, evals: &HashMap<(u32, u32, u8), u64>, faults: &[FaultSite]) {
        let all = sites(p);
        let mut failed: HashMap<usize, u64> = HashMap::new();
        for f in faults {
            if let Some(i) = attributed(&all, f) {
                *failed.entry(i).or_default() += 1;
            }
        }
        for (i, site) in all.iter().enumerate() {
            let fails = failed.get(&i).copied().unwrap_or(0);
            let evaluated = evals.get(&(site.start, site.end, site.tag)).copied().unwrap_or(0);
            let passed = if site.operand { evaluated > fails } else { evaluated > 0 };
            if passed {
                self.hit(CoverKind::Site, "", &site.key());
            }
            if fails > 0 {
                *self.hits.entry((CoverKind::SiteFailed, String::new(), site.key())).or_default() += fails;
            }
        }
    }
}

/// Eine implizite Pruefstelle der Quelle (4.1, 3.4, 3.5, 3.8): ein Knoten
/// `Checked`, geschluesselt wie in der Beweisdatei (`site <Anfang> <Ende>
/// <Art>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Anfang.
    pub start: u32,
    /// Ende.
    pub end: u32,
    /// Art (`takt_mir::analysis::walk::tag`).
    pub tag: u8,
    /// Sitzt der Knoten am Operanden einer Operation, die erst nach ihm
    /// faultet — Division, Definitionsbereich, der Index einer
    /// Zuweisungsstelle? Dann zaehlt seine Auswertung auch, wenn die
    /// Operation danach faultet.
    pub operand: bool,
    /// Die Folge im Durchlauf: Ein innerer Knoten derselben Stelle kommt
    /// nach dem aeusseren.
    order: usize,
}

impl Site {
    /// Die Stelle `tag` an `start..end`; `operand`, wenn der Knoten am
    /// Operanden sitzt (Division, Definitionsbereich).
    pub fn at(start: u32, end: u32, tag: u8, operand: bool) -> Site {
        Site { start, end, tag, operand, order: 0 }
    }

    /// Der Schluessel in Coverage und Ratsche: `<art> @<anfang>-<ende>`.
    pub fn key(&self) -> String {
        format!("{} @{}-{}", name_of_tag(self.tag), self.start, self.end)
    }

    /// Der Fault, den die Stelle ausloest.
    pub fn fault(&self) -> Option<FaultKind> {
        fault_of(self.tag)
    }

    /// Faellt ein Fault der Art `kind` an `span` dieser Stelle zu? Eine
    /// Stelle am Operanden meldet ihn an der Operation oder Anweisung, die
    /// sie umschliesst, jede andere an sich selbst. Dieselbe Regel bestaetigt
    /// einen Pfad des Solvers (`takt_prove::fault_at`).
    pub fn takes(&self, kind: FaultKind, span: Span) -> bool {
        let operand = self.operand || name_of_tag(self.tag) == "index";
        self.fault() == Some(kind)
            && if operand {
                span.start <= self.start && self.end <= span.end
            } else {
                span.start == self.start && span.end == self.end
            }
    }
}

/// Der Fault einer Pruefungsart (`CheckedKind::fault`) nach ihrem Namen.
pub fn fault_of(tag: u8) -> Option<FaultKind> {
    Some(match name_of_tag(tag) {
        "div" => FaultKind::Arithmetic(ArithKind::DivZero),
        "ovf" => FaultKind::Arithmetic(ArithKind::Overflow),
        "fin" => FaultKind::Arithmetic(ArithKind::NonFinite),
        "dom" => FaultKind::Arithmetic(ArithKind::Domain),
        "range" | "conv" | "shift" | "index" => FaultKind::Range,
        "missing" => FaultKind::MissingValue,
        "valid" => FaultKind::SensorFault,
        _ => return None,
    })
}

/// Die impliziten Pruefstellen der Quelle in den laufenden Maschinen, ihren
/// Funktionen und Bloecken; das Prelude zaehlt nicht.
pub fn sites(p: &Program) -> Vec<Site> {
    let mut out: Vec<Site> = Vec::new();
    let mut look = |e: &takt_mir::expr::Expr| {
        let ExprKind::Checked { expr, kind } = &e.kind else { return };
        if e.span.file.0 != takt_mir::analysis::proof::SOURCE {
            return;
        }
        let tag = takt_mir::analysis::walk::tag(kind);
        if out.iter().any(|s| (s.start, s.end, s.tag) == (e.span.start, e.span.end, tag)) {
            return;
        }
        let operand = match kind {
            CheckedKind::DivZero | CheckedKind::Domain => true,
            CheckedKind::Index { .. } => !matches!(expr.kind, ExprKind::Index { .. }),
            _ => false,
        };
        let order = out.len();
        out.push(Site { start: e.span.start, end: e.span.end, tag, operand, order });
    };
    let counted = |m: &&Machine| !matches!(m.kind, MachineKind::Template | MachineKind::Scenario);
    for m in p.machines.iter().filter(counted) {
        takt_mir::visit::for_each_expr_machine(m, &mut look);
    }
    for f in &p.fns {
        takt_mir::visit::for_each_expr_block(&f.body, &mut look);
    }
    for b in &p.blocks {
        for v in &b.state_vars {
            if let Some(init) = &v.init {
                takt_mir::visit::walk_expr(init, &mut look);
            }
        }
    }
    out
}

/// Die Stelle, der ein Fault zufaellt: unter denen, die ihn nehmen, die
/// engste, bei gleicher Spanne die innere.
fn attributed(sites: &[Site], f: &FaultSite) -> Option<usize> {
    if f.span.file.0 != takt_mir::analysis::proof::SOURCE {
        return None;
    }
    sites
        .iter()
        .enumerate()
        .filter(|(_, s)| s.takes(f.kind, f.span))
        .min_by_key(|(_, s)| (s.end - s.start, std::cmp::Reverse(s.order)))
        .map(|(i, _)| i)
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
    for s in sites(p) {
        let span = Span { file: takt_diag::FileId(takt_mir::analysis::proof::SOURCE), start: s.start, end: s.end };
        out.push(Item { kind: CoverKind::Site, machine: String::new(), key: s.key(), span });
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
