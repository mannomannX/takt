//! Pruefungen auf der MIR (plan/m1.md 3.10): Fault-Wald (9), Single-Writer
//! und Bindungen (7), Erreichbarkeit (10), Terminierung (11), Simulation (13),
//! ungenutzte Channels (15), Definite Assignment gehobener Variablen (6, 25).
//!
//! Sie laufen nach dem Lowering, aber vor dem Desugaring: dort sind Namen
//! aufgeloest, und die Sequenz-Oberflaeche zeigt noch ihre Segmentgrenzen.

use std::collections::{HashMap, HashSet};

use takt_diag::{Diagnostic, Span};
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::machine::*;
use takt_mir::program::{Binding, Direction};
use takt_mir::stmt::*;
use takt_mir::*;

use crate::lower::Lowerer;

/// Single-Writer, Richtung, Bindungen.
pub const SC7: &str = "SC-7";
/// Maschinenregeln.
pub const SC8: &str = "SC-8";
/// Fault-Wald.
pub const SC9: &str = "SC-9";
/// Erreichbarkeit und tote Uebergaenge.
pub const SC10: &str = "SC-10";
/// Terminierung.
pub const SC11: &str = "SC-11";
/// Simulation.
pub const SC13: &str = "SC-13";
/// Ungenutzte Channels.
pub const SC15: &str = "SC-15";
/// Definite Assignment.
pub const SC6: &str = "SC-6";

