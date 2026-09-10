//! Durchlauf ueber den Syntaxbaum mit Sichtbereichen: ruft je Anweisung,
//! Ausdruck und Muster einen Besucher und fuehrt dabei die deklarierten Typen
//! der sichtbaren Namen (Datei, Maschine, Zustand, Funktion, Block).

use std::collections::HashMap;

use takt_syntax::ast::*;

/// Deklarierter Typ eines Namens, soweit syntaktisch erkennbar.
#[derive(Clone, Debug)]
pub enum Declared {
    /// Typ aus der Deklaration (`var x : T`, `input x : T`, Parameter).
    Type(Box<Type>),
    /// Variable ohne Typangabe.
    Untyped,
}

/// Sichtbereiche, innen nach aussen.
#[derive(Default)]
pub struct Scopes {
    frames: Vec<HashMap<String, Declared>>,
}

impl Scopes {
    fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.frames.pop();
    }

    fn declare(&mut self, name: &Ident, declared: Declared) {
        if let Some(frame) = self.frames.last_mut() {
            frame.insert(name.name.clone(), declared);
        }
    }

    /// Deklaration eines Namens, innerster Bereich zuerst.
    pub fn lookup(&self, name: &str) -> Option<&Declared> {
        self.frames.iter().rev().find_map(|f| f.get(name))
    }
}

/// Besucher; jede Methode hat eine leere Vorgabe.
pub trait Visitor {
    /// Anweisung, mit den sichtbaren Namen.
    fn stmt(&mut self, _stmt: &Stmt, _scopes: &Scopes) {}
    /// Ausdruck, mit den sichtbaren Namen.
    fn expr(&mut self, _expr: &Expr, _scopes: &Scopes) {}
    /// Muster (in Guards, Handlern, `matches`).
    fn pattern(&mut self, _pattern: &Pattern) {}
    /// Sequenzschritt.
    fn seq_item(&mut self, _item: &SeqItem, _scopes: &Scopes) {}
}

/// Laeuft die ganze Datei ab.
pub fn walk_file(file: &File, v: &mut impl Visitor) {
    let mut scopes = Scopes::default();
    scopes.push();
    for item in &file.items {
        match item {
            Item::Channel(c) => scopes.declare(&c.name, Declared::Type(Box::new(c.ty.clone()))),
            Item::Param(p) => scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone()))),
            Item::Const(c) => {
                scopes.declare(&c.name, c.ty.clone().map_or(Declared::Untyped, |t| Declared::Type(Box::new(t))))
            }
            Item::Instance(_) | Item::Command(_) | Item::Stream(_) => {}
            _ => {}
        }
    }
    for item in &file.items {
        walk_item(item, v, &mut scopes);
    }
}

fn walk_item(item: &Item, v: &mut impl Visitor, scopes: &mut Scopes) {
    match item {
        Item::Fn(f) => {
            scopes.push();
            for p in &f.params {
                scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
            }
            walk_block(&f.body, v, scopes);
            scopes.pop();
        }
        Item::Block(b) => {
            scopes.push();
            for p in &b.params {
                scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
            }
            for var in &b.vars {
                declare_var(var, v, scopes);
            }
            if let Some(step) = &b.step {
                scopes.push();
                for p in &step.params {
                    scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
                }
                walk_block(&step.body, v, scopes);
                scopes.pop();
            }
            for m in &b.methods {
                scopes.push();
                for p in &m.params {
                    scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
                }
                walk_block(&m.body, v, scopes);
                scopes.pop();
            }
            scopes.pop();
        }
        Item::Machine(m) => {
            scopes.push();
            for p in &m.params {
                scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
            }
            walk_machine_body(&m.body, v, scopes);
            scopes.pop();
        }
        Item::Scenario(s) => {
            scopes.push();
            walk_machine_body(&s.body, v, scopes);
            scopes.pop();
        }
        Item::Property(p) => v.expr(&p.prop, scopes),
        Item::Trigger(t) => {
            walk_guard(&t.when, v, scopes);
            v.expr(&t.then.time, scopes);
            walk_block(&t.then.body, v, scopes);
        }
        Item::Instance(i) => walk_args(&i.args, v, scopes),
        Item::Const(c) => v.expr(&c.value, scopes),
        Item::Param(p) => v.expr(&p.value, scopes),
        Item::Channel(_) | Item::Command(_) | Item::Stream(_) | Item::Import(_) | Item::System(_) => {}
        Item::Type(_) | Item::Unitvec(_) | Item::Enum(_) | Item::Record(_) | Item::Unit(_) => {}
        Item::Port(_) | Item::Node(_) | Item::Profile(_) | Item::Native(_) | Item::Campaign(_) => {}
    }
}

fn declare_var(var: &VarDecl, v: &mut impl Visitor, scopes: &mut Scopes) {
    v.expr(&var.value, scopes);
    scopes.declare(&var.name, var.ty.clone().map_or(Declared::Untyped, |t| Declared::Type(Box::new(t))));
}

