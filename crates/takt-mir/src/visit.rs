//! Durchlaeufe ueber eine Maschine: jede Anweisung, jeder Ausdruck, auch
//! in Sequenzen, Meldungen und Uebergangsbedingungen. Pruefungen der Sema
//! und der Codegen fragen dieselben Stellen ab; ein zweiter Durchlauf
//! koennte eine uebersehen, die der erste kennt.
//!
//! **Ohne Platzhalter** (FB-377): Jede Variante und jedes Feld, das einen
//! Ausdruck oder Block tragen kann, steht hier beim Namen, auch in
//! [`Machine`], [`State`] und [`Handler`]. Eine neue Variante oder ein neues
//! Feld uebersetzt erst, wenn der Durchlauf sie kennt; vorher blieben ein
//! `within:` eines `check` und die Bedingung eines Handlers ungelesen
//! (FB-405).

use crate::expr::{Builtin, Expr, ExprKind};
use crate::machine::*;
use crate::pattern::Pattern;
use crate::stmt::*;

/// Jede Anweisung einer Maschine (auch geschachtelte Bloecke).
pub fn for_each_stmt(m: &Machine, f: &mut impl FnMut(&Stmt)) {
    for_each_stmt_ctx(m, &mut |s, _| f(s));
}

/// Wie `for_each_stmt`, mit Schleifentiefe.
pub fn for_each_stmt_ctx(m: &Machine, f: &mut impl FnMut(&Stmt, u32)) {
    for_each_block(m, &mut |b| walk_stmts(&b.stmts, 0, f));
    for s in &m.states {
        if let Some(seq) = &s.sequence {
            walk_seq(&seq.items, f);
        }
    }
}

/// Die Bloecke einer Maschine ausserhalb ihrer Sequenzen.
///
/// Ein Handler-Rumpf ist gewoehnlicher Code (8.7): er schreibt Outputs und
/// liest Channels wie jeder andere Block.
pub fn for_each_block(m: &Machine, f: &mut impl FnMut(&Block)) {
    for_each_block_in(m, &mut |b, _| f(b));
}

/// Wie `for_each_block`, mit der Angabe, ob der Block ein Handler-Rumpf
/// ist: Er laeuft je Element des Fensters, also mehrmals je Tick (8.7, 9.7).
pub fn for_each_block_in(m: &Machine, f: &mut impl FnMut(&Block, bool)) {
    let Machine {
        name: _,
        kind: _,
        driver: _,
        polling_unchecked: _,
        fault_is_fail: _,
        params: _,
        period: _,
        phase: _,
        follows: _,
        node: _,
        vars: _,
        persist: _,
        signals: _,
        fault_target: _,
        states,
        roots: _,
        initial: _,
        loop_block,
        handlers,
        faulted,
        layout: _,
        budget: _,
        declared_budget: _,
        meta: _,
        span: _,
    } = m;
    f(loop_block, false);
    for h in handlers {
        f(&h.body, true);
    }
    for t in &faulted.transitions {
        f(&t.actions, false);
    }
    for s in states {
        let State {
            name: _,
            parent: _,
            children: _,
            initial: _,
            idle: _,
            resume: _,
            vars: _,
            enter,
            exit,
            loop_block,
            handlers,
            transitions,
            fault_target: _,
            sequence: _,
            sequence_ticks: _,
            instances: _,
            step_name: _,
            meta: _,
            span: _,
        } = s;
        f(enter, false);
        f(exit, false);
        f(loop_block, false);
        for h in handlers {
            f(&h.body, true);
        }
        for t in transitions {
            f(&t.actions, false);
        }
    }
}

/// Jede Anweisung einer Sequenz (6.2), mit Schleifentiefe.
pub fn walk_seq(items: &[SeqItem], f: &mut dyn FnMut(&Stmt, u32)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => walk_stmts(std::slice::from_ref(s), 0, f),
            SeqItem::Until { timeout: Some(t), .. } => {
                if let TimeoutAction::Else(b) = &t.action {
                    walk_stmts(&b.stmts, 0, f);
                }
            }
            SeqItem::Repeat { body, .. } | SeqItem::Step { body, .. } => walk_seq(body, f),
            SeqItem::Until { timeout: None, .. } | SeqItem::Wait(_) | SeqItem::Expect { .. } => {}
        }
    }
}

