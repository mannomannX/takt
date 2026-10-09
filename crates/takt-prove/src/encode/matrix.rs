//! Matrizen im Modell (3.11, `matrix.rs` im Interpreter): eine Matrix ist
//! ihre Elemente zeilenweise, in der Breite von `float`. Summe, Differenz,
//! Skalierung und Transponierte rechnet das Modell wie `libtaktm::mat`, das
//! Produkt als `fma`-Kette ab null; `det`, `inv`, `solve` und `cholesky`
//! sieht der Solver uninterpretiert, die Auswertung rechnet sie genau.
//! Eine singulaere Matrix ist bei `inv` und `solve` ein `ArithmeticFault`;
//! nicht endliche Ergebnisse prueft der Knoten `Checked{NonFinite}` um die
//! Operation, `cholesky` selbst, wie der Interpreter.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::TypeId;
use takt_mir::expr::{BinaryOp, Expr, MatOp};
use takt_mir::machine::{ArithKind, FaultKind};
use takt_mir::types::{FloatWidth, Type};

use super::value::V;
use super::{Cx, Enc, Env, Exit, ExitKind, Flow, R, no};
use crate::term::{MatFun, Op, Sort, Term};

impl Enc<'_> {
    /// Zeilen und Spalten einer Matrix.
    pub(super) fn mat_dims(&self, ty: TypeId) -> Option<(u32, u32)> {
        match self.p.types.get(ty) {
            Type::Mat { rows, cols, .. } => Some((*rows, *cols)),
            _ => None,
        }
    }

    /// Die Sorte der Elemente: die Breite von `float` (4.2).
    pub(super) fn mat_sort(&self) -> Sort {
        match self.p.config.float_width {
            FloatWidth::F32 => Sort::F32,
            FloatWidth::F64 => Sort::F64,
        }
    }

    /// Die Elemente einer Matrix oder ein Skalar als einzelnes.
    fn mat_items(&mut self, e: &Expr, cx: &Cx<'_>, env: &Env, flow: &mut Flow) -> R<Vec<Term>> {
        match self.value(e, cx, env, flow)? {
            V::Node(parts) => parts.into_iter().map(|p| p.leaf(e.span)).collect(),
            V::Leaf(t) => Ok(vec![t]),
        }
    }

    /// `+`, `-`, `*`, `/` mit einer Matrix (`matrix::binary`): die Formen hat
    /// die Sema geprueft.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mat_binary(
        &mut self,
        op: BinaryOp,
        lhs: &Expr,
        rhs: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        let (a, b) = (self.mat_items(lhs, cx, env, flow)?, self.mat_items(rhs, cx, env, flow)?);
        let leaves = |items: Vec<Term>| V::Node(items.into_iter().map(V::Leaf).collect());
        let (left, right) = (self.mat_dims(lhs.ty), self.mat_dims(rhs.ty));
        Ok(leaves(match (op, left, right) {
            (BinaryOp::Add, Some(_), Some(_)) => a.into_iter().zip(b).map(|(x, y)| Term::bin(Op::FAdd, x, y)).collect(),
            (BinaryOp::Sub, Some(_), Some(_)) => a.into_iter().zip(b).map(|(x, y)| Term::bin(Op::FSub, x, y)).collect(),
            // `mat::mul`: je Element eine `fma`-Kette ab null.
            (BinaryOp::Mul, Some((m, k)), Some((_, n))) => {
                let (m, k, n) = (m as usize, k as usize, n as usize);
                let mut out = Vec::with_capacity(m * n);
                for i in 0..m {
                    for j in 0..n {
                        let mut acc = Term::float(0.0, self.mat_sort());
                        for l in 0..k {
                            acc = Term::app(Op::FFma, vec![a[i * k + l].clone(), b[l * n + j].clone(), acc]);
                        }
                        out.push(acc);
                    }
                }
                out
            }
            (BinaryOp::Mul, Some(_), None) => a.into_iter().map(|x| Term::bin(Op::FMul, x, b[0].clone())).collect(),
            (BinaryOp::Mul, None, Some(_)) => b.into_iter().map(|x| Term::bin(Op::FMul, x, a[0].clone())).collect(),
            (BinaryOp::Div, Some(_), None) => a.into_iter().map(|x| Term::bin(Op::FDiv, x, b[0].clone())).collect(),
            _ => return no(format!("`{op:?}` mit einer Matrix"), span),
        }))
    }

    /// `transpose`, `inv`, `solve` und `cholesky` (`matrix::op`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mat_op(
        &mut self,
        op: MatOp,
        args: &[Expr],
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<V> {
        let Some(first) = args.first() else { return no("Matrixoperation ohne Matrix", span) };
        let Some((m, n)) = self.mat_dims(first.ty) else { return no("Matrixoperation ohne Matrix", span) };
        let a = self.mat_items(first, cx, env, flow)?;
        let leaves = |items: Vec<Term>| V::Node(items.into_iter().map(V::Leaf).collect());
        if op == MatOp::Transpose {
            let (m, n) = (m as usize, n as usize);
            return Ok(leaves((0..n * m).map(|o| a[(o % m) * n + o / m].clone()).collect()));
        }
        let (f, mut operands, k) = match op {
            MatOp::Inv => (MatFun::Inv, a, 0),
            MatOp::Cholesky => (MatFun::Cholesky, a, 0),
            MatOp::Solve => {
                let Some(rhs) = args.get(1) else { return no("`solve` ohne rechte Seite", span) };
                let Some((_, k)) = self.mat_dims(rhs.ty) else { return no("`solve` ohne Matrix", span) };
                let b = self.mat_items(rhs, cx, env, flow)?;
                (MatFun::Solve, a.into_iter().chain(b).collect(), k)
            }
            MatOp::Det | MatOp::Transpose => return no("`det` liefert keine Matrix", span),
        };
        let (Ok(n8), Ok(k8)) = (u8::try_from(n), u8::try_from(k)) else { return no("Matrix ueber 255", span) };
        self.uninterpreted.insert(f.name().to_string());
        operands.shrink_to_fit();
        let part = |part: u16| Term::app(Op::Mat { f, n: n8, k: k8, part }, operands.clone());
        let count = f.elements(n8, k8);
        let items: Vec<Term> = (0..count).map(part).collect();
        let flag = part(count);
        match f {
            // 3.11: Eine singulaere Matrix ist ein Fault.
            MatFun::Inv | MatFun::Solve => {
                self.fault_exit(FaultKind::Arithmetic(ArithKind::Singular), flag, flow, span);
                Ok(leaves(items))
            }
            // `none`, wenn sie sich nicht zerlegen laesst; ein nicht endlicher
            // Faktor faultet im Interpreter (`matrix::value`).
            _ => {
                let finite = Term::and(items.iter().map(|x| Term::app(Op::IsFinite, vec![x.clone()])).collect());
                let fails = Term::and(vec![flag.clone(), finite.not()]);
                self.fault_exit(FaultKind::Arithmetic(ArithKind::NonFinite), fails, flow, span);
                Ok(V::Node(vec![V::Leaf(flag), leaves(items)]))
            }
        }
    }

    /// `m.det()`: die Determinante der LU, uninterpretiert.
    pub(super) fn mat_det(&mut self, args: &[Expr], cx: &Cx<'_>, env: &Env, flow: &mut Flow, span: Span) -> R<Term> {
        let Some(first) = args.first() else { return no("`det` ohne Matrix", span) };
        let Some((_, n)) = self.mat_dims(first.ty) else { return no("`det` ohne Matrix", span) };
        let Ok(n) = u8::try_from(n) else { return no("Matrix ueber 255", span) };
        let a = self.mat_items(first, cx, env, flow)?;
        self.uninterpreted.insert(MatFun::Det.name().to_string());
        Ok(Term::app(Op::Mat { f: MatFun::Det, n, k: 0, part: 0 }, a))
    }

    /// `m[r, c]` (`eval.rs`): ausserhalb ein `RangeFault`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn mat_element(
        &mut self,
        base: &Expr,
        row: &Expr,
        col: &Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
        span: Span,
    ) -> R<Term> {
        let Some((rows, cols)) = self.mat_dims(base.ty) else { return no("Index auf diesem Wert", span) };
        let items = self.mat_items(base, cx, env, flow)?;
        let (r, c) = (self.expr(row, cx, env, flow)?, self.expr(col, cx, env, flow)?);
        let outside = |x: &Term, n: u32| {
            Term::or(vec![
                Term::bin(Op::Lt, x.clone(), Term::int(0)),
                Term::bin(Op::Ge, x.clone(), Term::int(i64::from(n))),
            ])
        };
        self.fault_exit(FaultKind::Range, Term::or(vec![outside(&r, rows), outside(&c, cols)]), flow, span);
        let at = Term::bin(Op::Add, Term::bin(Op::Mul, r, Term::int(i64::from(cols))), c);
        Enc::select(items.into_iter().map(V::Leaf).collect(), &at).leaf(span)
    }

    /// Ein Fault-Ausgang, wo `fails` gilt; danach lebt der Pfad ohne ihn.
    pub(super) fn fault_exit(&mut self, kind: FaultKind, fails: Term, flow: &mut Flow, span: Span) {
        flow.exits.push(Exit {
            cond: Term::and(vec![flow.alive.clone(), fails.clone()]),
            kind: ExitKind::Fault(None, self.cause(kind, span)),
        });
        flow.alive = Term::and(vec![flow.alive.clone(), fails.not()]);
    }
}
