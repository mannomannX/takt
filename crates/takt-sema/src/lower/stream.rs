//! Stroeme im Lowering (Referenz 8.6, 8.7, 8.8): Handler, Stream-Guards,
//! `send` und die Cursor-Registrierung.
//!
//! Jeder Stream, den eine Maschine liest, bekommt einen Eintrag in
//! `Layout::cursors`; daraus entsteht zur Laufzeit `cur[s, m]` (9.6). Die
//! Bindung eines Handlers oder Guards ist eine gehobene Variable vom
//! Recordtyp ihrer Captures (plan/m2.md 1.6, 1.7).

use takt_diag::Span;
use takt_mir::expr::StreamRef;
use takt_mir::machine::{Guard, Handler};
use takt_mir::types::Type;
use takt_mir::{ChannelId, TypeId, VarId};
use takt_syntax::ast;

use super::{BlockKind, Lowerer, SC2, SC3};
use crate::symbols::Entity;

impl Lowerer<'_> {
    /// Loest einen Stream-Namen auf und traegt ihn als Konsument ein.
    /// Liefert Bezug und Elementtyp.
    pub fn stream_ref(&mut self, name: &ast::Ident) -> Option<(StreamRef, TypeId)> {
        let entity = self.lookup(name)?;
        let (r, elem) = match entity {
            Entity::Channel(c) => {
                let ty = self.program.channels[c.index()].ty;
                let Type::Stream(elem) = self.ty(ty).clone() else {
                    let n = self.type_name(ty);
                    self.error(SC3, name.span, format!("`{}` ist kein Stream, sondern `{n}`", name.name));
                    return None;
                };
                (StreamRef::Channel(c), elem)
            }
            Entity::Stream(s) => {
                let elem = self.program.streams[s.index()].elem;
                (StreamRef::Internal(s), elem)
            }
            _ => {
                self.error(SC3, name.span, format!("`{}` ist kein Stream", name.name));
                return None;
            }
        };
        self.add_cursor(r);
        Some((r, elem))
    }

    /// Traegt einen gelesenen Stream in `Layout::cursors` ein (9.6); die
    /// Reihenfolge ist die Deklarationsreihenfolge der Handler und Guards.
    fn add_cursor(&mut self, r: StreamRef) {
        let Some(m) = self.mctx.as_mut() else { return };
        if !m.machine.layout.cursors.contains(&r) {
            m.machine.layout.cursors.push(r);
        }
        if let StreamRef::Internal(s) = r {
            let id = m.id;
            let readers = &mut self.program.streams[s.index()].readers;
            if !readers.contains(&id) {
                readers.push(id);
            }
        }
    }

    /// `on s [matches P | has P] [as b]:` (8.7).
    pub fn handler(&mut self, h: &ast::OnHandler) -> Option<Handler> {
        let (stream, elem) = self.stream_ref(&h.stream)?;
        // Muster, Bindung und Rumpf liegen im selben Sichtbereich: die
        // Bindung ist nur im Rumpf sichtbar (8.7, „im dominierten Zweig").
        self.scopes.push();
        self.facts.push(Vec::new());
        let result = self.handler_body(h, stream, elem);
        self.facts.pop();
        self.scopes.pop();
        result
    }

    fn handler_body(&mut self, h: &ast::OnHandler, stream: StreamRef, elem: TypeId) -> Option<Handler> {
        let (pattern, binding) = match &h.pattern {
            Some((kind, p)) => {
                let lowered = self.pattern(p, Some(elem), h.span)?;
                let binding = self.bind(h.binding.as_ref(), &lowered.captures, elem, h.span)?;
                (Some((match_kind(*kind), lowered.pattern)), binding)
            }
            // Catch-all: jedes Element trifft, die Bindung traegt nur die
            // Felder des Elements.
            None => (None, self.bind_none(h.binding.as_ref(), elem, h.span)?),
        };
        let body = self.block(&h.body, BlockKind::Loop);
        Some(Handler { stream, pattern, binding, body, span: h.span })
    }

    /// Bindung eines Musters mit Captures.
    fn bind(
        &mut self,
        name: Option<&ast::Ident>,
        captures: &[(String, TypeId)],
        elem: TypeId,
        span: Span,
    ) -> Option<Option<VarId>> {
        match name {
            Some(n) => self.binding_var(n, captures, Some(elem), span).map(Some),
            None => Some(None),
        }
    }

    /// Bindung ohne Muster: nur die Felder des Elements.
    fn bind_none(&mut self, name: Option<&ast::Ident>, elem: TypeId, span: Span) -> Option<Option<VarId>> {
        self.bind(name, &[], elem, span)
    }

    /// `until s [matches P] as e`, `when s as e:` (8.7). Der Guard sucht das
    /// erste passende Element im Fenster und setzt `examined` auf dessen
    /// Nummer; die Elemente danach bleiben unkonsumiert.
    pub fn stream_guard(&mut self, subject: &ast::Expr, binding: Option<&ast::Ident>) -> Option<Guard> {
        let ast::ExprKind::Ident(name) = &subject.kind else {
            self.error(SC3, subject.span, "Stream-Guard verlangt einen Stream-Namen");
            return None;
        };
        let (stream, elem) = self.stream_ref(name)?;
        let var = self.bind_none(binding, elem, subject.span)?;
        Some(Guard::Next { stream, binding: var? })
    }

    /// `s matches P as m` als Guard (8.7): dasselbe Muster wie im Handler,
    /// aber als Uebergangsbedingung.
    pub fn match_guard(
        &mut self,
        subject: &ast::Expr,
        kind: ast::MatchKind,
        pattern: &ast::Pattern,
        binding: Option<&ast::Ident>,
        span: Span,
    ) -> Option<Guard> {
        // Ein Muster ueber einem Stream sucht im Fenster, eines ueber einem
        // Wert prueft ihn (8.7).
        if let ast::ExprKind::Ident(name) = &subject.kind {
            if self.is_stream(name) {
                // Das Subjekt bleibt ein Ausdruck mit Stream-Typ; der
                // Interpreter erkennt daran, dass er ein Fenster durchsucht.
                let (_, elem) = self.stream_ref(name)?;
                let subject_expr = self.expr(subject, None)?;
                let lowered = self.pattern(pattern, Some(elem), span)?;
                let var = self.bind(binding, &lowered.captures, elem, span)?;
                return Some(Guard::Match {
                    subject: subject_expr,
                    kind: match_kind(kind),
                    pattern: lowered.pattern,
                    binding: var,
                });
            }
        }
        // Muster ueber einem gewoehnlichen Wert: der Guard ist ein Ausdruck.
        let expr = self.expr(
            &ast::Expr {
                kind: ast::ExprKind::Match {
                    subject: Box::new(subject.clone()),
                    kind,
                    pattern: pattern.clone(),
                    binding: binding.cloned(),
                },
                span,
            },
            None,
        )?;
        Some(Guard::Expr(expr))
    }

    /// Ist der Name ein Stream (Channel oder intern)?
    pub fn is_stream(&mut self, name: &ast::Ident) -> bool {
        match self.peek(&name.name) {
            Some(Entity::Channel(c)) => {
                let ty = self.program.channels[c.index()].ty;
                matches!(self.ty(ty), Type::Stream(_))
            }
            Some(Entity::Stream(_)) => true,
            _ => false,
        }
    }

    /// Ziel eines `send`: ein Ausgabestrom oder ein interner Stream (8.8).
    /// Anders als beim Lesen entsteht dabei kein Cursor.
    pub fn send_target(&mut self, name: &ast::Ident) -> Option<(StreamRef, TypeId)> {
        match self.lookup(name)? {
            Entity::Channel(c) => {
                let ch = &self.program.channels[c.index()];
                if ch.dir != takt_mir::program::Direction::Output {
                    self.error_hint(
                        SC3,
                        name.span,
                        format!("`{}` ist kein Ausgabestrom", name.name),
                        "`send` schreibt in einen `output … : stream<E>` oder einen internen Stream (8.8)",
                    );
                    return None;
                }
                let ty = ch.ty;
                let Type::Stream(elem) = self.ty(ty).clone() else {
                    let n = self.type_name(ty);
                    self.error(SC3, name.span, format!("`{}` ist kein Stream, sondern `{n}`", name.name));
                    return None;
                };
                Some((StreamRef::Channel(c), elem))
            }
            Entity::Stream(s) => {
                let elem = self.program.streams[s.index()].elem;
                // 8.6: genau ein Schreiber je internem Stream (Pruefung 43).
                if let Some(m) = self.mctx.as_ref() {
                    let id = m.id;
                    let def = &mut self.program.streams[s.index()];
                    match def.writer {
                        None => def.writer = Some(id),
                        Some(w) if w == id => {}
                        Some(_) => {
                            let n = def.name.clone();
                            self.error_hint(
                                crate::checks::SC43,
                                name.span,
                                format!("interner Stream `{n}` hat mehrere Schreiber"),
                                "genau eine Maschine sendet; Single-Writer wie bei `pub var` (8.6)",
                            );
                            return None;
                        }
                    }
                }
                Some((StreamRef::Internal(s), elem))
            }
            _ => {
                self.error(SC2, name.span, format!("`{}` ist kein Stream", name.name));
                None
            }
        }
    }

    /// Elementtyp eines Streams, ohne ihn als Konsument einzutragen.
    pub fn stream_elem_of(&mut self, name: &ast::Ident) -> Option<TypeId> {
        match self.peek(&name.name) {
            Some(Entity::Channel(c)) => {
                let ty = self.program.channels[c.index()].ty;
                match self.ty(ty) {
                    Type::Stream(e) => Some(*e),
                    _ => None,
                }
            }
            Some(Entity::Stream(s)) => Some(self.program.streams[s.index()].elem),
            _ => None,
        }
    }

    /// Kapazitaet eines Ausgabestroms in Bytes (8.8).
    pub fn tx_capacity(&self, c: ChannelId) -> u32 {
        self.program.channels[c.index()].attrs.capacity.unwrap_or(256)
    }
}

