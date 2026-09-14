//! Das statische Gate (Referenz 3.4, 9.4.3, 11.5; plan/m3.md).
//!
//! Ein Durchlauf ueber die MIR beantwortet drei Fragen zugleich: Welche
//! Intervalle sind bewiesen (3.4), welche Variablen sind sicher zugewiesen
//! (Pruefungen 6 und 25), und welche Wrapper sind dominiert (Pruefung 5).
//! Aus den Intervallen folgen die Darstellungsverengung (`narrow`) und die
//! Kennzahl der impliziten Pruefungen; das Kostenmodell (`cost`) und das
//! Speicherbudget (`size`) bauen darauf auf.
//!
//! Die Analyse liegt hier und nicht in `takt-sema`, weil sie Kontrollfluss
//! braucht — den hat erst die MIR — und weil Codegen, `takt size` und der
//! Importer dieselben Intervalle benutzen (plan/m3.md 1.1).

pub mod cost;
pub mod domain;
pub mod facts;
pub mod graph;
pub mod latency;
pub mod narrow;
pub mod prove;
pub mod size;
pub mod stack;
pub mod walk;

use std::collections::BTreeMap;

use takt_diag::Diagnostic;

use crate::Program;
use crate::analysis::facts::Facts;
use crate::analysis::walk::{CheckCause, ImplicitCheck, Walk};
use crate::machine::Machine;
use crate::types::{Range, Type};

/// Code der Warnungen ueber implizite Pruefungen (Pruefung 24).
pub const SC24: &str = "SC-24";

/// Was die Analyse ueber ein Programm herausgefunden hat.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Implizite Pruefungen je Ursache (3.4, Kennzahl).
    pub checks: BTreeMap<&'static str, u32>,
    /// Davon in Schleifen oder Aktionsbloecken — nur diese warnen.
    pub warned: u32,
    /// Ausdruecke, deren Darstellung auf 32 Bit verengt wurde (Lemma 3.4).
    pub narrowed: u32,
    /// Ausdruecke mit Integer-Darstellung insgesamt.
    pub integer_exprs: u32,
}

impl Report {
    /// Summe der impliziten Pruefungen.
    pub fn total_checks(&self) -> u32 {
        self.checks.values().sum()
    }

    /// Eine Zeile je Kennzahl, wie sie `takt check` ausgibt.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        let by_cause: Vec<String> = self.checks.iter().map(|(k, v)| format!("{k} {v}")).collect();
        out.push(format!("implizite Pruefungen: {} ({})", self.total_checks(), by_cause.join(", ")));
        out.push(format!("davon mit Warnung:    {}", self.warned));
        out.push(format!("Darstellung:          {} von {} Ausdruecken in i32", self.narrowed, self.integer_exprs));
        out
    }
}

/// Fuehrt die Analyse aus: annotiert die MIR und liefert Diagnosen und
/// Kennzahlen.
pub fn analyze(program: &mut Program) -> (Vec<Diagnostic>, Report) {
    let mut diags = Vec::new();
    let mut report = Report::default();
    let mut all: Vec<ImplicitCheck> = Vec::new();
    let mut proofs = prove::Proofs::default();

    for id in 0..program.machines.len() {
        let w = analyze_machine(program, id);
        proofs.add(&w.proven, &w.ranges);
        all.extend(w.checks);
    }
    // Was bewiesen ist, verschwindet aus der MIR; die Intervalle bleiben als
    // Annotation stehen (3.4).
    prove::apply(program, &proofs);

    // Eine Pruefung ist eine *Stelle* im Programm, keine Ausfuehrung: Ein
    // abgerollter Schleifenkoerper besucht dieselbe Stelle mehrfach, zaehlt
    // aber einmal. Warnt einer der Besuche, warnt die Stelle.
    let mut seen: BTreeMap<(u32, u32), ImplicitCheck> = BTreeMap::new();
    for c in &all {
        let key = (c.span.file.0, c.span.start);
        seen.entry(key).and_modify(|e| e.warns |= c.warns).or_insert(*c);
    }
    for c in seen.values() {
        *report.checks.entry(c.cause.name()).or_default() += 1;
        if c.warns {
            report.warned += 1;
            diags.push(Diagnostic::warning(SC24, c.span, message(c.cause)).with_suggestion(
                "Range deklarieren, `clamp` benutzen oder eine range-typisierte Zwischengroesse einfuehren (3.4)",
            ));
        }
    }
    // Ursachen ohne Fund erscheinen mit 0, damit die Zeile stabil bleibt.
    for c in [CheckCause::Declared, CheckCause::Index, CheckCause::Convert, CheckCause::Arith] {
        report.checks.entry(c.name()).or_default();
    }

    let n = narrow::narrow(program);
    report.narrowed = n.0;
    report.integer_exprs = n.1;
    (diags, report)
}

