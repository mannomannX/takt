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

pub mod budget;
pub mod cost;
pub mod domain;
pub mod facts;
pub mod graph;
pub mod latency;
pub mod narrow;
pub mod proof;
pub mod prove;
pub mod schedulability;
pub mod schedule;
pub mod size;
pub mod stack;
pub mod term;
pub mod walk;

use std::collections::{BTreeMap, BTreeSet};

use takt_diag::Diagnostic;

use crate::analysis::facts::Facts;
use crate::analysis::walk::{CheckCause, ImplicitCheck, Walk};
use crate::expr::{CheckedKind, Expr, ExprKind};
use crate::machine::{Guard, Machine, MachineKind, TransTrigger};
use crate::stmt::{Block, Method, Observe, Place, Stmt, StmtKind};
use crate::types::{Range, RangeOrigin, Type};
use crate::{FnId, Program, TypeId};

/// Code der Warnungen ueber implizite Pruefungen (Pruefung 24).
pub const SC24: &str = "SC-24";

/// Code der Pruefung 9: Guards aus `FAULTED` ohne implizite Pruefung.
pub const SC9: &str = "SC-9";

/// Was die Analyse ueber ein Programm herausgefunden hat.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Implizite Pruefungen je Ursache (3.4, Kennzahl).
    pub checks: BTreeMap<&'static str, u32>,
    /// Davon in Schleifen oder Aktionsbloecken — nur diese warnen.
    pub warned: u32,
    /// Davon Oktagon-Kandidaten: eine Relation zweier Variablen koennte sie
    /// beweisen (plan/m6.md 2.12, Kennzahl vor der Entscheidung).
    pub relational: u32,
    /// Ausdruecke, deren Darstellung auf 32 Bit verengt wurde (Lemma 3.4).
    pub narrowed: u32,
    /// Ausdruecke mit Integer-Darstellung insgesamt.
    pub integer_exprs: u32,
    /// Die verbliebenen Pruefungen, je Stelle eine.
    pub sites: Vec<ImplicitCheck>,
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
        out.push(format!("davon relational:     {} (Oktagon-Kandidaten, plan/m6.md 2.12)", self.relational));
        out.push(format!("Darstellung:          {} von {} Ausdruecken in i32", self.narrowed, self.integer_exprs));
        out
    }
}

/// Fuehrt die Analyse aus: annotiert die MIR und liefert Diagnosen und
/// Kennzahlen. `external` sind Stellen aus einer Beweisdatei (11.3), als
/// `(Versatz, Art)`; sie gelten zusaetzlich zu den eigenen Beweisen.
pub fn analyze(program: &mut Program, external: &[(u32, u8)]) -> (Vec<Diagnostic>, Report) {
    let mut diags = Vec::new();
    let mut report = Report::default();
    let mut all: Vec<ImplicitCheck> = Vec::new();
    let mut proofs = prove::Proofs::default();

    for id in 0..program.machines.len() {
        let w = analyze_machine(program, id);
        proofs.add(&w.proven, &w.kept, &w.ranges);
        all.extend(w.checks);
    }
    for id in reachable_fns(program) {
        let w = analyze_fn(program, &program.fns[id.index()]);
        proofs.add(&w.proven, &w.kept, &w.ranges);
        all.extend(w.checks);
    }
    // Gewarnt wird im Programm des Nutzers, nicht im Prelude.
    let user: BTreeSet<u32> = program.machines.iter().map(|m| m.span.file.0).collect();
    // Was bewiesen ist, verschwindet aus der MIR; die Intervalle bleiben als
    // Annotation stehen (3.4).
    prove::apply(program, &proofs);
    if !external.is_empty() {
        let mut given = prove::Proofs::default();
        for file in &user {
            given.dropped.extend(external.iter().map(|(start, tag)| (*file, *start, *tag)));
        }
        prove::apply(program, &given);
    }
    diags.extend(faulted_guards(program));

    // Eine Pruefung ist eine *Stelle* im Programm, keine Ausfuehrung: Ein
    // abgerollter Schleifenkoerper besucht dieselbe Stelle mehrfach, zaehlt
    // aber einmal. Warnt einer der Besuche, warnt die Stelle.
    let mut seen: BTreeMap<(u32, u32, u8), ImplicitCheck> = BTreeMap::new();
    for c in &all {
        if user.contains(&c.span.file.0) && external.contains(&(c.span.start, c.tag)) {
            continue;
        }
        let key = (c.span.file.0, c.span.start, c.tag);
        seen.entry(key)
            .and_modify(|e| {
                e.warns |= c.warns;
                e.relational |= c.relational;
            })
            .or_insert(*c);
    }
    for c in seen.values() {
        *report.checks.entry(c.cause.name()).or_default() += 1;
        if c.relational {
            report.relational += 1;
        }
        if c.warns && user.contains(&c.span.file.0) {
            report.warned += 1;
            diags.push(Diagnostic::warning(SC24, c.span, message(c.cause)).with_suggestion(
                "Range deklarieren, `clamp` benutzen oder eine range-typisierte Zwischengroesse einfuehren (3.4)",
            ));
        }
    }
    report.sites = seen.values().copied().collect();
    // Ursachen ohne Fund erscheinen mit 0, damit die Zeile stabil bleibt.
    for c in [CheckCause::Declared, CheckCause::Index, CheckCause::Convert, CheckCause::Arith, CheckCause::NonFinite] {
        report.checks.entry(c.name()).or_default();
    }

    let n = narrow::narrow(program);
    report.narrowed = n.0;
    report.integer_exprs = n.1;
    (diags, report)
}

