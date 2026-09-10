//! Ausdruecke: die Vorrangkette aus `takt.ebnf` (expr bis primary) und die
//! Eigenschaftsausdruecke aus 13.3.

use super::{PResult, Parser};
use crate::ast::*;
use crate::token::TokenKind;

impl<'t, 's> Parser<'t, 's> {
    /// `expr := or_expr [ "if" or_expr "else" expr ]`
    pub(super) fn parse_expr(&mut self) -> PResult<Expr> {
        self.enter()?;
        let result = self.parse_expr_inner();
        self.leave();
        result
    }

    fn parse_expr_inner(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let then = self.parse_or_expr()?;
        if self.at_kw("if") {
            self.bump();
            let cond = self.parse_or_expr()?;
            self.expect_kw("else")?;
            let otherwise = self.parse_expr()?;
            return Ok(Expr {
                kind: ExprKind::Conditional {
                    then: Box::new(then),
                    cond: Box::new(cond),
                    otherwise: Box::new(otherwise),
                },
                span: self.span_from(start),
            });
        }
        Ok(then)
    }

    fn binary(&self, start: usize, op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr { kind: ExprKind::Binary { op, lhs: Box::new(lhs), rhs: Box::new(rhs) }, span: self.span_from(start) }
    }

    /// `or_expr := and_expr { "or" and_expr }`
    pub(super) fn parse_or_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_and_expr()?;
        while self.eat_kw("or") {
            let rhs = self.parse_and_expr()?;
            lhs = self.binary(start, BinaryOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `and_expr := not_expr { "and" not_expr }`
    fn parse_and_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_not_expr()?;
        while self.eat_kw("and") {
            let rhs = self.parse_not_expr()?;
            lhs = self.binary(start, BinaryOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `not_expr := "not" not_expr | cmp_expr`
    fn parse_not_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        if self.eat_kw("not") {
            let inner = self.parse_not_expr()?;
            return Ok(Expr {
                kind: ExprKind::Unary { op: UnaryOp::Not, expr: Box::new(inner) },
                span: self.span_from(start),
            });
        }
        self.parse_cmp_expr()
    }

    /// `cmp_expr := bitor_expr [ cmpop bitor_expr ] | bitor_expr ( "matches" | "has" ) pattern [ "as" IDENT ]`
    fn parse_cmp_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let lhs = self.parse_bitor_expr()?;
        let kind = if self.at_kw("matches") {
            Some(MatchKind::Matches)
        } else if self.at_kw("has") {
            Some(MatchKind::Has)
        } else {
            None
        };
        if let Some(kind) = kind {
            self.bump();
            let pattern = self.parse_pattern()?;
            let binding = if self.at_kw("as") && self.tok_at(1).kind == TokenKind::Ident {
                self.bump();
                Some(self.ident()?)
            } else {
                None
            };
            return Ok(Expr {
                kind: ExprKind::Match { subject: Box::new(lhs), kind, pattern, binding },
                span: self.span_from(start),
            });
        }
        let op = if self.at_op("<") {
            Some(BinaryOp::Lt)
        } else if self.at_op("<=") {
            Some(BinaryOp::Le)
        } else if self.at_op(">") && self.angle == 0 && !self.at_shift_right() {
            Some(BinaryOp::Gt)
        } else if self.at_op(">=") {
            Some(BinaryOp::Ge)
        } else if self.at_op("==") {
            Some(BinaryOp::Eq)
        } else if self.at_op("!=") {
            Some(BinaryOp::Ne)
        } else {
            None
        };
        if let Some(op) = op {
            self.bump();
            let rhs = self.parse_bitor_expr()?;
            return Ok(self.binary(start, op, lhs, rhs));
        }
        Ok(lhs)
    }

    /// `bitor_expr := bitxor_expr { "|" bitxor_expr }`
    fn parse_bitor_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_bitxor_expr()?;
        while self.eat_op("|") {
            let rhs = self.parse_bitxor_expr()?;
            lhs = self.binary(start, BinaryOp::BitOr, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `bitxor_expr := bitand_expr { "^" bitand_expr }`
    fn parse_bitxor_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_bitand_expr()?;
        while self.eat_op("^") {
            let rhs = self.parse_bitand_expr()?;
            lhs = self.binary(start, BinaryOp::BitXor, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `bitand_expr := shift_expr { "&" shift_expr }`
    fn parse_bitand_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_shift_expr()?;
        while self.eat_op("&") {
            let rhs = self.parse_shift_expr()?;
            lhs = self.binary(start, BinaryOp::BitAnd, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `shift_expr := add_expr { ( "<<" | ">>" ) add_expr }`; `>>` sind zwei anliegende `>`.
    fn parse_shift_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_add_expr()?;
        loop {
            let op = if self.at_op("<<") {
                self.bump();
                BinaryOp::Shl
            } else if self.angle == 0 && self.at_shift_right() {
                self.bump();
                self.bump();
                BinaryOp::Shr
            } else {
                break;
            };
            let rhs = self.parse_add_expr()?;
            lhs = self.binary(start, op, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `add_expr := mul_expr { ( "+" | "-" ) mul_expr }`
    fn parse_add_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_mul_expr()?;
        loop {
            let op = if self.at_op("+") {
                BinaryOp::Add
            } else if self.at_op("-") {
                BinaryOp::Sub
            } else {
                break;
            };
            self.bump();
            let rhs = self.parse_mul_expr()?;
            lhs = self.binary(start, op, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `mul_expr := unary { ( "*" | "/" | "%" ) unary }`
    fn parse_mul_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_unary()?;
        loop {
            let op = if self.at_op("*") {
                BinaryOp::Mul
            } else if self.at_op("/") {
                BinaryOp::Div
            } else if self.at_op("%") {
                BinaryOp::Rem
            } else {
                break;
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = self.binary(start, op, lhs, rhs);
        }
        Ok(lhs)
    }

    /// `unary := "-" unary | "~" unary | cast_expr`
    fn parse_unary(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let op = if self.at_op("-") {
            Some(UnaryOp::Neg)
        } else if self.at_op("~") {
            Some(UnaryOp::BitNot)
        } else {
            None
        };
        if let Some(op) = op {
            self.bump();
            let inner = self.parse_unary()?;
            return Ok(Expr { kind: ExprKind::Unary { op, expr: Box::new(inner) }, span: self.span_from(start) });
        }
        self.parse_cast_expr()
    }

    /// `cast_expr := postfix [ "as" scalar_type ]`; `as` vor einem anderen Wort ist eine Bindung.
    fn parse_cast_expr(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let expr = self.parse_postfix()?;
        if self.at_kw("as") {
            let next = self.tok_at(1);
            let is_scalar = Self::is_word(next.kind) && super::SCALAR_WORDS.contains(&self.text_of(next));
            if is_scalar {
                self.bump();
                let ty = self.parse_scalar_type()?;
                return Ok(Expr { kind: ExprKind::Cast { expr: Box::new(expr), ty }, span: self.span_from(start) });
            }
        }
        Ok(expr)
    }

    /// `postfix := primary { "." member [ "(" [ args ] ")" ] | "[" expr [ ".." expr ] "]" | "[" expr "," expr "]" }`
    pub(super) fn parse_postfix(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut expr = self.parse_primary()?;
        loop {
            if self.eat_op(".") {
                let name = self.parse_member()?;
                let args = if self.at_op("(") { Some(self.parse_arg_list()?) } else { None };
                expr =
                    Expr { kind: ExprKind::Member { base: Box::new(expr), name, args }, span: self.span_from(start) };
            } else if self.at_op("[") {
                self.bump();
                let first = self.plain(Self::parse_expr)?;
                let kind = if self.eat_op("..") {
                    let to = self.plain(Self::parse_expr)?;
                    ExprKind::Slice { base: Box::new(expr), from: Box::new(first), to: Box::new(to) }
                } else if self.eat_op(",") {
                    let col = self.plain(Self::parse_expr)?;
                    ExprKind::Index2 { base: Box::new(expr), row: Box::new(first), col: Box::new(col) }
                } else {
                    ExprKind::Index { base: Box::new(expr), index: Box::new(first) }
                };
                self.expect_op("]")?;
                expr = Expr { kind, span: self.span_from(start) };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    /// `member := IDENT | KEYWORD`
    fn parse_member(&mut self) -> PResult<Ident> {
        if matches!(self.kind(), TokenKind::Ident | TokenKind::Keyword) {
            let t = self.bump();
            Ok(self.ident_of(t))
        } else {
            Err(self.error_here("einen Membernamen"))
        }
    }

    /// `primary`
    fn parse_primary(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let kind = match self.kind() {
            TokenKind::Int | TokenKind::Hex | TokenKind::Bin | TokenKind::Oct | TokenKind::Float => {
                let value = self.parse_number()?;
                let unit = if self.at_unit_start() { Some(self.parse_unit_lit()?) } else { None };
                ExprKind::Number { value, unit }
            }
            TokenKind::Duration => ExprKind::Duration(self.parse_duration_lit()?),
            TokenKind::Str => ExprKind::Str(self.string()?),
            TokenKind::Keyword => match self.text() {
                "true" => {
                    self.bump();
                    ExprKind::Bool(true)
                }
                "false" => {
                    self.bump();
                    ExprKind::Bool(false)
                }
                "none" => {
                    self.bump();
                    ExprKind::None
                }
                "default" => {
                    self.bump();
                    ExprKind::Default
                }
                "always" | "never" | "eventually" | "stable" | "once" if self.temporal => {
                    self.parse_tprop_atom_kind()?
                }
                _ => return Err(self.error_here("einen Ausdruck")),
            },
            TokenKind::Ident => {
                let callee = self.ident()?;
                let generics =
                    if self.at_op("[") && self.generic_args_ahead() { self.parse_generic_args()? } else { Vec::new() };
                if self.at_op("(") {
                    let args = self.parse_arg_list()?;
                    ExprKind::Call { callee, generics, args }
                } else if !generics.is_empty() {
                    return Err(self.error_here("`(` nach einer expliziten Instanziierung"));
                } else {
                    ExprKind::Ident(callee)
                }
            }
            TokenKind::UpperIdent => {
                let name = self.upper()?;
                let args = if self.at_op("(") { Some(self.parse_arg_list()?) } else { None };
                ExprKind::Upper { name, args }
            }
            TokenKind::TypeIdent => {
                let name = self.type_ident()?;
                let args = if self.at_op("(") { Some(self.parse_arg_list()?) } else { None };
                ExprKind::TypeName { name, args }
            }
            TokenKind::Op if self.at_op("(") => {
                self.bump();
                // In einer Eigenschaft ist `( … )` ein tprop, also auch `(a implies b)`.
                let first = if self.temporal {
                    self.nested(Self::parse_tprop_implies)?
                } else {
                    self.nested(Self::parse_expr)?
                };
                if self.eat_op(",") {
                    let second = self.nested(Self::parse_expr)?;
                    self.expect_op(")")?;
                    ExprKind::Tuple(Box::new(first), Box::new(second))
                } else {
                    self.expect_op(")")?;
                    ExprKind::Paren(Box::new(first))
                }
            }
            TokenKind::Op if self.at_op("[") => {
                self.bump();
                let mut elems = Vec::new();
                if !self.at_op("]") {
                    let first = self.plain(Self::parse_expr)?;
                    // `[N] block(args)`: Array von Blockinstanzen (5.7)
                    if self.at_op("]") && self.tok_at(1).kind == TokenKind::Ident && self.at_op_at(2, "(") {
                        self.bump();
                        let template = self.ident()?;
                        let args = self.parse_arg_list()?;
                        return Ok(Expr {
                            kind: ExprKind::InstanceArray { count: Box::new(first), template, args },
                            span: self.span_from(start),
                        });
                    }
                    elems.push(first);
                    while self.eat_op(",") {
                        elems.push(self.plain(Self::parse_expr)?);
                    }
                }
                self.expect_op("]")?;
                ExprKind::Array(elems)
            }
            _ => return Err(self.error_here("einen Ausdruck")),
        };
        Ok(Expr { kind, span: self.span_from(start) })
    }

    /// Steht nach `[` eine Instanziierung (`… ] (`) oder ein Index?
    fn generic_args_ahead(&self) -> bool {
        let mut depth = 0usize;
        let mut n = 0;
        loop {
            let t = self.tok_at(n);
            match t.kind {
                TokenKind::Op if self.text_of(t) == "[" => depth += 1,
                TokenKind::Op if self.text_of(t) == "]" => {
                    depth -= 1;
                    if depth == 0 {
                        return self.at_op_at(n + 1, "(");
                    }
                }
                TokenKind::Newline | TokenKind::Eof | TokenKind::Indent | TokenKind::Dedent => return false,
                _ => {}
            }
            n += 1;
        }
    }

    /// `generic_args := "[" generic_arg { "," generic_arg } "]"`
    fn parse_generic_args(&mut self) -> PResult<Vec<GenericArg>> {
        self.expect_op("[")?;
        let args = self.plain(|p| {
            let mut args = vec![p.parse_generic_arg()?];
            while p.eat_op(",") {
                args.push(p.parse_generic_arg()?);
            }
            Ok(args)
        })?;
        self.expect_op("]")?;
        Ok(args)
    }

    /// `generic_arg := unit_expr | type | const_expr`; die Klasse folgt der Namensform
    /// und dem Folgetoken: ein Name vor `,`, `]`, `*`, `/`, `^` wird als Einheit versucht
    /// (was Einheiten-, Typ- oder Konstantenvariable ist, entscheidet die Semantik) und
    /// faellt auf einen Konstantenausdruck zurueck (`n * 2`); ein Typname vor `,` oder `]`
    /// ist ein Typ, vor `*`, `/`, `^` eine Einheit (`KiB/s`); ein Name vor `?` oder `!`
    /// eine Typvariable mit Huelle.
    fn parse_generic_arg(&mut self) -> PResult<GenericArg> {
        let type_words = ["bytes", "vec", "line", "stream", "samples", "table", "mat", "map"];
        let next_is = |p: &Self, ops: &[&str]| ops.iter().any(|op| p.at_op_at(1, op));
        let unit_candidate = match self.kind() {
            TokenKind::Ident | TokenKind::UpperIdent => next_is(self, &[",", "]", "*", "/", "^"]),
            TokenKind::TypeIdent => next_is(self, &["*", "/", "^"]),
            TokenKind::Int => self.text() == "1" && self.tok().joint && self.at_op_at(1, "/"),
            _ => false,
        };
        if unit_candidate {
            let start = self.pos;
            if let Ok(unit) = self.parse_unit_expr(false) {
                if self.at_op(",") || self.at_op("]") {
                    return Ok(GenericArg::Unit(unit));
                }
            }
            self.pos = start;
        }
        let wrapped_type_var = self.at(TokenKind::UpperIdent) && (self.at_op_at(1, "?") || self.at_op_at(1, "!"));
        if wrapped_type_var
            || self.at(TokenKind::TypeIdent)
            || self.at_op("[")
            || self.is_scalar_word()
            || (Self::is_word(self.kind()) && type_words.contains(&self.text()))
        {
            return Ok(GenericArg::Type(self.parse_type()?));
        }
        Ok(GenericArg::Const(self.parse_const_expr()?))
    }

    // ------------------------------------------------------------ Eigenschaften (13.3)

    /// `tprop := tprop_implies`
    pub(super) fn parse_tprop(&mut self) -> PResult<Expr> {
        let was = self.temporal;
        self.temporal = true;
        let result = self.parse_tprop_implies();
        self.temporal = was;
        result
    }

    /// `tprop_implies := tprop_or { "implies" tprop_or }`
    fn parse_tprop_implies(&mut self) -> PResult<Expr> {
        let start = self.pos;
        let mut lhs = self.parse_tprop_or()?;
        while self.eat_kw("implies") {
            let rhs = self.parse_tprop_or()?;
            lhs = Expr {
                kind: ExprKind::Implies { lhs: Box::new(lhs), rhs: Box::new(rhs) },
                span: self.span_from(start),
            };
        }
        Ok(lhs)
    }

    /// `tprop_or`, `tprop_and`, `tprop_not`: mit gesetztem `temporal` sind Temporal-
    /// operatoren Primaerausdruecke, also uebernimmt die gewoehnliche Vorrangkette bis
    /// `cmp_expr` (`tprop_atom`); die Bedingungsform gibt es in Eigenschaften nicht.
    fn parse_tprop_or(&mut self) -> PResult<Expr> {
        self.parse_or_expr()
    }

    /// `tprop_and := tprop_not { "and" tprop_not }`
    #[allow(dead_code)]
    fn parse_tprop_and(&mut self) -> PResult<Expr> {
        self.parse_and_expr()
    }

    /// `tprop_not := "not" tprop_not | tprop_atom`
    #[allow(dead_code)]
    fn parse_tprop_not(&mut self) -> PResult<Expr> {
        self.parse_not_expr()
    }

    /// `tprop_atom`: Temporaloperator mit Fenster und Klammer, oder ein Ausdruck.
    #[allow(dead_code)]
    fn parse_tprop_atom(&mut self) -> PResult<Expr> {
        self.parse_cmp_expr()
    }

    fn parse_tprop_atom_kind(&mut self) -> PResult<ExprKind> {
        let op = match self.text() {
            "always" => TemporalOp::Always,
            "never" => TemporalOp::Never,
            "eventually" => TemporalOp::Eventually,
            "stable" => TemporalOp::Stable,
            _ => TemporalOp::Once,
        };
        self.bump();
        let window = if matches!(op, TemporalOp::Eventually | TemporalOp::Stable | TemporalOp::Once) {
            self.expect_op("[")?;
            let d = self.parse_duration_lit()?;
            self.expect_op("]")?;
            Some(d)
        } else {
            None
        };
        self.expect_op("(")?;
        let inner = self.parse_tprop_implies()?;
        self.expect_op(")")?;
        Ok(ExprKind::Temporal { op, window, inner: Box::new(inner) })
    }

    /// `lvalue := IDENT { "." member | "[" expr "]" | "[" expr "," expr "]" }`:
    /// prueft die Form eines bereits geparsten Ausdrucks.
    pub(super) fn parse_lvalue(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Ident(_) => true,
            ExprKind::Member { base, args: None, .. } => self.parse_lvalue(base),
            ExprKind::Index { base, .. } | ExprKind::Index2 { base, .. } => self.parse_lvalue(base),
            _ => false,
        }
    }
}
