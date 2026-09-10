//! Anweisungen (plan/m1.md 3.7): Zuweisungen mit Range-Pruefung, `check`
//! mit Bestaetigungszeit, Beobachtungen, Kontrollfluss, Methodenaufrufe,
//! Aktionsblock-Regeln (5.5, Pruefung 8).

use takt_diag::{Span, Stage};
use takt_mir::expr::{BinaryOp, CheckedKind, Expr, ExprKind};
use takt_mir::machine::{CounterSite, FaultTarget, Target, VarDef, VarScope};
use takt_mir::stmt::*;
use takt_mir::types::Type;
use takt_mir::*;
use takt_syntax::ast;

use super::{BlockKind, Lowerer, SC3, SC8, is_literal};
use crate::checks::SC7;

/// Methoden, die ihren Empfaenger veraendern: sie sind Anweisungen, nie Teil
/// eines Ausdrucks (4.4, 5.7).
pub(crate) const MUTATING: &[&str] = &["push", "insert", "remove", "clear", "skip", "step", "reset"];
use crate::symbols::Entity;

/// Code der `every`/Handler-Regel.
pub const SC27: &str = "SC-27";
/// Code der Zeitregeln.
pub const SC14: &str = "SC-14";
/// Code der Bestaetigungszeit-Regel.
pub const SC36: &str = "SC-36";

