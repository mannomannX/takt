//! Oversampelte Inputs im Modell (8.9, `Image::through_edge`): je Tick ein
//! Array aus `N` Abtastwerten, die der Rand der Reihe nach prueft wie einen
//! skalaren Input; fuer das Array gilt die schlechteste Qualitaet, und
//! `Suspect` haelt das letzte gute Array. `Bad` fuehrt das Modell wie ein
//! `Bad` vom Treiber, das den Rand zuruecksetzt: Der Rand des Interpreters
//! zaehlt nach einem schlechten Abtastwert weiter, und das Modell laesst
//! danach hoechstens mehr zu.
//!
//! Eingaben je Tick: `i.<c>.d[j]` die gelieferten Abtastwerte, `i.<c>[j]`
//! das Array, das die Maschinen lesen, `i.<c>.v`, ob es einen Wert hat, dazu
//! Qualitaet und `held` wie bei einem skalaren Input.

use std::ops::Not;

use takt_diag::Span;
use takt_mir::ChannelId;

use super::value::V;
use super::{Cx, Edge, Enc, Env, Flow, R, quality};
use crate::term::{Op, Sort, Term};

/// Der Rand nach den Abtastwerten eines Ticks.
struct Gate {
    /// Die schlechteste Qualitaet, bei gleicher die erste.
    worst: Term,
    /// Gab es bei ihr einen guten Wert? Nur dann haelt `Suspect` ein Array.
    worst_held: Term,
    has: Term,
    good: Term,
    strikes: Term,
    /// Ein guter Abtastwert in diesem Tick; danach misst `max_slew` nicht
    /// mehr, denn der Zeitstempel ist derselbe.
    fresh: Term,
}

