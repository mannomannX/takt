//! `property` und `assumption` (13.3): eine beschraenkte Temporallogik
//! ueber Tick-Rand-Snapshots. Pruefung 56: Fenster positiv und ein
//! Vielfaches des Ticks, `always`/`never` nur ausserhalb der beschraenkten
//! Operatoren (sonst waere die Formel nicht ueberwachbar), Atome `bool`.
//! Eine Eigenschaft liest nur — Ausdruecke haben keine Seiteneffekte (4.4).

use takt_mir::expr::{TProp, TemporalOp};
use takt_mir::program::Property;
use takt_syntax::ast;

use super::{Lowerer, SC2};
use crate::checks::SC56;

impl Lowerer<'_> {
    /// `property p: …` / `assumption a: …`, nach den Maschinen, weil die
    /// Formel ihre Zustaende und `pub var`s liest.
    pub fn property_decl(&mut self, decl: &ast::PropertyDecl) {
        if self.program.properties.iter().any(|p| p.name == decl.name.name) {
            self.error(SC2, decl.name.span, format!("`{}` ist schon definiert", decl.name.name));
            return;
        }
        let Some(formula) = self.tprop(&decl.prop, true) else { return };
        self.program.properties.push(Property {
            name: decl.name.name.clone(),
            formula,
            monitor: decl.monitor,
            assumption: decl.kind == ast::PropertyKind::Assumption,
            span: decl.span,
        });
    }

    /// `unbounded`: nicht unter einem beschraenkten Operator.
    fn tprop(&mut self, e: &ast::Expr, unbounded: bool) -> Option<TProp> {
        match &e.kind {
            ast::ExprKind::Temporal { op, window, inner } => {
                let op = match op {
                    ast::TemporalOp::Always => TemporalOp::Always,
                    ast::TemporalOp::Never => TemporalOp::Never,
                    ast::TemporalOp::Eventually => TemporalOp::Eventually,
                    ast::TemporalOp::Stable => TemporalOp::Stable,
                    ast::TemporalOp::Once => TemporalOp::Once,
                };
                let bounded = matches!(op, TemporalOp::Eventually | TemporalOp::Stable | TemporalOp::Once);
                if !bounded && !unbounded {
                    self.error(SC56, e.span, "`always`/`never` nicht unter einem beschraenkten Operator (13.3)");
                    return None;
                }
                let window = match window {
                    Some(d) => Some(self.window(d)?),
                    None => None,
                };
                let inner = self.tprop(inner, unbounded && !bounded)?;
                Some(TProp::Temporal { op, window, inner: Box::new(inner) })
            }
            ast::ExprKind::Implies { lhs, rhs } => {
                let (a, b) = (self.tprop(lhs, unbounded)?, self.tprop(rhs, unbounded)?);
                Some(TProp::Implies(Box::new(a), Box::new(b)))
            }
            ast::ExprKind::Binary { op: ast::BinaryOp::And, lhs, rhs } => {
                let (a, b) = (self.tprop(lhs, unbounded)?, self.tprop(rhs, unbounded)?);
                Some(TProp::And(Box::new(a), Box::new(b)))
            }
            ast::ExprKind::Binary { op: ast::BinaryOp::Or, lhs, rhs } => {
                let (a, b) = (self.tprop(lhs, unbounded)?, self.tprop(rhs, unbounded)?);
                Some(TProp::Or(Box::new(a), Box::new(b)))
            }
            ast::ExprKind::Unary { op: ast::UnaryOp::Not, expr } => {
                Some(TProp::Not(Box::new(self.tprop(expr, unbounded)?)))
            }
            _ => {
                let bool = self.tys.bool;
                Some(TProp::Atom(self.check(e, bool)?))
            }
        }
    }

    /// Ein Fenster `[d]` in Nanosekunden (Pruefung 56).
    fn window(&mut self, d: &ast::DurationLit) -> Option<i64> {
        let tick = self.program.config.tick;
        if d.ns <= 0 || d.ns % tick != 0 {
            self.error(
                SC56,
                d.span,
                format!(
                    "Fenster {} muss ein positives Vielfaches des Ticks {} sein (13.3)",
                    takt_mir::dump::duration(d.ns),
                    takt_mir::dump::duration(tick)
                ),
            );
            return None;
        }
        Some(d.ns)
    }
}