impl Lowerer<'_> {
    /// Block von Anweisungen in einem neuen Bereich.
    pub fn block(&mut self, b: &ast::Block, kind: BlockKind) -> Block {
        self.scoped(|this| {
            let stmts = this.stmts(&b.stmts, kind);
            Block { stmts, span: b.span }
        })
    }

    /// Anweisungen; ein Fehler ueberspringt genau die betroffene Anweisung.
    pub fn stmts(&mut self, stmts: &[ast::Stmt], kind: BlockKind) -> Vec<Stmt> {
        let mut out = Vec::new();
        for s in stmts {
            // `pulse o = v for d` ist Zucker fuer zwei Anweisungen (7.5,
            // 6.2): setzen und die Wiederherstellung planen.
            if let ast::StmtKind::Pulse { output, value, duration } = &s.kind {
                if let Some(pair) = self.pulse(output, value, duration, s.span) {
                    out.extend(pair);
                }
                continue;
            }
            if let Some(m) = self.stmt(s, kind) {
                out.push(m);
            }
        }
        out
    }

    /// Eine Anweisung.
    pub fn stmt(&mut self, s: &ast::Stmt, kind: BlockKind) -> Option<Stmt> {
        let span = s.span;
        let forbidden = |this: &mut Self, what: &str| {
            this.error_hint(
                SC8,
                span,
                format!("`{what}` in einem Aktionsblock (5.5)"),
                "in `loop:` oder die Sequenz verschieben",
            );
            None
        };
        let mir = match &s.kind {
            ast::StmtKind::Assign { target, op, value } => return self.assign(target, *op, value, kind, span),
            ast::StmtKind::Var(decl) => self.var_stmt(decl, kind)?,
            ast::StmtKind::Job { .. } => {
                self.stage(span, "Jobs", Stage::V1_1);
                return None;
            }
            ast::StmtKind::Arm { .. } => {
                self.stage(span, "Trigger", Stage::V1_2);
                return None;
            }
            ast::StmtKind::Check { cond, message, confirm, target, req } => {
                if kind.is_action() {
                    return forbidden(self, "check");
                }
                if kind == BlockKind::Fn {
                    self.error(SC8, span, "`check` nur in Maschinen");
                    return None;
                }
                let cond = self.check_bool(cond)?;
                let message = match message {
                    Some(m) => Some(self.format(m)?),
                    None => None,
                };
                let confirm = self.confirm(confirm.as_ref(), span)?;
                let target = match target {
                    Some(t) => Some(self.target(t)?),
                    None => None,
                };
                let facts = Self::facts_of(&cond);
                if let Some(frame) = self.facts.last_mut() {
                    frame.extend(facts);
                }
                StmtKind::Check {
                    cond,
                    message,
                    confirm,
                    target,
                    req: req.as_ref().map(|r| r.value.clone()),
                    kind: CheckKind::Check,
                }
            }
            ast::StmtKind::Alert { cond, message, confirm } => {
                if kind.is_action() {
                    return forbidden(self, "alert");
                }
                let cond = self.observe_cond(cond);
                let message = self.format(message)?;
                let confirm = self.confirm(confirm.as_ref(), span)?;
                StmtKind::Observe(Observe::Alert { cond: cond?, message, confirm })
            }
            ast::StmtKind::Log(text) => StmtKind::Observe(Observe::Log(self.format(text)?)),
            ast::StmtKind::Goto(target) => {
                if matches!(kind, BlockKind::EnterExit | BlockKind::At | BlockKind::Fn) {
                    self.error(SC8, span, "`->` nur in `loop:`, Uebergaengen und Sequenzen");
                    return None;
                }
                StmtKind::Goto(self.target(target)?)
            }
            ast::StmtKind::Abort(message) => {
                if kind.is_action() {
                    return forbidden(self, "abort");
                }
                if self.mctx.is_none() {
                    self.error(SC8, span, "`abort` nur in Maschinen");
                    return None;
                }
                let message = match message {
                    Some(m) => Some(self.format(m)?),
                    None => None,
                };
                StmtKind::Abort { message }
            }
            ast::StmtKind::Return(value) => {
                let Some(ret) = self.fn_ctx.last().map(|c| c.ret) else {
                    self.error(SC8, span, "`return` nur in Funktionen");
                    return None;
                };
                let Some(ret) = ret else {
                    self.error(SC8, span, "Funktion ohne Rueckgabetyp");
                    return None;
                };
                StmtKind::Return(self.check(value, ret)?)
            }
            ast::StmtKind::Send { stream, value } => self.send(stream, value)?,
            ast::StmtKind::At(at) => self.at_stmt(at)?,
            // `pulse` wird in `stmts` zu zwei Anweisungen (7.5).
            ast::StmtKind::Pulse { .. } => {
                self.error(SC3, span, "`pulse` nur als eigenstaendige Anweisung");
                return None;
            }
            ast::StmtKind::Cancel(name) => {
                let Some(Entity::Channel(c)) = self.lookup(name) else {
                    self.error(SC3, name.span, format!("`{}` ist kein Output", name.name));
                    return None;
                };
                self.add_output_queue(c);
                StmtKind::Cancel(c)
            }
            ast::StmtKind::Measure { name, value } => {
                let v = self.observe_expr(value)?;
                StmtKind::Observe(Observe::Measure { name: name.name.clone(), value: v })
            }
            ast::StmtKind::Verify { cond, message, req } => {
                let cond = self.observe_cond(cond)?;
                let message = self.format(message)?;
                StmtKind::Observe(Observe::Verify { cond, message, req: req.as_ref().map(|r| r.value.clone()) })
            }
            ast::StmtKind::Verdict { pass, message } => {
                let message = match message {
                    Some(m) => Some(self.format(m)?),
                    None => None,
                };
                StmtKind::Observe(Observe::Verdict { pass: *pass, message })
            }
            ast::StmtKind::Raise(sig) => {
                let Some(m) = &self.mctx else {
                    self.error(SC8, span, "`raise` nur in Maschinen");
                    return None;
                };
                let Some(i) = m.machine.signals.iter().position(|s| s.name == sig.name) else {
                    self.error(SC8, sig.span, format!("kein Signal `{}`", sig.name));
                    return None;
                };
                StmtKind::Raise(SignalId(i as u32))
            }
            ast::StmtKind::Break => StmtKind::Break,
            ast::StmtKind::Pass => StmtKind::Pass,
            ast::StmtKind::Expr(e) => self.expr_stmt(e, span)?,
            ast::StmtKind::If { branches, otherwise } => self.if_stmt(branches, otherwise.as_ref(), kind)?,
            ast::StmtKind::For { target, iter, body } => self.for_stmt(target, iter, body, kind, span)?,
            ast::StmtKind::Match { subject, cases } => self.match_stmt(subject, cases, kind, span)?,
            ast::StmtKind::Every { period, body } => {
                if kind != BlockKind::Loop {
                    self.error(SC27, span, "`every` nur in `loop:`");
                    return None;
                }
                let dur = self.tys.duration;
                let period = self.check(period, dur)?;
                self.check_period_multiple(&period, span);
                let counter = self.new_counter(span, true);
                let body = self.block(body, kind);
                StmtKind::Every { period, counter, body }
            }
        };
        Some(Stmt::new(mir, span))
    }

    /// Bedingung einer Beobachtung: Channel-Lesen ohne Dominanz bleibt
    /// erlaubt, weil ungueltige Werte nie faulten (3.5).
    fn observe_cond(&mut self, cond: &ast::Expr) -> Option<Expr> {
        self.check_bool(cond)
    }

    fn observe_expr(&mut self, e: &ast::Expr) -> Option<Expr> {
        self.expr(e, None)
    }

    /// `for d` einer Bestaetigungszeit (5.6): Zaehlerstelle anlegen.
    fn confirm(&mut self, d: Option<&ast::Expr>, span: Span) -> Option<Option<Confirm>> {
        let Some(d) = d else { return Some(None) };
        if self.mctx.is_none() {
            self.error(SC8, span, "`for d` nur in Maschinen");
            return None;
        }
        let dur = self.tys.duration;
        let duration = self.check(d, dur)?;
        if let ExprKind::Duration(ns) = duration.kind {
            let period = self.machine_period_ns();
            if ns < period {
                self.warn(
                    SC36,
                    d.span,
                    format!("Bestaetigungszeit kuerzer als die Periode ({})", takt_mir::dump::duration(period)),
                );
            } else if ns % period != 0 {
                // Saettigend: das Produkt aus Quotient und Periode laeuft fuer
                // Dauern nahe `i64::MAX` ueber und brach den Compiler ab.
                let eff = div_ceil(ns, period).saturating_mul(period);
                self.warn(
                    SC36,
                    d.span,
                    format!("Bestaetigungszeit wird auf {} gerundet", takt_mir::dump::duration(eff)),
                );
            }
        }
        let site = self.new_site(span);
        Some(Some(Confirm { duration, site }))
    }

    /// Periode der aktuellen Maschine in Nanosekunden.
    pub fn machine_period_ns(&self) -> i64 {
        let period = self.mctx.as_ref().map_or(1, |m| m.machine.period);
        i64::from(period) * self.program.config.tick
    }

    /// Warnung 14: konstante Dauer, die kein Vielfaches der Periode ist.
    pub fn check_period_multiple(&mut self, d: &Expr, span: Span) {
        if let ExprKind::Duration(ns) = d.kind {
            let period = self.machine_period_ns();
            if period > 0 && ns % period != 0 {
                // Saettigend: das Produkt aus Quotient und Periode laeuft fuer
                // Dauern nahe `i64::MAX` ueber und brach den Compiler ab.
                let eff = div_ceil(ns, period).saturating_mul(period);
                self.warn(
                    SC14,
                    span,
                    format!(
                        "{} ist kein Vielfaches der Periode; wirkt als {}",
                        takt_mir::dump::duration(ns),
                        takt_mir::dump::duration(eff)
                    ),
                );
            }
        }
    }

    fn new_site(&mut self, span: Span) -> SiteId {
        let m = self.mctx.as_mut().expect("Maschine");
        let state = m.current;
        m.machine.layout.viol_sites.push(CounterSite { state, span });
        SiteId(m.machine.layout.viol_sites.len() as u32 - 1)
    }

    fn new_counter(&mut self, span: Span, _every: bool) -> CounterId {
        let m = self.mctx.as_mut().expect("Maschine");
        let state = m.current;
        m.machine.layout.every_counters.push(CounterSite { state, span });
        CounterId(m.machine.layout.every_counters.len() as u32 - 1)
    }

    /// Ziel eines Uebergangs oder `->`.
    pub fn target(&mut self, name: &ast::Ident) -> Option<Target> {
        if name.name == "FAULTED" {
            return Some(Target::Faulted);
        }
        let Some(m) = &self.mctx else {
            self.error(SC8, name.span, "Uebergang ausserhalb einer Maschine");
            return None;
        };
        match m.states.get(&name.name) {
            Some(s) => Some(Target::State(*s)),
            None => {
                let names: Vec<&str> = m.states.keys().map(String::as_str).collect();
                let hint =
                    crate::symbols::suggestion(&name.name, names.iter().copied()).map(|s| format!("meinst du `{s}`?"));
                let mut d =
                    takt_diag::Diagnostic::error(SC8, name.span, format!("unbekannter Zustand `{}`", name.name));
                if let Some(h) = hint {
                    d = d.with_suggestion(h);
                }
                self.diags.push(d);
                None
            }
        }
    }

    /// Fault-Ziel `fault -> X`.
    pub fn fault_target(&mut self, name: &ast::Ident) -> Option<FaultTarget> {
        match self.target(name)? {
            Target::State(s) => Some(FaultTarget::State(s)),
            Target::Faulted => Some(FaultTarget::Faulted),
            Target::Fault(_) => None,
        }
    }

    // ------------------------------------------------------------ Zuweisung

    fn assign(
        &mut self,
        target: &ast::Expr,
        op: ast::AssignOp,
        value: &ast::Expr,
        kind: BlockKind,
        span: Span,
    ) -> Option<Stmt> {
        let place = self.place(target)?;
        let ty = self.place_type(&place, target.span)?;
        if kind == BlockKind::At && !matches!(place, Place::Output(_)) {
            self.error(SC8, span, "`at`-Bloecke enthalten nur Zuweisungen an Outputs (5.5)");
            return None;
        }
        if kind == BlockKind::Fn && matches!(place, Place::Output(_)) {
            self.error(SC8, span, "Funktionen schreiben keine Outputs (4.4)");
            return None;
        }
        // `x = inst.step(...)` ist ein Statement mit Ziel (5.7, 14.7); eine
        // mutierende Methode steht nie in einem Ausdruck (4.4).
        if op == ast::AssignOp::Set {
            if let ast::ExprKind::Member { base, name, args: Some(args) } = &value.kind {
                if MUTATING.contains(&name.name.as_str()) {
                    let kind = self.method_call(Some(place), base, name, args, span)?;
                    return Some(Stmt::new(kind, span));
                }
            }
        }
        let rhs = match op {
            ast::AssignOp::Set => self.check(value, ty)?,
            _ => {
                let bop = match op {
                    ast::AssignOp::Add => BinaryOp::Add,
                    ast::AssignOp::Sub => BinaryOp::Sub,
                    ast::AssignOp::Mul => BinaryOp::Mul,
                    _ => BinaryOp::Div,
                };
                let lhs = self.place_expr(&place, ty, target.span);
                let hint =
                    if matches!(bop, BinaryOp::Mul | BinaryOp::Div) { self.without_unit(ty) } else { self.base(ty) };
                let r = self.expr(value, Some(hint))?;
                let r = self.coerce(r, hint)?;
                let result = self.base(ty);
                Expr::new(ExprKind::Binary { op: bop, lhs: Box::new(lhs), rhs: Box::new(r) }, result, span)
            }
        };
        let value = self.range_checked(rhs, ty, span);
        Some(Stmt::new(StmtKind::Assign { target: place, value }, span))
    }

    /// `Checked{Range}`, wenn das Ziel eine Range hat und der Wert kein
    /// Literal innerhalb ist (3.4; M3 entfernt beweisbare Pruefungen).
    pub fn range_checked(&mut self, value: Expr, target_ty: TypeId, span: Span) -> Expr {
        let Some(range) = self.range_of(target_ty) else { return value };
        if value.ty == target_ty || (is_literal(&value) && literal_in_range(&value, &range)) {
            return value;
        }
        let ty = target_ty;
        Expr::new(ExprKind::Checked { expr: Box::new(value), kind: CheckedKind::Range(range) }, ty, span)
    }

    /// Zuweisungsziel aus einem `lvalue`.
    pub fn place(&mut self, e: &ast::Expr) -> Option<Place> {
        match &e.kind {
            ast::ExprKind::Ident(name) => match self.lookup(name)? {
                Entity::Var(v, _) => Some(Place::Var(v)),
                Entity::Channel(c) => {
                    let ch = &self.program.channels[c.index()];
                    if ch.dir == takt_mir::program::Direction::Input {
                        // Die Channel-Richtung gehoert zu Pruefung 7 (10).
                        self.error_hint(
                            SC7,
                            e.span,
                            format!("Input `{}` ist nicht beschreibbar", name.name),
                            "Inputs kommen von der Hardware oder einem `sim`-Output",
                        );
                        return None;
                    }
                    if self.fn_ctx.last().is_some() {
                        self.error(SC8, e.span, "Outputs nur in Maschinen");
                        return None;
                    }
                    Some(Place::Output(c))
                }
                Entity::Param(..) => {
                    self.error(SC3, e.span, format!("Parameter `{}` ist nicht beschreibbar (8.4)", name.name));
                    None
                }
                Entity::Const(_) => {
                    self.error(SC3, e.span, format!("Konstante `{}` ist nicht beschreibbar", name.name));
                    None
                }
                _ => {
                    self.error(SC3, e.span, format!("`{}` ist kein Zuweisungsziel", name.name));
                    None
                }
            },
            ast::ExprKind::Index { base, index } => {
                let b = self.place(base)?;
                let bty = self.place_type(&b, base.span)?;
                let int = self.tys.int;
                let i = self.check(index, int)?;
                if !matches!(self.ty(bty), Type::Array { .. } | Type::Vec { .. } | Type::Bytes { .. }) {
                    let n = self.type_name(bty);
                    self.error(SC3, e.span, format!("Index auf `{n}`"));
                    return None;
                }
                Some(Place::Index(Box::new(b), i))
            }
            ast::ExprKind::Member { base, name, args: None } => {
                let b = self.place(base)?;
                let bty = self.place_type(&b, base.span)?;
                let Type::Record(r) = self.ty(bty).clone() else {
                    let n = self.type_name(bty);
                    self.error(SC3, e.span, format!("Feld `{}` auf `{n}`", name.name));
                    return None;
                };
                let def = &self.program.records[r.index()];
                match def.fields.iter().position(|f| f.name == name.name) {
                    Some(i) => Some(Place::Field(Box::new(b), i as u32)),
                    None => {
                        self.error(SC3, name.span, format!("`{}` hat kein Feld `{}`", def.name, name.name));
                        None
                    }
                }
            }
            ast::ExprKind::Index2 { .. } => {
                self.stage(e.span, "Matrizen", Stage::V1_1);
                None
            }
            _ => {
                self.error(SC3, e.span, "kein Zuweisungsziel");
                None
            }
        }
    }

    /// Deklarierter Typ einer Stelle.
    pub fn place_type(&mut self, p: &Place, span: Span) -> Option<TypeId> {
        match p {
            Place::Var(v) => self.var_type(*v).or_else(|| {
                self.error(SC3, span, "Variable ohne Typ");
                None
            }),
            Place::Output(c) => Some(self.program.channels[c.index()].ty),
            Place::Field(b, f) => {
                let bt = self.place_type(b, span)?;
                match self.ty(bt) {
                    Type::Record(r) => Some(self.program.records[r.index()].fields[*f as usize].ty),
                    _ => None,
                }
            }
            Place::Index(b, _) => {
                let bt = self.place_type(b, span)?;
                match self.ty(bt).clone() {
                    Type::Array { elem, .. } | Type::Vec { elem, .. } => Some(elem),
                    Type::Bytes { .. } => {
                        Some(self.intern(Type::Int { width: takt_mir::types::IntWidth::U8, unit: None, range: None }))
                    }
                    _ => None,
                }
            }
            Place::Index2(b, _, _) => self.place_type(b, span),
        }
    }

    /// Stelle als Ausdruck (fuer `+=`).
    fn place_expr(&mut self, p: &Place, ty: TypeId, span: Span) -> Expr {
        match p {
            Place::Var(v) => Expr::new(ExprKind::Var(*v), ty, span),
            Place::Output(c) => Expr::new(ExprKind::Output(*c), ty, span),
            Place::Field(b, f) => {
                let bt = self.place_type(b, span).unwrap_or(ty);
                let base = self.place_expr(b, bt, span);
                Expr::new(ExprKind::Field { base: Box::new(base), field: *f }, ty, span)
            }
            Place::Index(b, i) => {
                let bt = self.place_type(b, span).unwrap_or(ty);
                let base = self.place_expr(b, bt, span);
                Expr::new(ExprKind::Index { base: Box::new(base), index: Box::new(i.clone()) }, ty, span)
            }
            Place::Index2(b, i, j) => {
                let bt = self.place_type(b, span).unwrap_or(ty);
                let base = self.place_expr(b, bt, span);
                Expr::new(
                    ExprKind::Index2 { base: Box::new(base), row: Box::new(i.clone()), col: Box::new(j.clone()) },
                    ty,
                    span,
                )
            }
        }
    }

    // ------------------------------------------------------------ var

    /// `var x [: T] = e`: Variable im aktuellen Bereich plus Zuweisung.
    fn var_stmt(&mut self, decl: &ast::VarDecl, kind: BlockKind) -> Option<StmtKind> {
        if kind.is_action() && kind != BlockKind::Else {
            self.error(SC8, decl.span, "`var` in einem Aktionsblock (5.5)");
            return None;
        }
        let (ty, value) = self.var_init(decl)?;
        let scope = self.var_scope(kind);
        let id = self.new_var(VarDef {
            name: decl.name.name.clone(),
            ty,
            init: None,
            scope,
            public: decl.public,
            span: decl.span,
        });
        if !self.declare(&decl.name, Entity::Var(id, ty)) {
            return None;
        }
        let value = self.range_checked(value, ty, decl.span);
        Some(StmtKind::Assign { target: Place::Var(id), value })
    }

    /// Typ und Initialwert einer Variablendeklaration.
    pub fn var_init(&mut self, decl: &ast::VarDecl) -> Option<(TypeId, Expr)> {
        match &decl.ty {
            Some(t) => {
                let ty = self.resolve_type(t)?;
                let value = self.check(&decl.value, ty)?;
                Some((ty, value))
            }
            None => {
                let value = self.expr(&decl.value, None)?;
                if matches!(value.kind, ExprKind::None) {
                    self.error_hint(SC3, decl.span, "Typ von `none` nicht ableitbar", "`var x : T? = none`");
                    return None;
                }
                let ty = self.base(value.ty);
                Some((ty, value))
            }
        }
    }

    /// Sichtbereich einer neuen Variablen nach Kontext.
    fn var_scope(&self, kind: BlockKind) -> VarScope {
        if self.fn_ctx.last().is_some() {
            return VarScope::Local;
        }
        match self.mctx.as_ref() {
            Some(m) => match (kind, m.lift_to, m.current) {
                (BlockKind::Sequence, Some(s), _) => VarScope::Lifted(s),
                (_, _, Some(s)) => VarScope::State(s),
                _ => VarScope::Machine,
            },
            None => VarScope::Local,
        }
    }

    // ------------------------------------------------------------ Ausdrucksanweisung

    /// `f.step(x)`, `v.push(x)` ohne Ziel; reine Ausdruecke sind Fehler.
    fn expr_stmt(&mut self, e: &ast::Expr, span: Span) -> Option<StmtKind> {
        let ast::ExprKind::Member { base, name, args: Some(args) } = &e.kind else {
            self.error_hint(SC3, span, "Ausdruck ohne Wirkung", "Ergebnis zuweisen oder Anweisung entfernen");
            return None;
        };
        self.method_call(None, base, name, args, span)
    }

    /// Methodenaufruf `target = receiver.method(args)`.
    pub fn method_call(
        &mut self,
        target: Option<Place>,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<StmtKind> {
        // 8.6: `s.skip()` untersucht das ganze Fenster. Ein Strom ist keine
        // Stelle, darum vor `place` abgefangen.
        if name.name == "skip" {
            if let ast::ExprKind::Ident(id) = &base.kind {
                if self.is_stream(id) {
                    if target.is_some() {
                        self.error(SC3, span, "`skip` liefert keinen Wert");
                        return None;
                    }
                    self.method_args(args, &[], span)?;
                    let (stream, _) = self.stream_ref(id)?;
                    return Some(StmtKind::Skip(stream));
                }
            }
        }
        let receiver = self.place(base)?;
        let rty = self.place_type(&receiver, base.span)?;
        let rtype = self.ty(rty).clone();
        let member = name.name.as_str();
        let (method, result_ty, arg_exprs): (Method, Option<TypeId>, Vec<Expr>) = match (member, &rtype) {
            ("push", Type::Vec { elem, .. }) => {
                let elem = *elem;
                let a = self.method_args(args, &[elem], span)?;
                (Method::Push, Some(self.tys.bool), a)
            }
            ("push", Type::Bytes { .. }) => {
                let u8 = self.intern(Type::Int { width: takt_mir::types::IntWidth::U8, unit: None, range: None });
                let a = self.method_args(args, &[u8], span)?;
                (Method::Push, Some(self.tys.bool), a)
            }
            ("clear", Type::Vec { .. } | Type::Bytes { .. } | Type::Map { .. }) => {
                (Method::Clear, None, self.method_args(args, &[], span)?)
            }
            ("insert" | "remove", Type::Map { .. }) => {
                self.stage(span, "`map`", Stage::V1_1);
                return None;
            }
            (m, _) => {
                let Some(block) = self.block_of_place(&receiver) else {
                    let n = self.type_name(rty);
                    self.error(SC3, span, format!("keine Methode `{m}` auf `{n}`"));
                    return None;
                };
                let def = self.program.blocks[block.index()].clone();
                match m {
                    "step" => {
                        let Some(step) = def.step else {
                            self.error(SC3, span, format!("Block `{}` hat kein `step`", def.name));
                            return None;
                        };
                        let f = &self.program.fns[step.index()];
                        let tys: Vec<TypeId> = f.params.iter().map(|p| p.ty).collect();
                        let ret = f.ret;
                        let a = self.method_args(args, &tys, span)?;
                        (Method::Step, ret, a)
                    }
                    "reset" => (Method::Reset, None, self.method_args(args, &[], span)?),
                    other => {
                        let Some(fid) = def
                            .methods
                            .iter()
                            .copied()
                            .find(|f| self.program.fns[f.index()].name.ends_with(&format!(".{other}")))
                        else {
                            self.error(SC3, span, format!("Block `{}` hat keine Methode `{other}`", def.name));
                            return None;
                        };
                        let f = &self.program.fns[fid.index()];
                        let tys: Vec<TypeId> = f.params.iter().map(|p| p.ty).collect();
                        let ret = f.ret;
                        let a = self.method_args(args, &tys, span)?;
                        (Method::Block(fid), ret, a)
                    }
                }
            }
        };
        if let Some(t) = &target {
            let want = self.place_type(t, span)?;
            let Some(got) = result_ty else {
                self.error(SC3, span, format!("`{member}` liefert keinen Wert"));
                return None;
            };
            if !self.same_base(got, want) {
                let (w, g) = (self.type_name(want), self.type_name(got));
                self.error(SC3, span, format!("erwartet `{w}`, `{member}` liefert `{g}`"));
                return None;
            }
        }
        Some(StmtKind::MethodCall { target, receiver, method, args: arg_exprs })
    }

    fn method_args(&mut self, args: &[ast::Arg], tys: &[TypeId], span: Span) -> Option<Vec<Expr>> {
        if args.len() != tys.len() || args.iter().any(|a| a.name.is_some()) {
            self.error(SC3, span, format!("{} positionale Argumente erwartet", tys.len()));
            return None;
        }
        args.iter().zip(tys).map(|(a, t)| self.check(&a.value, *t)).collect()
    }

    /// Block einer Instanzvariablen (Typ `BlockInit` merkt sich der Elaborator
    /// ueber `Layout::block_instances`).
    pub fn block_of_place(&self, p: &Place) -> Option<BlockId> {
        let var = match p {
            Place::Var(v) => *v,
            Place::Index(b, _) => match b.as_ref() {
                Place::Var(v) => *v,
                _ => return None,
            },
            _ => return None,
        };
        if let Some(m) = &self.mctx {
            return m.machine.layout.block_instances.iter().find(|b| b.var == var).map(|b| b.block);
        }
        None
    }

    // ------------------------------------------------------------ Kontrollfluss

    fn if_stmt(
        &mut self,
        branches: &[(ast::Expr, ast::Block)],
        otherwise: Option<&ast::Block>,
        kind: BlockKind,
    ) -> Option<StmtKind> {
        let (cond, then) = &branches[0];
        let c = self.check_bool(cond)?;
        let facts = Self::facts_of(&c);
        self.facts.push(facts);
        let then_block = self.block(then, kind);
        self.facts.pop();
        let otherwise_block = if branches.len() > 1 {
            {
                let rest = self.if_stmt(&branches[1..], otherwise, kind)?;
                Block { stmts: vec![Stmt::new(rest, branches[1].1.span)], span: branches[1].1.span }
            }
        } else {
            match otherwise {
                Some(b) => self.block(b, kind),
                None => Block::default(),
            }
        };
        Some(StmtKind::If { cond: c, then: then_block, otherwise: otherwise_block })
    }

    fn for_stmt(
        &mut self,
        target: &ast::ForTarget,
        iter: &ast::ForIter,
        body: &ast::Block,
        kind: BlockKind,
        span: Span,
    ) -> Option<StmtKind> {
        let scope = self.var_scope(kind);
        match iter {
            ast::ForIter::Range(n) => {
                let ast::ForTarget::One(name) = target else {
                    self.error(SC3, span, "`range(N)` hat eine Laufvariable");
                    return None;
                };
                let count = self.const_int(n)?;
                if !(0..=1 << 20).contains(&count) {
                    self.error(SC3, n.span, "Schranke von `range` ausserhalb 0..2^20");
                    return None;
                }
                let range = takt_mir::types::Range {
                    lo: takt_mir::types::Const::Int(0),
                    hi: takt_mir::types::Const::Int((count - 1).max(0)),
                    origin: takt_mir::types::RangeOrigin::Declared,
                };
                let ty =
                    self.intern(Type::Int { width: takt_mir::types::IntWidth::I64, unit: None, range: Some(range) });
                let count_e = Expr::new(ExprKind::Int(count), self.tys.int, n.span);
                let body = self.scoped(|this| {
                    let id = this.new_var(VarDef {
                        name: name.name.clone(),
                        ty,
                        init: None,
                        scope,
                        public: false,
                        span: name.span,
                    });
                    if !this.declare(name, Entity::Var(id, ty)) {
                        return None;
                    }
                    let stmts = this.stmts(&body.stmts, kind);
                    Some((id, Block { stmts, span: body.span }))
                })?;
                Some(StmtKind::ForRange { var: body.0, count: count_e, body: body.1 })
            }
            ast::ForIter::Expr(e) => {
                let it = self.expr(e, None)?;
                let (elem_tys, names): (Vec<TypeId>, Vec<&ast::Ident>) = match (self.ty(it.ty).clone(), target) {
                    (
                        Type::Array { elem, .. } | Type::Vec { elem, .. } | Type::Samples { elem, .. },
                        ast::ForTarget::One(n),
                    ) => (vec![elem], vec![n]),
                    (Type::Bytes { .. }, ast::ForTarget::One(n)) => (
                        vec![self.intern(Type::Int { width: takt_mir::types::IntWidth::U8, unit: None, range: None })],
                        vec![n],
                    ),
                    (Type::Map { key, value, .. }, ast::ForTarget::Pair(k, v)) => (vec![key, value], vec![k, v]),
                    // 8.7: `for ev in s:` laeuft ueber das Fenster; die
                    // Schleifenvariable traegt wie eine Handler-Bindung die
                    // Felder des Elements samt `.t` und `.seq`.
                    (Type::Stream(elem), ast::ForTarget::One(n)) => {
                        let type_name = match &self.mctx {
                            Some(m) => format!("{}.{}.binding", m.machine.name, n.name),
                            None => format!("{}.binding", n.name),
                        };
                        // Wer das Fenster liest, braucht einen Cursor (9.6).
                        self.cursor_for(&it, e.span)?;
                        (vec![self.binding_type(&type_name, &[], Some(elem), n.span)], vec![n])
                    }
                    _ => {
                        let n = self.type_name(it.ty);
                        self.error(SC3, e.span, format!("`for` ueber `{n}`"));
                        return None;
                    }
                };
                let result = self.scoped(|this| {
                    let mut ids = Vec::new();
                    for (name, ty) in names.iter().zip(&elem_tys) {
                        let id = this.new_var(VarDef {
                            name: name.name.clone(),
                            ty: *ty,
                            init: None,
                            scope,
                            public: false,
                            span: name.span,
                        });
                        if !this.declare(name, Entity::Var(id, *ty)) {
                            return None;
                        }
                        ids.push(id);
                    }
                    let stmts = this.stmts(&body.stmts, kind);
                    Some((ids, Block { stmts, span: body.span }))
                })?;
                let vars = match result.0.as_slice() {
                    [a] => ForVars::One(*a),
                    [a, b] => ForVars::Pair(*a, *b),
                    _ => unreachable!(),
                };
                Some(StmtKind::ForEach { vars, iter: it, body: result.1 })
            }
        }
    }

    fn match_stmt(
        &mut self,
        subject: &ast::Expr,
        cases: &[ast::Case],
        kind: BlockKind,
        span: Span,
    ) -> Option<StmtKind> {
        let s = self.expr(subject, None)?;
        let sty = self.ty(s.ty).clone();
        let scope = self.var_scope(kind);
        let mut arms = Vec::new();
        let mut covered: Vec<u32> = Vec::new();
        let mut wild = false;
        let variants: Vec<(String, Vec<TypeId>)> = match &sty {
            Type::Enum(e) => self.program.enums[e.index()]
                .variants
                .iter()
                .map(|v| (v.name.clone(), v.fields.iter().map(|f| f.ty).collect()))
                .collect(),
            Type::Result { ok, err } => {
                vec![("OK".into(), vec![*ok]), ("ERR".into(), vec![self.intern(Type::Enum(*err))])]
            }
            Type::Optional(_) | Type::Bool | Type::Int { .. } | Type::Str { .. } | Type::Duration { .. } => Vec::new(),
            _ => {
                let n = self.type_name(s.ty);
                self.error(SC3, subject.span, format!("`match` ueber `{n}`"));
                return None;
            }
        };
        for case in cases {
            let arm = match &case.pattern {
                ast::CasePattern::Wild => {
                    wild = true;
                    let body = self.block(&case.body, kind);
                    Arm { pattern: ArmPattern::Wild, body, span: case.span }
                }
                ast::CasePattern::Variant { name, fields } => {
                    let Some(v) = variants.iter().position(|(n, _)| *n == name.name) else {
                        let n = self.type_name(s.ty);
                        self.error(SC3, name.span, format!("`{n}` hat keine Variante `{}`", name.name));
                        return None;
                    };
                    let field_tys = variants[v].1.clone();
                    // Pruefung 19, zweite Klausel: Variantenfelder vollstaendig
                    // gebunden (3.7).
                    if fields.len() != field_tys.len() {
                        self.error_hint(
                            "SC-19",
                            case.span,
                            format!(
                                "Variante `{}` hat {} Felder, gebunden sind {}",
                                name.name,
                                field_tys.len(),
                                fields.len()
                            ),
                            "jedes Feld der Variante braucht einen Namen (3.7)",
                        );
                        return None;
                    }
                    covered.push(v as u32);
                    let (ids, body) = self.scoped(|this| {
                        let mut ids = Vec::new();
                        for (f, ty) in fields.iter().zip(&field_tys) {
                            let id = this.new_var(VarDef {
                                name: f.name.clone(),
                                ty: *ty,
                                init: None,
                                scope,
                                public: false,
                                span: f.span,
                            });
                            if !this.declare(f, Entity::Var(id, *ty)) {
                                return None;
                            }
                            ids.push(id);
                        }
                        let stmts = this.stmts(&case.body.stmts, kind);
                        Some((ids, Block { stmts, span: case.body.span }))
                    })?;
                    Arm { pattern: ArmPattern::Variant { variant: v as u32, fields: ids }, body, span: case.span }
                }
                ast::CasePattern::Values(values) => {
                    let mut out = Vec::new();
                    for v in values {
                        let lo = self.check(&v.from, s.ty)?;
                        let hi = match &v.to {
                            Some(t) => Some(self.check(t, s.ty)?),
                            None => None,
                        };
                        out.push(CaseValue { lo, hi });
                    }
                    let body = self.block(&case.body, kind);
                    Arm { pattern: ArmPattern::Values(out), body, span: case.span }
                }
            };
            arms.push(arm);
        }
        // Erschoepfung (Pruefung 19) und offene Enums (Pruefung 51)
        let open = matches!(&sty, Type::Enum(e) if self.program.enums[e.index()].open);
        if !wild {
            if open {
                self.error_hint(
                    "SC-51",
                    span,
                    "`match` ueber ein offenes Enum braucht `case _`",
                    "`case _: pass` anfuegen (2.5)",
                );
                return None;
            }
            if !variants.is_empty() {
                let missing: Vec<&str> = variants
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !covered.contains(&(*i as u32)))
                    .map(|(_, (n, _))| n.as_str())
                    .collect();
                if !missing.is_empty() {
                    self.error_hint(
                        "SC-19",
                        span,
                        format!("`match` nicht erschoepfend: {} fehlt", missing.join(", ")),
                        "fehlende Varianten oder `case _` anfuegen (3.7)",
                    );
                    return None;
                }
            } else {
                self.error_hint("SC-19", span, "`match` ueber Werte braucht `case _`", "`case _: pass` anfuegen");
                return None;
            }
        }
        Some(StmtKind::Match { subject: s, arms })
    }
}

/// Aufrundende Division fuer positive Nenner (stabil auf Rust 1.85). Rechnet
/// in i128, weil `a + b - 1` fuer Dauern nahe `i64::MAX` sonst ueberlaeuft und
/// den Compiler abbricht; das Ergebnis passt immer in i64, weil `b >= 1`.
pub fn div_ceil(a: i64, b: i64) -> i64 {
    if b <= 0 {
        return a;
    }
    let (a, b) = (i128::from(a), i128::from(b));
    i64::try_from((a + b - 1).div_euclid(b)).unwrap_or(i64::MAX)
}

fn literal_in_range(e: &Expr, r: &takt_mir::types::Range) -> bool {
    let v = match &e.kind {
        ExprKind::Int(i) => takt_interp::Value::Int(*i),
        ExprKind::Float(f) => takt_interp::Value::F64(*f),
        ExprKind::Duration(d) => takt_interp::Value::Duration(*d),
        _ => return false,
    };
    takt_interp::eval::in_range(&v, r)
}
