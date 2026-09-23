//! Rueckverfolgung von Anforderungen (13.4): wo eine `req`-ID geprueft wird.
//!
//! `check p < LIMIT, "…" req "SR-12"` und `verify …, "…" req "SR-12"`
//! tragen die Referenz; dieser Index sammelt je ID ihre Stellen. Er liegt
//! in der MIR, weil `takt check --report` und `takt test` denselben Gang
//! brauchen und der Schluessel zur Coverage (13.2) nur einmal erklaert
//! sein soll.

use std::collections::BTreeMap;

use takt_diag::Span;

use crate::machine::{Machine, MachineKind};
use crate::stmt::{Block, CheckKind, Observe, Stmt, StmtKind};
use crate::{MachineId, Program};

/// Art der pruefenden Stelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteKind {
    /// `check`.
    Check,
    /// `expect`.
    Expect,
    /// `alert`.
    Alert,
    /// `verify` in einem Szenario.
    Verify,
}

impl SiteKind {
    /// Name im Report und im Coverage-Schluessel.
    pub fn name(self) -> &'static str {
        match self {
            SiteKind::Check => "check",
            SiteKind::Expect => "expect",
            SiteKind::Alert => "alert",
            SiteKind::Verify => "verify",
        }
    }
}

/// Eine Stelle, die eine Anforderung prueft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Art der Stelle.
    pub kind: SiteKind,
    /// Maschine, in der sie steht.
    pub machine: MachineId,
    /// Name der Maschine, wie ihn die Coverage fuehrt.
    pub machine_name: String,
    /// Der Block, in dem die Stelle steht (`enter`, `loop`, `on`, …).
    pub block: String,
    /// Position.
    pub span: Span,
}

impl Site {
    /// Der Coverage-Schluessel dieser Stelle (13.2): `(Maschine, Name)`.
    /// `verify` und `check` bilden ihn gleich, damit `takt test` je Stelle
    /// zaehlen kann, welche Szenarien sie durchliefen.
    pub fn cover_key(&self) -> (&str, String) {
        (self.machine_name.as_str(), format!("{} @{}", self.kind.name(), self.span.start))
    }
}

/// Je Anforderungs-ID ihre Stellen, in Quellreihenfolge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// Je Anforderungs-ID ihre Stellen.
    pub by_req: BTreeMap<String, Vec<Site>>,
}

impl Index {
    /// Zahl der Anforderungen.
    pub fn len(&self) -> usize {
        self.by_req.len()
    }

    /// Traegt das Programm ueberhaupt Anforderungen?
    pub fn is_empty(&self) -> bool {
        self.by_req.is_empty()
    }

    /// Alle Stellen, unabhaengig von der ID.
    pub fn sites(&self) -> impl Iterator<Item = &Site> {
        self.by_req.values().flatten()
    }
}

/// Sammelt die Anforderungsreferenzen des Programms.
pub fn index(p: &Program) -> Index {
    let mut out = Index::default();
    for (i, m) in p.machines.iter().enumerate() {
        if m.kind == MachineKind::Template {
            continue;
        }
        let id = MachineId(i as u32);
        for (label, block) in labelled_blocks(m) {
            block.walk(&mut |s| {
                if let Some((kind, req)) = referenced(s) {
                    let site =
                        Site { kind, machine: id, machine_name: m.name.clone(), block: label.clone(), span: s.span };
                    out.by_req.entry(req.to_string()).or_default().push(site);
                }
            });
        }
    }
    for sites in out.by_req.values_mut() {
        sites.sort_by_key(|s| (s.span.file.0, s.span.start));
    }
    out
}

/// Die Anweisung mit ihrer Anforderung, wenn sie eine traegt.
fn referenced(s: &Stmt) -> Option<(SiteKind, &str)> {
    match &s.kind {
        StmtKind::Check { req: Some(req), kind, .. } => {
            let kind = if *kind == CheckKind::Check { SiteKind::Check } else { SiteKind::Expect };
            Some((kind, req.as_str()))
        }
        StmtKind::Observe(Observe::Verify { req: Some(req), .. }) => Some((SiteKind::Verify, req.as_str())),
        StmtKind::Observe(Observe::Alert { req: Some(req), .. }) => Some((SiteKind::Alert, req.as_str())),
        _ => None,
    }
}

/// Die Bloecke einer Maschine mit ihrem Namen im Report.
fn labelled_blocks(m: &Machine) -> Vec<(String, &Block)> {
    let mut out: Vec<(String, &Block)> = vec![("loop".to_string(), &m.loop_block)];
    out.extend(m.handlers.iter().map(|h| ("on".to_string(), &h.body)));
    out.extend(m.faulted.transitions.iter().map(|t| ("FAULTED".to_string(), &t.actions)));
    for s in &m.states {
        out.extend([
            (format!("{}.enter", s.name), &s.enter),
            (format!("{}.exit", s.name), &s.exit),
            (format!("{}.loop", s.name), &s.loop_block),
        ]);
        out.extend(s.handlers.iter().map(|h| (format!("{}.on", s.name), &h.body)));
        out.extend(s.transitions.iter().map(|t| (format!("{}.->", s.name), &t.actions)));
    }
    out
}