impl Lowerer<'_> {
    /// Fuehrt alle MIR-Pruefungen aus.
    pub fn run_mir_checks(&mut self) {
        self.default_max_age();
        self.check_writers();
        self.check_fault_forest();
        self.check_reachability();
        self.check_termination();
        self.check_simulation();
        self.check_unused();
        self.check_definite_assignment();
    }

    /// Traegt den Default fuer `max_age` ein (3.5): das Doppelte der
    /// kuerzesten Periode unter den Maschinen, die den Channel lesen. Die
    /// Zusicherung „dieser Wert ist frisch genug" gilt damit fuer jeden
    /// Leser, auch den schnellsten; ohne Default wurde ein toter Sensor nie
    /// `Stale`, und der implizite Validitaets-Check griff nie.
    fn default_max_age(&mut self) {
        let mut fastest: HashMap<ChannelId, u32> = HashMap::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            let period = m.period.max(1);
            for_each_expr_machine(m, &mut |e| {
                if let ExprKind::Input { channel, .. } = &e.kind {
                    let slot = fastest.entry(*channel).or_insert(period);
                    *slot = (*slot).min(period);
                }
            });
        }
        let tick = self.program.config.tick;
        for (i, c) in self.program.channels.iter_mut().enumerate() {
            if c.dir != Direction::Input || c.attrs.max_age.is_some() {
                continue;
            }
            if let Some(period) = fastest.get(&ChannelId(i as u32)) {
                c.attrs.max_age = Some(2 * i64::from(*period) * tick);
            }
        }
    }

    /// Pruefung 7 und 15 (Teil): Besitzer je Output, `pub var` nur vom
    /// Besitzer, Inputs nie Ziel.
    fn check_writers(&mut self) {
        let mut writers: HashMap<ChannelId, Vec<(MachineId, Span)>> = HashMap::new();
        for (mi, m) in self.program.machines.iter().enumerate() {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut seen: HashSet<ChannelId> = HashSet::new();
            for_each_stmt(m, &mut |s| {
                if let StmtKind::Assign { target, .. } = &s.kind {
                    if let Some(c) = output_of(target) {
                        if seen.insert(c) {
                            writers.entry(c).or_default().push((MachineId(mi as u32), s.span));
                        }
                    }
                }
            });
        }
        let mut diags = Vec::new();
        for (c, list) in &writers {
            if list.len() > 1 {
                let name = self.program.channels[c.index()].name.clone();
                let first = &self.program.machines[list[0].0.index()].name;
                let mut d = Diagnostic::error(SC7, list[1].1, format!("Output `{name}` hat mehrere Schreiber"))
                    .with_note(list[0].1, format!("auch in `{first}` geschrieben"))
                    .with_suggestion("jeder Output gehoert genau einer Maschine (1.4)");
                d.span = list[1].1;
                diags.push(d);
            }
        }
        for (c, list) in writers {
            self.program.channels[c.index()].owner = Some(list[0].0);
        }
        self.diags.extend(diags);
    }

    /// Pruefung 9: Fault-Wald azyklisch, `φ(s) ≠ s`, `FAULTED` erreichbar.
    fn check_fault_forest(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            // 5.3: damit `FAULTED` nie scheitern kann, duerfen die Guards
            // seiner Transitionen keine impliziten Pruefungen enthalten.
            for t in &m.faulted.transitions {
                if let TransTrigger::When(Guard::Expr(e)) = &t.trigger {
                    if has_checked(e) {
                        diags.push(
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
            for (i, s) in m.states.iter().enumerate() {
                let id = StateId(i as u32);
                let mut seen = HashSet::new();
                let mut cur = id;
                loop {
                    if !seen.insert(cur) {
                        diags.push(
                            Diagnostic::error(
                                SC9,
                                m.states[cur.index()].span,
                                format!("Fault-Wald hat einen Zyklus ueber `{}`", m.states[cur.index()].name),
                            )
                            .with_suggestion("`fault -> X` so waehlen, dass jeder Pfad bei `FAULTED` endet (5.3)"),
                        );
                        break;
                    }
                    match fault_target_of(m, cur) {
                        FaultTarget::Faulted => break,
                        FaultTarget::State(next) => {
                            if next == cur {
                                diags.push(
                                    Diagnostic::error(
                                        SC9,
                                        m.states[cur.index()].span,
                                        format!("`{}` ist sein eigenes Fault-Ziel", m.states[cur.index()].name),
                                    )
                                    .with_suggestion("φ(s) ≠ s (5.3)"),
                                );
                                break;
                            }
                            cur = next;
                        }
                    }
                }
                let _ = s;
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 10: Erreichbarkeit im Uebergangsgraphen, Zustaende ohne Ausgang.
    fn check_reachability(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) || m.states.is_empty() {
                continue;
            }
            let mut reached: HashSet<StateId> = HashSet::new();
            let mut stack = vec![m.initial];
            // Fault-Ziele sind ebenfalls erreichbar
            for (i, _) in m.states.iter().enumerate() {
                if let FaultTarget::State(t) = fault_target_of(m, StateId(i as u32)) {
                    stack.push(t);
                }
            }
            for t in &m.faulted.transitions {
                if let Target::State(s) = t.target {
                    stack.push(s);
                }
            }
            while let Some(s) = stack.pop() {
                if !reached.insert(s) {
                    continue;
                }
                // Kinder und Eltern gehoeren zur Konfiguration
                if let Some(init) = m.states[s.index()].initial {
                    stack.push(init);
                }
                if let Some(p) = m.states[s.index()].parent {
                    stack.push(p);
                }
                let mut targets = Vec::new();
                collect_targets(m, s, &mut targets);
                stack.extend(targets);
            }
            for (i, s) in m.states.iter().enumerate() {
                let id = StateId(i as u32);
                if !reached.contains(&id) {
                    diags.push(
                        Diagnostic::warning(SC10, s.span, format!("Zustand `{}` ist nicht erreichbar", s.name))
                            .with_suggestion("Uebergang ergaenzen oder Zustand entfernen"),
                    );
                }
                let mut targets = Vec::new();
                collect_targets(m, id, &mut targets);
                let leaf = s.children.is_empty();
                if leaf
                    && targets.is_empty()
                    && s.sequence.is_none()
                    && !s.name.contains("DONE")
                    && !s.name.contains("SAFE")
                {
                    diags.push(
                        Diagnostic::warning(SC10, s.span, format!("Zustand `{}` hat keinen Ausgang", s.name))
                            .with_suggestion("`when`/`after` ergaenzen, wenn er nicht endgueltig ist"),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 11: Aufrufgraph azyklisch, `step` hoechstens einmal je Instanz
    /// und Aktivierung.
    fn check_termination(&mut self) {
        let mut diags = Vec::new();
        // Aufrufgraph der Funktionen
        let mut edges: Vec<Vec<FnId>> = vec![Vec::new(); self.program.fns.len()];
        for (i, f) in self.program.fns.iter().enumerate() {
            let mut callees = Vec::new();
            for_each_expr_block(&f.body, &mut |e| {
                if let ExprKind::Call { callee, .. } = &e.kind {
                    callees.push(*callee);
                }
            });
            edges[i] = callees;
        }
        let mut state = vec![0u8; edges.len()];
        for i in 0..edges.len() {
            if state[i] == 0 && has_cycle(i, &edges, &mut state) {
                diags.push(
                    Diagnostic::error(
                        SC11,
                        self.program.fns[i].span,
                        format!("Aufrufgraph von `{}` ist zyklisch", self.program.fns[i].name),
                    )
                    .with_suggestion("Rekursion gibt es nicht (9.4.2, T3)"),
                );
            }
        }
        // step je Instanz und Block
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            let mut in_loop = Vec::new();
            for_each_stmt_ctx(m, &mut |s, depth| {
                if let StmtKind::MethodCall { receiver, method: Method::Step, .. } = &s.kind {
                    if depth > 0 {
                        if let Place::Var(v) = receiver {
                            let array = m.layout.block_instances.iter().any(|b| b.var == *v && b.count > 1);
                            if !array {
                                in_loop.push(s.span);
                            }
                        } else {
                            in_loop.push(s.span);
                        }
                    }
                }
            });
            for span in in_loop {
                diags.push(Diagnostic::error(SC11, span, "`step` in einer Schleife".to_string()).with_suggestion(
                    "jede Instanz steppt hoechstens einmal je Tick; Arrays von Instanzen verwenden (5.7)",
                ));
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 13: `hw`-Inputs ohne `sim`-Quelle (Festlegung 6: Warnung in
    /// `takt sim`, Fehler ab `takt test`).
    fn check_simulation(&mut self) {
        if self.options.build != crate::Build::Sim {
            return;
        }
        let sims: HashSet<String> = self
            .program
            .channels
            .iter()
            .filter(|c| c.dir == Direction::Output)
            .filter_map(|c| match &c.binding {
                Binding::Sim(a) => Some(address_key(a)),
                _ => None,
            })
            .collect();
        let mut diags = Vec::new();
        for c in &self.program.channels {
            if c.dir != Direction::Input {
                continue;
            }
            if let Binding::Hw(a) = &c.binding {
                if !sims.contains(&address_key(a)) {
                    diags.push(
                        Diagnostic::warning(SC13, c.span, format!("Input `{}` hat keine `sim`-Quelle", c.name))
                            .with_suggestion(format!("`output {}_sim : … @ sim(\"…\")` oder Stimulus (8.3)", c.name)),
                    );
                }
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefung 15: Outputs ohne Schreiber, Inputs ohne Leser.
    fn check_unused(&mut self) {
        let mut read: HashSet<ChannelId> = HashSet::new();
        for m in &self.program.machines {
            for_each_expr_machine(m, &mut |e| {
                if let ExprKind::Input { channel, .. } = &e.kind {
                    read.insert(*channel);
                }
            });
        }
        let mut diags = Vec::new();
        for (i, c) in self.program.channels.iter().enumerate() {
            let id = ChannelId(i as u32);
            match c.dir {
                Direction::Output if c.owner.is_none() && !matches!(c.binding, Binding::Sim(_)) => {
                    diags.push(
                        Diagnostic::warning(SC15, c.span, format!("Output `{}` wird nie geschrieben", c.name))
                            .with_suggestion("Zuweisung ergaenzen oder Channel entfernen"),
                    );
                }
                Direction::Input if !read.contains(&id) => {
                    diags.push(
                        Diagnostic::warning(SC15, c.span, format!("Input `{}` wird nie gelesen", c.name))
                            .with_suggestion("Verwendung ergaenzen oder Channel entfernen"),
                    );
                }
                _ => {}
            }
        }
        self.diags.extend(diags);
    }

    /// Pruefungen 6 und 25: eine gehobene Variable wird in einem Segment
    /// gelesen, bevor ein frueheres sie zuweist.
    fn check_definite_assignment(&mut self) {
        let mut diags = Vec::new();
        for m in &self.program.machines {
            if matches!(m.kind, MachineKind::Template) {
                continue;
            }
            for s in &m.states {
                let Some(seq) = &s.sequence else { continue };
                let mut assigned: HashSet<VarId> = HashSet::new();
                let mut lifted: HashSet<VarId> = HashSet::new();
                for (i, v) in m.vars.iter().enumerate() {
                    if matches!(v.scope, VarScope::Lifted(x) if m.states[x.index()].name == s.name) && v.init.is_none()
                    {
                        lifted.insert(VarId(i as u32));
                    }
                }
                check_seq_items(&seq.items, &mut assigned, &lifted, m, &mut diags);
            }
        }
        self.diags.extend(diags);
    }
}

fn check_seq_items(
    items: &[SeqItem],
    assigned: &mut HashSet<VarId>,
    lifted: &HashSet<VarId>,
    m: &Machine,
    diags: &mut Vec<Diagnostic>,
) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                for_each_expr_stmt(s, &mut |e| {
                    if let ExprKind::Var(v) = &e.kind {
                        if lifted.contains(v) && !assigned.contains(v) {
                            diags.push(
                                Diagnostic::error(
                                    SC6,
                                    e.span,
                                    format!("`{}` wird gelesen, bevor sie zugewiesen ist", m.vars[v.index()].name),
                                )
                                .with_suggestion("Zuweisung in ein frueheres Segment legen (6.2, 5.8)"),
                            );
                        }
                    }
                });
                if let StmtKind::Assign { target: Place::Var(v), .. } = &s.kind {
                    assigned.insert(*v);
                }
            }
            SeqItem::Repeat { body, counter, .. } => {
                assigned.insert(*counter);
                check_seq_items(body, assigned, lifted, m, diags);
            }
            SeqItem::Step { body, .. } => check_seq_items(body, assigned, lifted, m, diags),
            SeqItem::Until { guard, .. } => {
                if let Guard::Match { binding: Some(b), .. } | Guard::Next { binding: b, .. } = guard {
                    assigned.insert(*b);
                }
            }
            SeqItem::Wait(_) | SeqItem::Expect { .. } => {}
        }
    }
}

fn address_key(a: &takt_mir::pattern::Address) -> String {
    a.segments.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join("/")
}

fn output_of(p: &Place) -> Option<ChannelId> {
    match p {
        Place::Output(c) => Some(*c),
        Place::Field(b, _) | Place::Index(b, _) | Place::Index2(b, _, _) => output_of(b),
        Place::Var(_) => None,
    }
}

/// Fault-Ziel φ(s) nach 5.3: explizit, sonst geerbt vom Elternzustand, sonst
/// von der Maschine — mit der Ausnahme, dass der als Fault-Ziel der Maschine
/// deklarierte Zustand nicht von ihr erbt, sondern `FAULTED` bekommt.
pub fn fault_target_of(m: &Machine, s: StateId) -> FaultTarget {
    let mut cur = Some(s);
    while let Some(id) = cur {
        if let Some(t) = m.states[id.index()].fault_target {
            return t;
        }
        cur = m.states[id.index()].parent;
    }
    if m.fault_target == FaultTarget::State(s) { FaultTarget::Faulted } else { m.fault_target }
}

fn collect_targets(m: &Machine, s: StateId, out: &mut Vec<StateId>) {
    let state = &m.states[s.index()];
    for t in &state.transitions {
        if let Target::State(x) = t.target {
            out.push(x);
        }
    }
    let mut push_goto = |b: &Block| {
        for_each_stmt_block(b, &mut |st| {
            if let StmtKind::Goto(Target::State(x)) = &st.kind {
                out.push(*x);
            }
            if let StmtKind::Check { target: Some(Target::State(x)), .. } = &st.kind {
                out.push(*x);
            }
        });
    };
    push_goto(&state.loop_block);
    push_goto(&state.enter);
    if let Some(seq) = &state.sequence {
        collect_seq_targets(&seq.items, out);
    }
    let _ = m;
}

fn collect_seq_targets(items: &[SeqItem], out: &mut Vec<StateId>) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => {
                if let StmtKind::Goto(Target::State(x)) = &s.kind {
                    out.push(*x);
                }
                for_each_stmt_block(&Block::new(vec![s.clone()]), &mut |st| {
                    if let StmtKind::Goto(Target::State(x)) = &st.kind {
                        out.push(*x);
                    }
                });
            }
            SeqItem::Until { timeout: Some(t), .. } => match &t.action {
                TimeoutAction::Goto(Target::State(x)) => out.push(*x),
                TimeoutAction::Else(b) => for_each_stmt_block(b, &mut |st| {
                    if let StmtKind::Goto(Target::State(x)) = &st.kind {
                        out.push(*x);
                    }
                }),
                _ => {}
            },
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => collect_seq_targets(body, out),
            _ => {}
        }
    }
}

fn has_cycle(i: usize, edges: &[Vec<FnId>], state: &mut [u8]) -> bool {
    state[i] = 1;
    for c in &edges[i] {
        let j = c.index();
        if j >= state.len() {
            continue;
        }
        if state[j] == 1 || (state[j] == 0 && has_cycle(j, edges, state)) {
            return true;
        }
    }
    state[i] = 2;
    false
}

// ------------------------------------------------------------ Durchlaeufe

/// Jede Anweisung einer Maschine (auch geschachtelte Bloecke).
pub fn for_each_stmt(m: &Machine, f: &mut impl FnMut(&Stmt)) {
    for_each_stmt_ctx(m, &mut |s, _| f(s));
}

/// Wie `for_each_stmt`, mit Schleifentiefe.
pub fn for_each_stmt_ctx(m: &Machine, f: &mut impl FnMut(&Stmt, u32)) {
    let visit_block = |b: &Block, f: &mut dyn FnMut(&Stmt, u32)| walk_stmts(&b.stmts, 0, f);
    visit_block(&m.loop_block, f);
    for t in &m.faulted.transitions {
        visit_block(&t.actions, f);
    }
    for s in &m.states {
        visit_block(&s.enter, f);
        visit_block(&s.exit, f);
        visit_block(&s.loop_block, f);
        for t in &s.transitions {
            visit_block(&t.actions, f);
        }
        if let Some(seq) = &s.sequence {
            walk_seq(&seq.items, f);
        }
    }
}

fn walk_seq(items: &[SeqItem], f: &mut dyn FnMut(&Stmt, u32)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => walk_stmts(std::slice::from_ref(s), 0, f),
            SeqItem::Until { timeout: Some(t), .. } => {
                if let TimeoutAction::Else(b) = &t.action {
                    walk_stmts(&b.stmts, 0, f);
                }
            }
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => walk_seq(body, f),
            _ => {}
        }
    }
}

fn walk_stmts(stmts: &[Stmt], depth: u32, f: &mut dyn FnMut(&Stmt, u32)) {
    for s in stmts {
        f(s, depth);
        match &s.kind {
            StmtKind::If { then, otherwise, .. } => {
                walk_stmts(&then.stmts, depth, f);
                walk_stmts(&otherwise.stmts, depth, f);
            }
            StmtKind::ForRange { body, .. } | StmtKind::ForEach { body, .. } => walk_stmts(&body.stmts, depth + 1, f),
            StmtKind::Every { body, .. } | StmtKind::At { body, .. } => walk_stmts(&body.stmts, depth, f),
            StmtKind::Match { arms, .. } => {
                for a in arms {
                    walk_stmts(&a.body.stmts, depth, f);
                }
            }
            _ => {}
        }
    }
}

/// Jede Anweisung eines Blocks.
pub fn for_each_stmt_block(b: &Block, f: &mut impl FnMut(&Stmt)) {
    walk_stmts(&b.stmts, 0, &mut |s, _| f(s));
}

/// Jeder Ausdruck einer Anweisung.
pub fn for_each_expr_stmt(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    walk_stmts(std::slice::from_ref(s), 0, &mut |s, _| stmt_exprs(s, &mut |e| walk_expr(e, f)));
}

/// Jeder Ausdruck eines Blocks.
pub fn for_each_expr_block(b: &Block, f: &mut impl FnMut(&Expr)) {
    walk_stmts(&b.stmts, 0, &mut |s, _| stmt_exprs(s, &mut |e| walk_expr(e, f)));
}

/// Jeder Ausdruck einer Maschine.
pub fn for_each_expr_machine(m: &Machine, f: &mut impl FnMut(&Expr)) {
    for v in &m.vars {
        if let Some(init) = &v.init {
            walk_expr(init, f);
        }
    }
    for_each_stmt(m, &mut |s| stmt_exprs(s, &mut |e| walk_expr(e, f)));
    for s in &m.states {
        for t in &s.transitions {
            trigger_exprs(&t.trigger, &mut |e| walk_expr(e, f));
        }
        if let Some(seq) = &s.sequence {
            seq_exprs(&seq.items, &mut |e| walk_expr(e, f));
        }
    }
    for t in &m.faulted.transitions {
        trigger_exprs(&t.trigger, &mut |e| walk_expr(e, f));
    }
}

fn trigger_exprs(t: &TransTrigger, f: &mut impl FnMut(&Expr)) {
    match t {
        TransTrigger::After(d) => f(d),
        TransTrigger::When(Guard::Expr(e)) => f(e),
        TransTrigger::When(Guard::Match { subject, .. }) => f(subject),
        TransTrigger::When(Guard::Next { .. }) => {}
    }
}

fn seq_exprs(items: &[SeqItem], f: &mut impl FnMut(&Expr)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => stmt_exprs(s, f),
            SeqItem::Wait(d) => f(d),
            SeqItem::Until { guard, timeout, .. } => {
                match guard {
                    Guard::Expr(e) => f(e),
                    Guard::Match { subject, .. } => f(subject),
                    Guard::Next { .. } => {}
                }
                if let Some(t) = timeout {
                    f(&t.duration);
                }
            }
            SeqItem::Expect { cond, .. } => f(cond),
            SeqItem::Repeat { count, body, .. } => {
                f(count);
                seq_exprs(body, f);
            }
            SeqItem::Step { body, .. } => seq_exprs(body, f),
        }
    }
}

fn stmt_exprs(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Assign { target, value } => {
            place_exprs(target, f);
            f(value);
        }
        StmtKind::Check { cond, message, confirm, .. } => {
            f(cond);
            if let Some(m) = message {
                format_exprs(m, f);
            }
            if let Some(c) = confirm {
                f(&c.duration);
            }
        }
        StmtKind::Abort { message: Some(m) } => format_exprs(m, f),
        StmtKind::If { cond, .. } => f(cond),
        StmtKind::ForRange { count, .. } => f(count),
        StmtKind::ForEach { iter, .. } => f(iter),
        StmtKind::Match { subject, arms } => {
            f(subject);
            for a in arms {
                if let ArmPattern::Values(vals) = &a.pattern {
                    for v in vals {
                        f(&v.lo);
                        if let Some(hi) = &v.hi {
                            f(hi);
                        }
                    }
                }
            }
        }
        StmtKind::Return(e) => f(e),
        StmtKind::Send { value, .. } => f(value),
        StmtKind::At { time, .. } => f(time),
        StmtKind::Job { args, .. } => {
            for a in args {
                f(a);
            }
        }
        StmtKind::Every { period, .. } => f(period),
        StmtKind::Observe(o) => match o {
            Observe::Alert { cond, message, confirm } => {
                f(cond);
                format_exprs(message, f);
                if let Some(c) = confirm {
                    f(&c.duration);
                }
            }
            Observe::Log(m) => format_exprs(m, f),
            Observe::Measure { value, .. } => f(value),
            Observe::Verify { cond, message, .. } => {
                f(cond);
                format_exprs(message, f);
            }
            Observe::Verdict { message: Some(m), .. } => format_exprs(m, f),
            Observe::Verdict { .. } => {}
        },
        StmtKind::MethodCall { target, receiver, args, .. } => {
            if let Some(t) = target {
                place_exprs(t, f);
            }
            place_exprs(receiver, f);
            for a in args {
                f(a);
            }
        }
        _ => {}
    }
}

fn format_exprs(m: &takt_mir::pattern::Format, f: &mut impl FnMut(&Expr)) {
    for p in &m.pieces {
        if let takt_mir::pattern::FormatPiece::Expr { expr, .. } = p {
            f(expr);
        }
    }
}

fn place_exprs(p: &Place, f: &mut impl FnMut(&Expr)) {
    match p {
        Place::Var(_) | Place::Output(_) => {}
        Place::Field(b, _) => place_exprs(b, f),
        Place::Index(b, i) => {
            place_exprs(b, f);
            f(i);
        }
        Place::Index2(b, i, j) => {
            place_exprs(b, f);
            f(i);
            f(j);
        }
    }
}

/// Ausdruck und alle Teilausdruecke.
pub fn walk_expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    let mut sub = |x: &Expr| walk_expr(x, f);
    match &e.kind {
        ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter().for_each(sub),
        ExprKind::Array(items) => items.iter().for_each(sub),
        ExprKind::Tuple(a, b) => {
            sub(a);
            sub(b);
        }
        ExprKind::BlockInit { args, .. }
        | ExprKind::Call { args, .. }
        | ExprKind::NativeCall { args, .. }
        | ExprKind::MatOp { args, .. }
        | ExprKind::Intrinsic { args, .. } => args.iter().for_each(sub),
        ExprKind::Field { base, .. } => sub(base),
        ExprKind::Index { base, index } => {
            sub(base);
            sub(index);
        }
        ExprKind::Index2 { base, row, col } => {
            sub(base);
            sub(row);
            sub(col);
        }
        ExprKind::Slice { base, from, to } => {
            sub(base);
            sub(from);
            sub(to);
        }
        ExprKind::Accessor { base, args, .. } => {
            sub(base);
            args.iter().for_each(sub);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Cast { expr, .. }
        | ExprKind::Convert { expr, .. }
        | ExprKind::Checked { expr, .. } => sub(expr),
        ExprKind::Lift(x) | ExprKind::Ok(x) | ExprKind::Err(x) => sub(x),
        ExprKind::Binary { lhs, rhs, .. } => {
            sub(lhs);
            sub(rhs);
        }
        ExprKind::Cond { cond, then, otherwise } => {
            sub(cond);
            sub(then);
            sub(otherwise);
        }
        ExprKind::Matches { subject, .. } => sub(subject),
        ExprKind::Decode { bytes, .. } => sub(bytes),
        ExprKind::Published { machine, .. } | ExprKind::StateOf(machine) | ExprKind::Signal { machine, .. } => {
            if let Some(i) = &machine.index {
                sub(i);
            }
        }
        _ => {}
    }
    let _ = SC8;
}

/// Enthaelt der Ausdruck eine implizite Pruefung (5.3, Pruefung 9)?
fn has_checked(e: &Expr) -> bool {
    let mut found = false;
    walk_expr(e, &mut |x| {
        if matches!(x.kind, ExprKind::Checked { .. }) {
            found = true;
        }
    });
    found
}
