//! Durchlaeufe ueber eine Maschine: jede Anweisung, jeder Ausdruck, auch
//! in Sequenzen, Meldungen und Uebergangsbedingungen. Pruefungen der Sema
//! und der Codegen fragen dieselben Stellen ab; ein zweiter Durchlauf
//! koennte eine uebersehen, die der erste kennt.

use crate::expr::{Builtin, Expr, ExprKind};
use crate::machine::*;
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
    f(&m.loop_block);
    for h in &m.handlers {
        f(&h.body);
    }
    for t in &m.faulted.transitions {
        f(&t.actions);
    }
    for s in &m.states {
        f(&s.enter);
        f(&s.exit);
        f(&s.loop_block);
        for h in &s.handlers {
            f(&h.body);
        }
        for t in &s.transitions {
            f(&t.actions);
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
            _ => {}
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

/// Die Ausdruecke einer Anweisung, mit Meldungen, ohne die ihrer Unterbloecke.
pub fn stmt_exprs(s: &Stmt, f: &mut impl FnMut(&Expr)) {
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
            Observe::Alert { cond, message, confirm, .. } => {
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
}

/// Liest die Maschine `last_fault` (5.3)? Nur dann fuehrt der erzeugte Code
/// Art, Zeile, Tick und Nachricht ihrer Faults.
pub fn reads_last_fault(m: &Machine) -> bool {
    let mut found = false;
    for_each_expr_machine(m, &mut |e| found |= matches!(e.kind, ExprKind::Builtin(Builtin::LastFault)));
    found
}