fn walk_machine_body(body: &MachineBody, v: &mut impl Visitor, scopes: &mut Scopes) {
    for p in &body.prelude {
        match p {
            MachinePrelude::Var(var) => declare_var(var, v, scopes),
            MachinePrelude::Persist(p) => {
                v.expr(&p.value, scopes);
                scopes.declare(&p.name, Declared::Type(Box::new(p.ty.clone())));
            }
            MachinePrelude::Signal(_) | MachinePrelude::Fault(_) => {}
        }
    }
    if let Some(l) = &body.loop_block {
        walk_block(l, v, scopes);
    }
    for h in &body.handlers {
        walk_handler(h, v, scopes);
    }
    for s in &body.states {
        walk_state(s, v, scopes);
    }
}

fn walk_state(state: &StateDecl, v: &mut impl Visitor, scopes: &mut Scopes) {
    scopes.push();
    let b = &state.body;
    for p in &b.prelude {
        match p {
            StatePrelude::Var(var) => declare_var(var, v, scopes),
            StatePrelude::Instance(i) => walk_args(&i.args, v, scopes),
            StatePrelude::Fault(_) => {}
        }
    }
    if let Some(e) = &b.enter {
        walk_block(e, v, scopes);
    }
    if let Some(l) = &b.loop_block {
        walk_block(l, v, scopes);
    }
    for h in &b.handlers {
        walk_handler(h, v, scopes);
    }
    if let Some(seq) = &b.sequence {
        walk_seq(seq, v, scopes);
    }
    for t in &b.transitions {
        scopes.push();
        match &t.trigger {
            Trigger::When(g) => walk_guard(g, v, scopes),
            Trigger::After(e) => v.expr(e, scopes),
        }
        for s in &t.actions {
            walk_stmt(s, v, scopes);
        }
        scopes.pop();
    }
    if let Some(e) = &b.exit {
        walk_block(e, v, scopes);
    }
    for child in &b.states {
        walk_state(child, v, scopes);
    }
    scopes.pop();
}

fn walk_handler(h: &OnHandler, v: &mut impl Visitor, scopes: &mut Scopes) {
    scopes.push();
    if let Some((_, p)) = &h.pattern {
        v.pattern(p);
    }
    if let Some(b) = &h.binding {
        scopes.declare(b, Declared::Untyped);
    }
    walk_block(&h.body, v, scopes);
    scopes.pop();
}

fn walk_guard(g: &Guard, v: &mut impl Visitor, scopes: &mut Scopes) {
    match g {
        Guard::Expr(e) => walk_expr(e, v, scopes),
        Guard::Next { subject, binding } => {
            walk_expr(subject, v, scopes);
            scopes.declare(binding, Declared::Untyped);
        }
    }
}

fn walk_seq(items: &[SeqItem], v: &mut impl Visitor, scopes: &mut Scopes) {
    for item in items {
        v.seq_item(item, scopes);
        match item {
            SeqItem::Stmt(s) => walk_stmt(s, v, scopes),
            SeqItem::Wait(e) => walk_expr(e, v, scopes),
            SeqItem::Until { guard, timeout, .. } => {
                walk_guard(guard, v, scopes);
                if let Some(t) = timeout {
                    walk_expr(&t.duration, v, scopes);
                    if let TimeoutAction::Else(b) = &t.action {
                        walk_block(b, v, scopes);
                    }
                }
            }
            SeqItem::Expect { cond, .. } => walk_expr(cond, v, scopes),
            SeqItem::Repeat { count, body, .. } => {
                walk_expr(count, v, scopes);
                walk_seq(body, v, scopes);
            }
            SeqItem::Step { body, .. } => walk_seq(body, v, scopes),
        }
    }
}

fn walk_block(b: &Block, v: &mut impl Visitor, scopes: &mut Scopes) {
    scopes.push();
    for s in &b.stmts {
        walk_stmt(s, v, scopes);
    }
    scopes.pop();
}

