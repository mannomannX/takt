//! Anweisungen (plan/m1.md 3.7): Zuweisungen mit Range-Pruefung, `check`
//! mit Bestaetigungszeit, Beobachtungen, Kontrollfluss, Methodenaufrufe,
//! Aktionsblock-Regeln (5.5, Pruefung 8).

use takt_diag::Span;
use takt_mir::expr::{BinaryOp, CheckedKind, Expr, ExprKind, UnaryOp};
use takt_mir::fns::NativeKind;
use takt_mir::machine::{CounterSite, FaultTarget, JobSlot, Target, VarDef, VarScope};
use takt_mir::stmt::*;
use takt_mir::types::{HandleKind, Type};
use takt_mir::*;
use takt_syntax::ast;

use super::{BlockKind, Lowerer, SC2, SC3, SC8, is_literal};
use crate::checks::{SC7, SC44};

/// Methoden der eingebauten Typen, die ihren Empfaenger veraendern: sie
/// sind Anweisungen, nie Teil eines Ausdrucks (4.4, 5.7).
///
/// Fuer eine Blockinstanz reicht die Liste nicht — ihre Methoden heissen,
/// wie der Block sie nennt. Dort entscheidet `is_mutating_call` am
/// Empfaenger.
pub(crate) const MUTATING: &[&str] = &["push", "append", "insert", "remove", "clear", "skip", "step", "reset"];
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
        // Der Puffer gehoert zur umgebenden Anweisung: Ein `if`-Rumpf darf
        // den Aufruf aus seiner Bedingung nicht schlucken.
        let outer = std::mem::take(&mut self.pending);
        let was = self.in_stmt;
        for s in stmts {
            // `pulse o = v for d` ist Zucker fuer zwei Anweisungen (7.5,
            // 6.2): setzen und die Wiederherstellung planen.
            if let ast::StmtKind::Pulse { output, value, duration } = &s.kind {
                if let Some(pair) = self.pulse(output, value, duration, s.span) {
                    out.extend(pair);
                }
                continue;
            }
            self.in_stmt = true;
            let m = self.stmt(s, kind);
            out.append(&mut self.pending);
            if let Some(m) = m {
                out.push(m);
            }
        }
        self.in_stmt = was;
        self.pending = outer;
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
            ast::StmtKind::Job { handle, callee, args } => self.job_stmt(handle, callee, args, kind, span)?,
            // 7.5: `arm t` / `disarm t`; die armierende Maschine besitzt
            // den Trigger, ihr Layout traegt sein `armed`.
            ast::StmtKind::Arm { arm, trigger } => {
                let Some(Entity::Trigger(id)) = self.lookup(trigger) else {
                    self.error(SC3, trigger.span, format!("`{}` ist kein Trigger", trigger.name));
                    return None;
                };
                let Some(mc) = self.mctx.as_mut() else {
                    self.error(SC3, span, "`arm` nur in einer Maschine (7.5)");
                    return None;
                };
                let owner = mc.id;
                if !mc.machine.layout.trigger_flags.contains(&id) {
                    mc.machine.layout.trigger_flags.push(id);
                }
                // Der erste `arm` bestimmt den Besitzer; Pruefung 55
                // meldet, wenn eine zweite Maschine ihn nennt.
                let t = &mut self.program.triggers[id.index()];
                if t.owner.is_none() {
                    t.owner = Some(owner);
                }
                StmtKind::Arm { trigger: id, on: *arm }
            }
            ast::StmtKind::Check { cond, message, confirm, within, target, req } => {
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
                let within = self.within(within.as_ref(), span)?;
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
                    within,
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
            ast::StmtKind::Break => {
                if self.for_depth == 0 {
                    self.error(SC8, span, "`break` nur in `for` (4.4)");
                    return None;
                }
                StmtKind::Break
            }
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
    /// `within d` (9.4.5): die geforderte Safe-State-Latenz. Geprueft wird
    /// gegen die *Tickzahl* — `within 5 ms` bei `T0 = 1 ms` heisst „in
    /// hoechstens fuenf Ticks". Das ist heute exakt entscheidbar; die
    /// Zeitspalte des Reports bekommt ihre Belastbarkeit erst mit der
    /// Schedulability (7.2, 13.8), die Regel aendert sich dadurch nicht.
    ///
    /// Die eigentliche Pruefung steht in `checks.rs` (Pruefung 61): Sie
    /// braucht den Fault-Wald der fertigen Maschine, den das Lowering hier
    /// noch nicht hat.
    fn within(&mut self, d: Option<&ast::Expr>, span: Span) -> Option<Option<Expr>> {
        let Some(d) = d else { return Some(None) };
        if self.mctx.is_none() {
            self.error(SC8, span, "`within d` nur in Maschinen");
            return None;
        }
        let dur = self.tys.duration;
        let within = self.check(d, dur)?;
        if let ExprKind::Duration(ns) = within.kind {
            if ns <= 0 {
                self.error(SC3, d.span, "`within` verlangt eine positive Dauer");
                return None;
            }
        }
        Some(Some(within))
    }

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
        // 3.7: ein benanntes Bitfeld ist eine Sicht auf sein Traegerfeld und
        // damit keine eigene Stelle. Die Zuweisung wird zum Lesen, Einsetzen
        // und Zurueckschreiben des Traegers.
        if let Some(stmt) = self.bitfield_assign(target, op, value, kind, span) {
            return stmt;
        }
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
                if self.is_mutating_call(base, &name.name) {
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
                    // 3.7: ein Traegerfeld mit Bitfeldern nennt sie im Hinweis.
                    match self.bitfield_names_at(base) {
                        Some(names) => self.error_hint(
                            SC3,
                            name.span,
                            format!("`{n}` hat kein Bitfeld `{}`", name.name),
                            format!("Bitfelder: {}", names.join(", ")),
                        ),
                        None => {
                            self.error(SC3, e.span, format!("Feld `{}` auf `{n}`", name.name));
                        }
                    }
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
            ast::ExprKind::Index2 { base, row, col } => {
                let b = self.place(base)?;
                let bty = self.place_type(&b, base.span)?;
                let Type::Mat { rows, cols, .. } = self.ty(bty).clone() else {
                    let n = self.type_name(bty);
                    self.error(SC3, e.span, format!("`[i, j]` auf `{n}`"));
                    return None;
                };
                let i = self.mat_index_expr(row, rows, "Zeile")?;
                let j = self.mat_index_expr(col, cols, "Spalte")?;
                Some(Place::Index2(Box::new(b), i, j))
            }
            _ => {
                self.error(SC3, e.span, "kein Zuweisungsziel");
                None
            }
        }
    }

    /// `rec.traeger.feld = wert` (3.7): Zuweisung an ein benanntes Bitfeld.
    /// Liefert `None`, wenn das Ziel keins ist; sonst das Ergebnis der
    /// Uebersetzung (auch ein Fehlschlag, damit der Aufrufer nicht erneut
    /// senkt).
    fn bitfield_assign(
        &mut self,
        target: &ast::Expr,
        op: ast::AssignOp,
        value: &ast::Expr,
        kind: BlockKind,
        span: Span,
    ) -> Option<Option<Stmt>> {
        let ast::ExprKind::Member { base: carrier, name, args: None } = &target.kind else { return None };
        let (lo, hi, bty) = self.bitfield_at(carrier, &name.name)?;
        Some((|| {
            if op != ast::AssignOp::Set {
                self.error(SC3, span, "Bitfelder nehmen nur `=`, kein `+=` und Verwandte");
                return None;
            }
            let place = self.place(carrier)?;
            let cty = self.place_type(&place, carrier.span)?;
            if kind == BlockKind::At && !matches!(place, Place::Output(_)) {
                self.error(SC8, span, "`at`-Bloecke enthalten nur Zuweisungen an Outputs (5.5)");
                return None;
            }
            let old = self.place_expr(&place, cty, target.span);
            let rhs = self.check(value, bty)?;
            let new = self.insert_bits(old, rhs, lo, hi, cty, span)?;
            Some(Stmt::new(StmtKind::Assign { target: place, value: new }, span))
        })())
    }

    /// Die Namen der Bitfelder eines Traegerfelds (3.7), falls es welche hat.
    fn bitfield_names_at(&mut self, carrier: &ast::Expr) -> Option<Vec<String>> {
        let ast::ExprKind::Member { base, name, args: None } = &carrier.kind else { return None };
        let b = self.place(base)?;
        let bty = self.place_type(&b, base.span)?;
        let Type::Record(r) = self.ty(bty).clone() else { return None };
        let def = self.program.records[r.index()].fields.iter().find(|f| f.name == name.name)?;
        if def.bits.is_empty() {
            return None;
        }
        Some(def.bits.iter().map(|x| x.name.clone()).collect())
    }

    /// Positionen und Typ eines Bitfelds, wenn `carrier` das Traegerfeld
    /// eines Records ist.
    fn bitfield_at(&mut self, carrier: &ast::Expr, name: &str) -> Option<(u8, u8, TypeId)> {
        let ast::ExprKind::Member { base, name: field, args: None } = &carrier.kind else { return None };
        let b = self.place(base).or_else(|| {
            self.diags.pop();
            None
        })?;
        let bty = self.place_type(&b, base.span)?;
        let Type::Record(r) = self.ty(bty).clone() else { return None };
        let def = self.program.records[r.index()].fields.iter().find(|f| f.name == field.name)?;
        let bits = def.bits.iter().find(|x| x.name == name)?;
        Some((bits.lo, bits.hi, bits.ty))
    }

    /// `(traeger & !maske) | ((wert << lo) & maske)` — das Einsetzen eines
    /// Bitfelds mit den vorhandenen Bitoperatoren (3.10).
    fn insert_bits(&mut self, carrier: Expr, value: Expr, lo: u8, hi: u8, cty: TypeId, span: Span) -> Option<Expr> {
        // Ein einzelnes Bit setzt `with_bit` (3.10); es nimmt den `bool`
        // direkt und braucht keine Verengung.
        if lo == hi && matches!(self.ty(value.ty), Type::Bool) {
            let int = self.tys.int;
            let pos = Expr::new(ExprKind::Int(i64::from(lo)), int, span);
            return Some(Expr::new(
                ExprKind::Accessor {
                    base: Box::new(carrier),
                    accessor: takt_mir::expr::Accessor::WithBit,
                    args: vec![pos, value],
                },
                cty,
                span,
            ));
        }
        let width = u32::from(hi - lo) + 1;
        let mask = if width >= 64 { !0u64 } else { ((1u64 << width) - 1) << lo };
        let lit = |v: i64| Expr::new(ExprKind::Int(v), cty, span);
        let bin = |op: BinaryOp, a: Expr, b: Expr, ty: TypeId| {
            Expr::new(ExprKind::Binary { op, lhs: Box::new(a), rhs: Box::new(b) }, ty, span)
        };
        // Der Wert wird auf die Traegerbreite gebracht, dann verschoben.
        let raw = Expr::new(ExprKind::Cast { expr: Box::new(value), to: cty }, cty, span);
        let shifted = if lo == 0 { raw } else { bin(BinaryOp::Shl, raw, lit(i64::from(lo)), cty) };
        let keep_mask = lit(!(mask as i64));
        let put_mask = lit(mask as i64);
        let kept = bin(BinaryOp::BitAnd, carrier, keep_mask, cty);
        let put = bin(BinaryOp::BitAnd, shifted, put_mask, cty);
        Some(bin(BinaryOp::BitOr, kept, put, cty))
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
            Place::Index2(b, i, j) => {
                let t = self.place_type(b, span)?;
                self.mat_index_type(t, i, j, span)
            }
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
        if let ast::ExprKind::Member { base, name, args: Some(args) } = &decl.value.kind {
            if self.is_mutating_call(base, &name.name) {
                return self.var_from_method(decl, kind, base, name, args);
            }
        }
        let (ty, value) = self.var_init(decl)?;
        let id = self.declare_var(decl, kind, ty)?;
        let value = self.range_checked(value, ty, decl.span);
        Some(StmtKind::Assign { target: Place::Var(id), value })
    }

    /// `job v = f(args)` (4.5, Pruefung 44): `f` ist ein `native job`, das
    /// Handle eine Variable der Maschine mit statischem Slot, und ein
    /// Handle gehoert zu genau einer Native — sonst haette `v.result`
    /// zwei Typen.
    fn job_stmt(
        &mut self,
        handle: &ast::Ident,
        callee: &ast::Ident,
        args: &[ast::Arg],
        kind: BlockKind,
        span: Span,
    ) -> Option<StmtKind> {
        let native = match self.peek(&callee.name).cloned() {
            Some(Entity::Native(n)) => n,
            Some(_) => {
                self.error_hint(
                    SC44,
                    callee.span,
                    format!("`{}` ist kein `native job` (4.5)", callee.name),
                    "`native job f(...) -> T with cost = ..., stack = ..., duration = ..., total` deklarieren",
                );
                return None;
            }
            None => {
                self.lookup(callee);
                return None;
            }
        };
        if self.program.natives[native.index()].kind != NativeKind::Job {
            self.error_hint(
                SC44,
                callee.span,
                format!("`{}` ist ein `native fn`, kein `native job` (4.5)", callee.name),
                "als `native job` mit `duration` deklarieren oder direkt aufrufen",
            );
            return None;
        }
        if self.mctx.is_none() || kind == BlockKind::Fn {
            self.error(SC44, span, "`job` nur in einer Maschine (4.5)");
            return None;
        }
        let params: Vec<(String, TypeId, Option<Expr>)> = self.program.natives[native.index()]
            .params
            .iter()
            .map(|p| (p.name.clone(), p.ty, p.default.clone()))
            .collect();
        let args = self.args(&params, args, span)?;
        let ty = self.intern(Type::Handle(HandleKind::Job));
        let id = match self.peek(&handle.name).cloned() {
            Some(Entity::Var(id, t)) if t == ty => id,
            Some(_) => {
                self.error(SC2, handle.span, format!("`{}` ist schon definiert", handle.name));
                return None;
            }
            None => {
                let scope = self.var_scope(kind);
                let id = self.new_var(VarDef {
                    name: handle.name.clone(),
                    ty,
                    init: None,
                    scope,
                    public: false,
                    span: handle.span,
                });
                if !self.declare(handle, Entity::Var(id, ty)) {
                    return None;
                }
                id
            }
        };
        let m = self.mctx.as_mut().expect("Maschine");
        let slots = &mut m.machine.layout.job_slots;
        match slots.iter().find(|s| s.handle == id) {
            Some(s) if s.native != native => {
                let other = self.program.natives[s.native.index()].name.clone();
                self.error_hint(
                    SC44,
                    handle.span,
                    format!("`{}` traegt schon `{other}`", handle.name),
                    "ein Handle gehoert zu einer Native; fuer eine zweite ein zweites Handle",
                );
                return None;
            }
            Some(_) => {}
            None => {
                slots.push(JobSlot { handle: id, native });
                let (n, max) = (slots.len() as u32, m.machine.layout.jobs_max);
                if n > max {
                    let name = m.machine.name.clone();
                    self.error_hint(
                        SC44,
                        span,
                        format!("`{name}` hat mehr als K_j = {max} Job-Handles (4.5)"),
                        "ein Handle je gleichzeitigem Job; ein `job` auf ein bestehendes Handle ersetzt den Lauf",
                    );
                    return None;
                }
            }
        }
        Some(StmtKind::Job { handle: id, native, args })
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

    /// `f.step(x)`, `v.push(x)` ohne Ziel, `fill(b, x)` mit `inout`; reine
    /// Ausdruecke sind Fehler.
    fn expr_stmt(&mut self, e: &ast::Expr, span: Span) -> Option<StmtKind> {
        if let ast::ExprKind::Call { args, .. } = &e.kind {
            return self.inout_call(e, args, span);
        }
        let ast::ExprKind::Member { base, name, args: Some(args) } = &e.kind else {
            self.error_hint(SC3, span, "Ausdruck ohne Wirkung", "Ergebnis zuweisen oder Anweisung entfernen");
            return None;
        };
        self.method_call(None, base, name, args, span)
    }

    /// `fill(b, x)` als Anweisung (3.9): Zucker fuer `b = fill(b, x)` — die
    /// Funktion gibt ihren `inout`-Parameter zurueck, und der geht an die
    /// Stelle des Arguments.
    fn inout_call(&mut self, e: &ast::Expr, args: &[ast::Arg], span: Span) -> Option<StmtKind> {
        let call = self.expr(e, None)?;
        let ExprKind::Call { callee, .. } = &call.kind else {
            self.error_hint(SC3, span, "Ausdruck ohne Wirkung", "Ergebnis zuweisen oder Anweisung entfernen");
            return None;
        };
        let params = &self.program.fns[callee.index()].params;
        let Some(i) = params.iter().position(|p| p.inout) else {
            self.error_hint(SC3, span, "Ausdruck ohne Wirkung", "Ergebnis zuweisen oder Anweisung entfernen");
            return None;
        };
        let name = params[i].name.clone();
        let arg = args
            .iter()
            .find(|a| a.name.as_ref().is_some_and(|n| n.name == name))
            .or_else(|| args.get(i).filter(|a| a.name.is_none()))?;
        let target = self.place(&arg.value)?;
        Some(StmtKind::Assign { target, value: call })
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
        self.method_call_typed(target, base, name, args, span).map(|(stmt, _)| stmt)
    }

    /// Wie `method_call`, liefert zusaetzlich den Ergebnistyp — `None`, wenn
    /// die Methode keinen Wert hat (`clear`, `reset`).
    fn method_call_typed(
        &mut self,
        target: Option<Place>,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
        span: Span,
    ) -> Option<(StmtKind, Option<TypeId>)> {
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
                    return Some((StmtKind::Skip(stream), None));
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
            // 3.9: `append` haengt eine ganze Folge an. Die Quelle darf eine
            // andere Kapazitaet haben als das Ziel — nur der Elementtyp muss
            // passen —, darum wird sie ohne Zieltyp gesenkt und selbst geprueft.
            ("append", Type::Vec { elem, .. }) => {
                let want = *elem;
                let what = format!("vec<{}, …>", self.type_name(want));
                let a = self.append_arg(args, span, |t| matches!(t, Type::Vec { elem, .. } if *elem == want), &what)?;
                (Method::Append, Some(self.tys.bool), a)
            }
            ("append", Type::Bytes { .. }) => {
                let a = self.append_arg(args, span, |t| matches!(t, Type::Bytes { .. }), "bytes<…>")?;
                (Method::Append, Some(self.tys.bool), a)
            }
            ("clear", Type::Vec { .. } | Type::Bytes { .. } | Type::Map { .. }) => {
                (Method::Clear, None, self.method_args(args, &[], span)?)
            }
            ("insert", Type::Map { key, value, .. }) => {
                let (key, value) = (*key, *value);
                (Method::Insert, Some(self.tys.bool), self.method_args(args, &[key, value], span)?)
            }
            ("remove", Type::Map { key, .. }) => {
                let key = *key;
                (Method::Remove, Some(self.tys.bool), self.method_args(args, &[key], span)?)
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
        Some((StmtKind::MethodCall { target, receiver, method, args: arg_exprs }, result_ty))
    }

    /// `var x = empfaenger.methode(...)` (3.9, 5.7): eine Deklaration, deren
    /// Initialisierer ein Methodenaufruf mit Ergebnis ist. Der Aufruf bleibt
    /// ein Statement — Ausdruecke sind seiteneffektfrei (4.4) —, nur ist sein
    /// Ziel die gerade deklarierte Variable.
    fn var_from_method(
        &mut self,
        decl: &ast::VarDecl,
        kind: BlockKind,
        base: &ast::Expr,
        name: &ast::Ident,
        args: &[ast::Arg],
    ) -> Option<StmtKind> {
        // Ohne Ziel gesenkt: das nennt den Ergebnistyp und meldet zugleich
        // jeden Fehler des Aufrufs.
        let (stmt, result) = self.method_call_typed(None, base, name, args, decl.span)?;
        let Some(result) = result else {
            self.error(SC3, decl.span, format!("`{}` liefert keinen Wert", name.name));
            return None;
        };
        let ty = match &decl.ty {
            Some(t) => {
                let want = self.resolve_type(t)?;
                if !self.same_base(result, want) {
                    let (w, g) = (self.type_name(want), self.type_name(result));
                    self.error(SC3, decl.span, format!("erwartet `{w}`, `{}` liefert `{g}`", name.name));
                    return None;
                }
                want
            }
            None => result,
        };
        let id = self.declare_var(decl, kind, ty)?;
        let StmtKind::MethodCall { receiver, method, args, .. } = stmt else { return None };
        Some(StmtKind::MethodCall { target: Some(Place::Var(id)), receiver, method, args })
    }

    /// Legt die Variable einer Deklaration an und macht sie sichtbar.
    fn declare_var(&mut self, decl: &ast::VarDecl, kind: BlockKind, ty: TypeId) -> Option<VarId> {
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
        Some(id)
    }

    /// Das eine Argument von `append`: genau eines, positional, und sein Typ
    /// muss `ok` erfuellen. Anders als [`Self::method_args`] gibt es keinen
    /// Zieltyp — `bytes<24>` an `bytes<264>` anzuhaengen ist der Normalfall.
    fn append_arg(
        &mut self,
        args: &[ast::Arg],
        span: Span,
        ok: impl Fn(&Type) -> bool,
        what: &str,
    ) -> Option<Vec<Expr>> {
        if args.len() != 1 || args[0].name.is_some() {
            self.error(SC3, span, "ein positionales Argument erwartet");
            return None;
        }
        let x = self.expr(&args[0].value, None)?;
        if !ok(self.ty(x.ty)) {
            let got = self.type_name(x.ty);
            self.error(SC3, args[0].value.span, format!("`append` erwartet `{what}`, gefunden `{got}`"));
            return None;
        }
        Some(vec![x])
    }

    pub(crate) fn method_args(&mut self, args: &[ast::Arg], tys: &[TypeId], span: Span) -> Option<Vec<Expr>> {
        if args.len() != tys.len() || args.iter().any(|a| a.name.is_some()) {
            self.error(SC3, span, format!("{} positionale Argumente erwartet", tys.len()));
            return None;
        }
        args.iter().zip(tys).map(|(a, t)| self.check(&a.value, *t)).collect()
    }

    /// Ist `base.name(...)` ein veraendernder Methodenaufruf?
    ///
    /// Fuer die eingebauten Typen entscheidet der Name (`MUTATING`), fuer
    /// eine Blockinstanz der Empfaenger: Jeder ihrer Methodenaufrufe
    /// veraendert sie, weil er ihren Zustand fortschreibt (5.7). Der
    /// Aufruf darf darum nie in einem groesseren Ausdruck stehen — sonst
    /// wuerde die Reihenfolge der Teilausdruecke sichtbar (4.4).
    pub fn is_mutating_call(&self, base: &ast::Expr, name: &str) -> bool {
        MUTATING.contains(&name) || self.block_of_expr(base).is_some()
    }

    /// Block einer Instanzvariablen, ueber den Ausdruck.
    ///
    /// Wie `block_of_place`, aber ohne zu senken und ohne zu melden: Die
    /// Frage stellt sich, *bevor* entschieden ist, ob der Ausdruck eine
    /// Anweisung oder ein Wert ist.
    pub fn block_of_expr(&self, e: &ast::Expr) -> Option<BlockId> {
        let name = match &e.kind {
            ast::ExprKind::Ident(n) => &n.name,
            ast::ExprKind::Index { base, .. } => match &base.kind {
                ast::ExprKind::Ident(n) => &n.name,
                _ => return None,
            },
            _ => return None,
        };
        let Some(Entity::Var(var, _)) = self.peek(name) else { return None };
        let m = self.mctx.as_ref()?;
        m.machine.layout.block_instances.iter().find(|b| b.var == *var).map(|b| b.block)
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
        // 3.5: Dominanz "durch dieselbe Flussanalyse wie Definite
        // Assignment" — verlaesst der Zweig unter `not x.valid` den Block
        // immer, gilt `x.valid` fuer den Rest wie nach `check x.valid` (3.8).
        if branches.len() == 1 && otherwise.is_none() && always_exits(&then_block.stmts) {
            if let ExprKind::Unary { op: UnaryOp::Not, expr } = &c.kind {
                let facts = Self::facts_of(expr);
                if let Some(frame) = self.facts.last_mut() {
                    frame.extend(facts);
                }
            }
        }
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
        self.for_depth += 1;
        let out = self.for_body(target, iter, body, kind, span);
        self.for_depth -= 1;
        out
    }

    fn for_body(
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

pub(super) fn literal_in_range(e: &Expr, r: &takt_mir::types::Range) -> bool {
    let v = match &e.kind {
        ExprKind::Int(i) => takt_interp::Value::Int(*i),
        ExprKind::Float(f) => takt_interp::Value::F64(*f),
        ExprKind::Duration(d) => takt_interp::Value::Duration(*d),
        _ => return false,
    };
    takt_interp::eval::in_range(&v, r)
}

/// Verlaesst jeder Pfad den Block (`return`, `break`, `abort`)? Eine
/// Transition nicht: im Entry-Tick ist `->` wirkungslos (5.2).
fn always_exits(stmts: &[Stmt]) -> bool {
    match stmts.last().map(|s| &s.kind) {
        Some(StmtKind::Return(_) | StmtKind::Break | StmtKind::Abort { .. }) => true,
        Some(StmtKind::If { then, otherwise, .. }) => always_exits(&then.stmts) && always_exits(&otherwise.stmts),
        Some(StmtKind::Match { arms, .. }) => !arms.is_empty() && arms.iter().all(|a| always_exits(&a.body.stmts)),
        _ => false,
    }
}
