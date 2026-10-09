//! Inputs zusammengesetzter Typen im Modell (3.5, `Image::through_edge`):
//! je Blatt des Typs eine Eingabe. Der Rand prueft an ihnen weder Range noch
//! `max_slew` (`limits_of` kennt sie nur fuer Zahlen), also ist eine
//! Lieferung nie `Suspect`; eine gute haelt die Invarianten ihres Typs, eine
//! ungueltige ist null, und eine gehaltene ist die vorige wie bei einem
//! skalaren Input.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::ChannelId;

use super::value::V;
use super::{Cx, Edge, Enc, Env, Flow, R, no, quality};
use crate::term::{Op, Term};

impl Enc<'_> {
    /// Der Wert, wie die Maschinen ihn lesen: die Blaetter `i.<c><pfad>`.
    fn composite_items(&mut self, edge: &Edge, span: Span) -> R<V> {
        let Some((ty, _)) = edge.composite.clone() else { return no("Input ohne Blaetter", span) };
        let shape = self.shape(ty, span)?;
        self.gather(&format!("i.{}", edge.name), &shape, &mut |enc, loc, s| {
            let sort = enc.leaf_sort(s, span)?;
            Ok(enc.input(loc.to_string(), sort))
        })
    }

    /// Was der Rand ueber eine Lieferung zusichert.
    pub(super) fn composite_assumptions(&mut self, edge: &Edge, out: &mut Vec<Term>) -> R<()> {
        let span = Span::default();
        let Some((ty, leaves)) = edge.composite.clone() else { return no("Input ohne Blaetter", span) };
        let q = self.quality(edge);
        let held = self.held(edge);
        let implies = |a: Term, b: Term| Term::or(vec![a.not(), b]);
        out.push(Term::and(vec![
            Term::bin(Op::Ge, q.clone(), Term::int(quality::GOOD)),
            Term::bin(Op::Le, q.clone(), Term::int(quality::BAD)),
        ]));
        out.push(Term::eq(q.clone(), Term::int(quality::SUSPECT)).not());
        let x: Vec<Term> =
            leaves.iter().map(|(path, sort)| self.input(format!("i.{}{path}", edge.name), *sort)).collect();
        let prev = |path: &str, sort| Term::var(edge.loc(&format!("x{path}.prev")), sort);
        // Eine gehaltene Abtastung ist die vorige (`age_inputs`).
        let mut kept = vec![Term::eq(q.clone(), self.kept_quality(edge))];
        kept.extend(x.iter().zip(&leaves).map(|(t, (path, sort))| Term::eq(t.clone(), prev(path, *sort))));
        out.push(implies(held.clone(), Term::and(kept)));
        let good = Term::and(vec![held.clone().not(), Term::eq(q.clone(), Term::int(quality::GOOD))]);
        let invalid = Term::and(vec![held.not(), good.clone().not()]);
        let zero = x.iter().map(|t| Term::eq(t.clone(), Enc::zero(t.sort()))).collect();
        out.push(implies(invalid, Term::and(zero)));
        // Eine gute Lieferung ist ein Wert ihres Typs (3.4): Varianten,
        // Breiten, Laengen.
        let shape = self.shape(ty, span)?;
        let mut locs = Vec::new();
        Enc::leaf_locs("", &shape, &mut locs);
        let valid: Vec<Term> =
            x.iter().zip(&locs).filter_map(|(t, (_, s))| self.leaf_invariant(t.clone(), s)).collect();
        if !valid.is_empty() {
            out.push(implies(good, Term::and(valid)));
        }
        Ok(())
    }

    /// Die Abtastung nach diesem Tick: ihre Blaetter, die Qualitaet, das Alter.
    pub(super) fn composite_next(&mut self, edge: &Edge, pre: &Env) -> R<Vec<(String, Term)>> {
        let Some((_, leaves)) = edge.composite.clone() else { return no("Input ohne Blaetter", Span::default()) };
        let q = self.quality(edge);
        let held = self.held(edge);
        let mut next: Vec<(String, Term)> = leaves
            .iter()
            .map(|(path, sort)| (format!("x{path}"), self.input(format!("i.{}{path}", edge.name), *sort)))
            .collect();
        next.push(("q".into(), q));
        if edge.fresh_ticks.is_some() {
            let age = pre[&edge.loc("age")].clone();
            next.push(("age".into(), Term::ite(held, Term::bin(Op::Add, age, Term::int(1)), Term::int(0))));
        }
        Ok(next)
    }

    /// Lesbar ist nur ein guter Wert: `Suspect` gibt es hier nicht.
    pub(super) fn composite_readable(&mut self, edge: &Edge) -> Term {
        Term::eq(self.quality(edge), Term::int(quality::GOOD))
    }

    /// Der Wert eines zusammengesetzten Inputs oder Arrays aus Abtastwerten;
    /// ist er nicht lesbar, faultet das Lesen mit `SensorFault` (3.5).
    pub(super) fn input_value(&mut self, c: ChannelId, cx: &Cx<'_>, flow: &mut Flow, span: Span) -> R<V> {
        let edge = self.edge_of(c)?;
        self.require_readable(&edge, cx, flow, span);
        self.input_items(&edge, span)
    }

    fn input_items(&mut self, edge: &Edge, span: Span) -> R<V> {
        match edge.samples {
            Some(_) => Ok(self.sample_array(edge)),
            None => self.composite_items(edge, span),
        }
    }

    /// `x.or(d)` (3.5): der Ersatz nur, wo der Input nicht lesbar ist.
    pub(super) fn input_or(
        &mut self,
        c: ChannelId,
        default: &super::Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<V> {
        let edge = self.edge_of(c)?;
        let ok = self.readable(&edge, cx.pre);
        let items = self.input_items(&edge, default.span)?;
        let d = self.guarded(&ok.clone().not(), flow, |enc, flow| enc.value(default, cx, env, flow))?;
        Ok(V::ite(&ok, items, d))
    }
}