/// `matches` oder `has` (8.7).
fn match_kind(k: ast::MatchKind) -> takt_mir::expr::MatchKind {
    match k {
        ast::MatchKind::Matches => takt_mir::expr::MatchKind::Matches,
        ast::MatchKind::Has => takt_mir::expr::MatchKind::Has,
    }
}

impl Lowerer<'_> {
    /// `send tx, wert` (8.8, 8.6). Die statische Hoechstlaenge des Werts geht
    /// in den Knoten; Pruefung 20 summiert sie spaeter je Aktivierung.
    pub fn send(&mut self, name: &ast::Ident, value: &ast::Expr) -> Option<takt_mir::stmt::StmtKind> {
        let (stream, elem) = self.send_target(name)?;
        let v = self.check(value, elem)?;
        let len_max = self.max_len(&v);
        Some(takt_mir::stmt::StmtKind::Send { stream, value: v, len_max })
    }

    /// Hoechstlaenge eines Werts in Bytes (8.8): die deklarierte Kapazitaet
    /// eines Puffers, die Groesse eines Records, sonst ein Byte.
    fn max_len(&mut self, v: &takt_mir::expr::Expr) -> u32 {
        match self.ty(v.ty) {
            Type::Bytes { cap } | Type::Line { cap } | Type::Str { cap } => *cap,
            Type::Int { width, .. } => width.bits() / 8,
            Type::Record(r) => self.program.records[r.index()].wire_size.unwrap_or(1),
            _ => 1,
        }
    }

    /// `at T: b` (7.5, 9.8): `T` ist eine Dauer, der Block enthaelt nur
    /// Zuweisungen an skalare Outputs (Pruefung 21).
    pub fn at_stmt(&mut self, at: &ast::AtStmt) -> Option<takt_mir::stmt::StmtKind> {
        let duration = self.tys.duration;
        let time = self.check(&at.time, duration)?;
        let body = self.block(&at.body, BlockKind::At);
        for stmt in &body.stmts {
            match &stmt.kind {
                takt_mir::stmt::StmtKind::Assign { target: takt_mir::stmt::Place::Output(c), .. } => {
                    self.add_output_queue(*c);
                }
                _ => {
                    self.error_hint(
                        crate::checks::SC21,
                        stmt.span,
                        "`at`-Block enthaelt mehr als Zuweisungen an skalare Outputs",
                        "nur `o = v` mit einem skalaren Output (5.5, 7.5)",
                    );
                    return None;
                }
            }
        }
        Some(takt_mir::stmt::StmtKind::At { time, body })
    }

    /// `pulse o = v for d` (7.5): Zucker fuer `o = v; at now + d: o = <Latch
    /// vor dem Statement>`. Das Desugaring erzeugt beide Anweisungen; die
    /// Wiederherstellung liest den Latch zur Planungszeit.
    pub fn pulse(
        &mut self,
        output: &ast::Ident,
        value: &ast::Expr,
        duration: &ast::Expr,
        span: Span,
    ) -> Option<Vec<takt_mir::stmt::Stmt>> {
        let Some(Entity::Channel(c)) = self.lookup(output) else {
            self.error(SC3, output.span, format!("`{}` ist kein Output", output.name));
            return None;
        };
        let ty = self.program.channels[c.index()].ty;
        let v = self.check(value, ty)?;
        let dur = self.tys.duration;
        let d = self.check(duration, dur)?;
        self.add_output_queue(c);
        // `now + d` als Planungszeitpunkt.
        let now = takt_mir::expr::Expr::new(takt_mir::expr::ExprKind::Builtin(takt_mir::expr::Builtin::Now), dur, span);
        let at = takt_mir::expr::Expr::new(
            takt_mir::expr::ExprKind::Binary {
                op: takt_mir::expr::BinaryOp::Add,
                lhs: Box::new(now),
                rhs: Box::new(d),
            },
            dur,
            span,
        );
        // Der wiederhergestellte Wert ist der Latch vor diesem Statement.
        let latch = takt_mir::expr::Expr::new(takt_mir::expr::ExprKind::Output(c), ty, span);
        let restore = takt_mir::stmt::Stmt::new(
            takt_mir::stmt::StmtKind::Assign { target: takt_mir::stmt::Place::Output(c), value: latch },
            span,
        );
        let body = takt_mir::stmt::Block { stmts: vec![restore], span };
        // Erst setzen, dann die Wiederherstellung planen.
        let set = takt_mir::stmt::Stmt::new(
            takt_mir::stmt::StmtKind::Assign { target: takt_mir::stmt::Place::Output(c), value: v },
            span,
        );
        let plan = takt_mir::stmt::Stmt::new(takt_mir::stmt::StmtKind::At { time: at, body }, span);
        Some(vec![set, plan])
    }

    /// Traegt einen Output mit geplanten Schreibvorgaengen in `Layout` ein
    /// (9.8): nur fuer sie legt das Abbild eine Warteschlange an.
    pub fn add_output_queue(&mut self, c: ChannelId) {
        if let Some(m) = self.mctx.as_mut() {
            if !m.machine.layout.output_queues.contains(&c) {
                m.machine.layout.output_queues.push(c);
            }
        }
    }
}