/// Jede Anweisung, auch in geschachtelten Bloecken; `for` erhoeht die Tiefe.
pub fn walk_stmts(stmts: &[Stmt], depth: u32, f: &mut dyn FnMut(&Stmt, u32)) {
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
            StmtKind::Assign { .. }
            | StmtKind::Check { .. }
            | StmtKind::Goto(_)
            | StmtKind::Abort { .. }
            | StmtKind::Return(_)
            | StmtKind::Send { .. }
            | StmtKind::Cancel(_)
            | StmtKind::Skip(_)
            | StmtKind::Raise(_)
            | StmtKind::Job { .. }
            | StmtKind::Break
            | StmtKind::Observe(_)
            | StmtKind::Arm { .. }
            | StmtKind::MethodCall { .. }
            | StmtKind::Pass => {}
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
    for prm in &m.params {
        if let Some(d) = &prm.default {
            walk_expr(d, f);
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
    for h in m.handlers.iter().chain(m.states.iter().flat_map(|s| &s.handlers)) {
        handler_exprs(h, &mut |e| walk_expr(e, f));
    }
}

/// Muster und Bedingung eines Handlers; den Rumpf liefert [`for_each_stmt`].
fn handler_exprs(h: &Handler, f: &mut impl FnMut(&Expr)) {
    let Handler { stream: _, pattern, binding: _, guard, body: _, span: _ } = h;
    if let Some((_, p)) = pattern {
        pattern_exprs(p, f);
    }
    if let Some(g) = guard {
        f(g);
    }
}

fn pattern_exprs(p: &Pattern, f: &mut impl FnMut(&Expr)) {
    match p {
        Pattern::Text { pieces: _ } => {}
        Pattern::Record { record: _, fields } => {
            for (_, e) in fields {
                f(e);
            }
        }
    }
}

fn guard_exprs(g: &Guard, f: &mut impl FnMut(&Expr)) {
    match g {
        Guard::Expr(e) => f(e),
        Guard::Match { subject, kind: _, pattern, binding: _ } => {
            f(subject);
            pattern_exprs(pattern, f);
        }
        Guard::Next { stream: _, binding: _ } => {}
    }
}

fn trigger_exprs(t: &TransTrigger, f: &mut impl FnMut(&Expr)) {
    match t {
        TransTrigger::After(d) => f(d),
        TransTrigger::When(g) => guard_exprs(g, f),
    }
}

fn seq_exprs(items: &[SeqItem], f: &mut impl FnMut(&Expr)) {
    for item in items {
        match item {
            SeqItem::Stmt(s) => stmt_exprs(s, f),
            SeqItem::Wait(d) => f(d),
            SeqItem::Until { guard, timeout, .. } => {
                guard_exprs(guard, f);
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

/// Die Ausdruecke einer Anweisung, mit Meldungen, ohne die ihrer Unterbloecke.
pub fn stmt_exprs(s: &Stmt, f: &mut impl FnMut(&Expr)) {
    match &s.kind {
        StmtKind::Assign { target, value } => {
            place_exprs(target, f);
            f(value);
        }
        StmtKind::Check { cond, message, confirm, within, target: _, req: _, kind: _ } => {
            f(cond);
            if let Some(m) = message {
                format_exprs(m, f);
            }
            if let Some(c) = confirm {
                f(&c.duration);
            }
            if let Some(w) = within {
                f(w);
            }
        }
        StmtKind::Abort { message: Some(m) } => format_exprs(m, f),
        StmtKind::Abort { message: None } => {}
        StmtKind::If { cond, then: _, otherwise: _ } => f(cond),
        StmtKind::ForRange { var: _, count, body: _ } => f(count),
        StmtKind::ForEach { vars: _, iter, body: _ } => f(iter),
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
        StmtKind::Send { stream: _, value, len_max: _ } => f(value),
        StmtKind::At { time, body: _ } => f(time),
        StmtKind::Job { handle: _, native: _, args } => {
            for a in args {
                f(a);
            }
        }
        StmtKind::Every { period, counter: _, body: _ } => f(period),
        StmtKind::Observe(o) => match o {
            Observe::Alert { cond, message, confirm, req: _ } => {
                f(cond);
                format_exprs(message, f);
                if let Some(c) = confirm {
                    f(&c.duration);
                }
            }
            Observe::Log(m) => format_exprs(m, f),
            Observe::Measure { name: _, value } => f(value),
            Observe::Verify { cond, message, req: _ } => {
                f(cond);
                format_exprs(message, f);
            }
            Observe::Verdict { message: Some(m), .. } => format_exprs(m, f),
            Observe::Verdict { .. } => {}
        },
        StmtKind::MethodCall { target, receiver, method: _, args } => {
            if let Some(t) = target {
                place_exprs(t, f);
            }
            place_exprs(receiver, f);
            for a in args {
                f(a);
            }
        }
        StmtKind::Goto(_)
        | StmtKind::Cancel(_)
        | StmtKind::Skip(_)
        | StmtKind::Raise(_)
        | StmtKind::Break
        | StmtKind::Arm { .. }
        | StmtKind::Pass => {}
    }
}

fn format_exprs(m: &crate::pattern::Format, f: &mut impl FnMut(&Expr)) {
    for p in &m.pieces {
        if let crate::pattern::FormatPiece::Expr { expr, .. } = p {
            f(expr);
        }
    }
}

fn place_exprs(p: &Place, f: &mut impl FnMut(&Expr)) {
    match p {
        Place::Var(_) | Place::Output(_) | Place::Port(_) => {}
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
        ExprKind::Variant { enum_id: _, variant: _, fields } | ExprKind::Record { record: _, fields } => {
            fields.iter().for_each(sub);
        }
        ExprKind::Array(items) => items.iter().for_each(sub),
        ExprKind::Tuple(a, b) => {
            sub(a);
            sub(b);
        }
        ExprKind::BlockInit { block: _, args, count: _ }
        | ExprKind::Call { callee: _, args }
        | ExprKind::NativeCall { native: _, args }
        | ExprKind::MatOp { op: _, args }
        | ExprKind::Intrinsic { op: _, args } => args.iter().for_each(sub),
        ExprKind::Field { base, field: _ } => sub(base),
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
        ExprKind::Accessor { base, accessor: _, args } => {
            sub(base);
            args.iter().for_each(sub);
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Cast { expr, to: _ }
        | ExprKind::Convert { expr, kind: _, unit: _ }
        | ExprKind::Checked { expr, kind: _ } => sub(expr),
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
        ExprKind::Matches { subject, kind: _, pattern: _, binding: _ } => sub(subject),
        ExprKind::Decode { record: _, bytes } => sub(bytes),
        ExprKind::Format(m) => format_exprs(m, &mut sub),
        ExprKind::Published { machine, var: _ }
        | ExprKind::StateOf(machine)
        | ExprKind::Signal { machine, signal: _ } => {
            if let Some(i) = &machine.index {
                sub(i);
            }
        }
        ExprKind::Bool(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Duration(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Default
        | ExprKind::Var(_)
        | ExprKind::Param(_)
        | ExprKind::Command(_)
        | ExprKind::Input { .. }
        | ExprKind::Output(_)
        | ExprKind::Builtin(_)
        | ExprKind::Armed(_)
        | ExprKind::PortRead(_)
        | ExprKind::JobState { .. }
        | ExprKind::Stream(_) => {}
    }
}

/// Liest die Maschine `last_fault` (5.3)? Nur dann fuehrt der erzeugte Code
/// Art, Zeile, Tick und Nachricht ihrer Faults.
pub fn reads_last_fault(m: &Machine) -> bool {
    let mut found = false;
    for_each_expr_machine(m, &mut |e| found |= matches!(e.kind, ExprKind::Builtin(Builtin::LastFault)));
    found
}
