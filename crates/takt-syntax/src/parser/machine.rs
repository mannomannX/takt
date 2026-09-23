//! Maschinen: Rumpf, Zustaende, Uebergaenge, Handler, Sequenzen.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

/// Abschnitte eines `state_body` in der Reihenfolge der Grammatik.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Phase {
    Prelude,
    Initial,
    Enter,
    Loop,
    On,
    Sequence,
    Transition,
    Exit,
    State,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Prelude => "fault/var/instance",
            Phase::Initial => "initial",
            Phase::Enter => "enter",
            Phase::Loop => "loop",
            Phase::On => "on",
            Phase::Sequence => "sequence",
            Phase::Transition => "when/after",
            Phase::Exit => "exit",
            Phase::State => "state",
        }
    }
}

const ORDER: &str = "Reihenfolge im Zustand: fault/var/instance, initial, enter, loop, on, sequence, when/after, exit, state (2.3: state_body)";

impl<'t, 's> Parser<'t, 's> {
    /// `machine_decl`
    pub(super) fn parse_machine_decl(&mut self) -> PResult<MachineDecl> {
        let start = self.pos;
        let driver = self.eat_kw("driver");
        self.expect_kw("machine")?;
        let name = self.ident()?;
        let params = if self.at_op("(") { self.parse_param_list()? } else { Vec::new() };
        let mut follows = Vec::new();
        if self.eat_word("follows") {
            follows.push(self.ident()?);
            while self.eat_op(",") {
                follows.push(self.ident()?);
            }
        }
        let node = if self.eat_kw("node") { Some(self.ident()?) } else { None };
        let every = if self.eat_kw("every") { Some(self.parse_duration_lit()?) } else { None };
        let phase = if self.eat_word("phase") { Some(self.parse_duration_lit()?) } else { None };
        let attrs = self.parse_with_attrs()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let body = self.parse_machine_body()?;
        self.expect_dedent()?;
        Ok(MachineDecl { driver, name, params, follows, node, every, phase, attrs, body, span: self.span_from(start) })
    }

