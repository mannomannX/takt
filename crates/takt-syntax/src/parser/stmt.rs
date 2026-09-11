//! Anweisungen und Bloecke.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

impl<'t, 's> Parser<'t, 's> {
    /// `block := NEWLINE INDENT stmt { stmt } DEDENT | simple_stmt NEWLINE`
    pub(super) fn parse_block(&mut self) -> PResult<Block> {
        self.enter()?;
        let result = self.parse_block_inner();
        self.leave();
        result
    }

    fn parse_block_inner(&mut self) -> PResult<Block> {
        let start = self.pos;
        if self.eat(TokenKind::Newline) {
            self.expect_indent()?;
            let mut stmts = Vec::new();
            while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
                let stmt_start = self.pos;
                match self.parse_stmt() {
                    Ok(s) => stmts.push(s),
                    Err(e) => {
                        self.report(e);
                        self.recover(stmt_start);
                    }
                }
            }
            self.expect_dedent()?;
            return Ok(Block { stmts, span: self.span_from(start) });
        }
        let stmt = self.parse_simple_stmt()?;
        self.expect_newline()?;
        Ok(Block { stmts: vec![stmt], span: self.span_from(start) })
    }

    /// `action_block := block` (die Einschraenkungen aus 5.5 prueft die Semantik)
    pub(super) fn parse_action_block(&mut self) -> PResult<Block> {
        self.parse_block()
    }

    /// `stmt := simple_stmt NEWLINE | if_stmt | for_stmt | match_stmt | at_stmt | every_stmt`
    pub(super) fn parse_stmt(&mut self) -> PResult<Stmt> {
        let start = self.pos;
        if self.kind() == TokenKind::Keyword {
            let kind = match self.text() {
                "if" => Some(self.parse_if_stmt()?),
                "for" => Some(self.parse_for_stmt()?),
                "match" => Some(self.parse_match_stmt()?),
                "at" => Some(StmtKind::At(self.parse_at_stmt()?)),
                "every" => Some(self.parse_every_stmt()?),
                _ => None,
            };
            if let Some(kind) = kind {
                return Ok(Stmt { kind, span: self.span_from(start) });
            }
        }
        let stmt = self.parse_simple_stmt()?;
        self.expect_newline()?;
        Ok(stmt)
    }

    /// `simple_stmt`
    pub(super) fn parse_simple_stmt(&mut self) -> PResult<Stmt> {
        let start = self.pos;
        if self.at_op("->") {
            let target = self.parse_goto_stmt()?;
            return Ok(Stmt { kind: StmtKind::Goto(target), span: self.span_from(start) });
        }
        if self.kind() == TokenKind::Keyword {
            let kind = match self.text() {
                "var" | "pub" => Some(StmtKind::Var(self.parse_var_decl()?)),
                "job" => Some(self.parse_job_stmt()?),
                "arm" | "disarm" => Some(self.parse_arm_stmt()?),
                "check" => Some(self.parse_check_stmt()?),
                "alert" => Some(self.parse_alert_stmt()?),
                "log" => Some(self.parse_log_stmt()?),
                "abort" => Some(self.parse_abort_stmt()?),
                "return" => Some(self.parse_return_stmt()?),
                "send" => Some(self.parse_send_stmt()?),
                "pulse" => Some(self.parse_pulse_stmt()?),
                "cancel" => Some(self.parse_cancel_stmt()?),
                "measure" => Some(self.parse_measure_stmt()?),
                "verify" => Some(self.parse_verify_stmt()?),
                "verdict" => Some(self.parse_verdict_stmt()?),
                "raise" => Some(self.parse_raise_stmt()?),
                "break" => {
                    self.bump();
                    Some(StmtKind::Break)
                }
                "pass" => {
                    self.bump();
                    Some(StmtKind::Pass)
                }
                _ => None,
            };
            if let Some(kind) = kind {
                return Ok(Stmt { kind, span: self.span_from(start) });
            }
        }
        let expr = self.parse_expr()?;
        let assign_op = if self.at_op("=") {
            Some(AssignOp::Set)
        } else if self.at_op("+=") {
            Some(AssignOp::Add)
        } else if self.at_op("-=") {
            Some(AssignOp::Sub)
        } else if self.at_op("*=") {
            Some(AssignOp::Mul)
        } else if self.at_op("/=") {
            Some(AssignOp::Div)
        } else {
            None
        };
        let kind = match assign_op {
            Some(op) => self.parse_assign(expr, op)?,
            None => StmtKind::Expr(expr),
        };
        Ok(Stmt { kind, span: self.span_from(start) })
    }

    /// `assign := lvalue ( "=" | "+=" | "-=" | "*=" | "/=" ) expr`; die linke
    /// Seite ist bereits als Ausdruck gelesen und wird auf ihre Form geprueft.
    fn parse_assign(&mut self, target: Expr, op: AssignOp) -> PResult<StmtKind> {
        if !self.parse_lvalue(&target) {
            return Err(self.error_at(
                self.tok(),
                "die linke Seite einer Zuweisung ist eine Variable, ein Output, ein Feld oder ein Element",
                Some("Form: name, name.feld, name[i], name[i, j] (2.3: lvalue)"),
            ));
        }
        self.bump();
        let value = self.parse_expr()?;
        Ok(StmtKind::Assign { target, op, value })
    }

    /// `var_decl := [ "pub" ] "var" IDENT [ ":" type ] "=" expr`
    pub(super) fn parse_var_decl(&mut self) -> PResult<VarDecl> {
        let start = self.pos;
        let public = self.eat_kw("pub");
        self.expect_kw("var")?;
        let name = self.ident()?;
        let ty = if self.eat_op(":") { Some(self.parse_type()?) } else { None };
        self.expect_op("=")?;
        let value = self.parse_expr()?;
        Ok(VarDecl { public, name, ty, value, span: self.span_from(start) })
    }

    /// `if_stmt`
    fn parse_if_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("if")?;
        let cond = self.parse_expr()?;
        self.expect_op(":")?;
        let mut branches = vec![(cond, self.parse_block()?)];
        let mut otherwise = None;
        loop {
            if self.at_kw("elif") {
                self.bump();
                let cond = self.parse_expr()?;
                self.expect_op(":")?;
                branches.push((cond, self.parse_block()?));
            } else if self.at_kw("else") {
                self.bump();
                self.expect_op(":")?;
                otherwise = Some(self.parse_block()?);
                break;
            } else {
                break;
            }
        }
        Ok(StmtKind::If { branches, otherwise })
    }

    /// `for_stmt`
    fn parse_for_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("for")?;
        let target = if self.eat_op("(") {
            let a = self.ident()?;
            self.expect_op(",")?;
            let b = self.ident()?;
            self.expect_op(")")?;
            ForTarget::Pair(a, b)
        } else {
            ForTarget::One(self.ident()?)
        };
        self.expect_kw("in")?;
        let iter = if self.at_kw("range") && self.at_op_at(1, "(") {
            self.bump();
            self.bump();
            let n = self.parse_const_expr()?;
            self.expect_op(")")?;
            ForIter::Range(n)
        } else {
            ForIter::Expr(self.parse_expr()?)
        };
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(StmtKind::For { target, iter, body })
    }

    /// `match_stmt`
    fn parse_match_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("match")?;
        let subject = self.parse_expr()?;
        self.expect_op(":")?;
        self.expect_newline()?;
        self.expect_indent()?;
        let mut cases = Vec::new();
        while !self.at(TokenKind::Dedent) && !self.at(TokenKind::Eof) {
            let case_start = self.pos;
            let result: PResult<Case> = (|| {
                self.expect_kw("case")?;
                let pattern = self.parse_case_pattern()?;
                self.expect_op(":")?;
                let body = self.parse_block()?;
                Ok(Case { pattern, body, span: self.span_from(case_start) })
            })();
            match result {
                Ok(c) => cases.push(c),
                Err(e) => {
                    self.report(e);
                    self.recover(case_start);
                }
            }
        }
        self.expect_dedent()?;
        Ok(StmtKind::Match { subject, cases })
    }

    /// `case_pattern`
    fn parse_case_pattern(&mut self) -> PResult<CasePattern> {
        if self.eat(TokenKind::Wild) {
            return Ok(CasePattern::Wild);
        }
        if self.at(TokenKind::UpperIdent) {
            let name = self.upper()?;
            let mut fields = Vec::new();
            if self.eat_op("(") {
                loop {
                    fields.push(self.ident()?);
                    if !self.eat_op(",") {
                        break;
                    }
                }
                self.expect_op(")")?;
            }
            return Ok(CasePattern::Variant { name, fields });
        }
        let mut values = Vec::new();
        loop {
            let from = self.parse_const_expr()?;
            let to = if self.eat_op("..") { Some(self.parse_const_expr()?) } else { None };
            values.push(CaseValue { from, to });
            if !self.eat_op(",") {
                break;
            }
        }
        Ok(CasePattern::Values(values))
    }

    /// `at_stmt := "at" duration_expr ":" action_block`
    pub(super) fn parse_at_stmt(&mut self) -> PResult<AtStmt> {
        let start = self.pos;
        self.expect_kw("at")?;
        let time = self.parse_duration_expr()?;
        self.expect_op(":")?;
        let body = self.parse_action_block()?;
        Ok(AtStmt { time, body, span: self.span_from(start) })
    }

    /// `every_stmt`
    fn parse_every_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("every")?;
        let period = self.parse_duration_expr()?;
        self.expect_op(":")?;
        let body = self.parse_block()?;
        Ok(StmtKind::Every { period, body })
    }

    /// `check_stmt`
    fn parse_check_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("check")?;
        let cond = self.parse_expr()?;
        let message = if self.eat_op(",") { Some(self.string()?) } else { None };
        let confirm = if self.eat_kw("for") { Some(self.parse_duration_expr()?) } else { None };
        let within = if self.eat_word("within") { Some(self.parse_duration_expr()?) } else { None };
        let target = if self.at_op("->") { Some(self.parse_goto_stmt()?) } else { None };
        let req = if self.eat_word("req") { Some(self.string()?) } else { None };
        Ok(StmtKind::Check { cond, message, confirm, within, target, req })
    }

    /// `alert_stmt`
    fn parse_alert_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("alert")?;
        let cond = self.parse_expr()?;
        self.expect_op(",")?;
        let message = self.string()?;
        let confirm = if self.eat_kw("for") { Some(self.parse_duration_expr()?) } else { None };
        Ok(StmtKind::Alert { cond, message, confirm })
    }

    /// `log_stmt`
    fn parse_log_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("log")?;
        Ok(StmtKind::Log(self.string()?))
    }

    /// `send_stmt`
    fn parse_send_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("send")?;
        let stream = self.ident()?;
        self.expect_op(",")?;
        let value = self.parse_expr()?;
        Ok(StmtKind::Send { stream, value })
    }

    /// `pulse_stmt`
    fn parse_pulse_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("pulse")?;
        let output = self.ident()?;
        self.expect_op("=")?;
        let value = self.parse_expr()?;
        self.expect_kw("for")?;
        let duration = self.parse_duration_expr()?;
        Ok(StmtKind::Pulse { output, value, duration })
    }

    /// `cancel_stmt`
    fn parse_cancel_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("cancel")?;
        Ok(StmtKind::Cancel(self.ident()?))
    }

    /// `measure_stmt`
    fn parse_measure_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("measure")?;
        let name = self.ident()?;
        self.expect_op("=")?;
        let value = self.parse_expr()?;
        Ok(StmtKind::Measure { name, value })
    }

    /// `job_stmt`
    fn parse_job_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("job")?;
        let handle = self.ident()?;
        self.expect_op("=")?;
        let callee = self.ident()?;
        let args = self.parse_arg_list()?;
        Ok(StmtKind::Job { handle, callee, args })
    }

    /// `arm_stmt`
    fn parse_arm_stmt(&mut self) -> PResult<StmtKind> {
        let arm = self.eat_kw("arm");
        if !arm {
            self.expect_kw("disarm")?;
        }
        Ok(StmtKind::Arm { arm, trigger: self.ident()? })
    }

    /// `verify_stmt`
    fn parse_verify_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("verify")?;
        let cond = self.parse_expr()?;
        self.expect_op(",")?;
        let message = self.string()?;
        let req = if self.eat_word("req") { Some(self.string()?) } else { None };
        Ok(StmtKind::Verify { cond, message, req })
    }

    /// `verdict_stmt`
    fn parse_verdict_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("verdict")?;
        let pass = if self.eat_kw("pass") {
            true
        } else if self.eat_word("fail") {
            false
        } else {
            return Err(self.error_here("`pass` oder `fail`"));
        };
        let message = if self.at(TokenKind::Str) { Some(self.string()?) } else { None };
        Ok(StmtKind::Verdict { pass, message })
    }

    /// `raise_stmt`
    fn parse_raise_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("raise")?;
        Ok(StmtKind::Raise(self.ident()?))
    }

    /// `goto_stmt := "->" UPPER_IDENT`
    pub(super) fn parse_goto_stmt(&mut self) -> PResult<Ident> {
        self.expect_op("->")?;
        self.upper()
    }

    /// `abort_stmt`
    fn parse_abort_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("abort")?;
        let message = if self.at(TokenKind::Str) { Some(self.string()?) } else { None };
        Ok(StmtKind::Abort(message))
    }

    /// `return_stmt`
    fn parse_return_stmt(&mut self) -> PResult<StmtKind> {
        self.expect_kw("return")?;
        Ok(StmtKind::Return(self.parse_expr()?))
    }
}