/// Pruefung 9 (5.3): Ein Guard aus `FAULTED` darf nicht faulten — was die
/// Analyse nicht wegbeweist, ist ein Fehler.
fn faulted_guards(program: &Program) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for m in &program.machines {
        if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
            continue;
        }
        for t in &m.faulted.transitions {
            if let TransTrigger::When(Guard::Expr(e)) = &t.trigger
                && has_check(e)
            {
                out.push(
                    Diagnostic::error(
                        SC9,
                        t.span,
                        format!("Guard aus `FAULTED` von `{}` enthaelt eine implizite Pruefung", m.name),
                    )
                    .with_suggestion(
                        "Channel nur unter `.valid` oder mit `.or(...)` lesen; keine Range- oder \
                         Arithmetik-Pruefung (5.3)",
                    ),
                );
            }
        }
    }
    out
}

/// Bleibt im Ausdruck eine Pruefung stehen?
fn has_check(e: &Expr) -> bool {
    let own = match &e.kind {
        ExprKind::Checked { kind: CheckedKind::Range(r), .. } => r.origin != RangeOrigin::Proven,
        ExprKind::Checked { .. } => true,
        _ => false,
    };
    own || e.children().iter().any(|c| has_check(c))
}

/// Die Meldung zu einer Ursache.
fn message(c: CheckCause) -> String {
    match c {
        CheckCause::Declared => "impliziter Range-Check in einer Schleife oder einem Aktionsblock",
        CheckCause::Index => "impliziter Index-Check in einer Schleife oder einem Aktionsblock",
        CheckCause::Convert => "implizite Konversionspruefung in einer Schleife oder einem Aktionsblock",
        CheckCause::Arith => "implizite Arithmetikpruefung in einer Schleife oder einem Aktionsblock",
        CheckCause::NonFinite => "implizite Endlichkeitspruefung in einer Schleife oder einem Aktionsblock",
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
    m.vars.iter().map(|v| range_of(program, v.ty)).collect()
}

fn range_of(program: &Program, ty: TypeId) -> Option<Range> {
    match program.types.list.get(ty.index()) {
        Some(Type::Int { range, .. } | Type::Float { range, .. } | Type::Duration { range }) => *range,
        _ => None,
    }
}

/// Eine Funktion: die Parameter tragen ihre Range hinein (3.4), die
/// lokalen Variablen werden im Rumpf zugewiesen.
fn analyze_fn<'p>(program: &'p Program, f: &'p crate::fns::Fn) -> Walk<'p> {
    let declared = f.locals.iter().map(|v| range_of(program, v.ty)).collect();
    let mut w = Walk::new(program, declared);
    let mut facts = Facts::entry();
    for (i, v) in f.locals.iter().enumerate().take(f.params.len()) {
        let start = range_of(program, v.ty).map_or(domain::Interval::Top, |r| domain::Interval::from_range(&r));
        facts.declare(crate::VarId(i as u32), start);
    }
    w.block(&f.body, &mut facts);
    w
}

/// Funktionen, die Maschinencode erreicht; Bibliothekscode, den niemand
/// ruft, gehoert nicht zum Programm (Lemma 3.4).
fn reachable_fns(program: &Program) -> Vec<FnId> {
    let mut calls = Vec::new();
    for m in &program.machines {
        for init in m.vars.iter().filter_map(|v| v.init.as_ref()) {
            calls_in(init, program, &mut calls);
        }
        for h in m.handlers.iter().chain(m.states.iter().flat_map(|s| &s.handlers)) {
            if let Some(g) = &h.guard {
                calls_in(g, program, &mut calls);
            }
        }
        for t in m.faulted.transitions.iter().chain(m.states.iter().flat_map(|s| &s.transitions)) {
            match &t.trigger {
                TransTrigger::When(Guard::Expr(e)) | TransTrigger::After(e) => calls_in(e, program, &mut calls),
                TransTrigger::When(Guard::Match { subject, .. }) => calls_in(subject, program, &mut calls),
                TransTrigger::When(Guard::Next { .. }) => {}
            }
        }
        for b in m.blocks() {
            calls_in_block(b, program, &mut calls);
        }
    }
    let mut seen = vec![false; program.fns.len()];
    let mut out = Vec::new();
    while let Some(f) = calls.pop() {
        if std::mem::replace(&mut seen[f.index()], true) {
            continue;
        }
        out.push(f);
        let def = &program.fns[f.index()];
        for init in def.locals.iter().filter_map(|v| v.init.as_ref()) {
            calls_in(init, program, &mut calls);
        }
        calls_in_block(&def.body, program, &mut calls);
    }
    out.sort_by_key(|f| f.index());
    out
}

fn calls_in_block(b: &Block, program: &Program, out: &mut Vec<FnId>) {
    b.walk(&mut |s| {
        if let StmtKind::MethodCall { method: Method::Block(f), .. } = &s.kind {
            out.push(*f);
        }
        for e in stmt_exprs(s) {
            calls_in(e, program, out);
        }
    });
}

fn calls_in(e: &Expr, program: &Program, out: &mut Vec<FnId>) {
    match &e.kind {
        ExprKind::Call { callee, .. } => out.push(*callee),
        ExprKind::BlockInit { block, .. } => {
            let b = &program.blocks[block.index()];
            out.extend(b.step.iter().chain(&b.methods).copied());
        }
        _ => {}
    }
    for c in e.children() {
        calls_in(c, program, out);
    }
}

/// Die Ausdruecke einer Anweisung, ohne die der Unterbloecke.
fn stmt_exprs(s: &Stmt) -> Vec<&Expr> {
    match &s.kind {
        StmtKind::Assign { target, value } => {
            let mut v = place_exprs(target);
            v.push(value);
            v
        }
        StmtKind::Check { cond, confirm, .. } => {
            let mut v = vec![cond];
            v.extend(confirm.iter().map(|c| &c.duration));
            v
        }
        StmtKind::If { cond, .. } => vec![cond],
        StmtKind::ForRange { count, .. } => vec![count],
        StmtKind::ForEach { iter, .. } => vec![iter],
        StmtKind::Match { subject, .. } => vec![subject],
        StmtKind::Every { period, .. } => vec![period],
        StmtKind::At { time, .. } => vec![time],
        StmtKind::Send { value, .. } | StmtKind::Return(value) => vec![value],
        StmtKind::MethodCall { target, args, .. } => {
            let mut v = target.as_ref().map(place_exprs).unwrap_or_default();
            v.extend(args);
            v
        }
        StmtKind::Job { args, .. } => args.iter().collect(),
        StmtKind::Observe(Observe::Alert { cond, confirm, .. }) => {
            let mut v = vec![cond];
            v.extend(confirm.iter().map(|c| &c.duration));
            v
        }
        StmtKind::Observe(Observe::Measure { value, .. }) => vec![value],
        StmtKind::Observe(Observe::Verify { cond, .. }) => vec![cond],
        _ => Vec::new(),
    }
}

fn place_exprs(p: &Place) -> Vec<&Expr> {
    match p {
        Place::Var(_) | Place::Output(_) | Place::Port(_) => Vec::new(),
        Place::Field(b, _) => place_exprs(b),
        Place::Index(b, i) => {
            let mut v = place_exprs(b);
            v.push(i);
            v
        }
        Place::Index2(b, r, c) => {
            let mut v = place_exprs(b);
            v.extend([r, c]);
            v
        }
    }
}