/// Die Meldung zu einer Ursache.
fn message(c: CheckCause) -> String {
    match c {
        CheckCause::Declared => "impliziter Range-Check in einer Schleife oder einem Aktionsblock",
        CheckCause::Index => "impliziter Index-Check in einer Schleife oder einem Aktionsblock",
        CheckCause::Convert => "implizite Konversionspruefung in einer Schleife oder einem Aktionsblock",
        CheckCause::Arith => "implizite Arithmetikpruefung in einer Schleife oder einem Aktionsblock",
    }
    .to_string()
}

/// Eine Maschine: jeder Zustand ist ein eigener Einstiegspunkt, weil
/// zustandslokale Variablen bei jedem Eintritt neu initialisiert sind (5.8).
fn analyze_machine<'p>(program: &'p Program, id: usize) -> Walk<'p> {
    let m = &program.machines[id];
    let declared = declared_ranges(program, m);
    let mut w = Walk::new(program, declared);

    // Maschinenweiter `loop:` und die Fault-Zustaende.
    let mut f = machine_entry(program, m);
    w.block(&m.loop_block, &mut f);
    for h in &m.handlers {
        let mut inner = f.clone();
        w.block(&h.body, &mut inner);
    }
    for t in &m.faulted.transitions {
        let mut inner = f.clone();
        w.action(|w| w.block(&t.actions, &mut inner));
    }

    for s in &m.states {
        let mut f = machine_entry(program, m);
        w.action(|w| w.block(&s.enter, &mut f));
        w.block(&s.loop_block, &mut f);
        for h in &s.handlers {
            let mut inner = f.clone();
            w.block(&h.body, &mut inner);
        }
        if let Some(seq) = &s.sequence {
            let mut inner = f.clone();
            for item in &seq.items {
                if let crate::machine::SeqItem::Stmt(st) = item {
                    w.stmt(st, &mut inner);
                }
            }
        }
        for t in &s.transitions {
            let mut inner = f.clone();
            w.action(|w| w.block(&t.actions, &mut inner));
        }
        let mut exit = f.clone();
        w.action(|w| w.block(&s.exit, &mut exit));
    }
    w
}

/// Der Anfangszustand einer Maschine: jede Variable traegt ihre deklarierte
/// Range und gilt als zugewiesen, weil jede `var` einen Initialisierer hat
/// (2.3, `var_decl`).
fn machine_entry(program: &Program, m: &Machine) -> Facts {
    let mut f = Facts::entry();
    for (i, v) in m.vars.iter().enumerate() {
        // 3.4: „Ranges sind Tick-Rand-Invarianten" — eine Variable mit
        // deklarierter Range traegt sie an jedem Tick-Anfang, weil der
        // Compiler jede Zuweisung dagegen prueft.
        let start = match program.types.list.get(v.ty.index()) {
            Some(Type::Int { range: Some(r), .. } | Type::Duration { range: Some(r) }) => {
                domain::Interval::from_range(r)
            }
            _ => domain::Interval::Top,
        };
        f.declare(crate::VarId(i as u32), start);
    }
    f
}

/// Deklarierte Range je Variable, indiziert wie `Machine::vars`.
fn declared_ranges(program: &Program, m: &Machine) -> Vec<Option<Range>> {
    m.vars
        .iter()
        .map(|v| match program.types.list.get(v.ty.index()) {
            Some(Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range }) => *range,
            _ => None,
        })
        .collect()
}
