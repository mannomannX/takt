//! Matrizen fester Groesse (3.11): Typen, Literale, Operatoren, Methoden
//! und `solve`, mit den Pruefungen 30 und 42. Die Einheiten sind hier die
//! uniforme Form `mat<R, C>[U]`; die dimensionierte Form (Pruefung 34)
//! setzt darauf auf.

use takt_diag::Span;
use takt_mir::expr::{BinaryOp, Expr, ExprKind, MatOp};
use takt_mir::types::{Const, MatUnits, Type};
use takt_mir::*;
use takt_syntax::ast;

use super::{Lowerer, SC3};
use crate::checks::SC30;
use crate::units::Unit;

impl Lowerer<'_> {
    /// Der Typ einer Matrix.
    pub fn mat_type(&mut self, rows: u32, cols: u32, units: MatUnits) -> TypeId {
        self.intern(Type::Mat { rows, cols, units })
    }

    /// Form und Einheiten, wenn `ty` eine Matrix ist.
    fn mat_of(&self, ty: TypeId) -> Option<(u32, u32, MatUnits)> {
        match self.ty(ty) {
            Type::Mat { rows, cols, units } => Some((*rows, *cols, units.clone())),
            _ => None,
        }
    }

    /// Die Einheit aller Elemente der uniformen Form.
    fn uniform_unit(&self, units: &MatUnits) -> Unit {
        match units {
            MatUnits::Uniform(Some(id)) => self.units.unit_of(*id),
            MatUnits::Uniform(None) | MatUnits::Dimensioned { .. } => Unit::one(),
        }
    }

    /// Die uniformen Einheiten zu einer Einheit.
    fn uniform(&mut self, unit: &Unit, span: Span) -> Option<MatUnits> {
        Some(MatUnits::Uniform(if unit.is_one() { None } else { Some(self.unit_id(unit, span)?) }))
    }

    /// Der Elementtyp: `float` in Programmbreite mit der Einheit.
    pub fn mat_elem_type(&mut self, units: &MatUnits, span: Span) -> Option<TypeId> {
        let unit = self.uniform_unit(units);
        let width = self.float_width();
        self.float_type(width, &unit, None, span)
    }

    /// Ein Literal `[[e, ..], ..]` fuer `mat<R, C>`; fuer eine Spalte reicht
    /// `[e, ..]` (3.11).
    pub fn mat_literal(
        &mut self,
        items: &[ast::Expr],
        rows: u32,
        cols: u32,
        units: MatUnits,
        ty: TypeId,
        span: Span,
    ) -> Option<Expr> {
        if items.len() != rows as usize {
            self.error(SC30, span, format!("{rows} Zeilen erwartet, {} gefunden (3.11)", items.len()));
            return None;
        }
        let elem = self.mat_elem_type(&units, span)?;
        let row_ty = self.intern(Type::Array { elem, len: cols });
        let mut out = Vec::with_capacity(items.len());
        for row in items {
            let cells: Vec<&ast::Expr> = match &row.kind {
                ast::ExprKind::Array(cells) => cells.iter().collect(),
                _ if cols == 1 => vec![row],
                _ => {
                    self.error(SC30, row.span, format!("eine Zeile mit {cols} Elementen erwartet (3.11)"));
                    return None;
                }
            };
            if cells.len() != cols as usize {
                self.error(SC30, row.span, format!("{cols} Elemente je Zeile erwartet, {} gefunden", cells.len()));
                return None;
            }
            let lowered = cells.iter().map(|c| self.check(c, elem)).collect::<Option<Vec<_>>>()?;
            out.push(Expr::new(ExprKind::Array(lowered), row_ty, row.span));
        }
        Some(Expr::new(ExprKind::Array(out), ty, span))
    }

    /// `+`, `-`, `*`, `/` mit einer Matrix (3.11, Pruefung 30).
    pub fn mat_binary(&mut self, op: BinaryOp, a: Expr, b: Expr, span: Span) -> Option<Expr> {
        let (ma, mb) = (self.mat_of(a.ty), self.mat_of(b.ty));
        let width = self.float_width();
        let scalar = |this: &mut Self, t: TypeId| -> Option<Unit> {
            match this.ty(t) {
                Type::Float { width: w, .. } if *w == width => this.unit_of_type(t),
                _ => None,
            }
        };
        let result = match (op, ma, mb) {
            (BinaryOp::Add | BinaryOp::Sub, Some((ra, ca, ua)), Some((rb, cb, ub))) => {
                if (ra, ca) != (rb, cb) {
                    self.error(
                        SC30,
                        span,
                        format!("`+`/`-` verlangt gleiche Form: mat<{ra}, {ca}> und mat<{rb}, {cb}> (3.11)"),
                    );
                    return None;
                }
                if self.uniform_unit(&ua) != self.uniform_unit(&ub) {
                    let (x, y) = (self.type_name(a.ty), self.type_name(b.ty));
                    self.error(SC3, span, format!("Einheiten von `{x}` und `{y}` passen nicht (3.11)"));
                    return None;
                }
                a.ty
            }
            (BinaryOp::Mul, Some((ra, ca, ua)), Some((rb, cb, ub))) => {
                if ca != rb {
                    self.error(
                        SC30,
                        span,
                        format!("Matrixprodukt verlangt Spalten = Zeilen: mat<{ra}, {ca}> · mat<{rb}, {cb}> (3.11)"),
                    );
                    return None;
                }
                let unit = self.uniform_unit(&ua).mul(&self.uniform_unit(&ub));
                let units = self.uniform(&unit, span)?;
                self.mat_type(ra, cb, units)
            }
            (BinaryOp::Mul | BinaryOp::Div, Some((r, c, u)), None) => {
                let Some(s) = scalar(self, b.ty) else {
                    let n = self.type_name(b.ty);
                    self.error(SC3, span, format!("Matrix mal `{n}`: ein Skalar hat die Breite von `float` (3.11)"));
                    return None;
                };
                let unit =
                    if op == BinaryOp::Mul { self.uniform_unit(&u).mul(&s) } else { self.uniform_unit(&u).div(&s) };
                let units = self.uniform(&unit, span)?;
                self.mat_type(r, c, units)
            }
            (BinaryOp::Mul, None, Some((r, c, u))) => {
                let Some(s) = scalar(self, a.ty) else {
                    let n = self.type_name(a.ty);
                    self.error(SC3, span, format!("`{n}` mal Matrix: ein Skalar hat die Breite von `float` (3.11)"));
                    return None;
                };
                let unit = s.mul(&self.uniform_unit(&u));
                let units = self.uniform(&unit, span)?;
                self.mat_type(r, c, units)
            }
            _ => {
                let (x, y) = (self.type_name(a.ty), self.type_name(b.ty));
                self.error(SC30, span, format!("`{op:?}` zwischen `{x}` und `{y}` (3.11)"));
                return None;
            }
        };
        Some(Expr::new(ExprKind::Binary { op, lhs: Box::new(a), rhs: Box::new(b) }, result, span))
    }

    /// `A.transpose()`, `A.inv()`, `A.det()`, `A.cholesky()` (3.11).
    pub fn mat_member(&mut self, b: Expr, member: &str, args: Option<&[ast::Arg]>, span: Span) -> Option<Expr> {
        if args.is_some_and(|a| !a.is_empty()) {
            self.error(SC3, span, format!("`{member}()` nimmt keine Argumente"));
            return None;
        }
        let (rows, cols, units) = self.mat_of(b.ty)?;
        let unit = self.uniform_unit(&units);
        let square = |this: &mut Self| {
            if rows != cols {
                this.error(
                    SC30,
                    span,
                    format!("`{member}` verlangt eine quadratische Matrix, mat<{rows}, {cols}> (3.11)"),
                );
                return None;
            }
            Some(())
        };
        let (op, ty) = match member {
            "transpose" => (MatOp::Transpose, self.mat_type(cols, rows, units)),
            "inv" => {
                square(self)?;
                let inverse = self.uniform(&unit.pow(-1), span)?;
                (MatOp::Inv, self.mat_type(rows, rows, inverse))
            }
            "det" => {
                square(self)?;
                let width = self.float_width();
                let ty = self.float_type(width, &unit.pow(rows as i32), None, span)?;
                (MatOp::Det, ty)
            }
            "cholesky" => {
                square(self)?;
                let Some(half) = half_unit(&unit) else {
                    let u = self.units.display(&self.program, &unit);
                    self.error(
                        SC3,
                        span,
                        format!("`cholesky` verlangt eine Quadrateinheit, `[{u}]` hat keine Wurzel (3.11)"),
                    );
                    return None;
                };
                let half = self.uniform(&half, span)?;
                let inner = self.mat_type(rows, rows, half);
                (MatOp::Cholesky, self.intern(Type::Optional(inner)))
            }
            _ => return None,
        };
        Some(Expr::new(ExprKind::MatOp { op, args: vec![b] }, ty, span))
    }

    /// `solve(A, b)`: `x` mit `A · x = b`, getypt wie `A.inv() * b` (3.11).
    pub fn mat_solve(&mut self, args: &[ast::Arg], span: Span) -> Option<Expr> {
        if args.len() != 2 || args.iter().any(|a| a.name.is_some()) {
            self.error(SC3, span, "`solve(A, b)` verlangt zwei positionale Argumente (3.11)");
            return None;
        }
        let a = self.expr(&args[0].value, None)?;
        let Some((n, m, ua)) = self.mat_of(a.ty) else {
            let t = self.type_name(a.ty);
            self.error(SC30, span, format!("`solve` verlangt eine Matrix, gefunden `{t}`"));
            return None;
        };
        if n != m {
            self.error(SC30, span, format!("`solve` verlangt eine quadratische Matrix, mat<{n}, {m}> (3.11)"));
            return None;
        }
        // Ein Literal fuer `b` bekommt die Form aus `A` und die Einheit aus
        // seinem ersten Element.
        let hint = match &args[1].value.kind {
            ast::ExprKind::Array(items) => {
                let first = items.first().and_then(|i| match &i.kind {
                    ast::ExprKind::Array(cells) => cells.first(),
                    _ => Some(i),
                });
                let unit = match first {
                    Some(e) => {
                        let lowered = self.expr(e, None)?;
                        self.unit_of_type(lowered.ty).unwrap_or_else(Unit::one)
                    }
                    None => Unit::one(),
                };
                let units = self.uniform(&unit, span)?;
                Some(self.mat_type(n, 1, units))
            }
            _ => None,
        };
        let b = self.expr(&args[1].value, hint)?;
        let Some((rb, k, ub)) = self.mat_of(b.ty) else {
            let t = self.type_name(b.ty);
            self.error(SC30, span, format!("`solve` verlangt eine rechte Seite als Matrix, gefunden `{t}`"));
            return None;
        };
        if rb != n {
            self.error(
                SC30,
                span,
                format!("`solve`: rechte Seite mit {n} Zeilen erwartet, mat<{rb}, {k}> gefunden (3.11)"),
            );
            return None;
        }
        let unit = self.uniform_unit(&ub).div(&self.uniform_unit(&ua));
        let units = self.uniform(&unit, span)?;
        let ty = self.mat_type(n, k, units);
        Some(Expr::new(ExprKind::MatOp { op: MatOp::Solve, args: vec![a, b] }, ty, span))
    }

    /// Ein Index vom Typ `int in 0..len-1`: ein Literal in der Range oder
    /// eine Ganzzahl, deren Range darin liegt (3.11, Pruefung 30).
    pub fn mat_index_expr(&mut self, e: &ast::Expr, len: u32, what: &str) -> Option<Expr> {
        let int = self.tys.int;
        let i = self.check(e, int)?;
        let inside = match &i.kind {
            ExprKind::Int(n) => *n >= 0 && *n < i64::from(len),
            _ => self.range_of(i.ty).is_some_and(|r| match (r.lo, r.hi) {
                (Const::Int(lo), Const::Int(hi)) => lo >= 0 && hi < i64::from(len),
                _ => false,
            }),
        };
        if !inside {
            let shown = match &i.kind {
                ExprKind::Int(n) => n.to_string(),
                _ => self.type_name(i.ty),
            };
            self.error(SC30, e.span, format!("{what} `{shown}` nicht in 0..{} (3.11)", len - 1));
            return None;
        }
        Some(i)
    }

    /// `A[i, j]` als Ausdruck.
    pub fn mat_index(&mut self, base: &ast::Expr, row: &ast::Expr, col: &ast::Expr, span: Span) -> Option<Expr> {
        let b = self.expr(base, None)?;
        let Some((rows, cols, units)) = self.mat_of(b.ty) else {
            let n = self.type_name(b.ty);
            self.error(SC3, span, format!("`[i, j]` auf `{n}`"));
            return None;
        };
        let i = self.mat_index_expr(row, rows, "Zeile")?;
        let j = self.mat_index_expr(col, cols, "Spalte")?;
        let ty = self.mat_elem_type(&units, span)?;
        Some(Expr::new(ExprKind::Index2 { base: Box::new(b), row: Box::new(i), col: Box::new(j) }, ty, span))
    }
}

/// Die Haelfte jedes Exponenten, wenn alle gerade sind.
fn half_unit(unit: &Unit) -> Option<Unit> {
    if unit.factors.iter().any(|(_, e)| e % 2 != 0) {
        return None;
    }
    Some(Unit { factors: unit.factors.iter().map(|(a, e)| (*a, e / 2)).collect(), overflow: false })
}