    /// `machine_body`
    pub(super) fn parse_machine_body(&mut self) -> PResult<MachineBody> {
        let start = self.pos;
        let mut prelude = Vec::new();
        loop {
            let item_start = self.pos;
            let result: PResult<bool> = (|| {
                if self.at_kw("var") || self.at_kw("pub") {
                    let v = self.parse_var_decl()?;
                    self.expect_newline()?;
                    prelude.push(MachinePrelude::Var(v));
                } else if self.at_kw("persist") {
                    prelude.push(MachinePrelude::Persist(self.parse_persist_decl()?));
                } else if self.at_kw("signal") {
                    prelude.push(MachinePrelude::Signal(self.parse_signal_decl()?));
                } else if self.at_kw("fault") {
                    prelude.push(MachinePrelude::Fault(self.parse_fault_clause()?));
                } else {
                    return Ok(false);
                }
                Ok(true)
            })();
            match result {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    self.report(e);
                    self.recover(item_start);
                }
            }
        }
        if !self.at_kw("initial") {
            return Err(self.error_at(
                self.tok(),
                format!("erwartet `initial ZUSTAND`, gefunden {}", self.describe(self.tok())),
                Some("jede Maschine nennt vor ihren Zustaenden den Anfangszustand (2.3: machine_body)"),
            ));
        }
        self.bump();
        let initial = self.upper()?;
        self.expect_newline()?;
        let loop_block = if self.at_word("loop") { Some(self.parse_loop_block()?) } else { None };
        let mut handlers = Vec::new();
        while self.at_word("on") {
            handlers.push(self.parse_on_handler()?);
        }
        let mut states = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            if self.at_kw("state") {
                match self.parse_state_decl() {
                    Ok(s) => states.push(s),
                    Err(e) => {
                        self.report(e);
                        self.recover(item_start);
                    }
                }
            } else {
                let e = self.error_at(
                    self.tok(),
                    format!("unerwartet {} im Maschinenrumpf", self.describe(self.tok())),
                    Some("Reihenfolge in der Maschine: var/persist/signal/fault, initial, loop, on, state (2.3: machine_body)"),
                );
                self.report(e);
                self.recover(item_start);
            }
        }
        Ok(MachineBody { prelude, initial, loop_block, handlers, states, span: self.span_from(start) })
    }

    /// `persist_decl`
    pub(super) fn parse_persist_decl(&mut self) -> PResult<PersistDecl> {
        let start = self.pos;
        self.expect_kw("persist")?;
        self.expect_kw("var")?;
        let name = self.ident()?;
        self.expect_op(":")?;
        let ty = self.parse_type()?;
        self.expect_op("=")?;
        let value = self.parse_const_expr()?;
        let min_interval = if self.eat_kw("with") {
            self.expect_word("min_interval")?;
            self.expect_op("=")?;
            Some(self.parse_duration_lit()?)
        } else {
            None
        };
        self.expect_newline()?;
        Ok(PersistDecl { name, ty, value, min_interval, span: self.span_from(start) })
    }

    /// `signal_decl`
    pub(super) fn parse_signal_decl(&mut self) -> PResult<Ident> {
        self.expect_kw("signal")?;
        let name = self.ident()?;
        self.expect_newline()?;
        Ok(name)
    }

    /// `fault_clause`
    pub(super) fn parse_fault_clause(&mut self) -> PResult<Ident> {
        self.expect_kw("fault")?;
        self.expect_op("->")?;
        let target = self.upper()?;
        self.expect_newline()?;
        Ok(target)
    }

    /// `state_decl`
    pub(super) fn parse_state_decl(&mut self) -> PResult<StateDecl> {
        let start = self.pos;
        self.expect_kw("state")?;
        let name = self.upper()?;
        let idle = self.eat_word("idle");
        let resume = self.eat_word("resume");
        let attrs = self.parse_with_attrs()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let body = self.parse_state_body()?;
        self.expect_dedent()?;
        Ok(StateDecl { name, idle, resume, attrs, body, span: self.span_from(start) })
    }

    /// `state_body`; die Reihenfolge der Abschnitte ist fest, Verstoesse werden
    /// gemeldet, der Abschnitt aber trotzdem gelesen.
    pub(super) fn parse_state_body(&mut self) -> PResult<StateBody> {
        let mut body = StateBody::default();
        let mut phase = Phase::Prelude;
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            let item_phase = match self.kind() {
                // Die Klauselwoerter sind kontextuell (2.2, FB-92): hier
                // stehen keine Anweisungen, also ist `on` eine Klausel.
                k if Self::is_word(k) => match self.text() {
                    "fault" | "var" | "pub" | "instance" => Phase::Prelude,
                    "initial" => Phase::Initial,
                    "enter" => Phase::Enter,
                    "loop" => Phase::Loop,
                    "on" => Phase::On,
                    "sequence" => Phase::Sequence,
                    "when" | "after" => Phase::Transition,
                    "exit" => Phase::Exit,
                    "state" => Phase::State,
                    _ => {
                        let e = self.error_at(
                            self.tok(),
                            format!("unerwartet {} im Zustand", self.describe(self.tok())),
                            Some(ORDER),
                        );
                        self.report(e);
                        self.recover(item_start);
                        continue;
                    }
                },
                _ => {
                    let e = self.error_at(
                        self.tok(),
                        format!("unerwartet {} im Zustand", self.describe(self.tok())),
                        Some(ORDER),
                    );
                    self.report(e);
                    self.recover(item_start);
                    continue;
                }
            };
            if item_phase < phase {
                let e = self.error_at(
                    self.tok(),
                    format!("`{}` muss vor `{}` stehen", item_phase.name(), phase.name()),
                    Some(ORDER),
                );
                self.report(e);
            } else {
                phase = item_phase;
            }
            let result = self.parse_state_item(&mut body, item_phase);
            if let Err(e) = result {
                self.report(e);
                self.recover(item_start);
            }
        }
        Ok(body)
    }

    fn parse_state_item(&mut self, body: &mut StateBody, phase: Phase) -> PResult<()> {
        let duplicate =
            |p: &Self, what: &str| p.error_at(p.tok(), format!("`{what}` ist in diesem Zustand doppelt"), None);
        match phase {
            Phase::Prelude => {
                if self.at_kw("fault") {
                    body.prelude.push(StatePrelude::Fault(self.parse_fault_clause()?));
                } else if self.at_kw("instance") {
                    body.prelude.push(StatePrelude::Instance(self.parse_instance_decl()?));
                } else {
                    let v = self.parse_var_decl()?;
                    self.expect_newline()?;
                    body.prelude.push(StatePrelude::Var(v));
                }
            }
            Phase::Initial => {
                if body.initial.is_some() {
                    return Err(duplicate(self, "initial"));
                }
                self.bump();
                body.initial = Some(self.upper()?);
                self.expect_newline()?;
            }
            Phase::Enter => {
                if body.enter.is_some() {
                    return Err(duplicate(self, "enter"));
                }
                body.enter = Some(self.parse_enter_block()?);
            }
            Phase::Loop => {
                if body.loop_block.is_some() {
                    return Err(duplicate(self, "loop"));
                }
                body.loop_block = Some(self.parse_loop_block()?);
            }
            Phase::On => body.handlers.push(self.parse_on_handler()?),
            Phase::Sequence => {
                if body.sequence.is_some() {
                    return Err(duplicate(self, "sequence"));
                }
                let (timeout, items) = self.parse_sequence_block()?;
                body.sequence_timeout = timeout;
                body.sequence = Some(items);
            }
            Phase::Transition => body.transitions.push(self.parse_transition()?),
            Phase::Exit => {
                if body.exit.is_some() {
                    return Err(duplicate(self, "exit"));
                }
                body.exit = Some(self.parse_exit_block()?);
            }
            Phase::State => body.states.push(self.parse_state_decl()?),
        }
        Ok(())
    }

    /// `enter_block`
    pub(super) fn parse_enter_block(&mut self) -> PResult<Block> {
        self.expect_word("enter")?;
        self.expect_op(":")?;
        self.parse_action_block()
    }

    /// `exit_block`
    pub(super) fn parse_exit_block(&mut self) -> PResult<Block> {
        self.expect_word("exit")?;
        self.expect_op(":")?;
        self.parse_action_block()
    }

    /// `loop_block`
    pub(super) fn parse_loop_block(&mut self) -> PResult<Block> {
        self.expect_word("loop")?;
        self.expect_op(":")?;
        self.parse_block()
    }

    /// `on_handler`
    pub(super) fn parse_on_handler(&mut self) -> PResult<OnHandler> {
        let start = self.pos;
        self.expect_word("on")?;
        let stream = self.ident()?;
        let pattern = if self.at_kw("matches") || self.at_kw("has") {
            let kind = if self.eat_kw("matches") {
                MatchKind::Matches
            } else {
                self.bump();
                MatchKind::Has
            };
            Some((kind, self.parse_pattern()?))
        } else {
            None
        };
        let binding = if self.eat_kw("as") { Some(self.ident()?) } else { None };
        let guard = if self.eat_word("when") { Some(self.parse_expr()?) } else { None };
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(OnHandler { stream, pattern, binding, guard, body, span: self.span_from(start) })
    }

    /// `transition`
    pub(super) fn parse_transition(&mut self) -> PResult<Transition> {
        let start = self.pos;
        let trigger = if self.eat_word("when") {
            Trigger::When(self.parse_guard()?)
        } else {
            self.expect_word("after")?;
            Trigger::After(self.parse_duration_expr()?)
        };
        self.expect_op(":")?;
        let (actions, target) = self.parse_trans_block()?;
        Ok(Transition { trigger, actions, target, span: self.span_from(start) })
    }

    /// `guard`: ein Ausdruck (auch `x matches P as m`) oder `s as e` fuer das
    /// naechste Element eines Streams.
    pub(super) fn parse_guard(&mut self) -> PResult<Guard> {
        let expr = self.parse_expr()?;
        if self.at_kw("as") {
            self.bump();
            let binding = self.ident()?;
            return Ok(Guard::Next { subject: expr, binding });
        }
        Ok(Guard::Expr(expr))
    }

    /// `trans_block := goto_stmt NEWLINE | NEWLINE INDENT { stmt } goto_stmt NEWLINE DEDENT`
    fn parse_trans_block(&mut self) -> PResult<(Vec<Stmt>, Ident)> {
        if self.at_op("->") {
            let target = self.parse_goto_stmt()?;
            self.expect_newline()?;
            return Ok((Vec::new(), target));
        }
        self.expect_newline()?;
        self.expect_indent()?;
        let mut stmts = Vec::new();
        let mut last_start = self.pos;
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            last_start = self.pos;
            match self.parse_stmt() {
                Ok(s) => stmts.push(s),
                Err(e) => {
                    self.report(e);
                    self.recover(last_start);
                }
            }
        }
        self.expect_dedent()?;
        match stmts.pop() {
            Some(Stmt { kind: StmtKind::Goto(target), .. }) => Ok((stmts, target)),
            _ => {
                let t = &self.toks.tokens[last_start.min(self.toks.tokens.len() - 1)];
                Err(self.error_at(
                    t,
                    "Transitionsblock endet nicht mit `-> ZIEL`",
                    Some("die letzte Zeile eines when-/after-Blocks ist der Uebergang (2.3: trans_block)"),
                ))
            }
        }
    }

    /// `sequence_block`
    pub(super) fn parse_sequence_block(&mut self) -> PResult<(Option<Timeout>, Vec<SeqItem>)> {
        self.expect_kw("sequence")?;
        // Segment-Default (6.2): `with timeout = d [-> X]`.
        let timeout = if self.eat_kw("with") {
            if !self.eat_word("timeout") {
                return Err(self.error_here("`timeout`"));
            }
            self.expect_op("=")?;
            let duration = self.parse_duration_expr()?;
            let action =
                if self.at_op("->") { TimeoutAction::Goto(self.parse_goto_stmt()?) } else { TimeoutAction::Fault };
            Some(Timeout { duration, action })
        } else {
            None
        };
        self.expect_op(":")?;
        Ok((timeout, self.parse_seq_items()?))
    }

    /// `NEWLINE INDENT { seq_item } DEDENT`
    fn parse_seq_items(&mut self) -> PResult<Vec<SeqItem>> {
        self.expect_newline()?;
        self.expect_indent()?;
        let mut items = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let item_start = self.pos;
            match self.parse_seq_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    self.report(e);
                    self.recover(item_start);
                }
            }
        }
        self.expect_dedent()?;
        Ok(items)
    }

    /// `seq_item`
    pub(super) fn parse_seq_item(&mut self) -> PResult<SeqItem> {
        let start = self.pos;
        if self.kind() != TokenKind::Keyword {
            return Ok(SeqItem::Stmt(self.parse_stmt()?));
        }
        match self.text() {
            "wait" => {
                self.bump();
                let d = self.parse_duration_expr()?;
                self.expect_newline()?;
                Ok(SeqItem::Wait(d))
            }
            "until" => {
                self.bump();
                let guard = self.parse_guard()?;
                let timeout = if self.eat_word("timeout") {
                    let duration = self.parse_duration_expr()?;
                    let action = if self.at_op("->") {
                        let target = self.parse_goto_stmt()?;
                        self.expect_newline()?;
                        TimeoutAction::Goto(target)
                    } else if self.eat_kw("else") {
                        self.expect_op(":")?;
                        TimeoutAction::Else(self.parse_action_block()?)
                    } else {
                        self.expect_newline()?;
                        TimeoutAction::Fault
                    };
                    Some(Timeout { duration, action })
                } else {
                    self.expect_newline()?;
                    None
                };
                Ok(SeqItem::Until { guard, timeout, span: self.span_from(start) })
            }
            "expect" => {
                self.bump();
                let cond = self.parse_expr()?;
                let message = if self.eat_op(",") { Some(self.string()?) } else { None };
                let req = if self.eat_word("req") { Some(self.string()?) } else { None };
                self.expect_newline()?;
                Ok(SeqItem::Expect { cond, message, req, span: self.span_from(start) })
            }
            "repeat" => {
                self.bump();
                let count = self.parse_const_expr()?;
                self.expect_op(":")?;
                let body = self.parse_seq_items()?;
                Ok(SeqItem::Repeat { count, body, span: self.span_from(start) })
            }
            "step" => {
                self.bump();
                let name = self.string()?;
                self.expect_op(":")?;
                let body = self.parse_seq_items()?;
                Ok(SeqItem::Step { name, body, span: self.span_from(start) })
            }
            _ => Ok(SeqItem::Stmt(self.parse_stmt()?)),
        }
    }
}
