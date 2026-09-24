//! Matrizen fester Groesse (3.11): Typen, Literale, Operatoren, Methoden
//! und `solve`, mit den Pruefungen 30, 34 und 42. Jede Matrix traegt ein
//! Zeilen- und ein Spaltentupel von Einheiten (Hart 1995); Element (i, j)
//! hat `r_i * c_j`. Die uniforme Form `mat<m, n>[U]` ist der Sonderfall
//! `mat[(U, .., U), (1, .., 1)]`, und die kanonische Form `c_1 = 1` macht
//! Gleichheit nach Hart zur Gleichheit der internierten Typen.

use takt_diag::Span;
use takt_mir::expr::{BinaryOp, Expr, ExprKind, MatOp};
use takt_mir::types::{Const, FloatWidth, MatUnits, Type};
use takt_mir::*;
use takt_syntax::ast;

use super::{Lowerer, SC3};
use crate::checks::{SC30, SC34, SC42};
use crate::symbols::Entity;
use crate::units::Unit;

/// Zeilen- und Spalteneinheiten einer Matrix.
pub type Tuples = (Vec<Unit>, Vec<Unit>);

impl Lowerer<'_> {
    /// Die Einheitentupel, wenn `ty` eine Matrix ist.
    fn mat_of(&self, ty: TypeId) -> Option<Tuples> {
        let Type::Mat { rows, cols, units } = self.ty(ty) else { return None };
        Some(match units {
            MatUnits::Uniform(u) => {
                let unit = u.map(|id| self.units.unit_of(id)).unwrap_or_else(Unit::one);
                (vec![unit; *rows as usize], vec![Unit::one(); *cols as usize])
            }
            MatUnits::Dimensioned { rows, cols } => (
                rows.iter().map(|id| self.units.unit_of(*id)).collect(),
                cols.iter().map(|id| self.units.unit_of(*id)).collect(),
            ),
        })
    }

    /// Der Matrixtyp zu den Tupeln in kanonischer Form: `c_1 = 1`; sind
    /// alle Zeilen gleich und alle Spalten eins, ist es die uniforme Form.
    pub fn mat_type(&mut self, tuples: Tuples, span: Span) -> Option<TypeId> {
        let (rows, cols) = tuples;
        let c1 = cols[0].clone();
        let rows: Vec<Unit> = rows.iter().map(|r| r.mul(&c1)).collect();
        let cols: Vec<Unit> = cols.iter().map(|c| c.div(&c1)).collect();
        let (m, n) = (rows.len() as u32, cols.len() as u32);
        let units = if rows.iter().all(|r| *r == rows[0]) && cols.iter().all(Unit::is_one) {
            MatUnits::Uniform(if rows[0].is_one() { None } else { Some(self.unit_id(&rows[0], span)?) })
        } else {
            let rows = rows.iter().map(|u| self.tuple_id(u, span)).collect::<Option<Vec<_>>>()?;
            let cols = cols.iter().map(|u| self.tuple_id(u, span)).collect::<Option<Vec<_>>>()?;
            MatUnits::Dimensioned { rows, cols }
        };
        Some(self.intern(Type::Mat { rows: m, cols: n, units }))
    }

    /// Merkt den Scratch einer Operation fuer `takt size` vor (11.2, 11.5):
    /// `floats` Elemente in der Breite von `float` und `ints` Zeilenindizes.
    fn note_scratch(&mut self, floats: usize, ints: usize) {
        let width = match self.float_width() {
            FloatWidth::F32 => 4,
            FloatWidth::F64 => 8,
        };
        let bytes = (floats * width + ints * 4) as u32;
        if let Some(mc) = self.mctx.as_mut() {
            mc.machine.layout.scratch_bytes = Some(mc.machine.layout.scratch_bytes.unwrap_or(0).max(bytes));
        }
    }

    /// Eine Einheit eines Tupels; `1` wird als eigene Einheit interniert.
    fn tuple_id(&mut self, unit: &Unit, span: Span) -> Option<UnitId> {
        if !unit.is_one() {
            return self.unit_id(unit, span);
        }
        match self.units.intern(&mut self.program, unit) {
            Ok(id) => Some(id),
            Err(msg) => {
                self.error(SC3, span, msg);
                None
            }
        }
    }

    /// Ein Einheitentupel: `unitvec`-Name, sein Kehrwert oder ein Literal (3.11).
    pub fn unit_tuple(&mut self, t: &ast::UnitTuple) -> Option<Vec<Unit>> {
        match t {
            ast::UnitTuple::Named(name) | ast::UnitTuple::Inverse(name) => {
                let Some(Entity::Unitvec(units)) = self.peek(&name.name).cloned() else {
                    self.error(SC3, name.span, format!("`{}` ist kein `unitvec` (3.11)", name.name));
                    return None;
                };
                Some(match t {
                    ast::UnitTuple::Inverse(_) => units.iter().map(|u| u.pow(-1)).collect(),
                    _ => units,
                })
            }
            ast::UnitTuple::Literal(exprs) => exprs.iter().map(|u| self.unit_expr(u)).collect(),
        }
    }

    /// Pruefung 30 (leere Form) und Pruefung 42 (Lint ab 16, 3.11).
    pub fn mat_shape(&mut self, rows: u32, cols: u32, span: Span) -> Option<()> {
        if rows == 0 || cols == 0 {
            self.error(SC30, span, "eine Matrix hat mindestens eine Zeile und eine Spalte (3.11)");
            return None;
        }
        if rows > 16 || cols > 16 {
            let bytes = u64::from(rows) * u64::from(cols) * 8;
            self.warn(SC42, span, format!("Matrix {rows}×{cols}: Kosten n³, Scratch {bytes} Byte (3.11)"));
        }
        Some(())
    }

    /// Die Einheit von `A[i, j]`: `r_i * c_j`; bei variablem Index muessen
    /// alle betroffenen Elemente dieselbe Einheit haben (Pruefung 34).
    fn mat_elem_unit(&mut self, tuples: &Tuples, i: Option<usize>, j: Option<usize>, span: Span) -> Option<Unit> {
        let (rows, cols) = tuples;
        let is = i.map_or(0..rows.len(), |i| i..i + 1);
        let js = j.map_or(0..cols.len(), |j| j..j + 1);
        let mut first: Option<Unit> = None;
        for r in is {
            for c in js.clone() {
                let unit = rows[r].mul(&cols[c]);
                match &first {
                    None => first = Some(unit),
                    Some(f) if *f != unit => {
                        self.error(
                            SC34,
                            span,
                            "variabler Index nur, wenn die betroffenen Elemente dieselbe Einheit haben (3.11)",
                        );
                        return None;
                    }
                    Some(_) => {}
                }
            }
        }
        first
    }

    /// Der Elementtyp von `A[i, j]` zu gesenkten Indizes.
    pub fn mat_index_type(&mut self, mat: TypeId, i: &Expr, j: &Expr, span: Span) -> Option<TypeId> {
        let tuples = self.mat_of(mat)?;
        let unit = self.mat_elem_unit(&tuples, const_index(i), const_index(j), span)?;
        let width = self.float_width();
        self.float_type(width, &unit, None, span)
    }

    /// Ein Literal `[[e, ..], ..]` fuer eine Matrix; fuer eine Spalte reicht
    /// `[e, ..]`. Jedes Element hat die Einheit `r_i * c_j` (3.11).
    pub fn mat_literal(&mut self, items: &[ast::Expr], ty: TypeId, span: Span) -> Option<Expr> {
        let (rows, cols) = self.mat_of(ty)?;
        let (m, n) = (rows.len(), cols.len());
        if items.len() != m {
            self.error(SC30, span, format!("{m} Zeilen erwartet, {} gefunden (3.11)", items.len()));
            return None;
        }
        let width = self.float_width();
        let row_ty = self.intern(Type::Array { elem: self.tys.float, len: n as u32 });
        let mut out = Vec::with_capacity(m);
        for (i, row) in items.iter().enumerate() {
            let cells: Vec<&ast::Expr> = match &row.kind {
                ast::ExprKind::Array(cells) => cells.iter().collect(),
                _ if n == 1 => vec![row],
                _ => {
                    self.error(SC30, row.span, format!("eine Zeile mit {n} Elementen erwartet (3.11)"));
                    return None;
                }
            };
            if cells.len() != n {
                self.error(SC30, row.span, format!("{n} Elemente je Zeile erwartet, {} gefunden", cells.len()));
                return None;
            }
            let mut lowered = Vec::with_capacity(n);
            for (j, cell) in cells.iter().enumerate() {
                let elem = self.float_type(width, &rows[i].mul(&cols[j]), None, cell.span)?;
                lowered.push(self.check(cell, elem)?);
            }
            out.push(Expr::new(ExprKind::Array(lowered), row_ty, row.span));
        }
        Some(Expr::new(ExprKind::Array(out), ty, span))
    }

    /// `+`, `-`, `*`, `/` mit einer Matrix (3.11, Pruefungen 30 und 34).
    pub fn mat_binary(&mut self, op: BinaryOp, a: Expr, b: Expr, span: Span) -> Option<Expr> {
        let (ma, mb) = (self.mat_of(a.ty), self.mat_of(b.ty));
        let width = self.float_width();
        let scalar = |this: &mut Self, t: TypeId| -> Option<Unit> {
            match this.ty(t) {
                Type::Float { width: w, .. } if *w == width => this.unit_of_type(t),
                _ => None,
            }
        };
        let names = |this: &Self| (this.type_name(a.ty), this.type_name(b.ty));
        let result = match (op, ma, mb) {
            (BinaryOp::Add | BinaryOp::Sub, Some((ra, ca)), Some((rb, cb))) => {
                if (ra.len(), ca.len()) != (rb.len(), cb.len()) {
                    let (x, y) = names(self);
                    self.error(SC30, span, format!("`+`/`-` verlangt gleiche Form: `{x}` und `{y}` (3.11)"));
                    return None;
                }
                if a.ty != b.ty {
                    let (x, y) = names(self);
                    self.error(SC34, span, format!("Einheiten von `{x}` und `{y}` passen nicht (3.11)"));
                    return None;
                }
                a.ty
            }
            (BinaryOp::Mul, Some((ra, ca)), Some((rb, cb))) => {
                if ca.len() != rb.len() {
                    let (x, y) = names(self);
                    self.error(SC30, span, format!("Matrixprodukt verlangt Spalten = Zeilen: `{x}` · `{y}` (3.11)"));
                    return None;
                }
                let k = ca[0].mul(&rb[0]);
                if ca.iter().zip(&rb).any(|(c, r)| c.mul(r) != k) {
                    let (x, y) = names(self);
                    self.error(
                        SC34,
                        span,
                        format!("Spalteneinheiten von `{x}` passen nicht zu den Zeileneinheiten von `{y}` (3.11)"),
                    );
                    return None;
                }
                let rows = ra.iter().map(|r| r.mul(&k)).collect();
                self.mat_type((rows, cb), span)?
            }
            (BinaryOp::Mul | BinaryOp::Div, Some((r, c)), None) => {
                let Some(s) = scalar(self, b.ty) else {
                    let n = self.type_name(b.ty);
                    self.error(SC3, span, format!("Matrix mal `{n}`: ein Skalar hat die Breite von `float` (3.11)"));
                    return None;
                };
                let rows = r.iter().map(|u| if op == BinaryOp::Mul { u.mul(&s) } else { u.div(&s) }).collect();
                self.mat_type((rows, c), span)?
            }
            (BinaryOp::Mul, None, Some((r, c))) => {
                let Some(s) = scalar(self, a.ty) else {
                    let n = self.type_name(a.ty);
                    self.error(SC3, span, format!("`{n}` mal Matrix: ein Skalar hat die Breite von `float` (3.11)"));
                    return None;
                };
                let rows = r.iter().map(|u| s.mul(u)).collect();
                self.mat_type((rows, c), span)?
            }
            _ => {
                let (x, y) = names(self);
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
        let (rows, cols) = self.mat_of(b.ty)?;
        let n = rows.len();
        let square = |this: &mut Self| {
            if n != cols.len() {
                let t = this.type_name(b.ty);
                this.error(SC30, span, format!("`{member}` verlangt eine quadratische Matrix, `{t}` (3.11)"));
                return None;
            }
            Some(())
        };
        let (op, ty) = match member {
            "transpose" => (MatOp::Transpose, self.mat_type((cols, rows), span)?),
            "inv" => {
                square(self)?;
                self.note_scratch(2 * n * n, n);
                let r = cols.iter().map(|c| c.pow(-1)).collect();
                let c = rows.iter().map(|r| r.pow(-1)).collect();
                (MatOp::Inv, self.mat_type((r, c), span)?)
            }
            "det" => {
                square(self)?;
                self.note_scratch(n * n, n);
                let unit = rows.iter().chain(&cols).fold(Unit::one(), |acc, u| acc.mul(u));
                let width = self.float_width();
                (MatOp::Det, self.float_type(width, &unit, None, span)?)
            }
            "cholesky" => {
                square(self)?;
                self.note_scratch(n * n, 0);
                // `A = L·Lᵀ` verlangt `r_i = r_1 * c_i` und eine Wurzel von `r_1`.
                if rows.iter().zip(&cols).any(|(r, c)| *r != rows[0].mul(c)) {
                    let t = self.type_name(b.ty);
                    self.error(SC34, span, format!("`cholesky` verlangt symmetrische Einheiten, `{t}` (3.11)"));
                    return None;
                }
                let Some(root) = half_unit(&rows[0]) else {
                    let u = self.units.display(&self.program, &rows[0]);
                    self.error(SC34, span, format!("`cholesky`: `[{u}]` hat keine Wurzel (3.11)"));
                    return None;
                };
                let l_rows = cols.iter().map(|c| root.mul(c)).collect();
                let inner = self.mat_type((l_rows, vec![Unit::one(); n]), span)?;
                (MatOp::Cholesky, self.intern(Type::Optional(inner)))
            }
            _ => return None,
        };
        let e = Expr::new(ExprKind::MatOp { op, args: vec![b] }, ty, span);
        Some(if matches!(op, MatOp::Inv | MatOp::Det) { self.finite(e) } else { e })
    }

    /// `solve(A, b)`: `x` mit `A · x = b`, getypt wie `A.inv() * b` (3.11).
    pub fn mat_solve(&mut self, args: &[ast::Arg], span: Span) -> Option<Expr> {
        if args.len() != 2 || args.iter().any(|a| a.name.is_some()) {
            self.error(SC3, span, "`solve(A, b)` verlangt zwei positionale Argumente (3.11)");
            return None;
        }
        let a = self.expr(&args[0].value, None)?;
        let Some((ra, ca)) = self.mat_of(a.ty) else {
            let t = self.type_name(a.ty);
            self.error(SC30, span, format!("`solve` verlangt eine Matrix, gefunden `{t}`"));
            return None;
        };
        let n = ra.len();
        if n != ca.len() {
            let t = self.type_name(a.ty);
            self.error(SC30, span, format!("`solve` verlangt eine quadratische Matrix, `{t}` (3.11)"));
            return None;
        }
        // Ein Literal fuer `b` bekommt die Form aus `A` und den Faktor `k`
        // seiner Zeileneinheiten `k * r_i` aus seinem ersten Element.
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
                let k = unit.div(&ra[0]);
                let rows = ra.iter().map(|r| r.mul(&k)).collect();
                Some(self.mat_type((rows, vec![Unit::one()]), span)?)
            }
            _ => None,
        };
        let b = self.expr(&args[1].value, hint)?;
        let Some((rb, cb)) = self.mat_of(b.ty) else {
            let t = self.type_name(b.ty);
            self.error(SC30, span, format!("`solve` verlangt eine rechte Seite als Matrix, gefunden `{t}`"));
            return None;
        };
        if rb.len() != n {
            let t = self.type_name(b.ty);
            self.error(SC30, span, format!("`solve`: rechte Seite mit {n} Zeilen erwartet, `{t}` gefunden (3.11)"));
            return None;
        }
        let k = rb[0].div(&ra[0]);
        if ra.iter().zip(&rb).any(|(r, rb)| r.mul(&k) != *rb) {
            let (x, y) = (self.type_name(a.ty), self.type_name(b.ty));
            self.error(SC34, span, format!("`solve`: Zeileneinheiten von `{y}` passen nicht zu `{x}` (3.11)"));
            return None;
        }
        self.note_scratch(n * n + n * cb.len(), n);
        let rows = ca.iter().map(|c| k.div(c)).collect();
        let ty = self.mat_type((rows, cb), span)?;
        let e = Expr::new(ExprKind::MatOp { op: MatOp::Solve, args: vec![a, b] }, ty, span);
        Some(self.finite(e))
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
        let Type::Mat { rows, cols, .. } = self.ty(b.ty).clone() else {
            let n = self.type_name(b.ty);
            self.error(SC3, span, format!("`[i, j]` auf `{n}`"));
            return None;
        };
        let i = self.mat_index_expr(row, rows, "Zeile")?;
        let j = self.mat_index_expr(col, cols, "Spalte")?;
        let ty = self.mat_index_type(b.ty, &i, &j, span)?;
        Some(Expr::new(ExprKind::Index2 { base: Box::new(b), row: Box::new(i), col: Box::new(j) }, ty, span))
    }
}

/// Ein konstanter Index.
fn const_index(e: &Expr) -> Option<usize> {
    match &e.kind {
        ExprKind::Int(n) => usize::try_from(*n).ok(),
        _ => None,
    }
}

/// Die Haelfte jedes Exponenten, wenn alle gerade sind.
fn half_unit(unit: &Unit) -> Option<Unit> {
    if unit.factors.iter().any(|(_, e)| e % 2 != 0) {
        return None;
    }
    Some(Unit { factors: unit.factors.iter().map(|(a, e)| (*a, e / 2)).collect(), overflow: false })
}