fn walk_stmt(s: &Stmt, v: &mut impl Visitor, scopes: &mut Scopes) {
    v.stmt(s, scopes);
    match &s.kind {
        StmtKind::Assign { target, value, .. } => {
            walk_expr(target, v, scopes);
            walk_expr(value, v, scopes);
        }
        StmtKind::Var(var) => declare_var(var, v, scopes),
        StmtKind::Job { args, .. } => walk_args(args, v, scopes),
        StmtKind::Check { cond, confirm, .. } => {
            walk_expr(cond, v, scopes);
            if let Some(c) = confirm {
                walk_expr(c, v, scopes);
            }
        }
        StmtKind::Alert { cond, confirm, .. } => {
            walk_expr(cond, v, scopes);
            if let Some(c) = confirm {
                walk_expr(c, v, scopes);
            }
        }
        StmtKind::Return(e) | StmtKind::Expr(e) => walk_expr(e, v, scopes),
        StmtKind::Send { value, .. } | StmtKind::Measure { value, .. } => walk_expr(value, v, scopes),
        StmtKind::Pulse { value, duration, .. } => {
            walk_expr(value, v, scopes);
            walk_expr(duration, v, scopes);
        }
        StmtKind::Verify { cond, .. } => walk_expr(cond, v, scopes),
        StmtKind::If { branches, otherwise } => {
            for (cond, body) in branches {
                walk_expr(cond, v, scopes);
                walk_block(body, v, scopes);
            }
            if let Some(b) = otherwise {
                walk_block(b, v, scopes);
            }
        }
        StmtKind::For { target, iter, body } => {
            match iter {
                ForIter::Range(n) => walk_expr(n, v, scopes),
                ForIter::Expr(e) => walk_expr(e, v, scopes),
            }
            scopes.push();
            match target {
                ForTarget::One(i) => scopes.declare(i, Declared::Untyped),
                ForTarget::Pair(a, b) => {
                    scopes.declare(a, Declared::Untyped);
                    scopes.declare(b, Declared::Untyped);
                }
            }
            walk_block(body, v, scopes);
            scopes.pop();
        }
        StmtKind::Match { subject, cases } => {
            walk_expr(subject, v, scopes);
            for c in cases {
                scopes.push();
                match &c.pattern {
                    CasePattern::Variant { fields, .. } => {
                        for f in fields {
                            scopes.declare(f, Declared::Untyped);
                        }
                    }
                    CasePattern::Values(values) => {
                        for val in values {
                            walk_expr(&val.from, v, scopes);
                            if let Some(to) = &val.to {
                                walk_expr(to, v, scopes);
                            }
                        }
                    }
                    CasePattern::Wild => {}
                }
                walk_block(&c.body, v, scopes);
                scopes.pop();
            }
        }
        StmtKind::At(a) => {
            walk_expr(&a.time, v, scopes);
            walk_block(&a.body, v, scopes);
        }
        StmtKind::Every { period, body } => {
            walk_expr(period, v, scopes);
            walk_block(body, v, scopes);
        }
        StmtKind::Arm { .. }
        | StmtKind::Log(_)
        | StmtKind::Goto(_)
        | StmtKind::Abort(_)
        | StmtKind::Cancel(_)
        | StmtKind::Verdict { .. }
        | StmtKind::Raise(_)
        | StmtKind::Break
        | StmtKind::Pass => {}
    }
}

fn walk_args(args: &[Arg], v: &mut impl Visitor, scopes: &mut Scopes) {
    for a in args {
        walk_expr(&a.value, v, scopes);
    }
}

fn walk_expr(e: &Expr, v: &mut impl Visitor, scopes: &mut Scopes) {
    v.expr(e, scopes);
    match &e.kind {
        ExprKind::Call { args, .. } | ExprKind::InstanceArray { args, .. } => walk_args(args, v, scopes),
        ExprKind::Upper { args, .. } | ExprKind::TypeName { args, .. } => {
            if let Some(args) = args {
                walk_args(args, v, scopes);
            }
        }
        ExprKind::Paren(inner) | ExprKind::Cast { expr: inner, .. } | ExprKind::Unary { expr: inner, .. } => {
            walk_expr(inner, v, scopes);
        }
        ExprKind::Tuple(a, b) => {
            walk_expr(a, v, scopes);
            walk_expr(b, v, scopes);
        }
        ExprKind::Array(items) => {
            for item in items {
                walk_expr(item, v, scopes);
            }
        }
        ExprKind::Member { base, args, .. } => {
            walk_expr(base, v, scopes);
            if let Some(args) = args {
                walk_args(args, v, scopes);
            }
        }
        ExprKind::Index { base, index } => {
            walk_expr(base, v, scopes);
            walk_expr(index, v, scopes);
        }
        ExprKind::Slice { base, from, to } => {
            walk_expr(base, v, scopes);
            walk_expr(from, v, scopes);
            walk_expr(to, v, scopes);
        }
        ExprKind::Index2 { base, row, col } => {
            walk_expr(base, v, scopes);
            walk_expr(row, v, scopes);
            walk_expr(col, v, scopes);
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::Implies { lhs, rhs } => {
            walk_expr(lhs, v, scopes);
            walk_expr(rhs, v, scopes);
        }
        ExprKind::Match { subject, pattern, binding, .. } => {
            walk_expr(subject, v, scopes);
            v.pattern(pattern);
            if let Some(b) = binding {
                scopes.declare(b, Declared::Untyped);
            }
        }
        ExprKind::Conditional { then, cond, otherwise } => {
            walk_expr(then, v, scopes);
            walk_expr(cond, v, scopes);
            walk_expr(otherwise, v, scopes);
        }
        ExprKind::Temporal { inner, .. } => walk_expr(inner, v, scopes),
        ExprKind::Number { .. }
        | ExprKind::Duration(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::None
        | ExprKind::Default
        | ExprKind::Ident(_) => {}
    }
}