impl Enc<'_> {
    /// Die Abtastwerte `i.<c><part>[j]` eines Ticks.
    fn sample_items(&mut self, edge: &Edge, part: &str) -> Vec<Term> {
        (0..edge.samples.unwrap_or(0)).map(|j| self.input(format!("i.{}{part}[{j}]", edge.name), edge.sort)).collect()
    }

    fn sample_valued(&mut self, edge: &Edge) -> Term {
        self.input(format!("i.{}.v", edge.name), Sort::Bool)
    }

    /// `through_edge` ueber die gelieferten Abtastwerte `d`, der Reihe nach
    /// wie `Gate::check`; `get` liefert den Rand davor.
    fn gate(&mut self, edge: &Edge, d: &[Term], get: &dyn Fn(&str, Sort) -> Term) -> Gate {
        let state = |part: &str, sort, none: Term| if edge.stateful() { get(part, sort) } else { none };
        let mut g = Gate {
            worst: Term::int(quality::GOOD),
            worst_held: Term::bool(false),
            has: state("has", Sort::Bool, Term::bool(false)),
            good: state("good", edge.sort, Enc::zero(edge.sort)),
            strikes: state("strikes", Sort::Int, Term::int(0)),
            fresh: Term::bool(false),
        };
        let gap = if edge.slew.is_some() { get("gap", Sort::Int) } else { Term::int(1) };
        for x in d {
            let mut bad: Vec<Term> = edge.inside(x).into_iter().map(Not::not).collect();
            if edge.slew.is_some() {
                let within = self.slew_within(edge, Enc::slew_diff(x, &g.good), gap.clone());
                bad.push(Term::and(vec![g.has.clone(), g.fresh.clone().not(), within.not()]));
            }
            let bad = Term::or(bad);
            let strikes = Term::ite(bad.clone(), Term::bin(Op::Add, g.strikes.clone(), Term::int(1)), Term::int(0));
            let suspect = Term::bin(Op::Le, strikes.clone(), Term::int(i64::from(edge.debounce)));
            let verdict = Term::ite(
                bad.clone(),
                Term::ite(suspect.clone(), Term::int(quality::SUSPECT), Term::int(quality::BAD)),
                Term::int(quality::GOOD),
            );
            // Die Codes steigen mit der Schwere: Good, Suspect, Bad.
            let worse = Term::bin(Op::Gt, verdict.clone(), g.worst.clone());
            g.worst_held = Term::ite(worse.clone(), g.has.clone(), g.worst_held);
            g.worst = Term::ite(worse, verdict, g.worst);
            g.has = Term::ite(bad.clone(), Term::and(vec![suspect, g.has.clone()]), Term::bool(true));
            g.good = Term::ite(bad.clone(), g.good, x.clone());
            g.fresh = Term::or(vec![g.fresh, bad.not()]);
            g.strikes = strikes;
        }
        g
    }

    /// Was der Rand ueber das Array eines Ticks zusichert: Gehalten ist es
    /// das vorige; geliefert mit Werten ist seine Qualitaet die des Randes,
    /// `Good` zeigt die Lieferung, `Suspect` das letzte gute Array; ohne
    /// Werte ist nichts zu sehen.
    pub(super) fn samples_assumptions(&mut self, edge: &Edge, out: &mut Vec<Term>) {
        let q = self.quality(edge);
        let held = self.held(edge);
        let v = self.sample_valued(edge);
        let x = self.sample_items(edge, "");
        let d = self.sample_items(edge, ".d");
        let prev = |part: &str, sort| Term::var(edge.loc(&format!("{part}.prev")), sort);
        let implies = |a: Term, b: Term| Term::or(vec![a.not(), b]);
        let zero =
            |items: &[Term]| Term::and(items.iter().map(|t| Term::eq(t.clone(), Enc::zero(edge.sort))).collect());
        out.push(Term::and(vec![
            Term::bin(Op::Ge, q.clone(), Term::int(quality::GOOD)),
            Term::bin(Op::Le, q.clone(), Term::int(quality::BAD)),
        ]));
        let mut kept = vec![Term::eq(q.clone(), self.kept_quality(edge)), Term::eq(v.clone(), prev("v", Sort::Bool))];
        kept.extend(x.iter().enumerate().map(|(j, t)| Term::eq(t.clone(), prev(&format!("x[{j}]"), edge.sort))));
        out.push(implies(held.clone(), Term::and(kept)));
        let is = |code| Term::and(vec![held.clone().not(), Term::eq(q.clone(), Term::int(code))]);
        let valued = Term::or(vec![is(quality::GOOD), is(quality::SUSPECT)]);
        out.push(implies(valued.clone().not(), zero(&d)));
        out.push(implies(
            Term::or(vec![is(quality::STALE), is(quality::BAD)]),
            Term::and(vec![v.clone().not(), zero(&x)]),
        ));
        // 4.1: NaN und Unendlich gibt es in der Sprache nicht.
        if matches!(edge.sort, Sort::F32 | Sort::F64) {
            let finite = d.iter().map(|t| Term::app(Op::IsFinite, vec![t.clone()])).collect();
            out.push(implies(valued.clone(), Term::and(finite)));
        }
        let gate = self.gate(edge, &d, &|part, sort| prev(part, sort));
        out.push(implies(valued, Term::eq(q.clone(), gate.worst)));
        let mut good = vec![v.clone()];
        good.extend(x.iter().zip(&d).map(|(a, b)| Term::eq(a.clone(), b.clone())));
        out.push(implies(is(quality::GOOD), Term::and(good)));
        if edge.suspect() {
            let present = Term::and(vec![gate.worst_held, prev("last.has", Sort::Bool)]);
            let mut suspect = vec![Term::eq(v.clone(), present.clone())];
            suspect.extend(x.iter().enumerate().map(|(j, t)| {
                let last = Term::ite(present.clone(), prev(&format!("last[{j}]"), edge.sort), Enc::zero(edge.sort));
                Term::eq(t.clone(), last)
            }));
            out.push(implies(is(quality::SUSPECT), Term::and(suspect)));
        }
    }

    /// Der Rand und das Array nach diesem Tick; `pre` traegt sie davor.
    pub(super) fn samples_next(&mut self, edge: &Edge, pre: &Env) -> Vec<(String, Term)> {
        let q = self.quality(edge);
        let held = self.held(edge);
        let v = self.sample_valued(edge);
        let x = self.sample_items(edge, "");
        let d = self.sample_items(edge, ".d");
        let get = |part: &str| pre[&edge.loc(part)].clone();
        let mut next: Vec<(String, Term)> = x.iter().enumerate().map(|(j, t)| (format!("x[{j}]"), t.clone())).collect();
        next.push(("v".into(), v));
        next.push(("q".into(), q.clone()));
        if edge.fresh_ticks.is_some() {
            next.push((
                "age".into(),
                Term::ite(held.clone(), Term::bin(Op::Add, get("age"), Term::int(1)), Term::int(0)),
            ));
        }
        let is = |code| Term::and(vec![held.clone().not(), Term::eq(q.clone(), Term::int(code))]);
        let valued = Term::or(vec![is(quality::GOOD), is(quality::SUSPECT)]);
        if edge.stateful() {
            let gate = self.gate(edge, &d, &|part, _| get(part));
            let bad = is(quality::BAD);
            let has = Term::ite(valued.clone(), gate.has, Term::and(vec![bad.clone().not(), get("has")]));
            let strikes = Term::ite(valued.clone(), gate.strikes, Term::ite(bad, Term::int(0), get("strikes")));
            next.push(("has".into(), has));
            next.push(("good".into(), Term::ite(valued.clone(), gate.good, get("good"))));
            next.push(("strikes".into(), strikes));
            if edge.slew.is_some() {
                let fresh = Term::and(vec![valued, gate.fresh]);
                next.push(("gap".into(), Term::ite(fresh, Term::int(1), Term::bin(Op::Add, get("gap"), Term::int(1)))));
            }
        }
        if edge.suspect() {
            let (good, bad) = (is(quality::GOOD), is(quality::BAD));
            let has = Term::ite(good.clone(), Term::bool(true), Term::and(vec![bad.not(), get("last.has")]));
            next.push(("last.has".into(), has));
            for (j, t) in d.iter().enumerate() {
                let at = format!("last[{j}]");
                next.push((at.clone(), Term::ite(good.clone(), t.clone(), get(&at))));
            }
        }
        next
    }

    /// Lesbar ist das Array, wenn es `Good` ist oder `Suspect` mit Wert.
    pub(super) fn samples_readable(&mut self, edge: &Edge) -> Term {
        let q = self.quality(edge);
        let v = self.sample_valued(edge);
        Term::or(vec![
            Term::eq(q.clone(), Term::int(quality::GOOD)),
            Term::and(vec![Term::eq(q, Term::int(quality::SUSPECT)), v]),
        ])
    }

    /// Das Array, das die Maschinen lesen; ist es nicht lesbar, faultet das
    /// Lesen mit `SensorFault` (3.5).
    pub(super) fn samples_value(&mut self, c: ChannelId, cx: &Cx<'_>, flow: &mut Flow, span: Span) -> R<V> {
        let edge = self.edge_of(c)?;
        self.require_readable(&edge, cx, flow, span);
        Ok(V::Node(self.sample_items(&edge, "").into_iter().map(V::Leaf).collect()))
    }

    /// `x.or(d)` (3.5): der Ersatz nur, wo das Array nicht lesbar ist.
    pub(super) fn samples_or(
        &mut self,
        c: ChannelId,
        default: &super::Expr,
        cx: &Cx<'_>,
        env: &Env,
        flow: &mut Flow,
    ) -> R<V> {
        let edge = self.edge_of(c)?;
        let ok = self.readable(&edge, cx.pre);
        let items = V::Node(self.sample_items(&edge, "").into_iter().map(V::Leaf).collect());
        let d = self.guarded(&ok.clone().not(), flow, |enc, flow| enc.value(default, cx, env, flow))?;
        Ok(V::ite(&ok, items, d))
    }
}
