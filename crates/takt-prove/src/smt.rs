//! SMT-LIB2-Ausgabe (plan/m6.md 2.8): Zustand und Eingaben je Schritt als
//! Konstanten, jeder innere Term einmal als `define-fun` (der Graph bleibt
//! ein Graph), dazu je Eigenschaft die BMC-Anfrage bis zur Tiefe und der
//! Induktionsschritt ohne Anfangszustand (k-Induktion mit k = Tiefe). Eine
//! Pruefstelle (`check`, B3) ist ein Ziel ueber den Uebergaengen: Sie
//! feuert nie.

use std::collections::HashMap;
use std::fmt::Write;
use std::rc::Rc;

use crate::encode::Model;
use crate::term::{Node, Op, Sort, Term};

fn sort_text(s: Sort) -> &'static str {
    match s {
        Sort::Bool => "Bool",
        Sort::Int => "(_ BitVec 64)",
        Sort::F32 => "Float32",
        Sort::F64 => "Float64",
    }
}

/// Ein Variablenname mit Schritt: `|s.m.x@3|`.
pub fn at(name: &str, step: u32, tag: &str) -> String {
    format!("|{name}{tag}{step}|")
}

/// Bitmuster eines Fliesskommawerts als `(fp …)`.
fn fp_literal(bits: u64, exp: u32, mant: u32) -> String {
    let sign = (bits >> (exp + mant)) & 1;
    let e = (bits >> mant) & ((1u64 << exp) - 1);
    let m = bits & ((1u64 << mant) - 1);
    format!("(fp #b{sign} #b{e:0width_e$b} #b{m:0width_m$b})", width_e = exp as usize, width_m = mant as usize)
}

/// Der Drucker: benennt Knoten je (Term, Zustandsschritt, Eingabeschritt).
struct Printer<'a> {
    out: &'a mut String,
    tag: &'static str,
    defs: HashMap<(usize, u32, u32), String>,
    next: usize,
}

impl Printer<'_> {
    /// Der Name eines Terms; innere Knoten bekommen ein `define-fun`.
    fn name(&mut self, t: &Term, state: u32, input: u32) -> String {
        match &*t.0 {
            Node::Bool(b) => b.to_string(),
            Node::Int(i) => {
                if *i >= 0 {
                    format!("(_ bv{i} 64)")
                } else {
                    format!("(bvneg (_ bv{} 64))", i.unsigned_abs())
                }
            }
            Node::F32(f) => fp_literal(u64::from(f.to_bits()), 8, 23),
            Node::F64(f) => fp_literal(f.to_bits(), 11, 52),
            Node::Var(v, _) => {
                if v.starts_with("i.") {
                    at(v, input, self.tag)
                } else {
                    at(v, state, self.tag)
                }
            }
            Node::App(op, args) => {
                let key = (Rc::as_ptr(&t.0) as usize, state, input);
                if let Some(n) = self.defs.get(&key) {
                    return n.clone();
                }
                let parts: Vec<String> = args.iter().map(|a| self.name(a, state, input)).collect();
                let body = app_text(*op, &parts);
                let n = format!("|d{}{}|", self.tag, self.next);
                self.next += 1;
                let _ = writeln!(self.out, "(define-fun {n} () {} {body})", sort_text(t.sort()));
                self.defs.insert(key, n.clone());
                n
            }
        }
    }
}

fn app_text(op: Op, a: &[String]) -> String {
    let rm = "RNE";
    match op {
        Op::Not => format!("(not {})", a[0]),
        Op::And => format!("(and {})", a.join(" ")),
        Op::Or => format!("(or {})", a.join(" ")),
        Op::Eq => format!("(= {} {})", a[0], a[1]),
        Op::Ite => format!("(ite {} {} {})", a[0], a[1], a[2]),
        Op::Neg => format!("(bvneg {})", a[0]),
        Op::Add => format!("(bvadd {} {})", a[0], a[1]),
        Op::Sub => format!("(bvsub {} {})", a[0], a[1]),
        Op::Mul => format!("(bvmul {} {})", a[0], a[1]),
        Op::Div => format!("(bvsdiv {} {})", a[0], a[1]),
        Op::Rem => format!("(bvsrem {} {})", a[0], a[1]),
        Op::Lt => format!("(bvslt {} {})", a[0], a[1]),
        Op::Le => format!("(bvsle {} {})", a[0], a[1]),
        Op::Gt => format!("(bvsgt {} {})", a[0], a[1]),
        Op::Ge => format!("(bvsge {} {})", a[0], a[1]),
        Op::BitAnd => format!("(bvand {} {})", a[0], a[1]),
        Op::BitOr => format!("(bvor {} {})", a[0], a[1]),
        Op::BitXor => format!("(bvxor {} {})", a[0], a[1]),
        Op::Shl => format!("(bvshl {} {})", a[0], a[1]),
        Op::Shr => format!("(bvashr {} {})", a[0], a[1]),
        Op::FNeg => format!("(fp.neg {})", a[0]),
        Op::FAbs => format!("(fp.abs {})", a[0]),
        Op::FSqrt => format!("(fp.sqrt {rm} {})", a[0]),
        Op::FAdd => format!("(fp.add {rm} {} {})", a[0], a[1]),
        Op::FSub => format!("(fp.sub {rm} {} {})", a[0], a[1]),
        Op::FMul => format!("(fp.mul {rm} {} {})", a[0], a[1]),
        Op::FDiv => format!("(fp.div {rm} {} {})", a[0], a[1]),
        Op::FFma => format!("(fp.fma {rm} {} {} {})", a[0], a[1], a[2]),
        Op::FLt => format!("(fp.lt {} {})", a[0], a[1]),
        Op::FLe => format!("(fp.leq {} {})", a[0], a[1]),
        Op::FGt => format!("(fp.gt {} {})", a[0], a[1]),
        Op::FGe => format!("(fp.geq {} {})", a[0], a[1]),
        Op::FEq => format!("(fp.eq {} {})", a[0], a[1]),
        Op::ToF32 => format!("((_ to_fp 8 24) {rm} {})", a[0]),
        Op::ToF64 => format!("((_ to_fp 11 53) {rm} {})", a[0]),
        Op::IsFinite => format!("(not (or (fp.isNaN {0}) (fp.isInfinite {0})))", a[0]),
    }
}

/// Die Anfrage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// Anfangszustand, `depth` Schritte, das Ziel irgendwo verletzt.
    Bmc,
    /// Beliebiger Zustand, das Ziel `depth` Schritte lang gueltig, im
    /// naechsten verletzt.
    Induction,
}

/// Was geprueft wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Eine Eigenschaft (Index in `Model::properties`).
    Property(usize),
    /// Eine Pruefstelle (Index in `Model::checks`).
    Check(usize),
}

/// Ein Block mit Deklarationen, Uebergaengen und den Anfragen der Ziele.
fn block(
    out: &mut String,
    model: &Model,
    tag: &'static str,
    kind: Query,
    depth: u32,
    targets: &[Target],
    values: bool,
) {
    let steps = depth;
    let _ = writeln!(out, "(push 1)");
    let _ = writeln!(out, "; {}", if kind == Query::Induction { "Induktionsschritt" } else { "BMC" });
    for k in 0..=steps {
        for v in &model.state {
            let _ = writeln!(out, "(declare-const {} {})", at(&v.name, k, tag), sort_text(v.sort));
        }
        for (name, sort) in &model.inputs {
            let _ = writeln!(out, "(declare-const {} {})", at(name, k, tag), sort_text(*sort));
        }
    }
    let mut p = Printer { out, tag, defs: HashMap::new(), next: 0 };
    if kind == Query::Bmc {
        for v in &model.state {
            let init = p.name(&v.init, 0, 0);
            let _ = writeln!(p.out, "(assert (= {} {init}))", at(&v.name, 0, tag));
        }
    }
    for k in 0..steps {
        for v in &model.state {
            let next = p.name(&v.next, k, k + 1);
            let _ = writeln!(p.out, "(assert (= {} {next}))", at(&v.name, k + 1, tag));
        }
    }
    for k in 0..=steps {
        for a in model.assumptions.iter().chain(&model.invariants) {
            let t = p.name(a, k, k);
            let _ = writeln!(p.out, "(assert {t})");
        }
    }
    for &target in targets {
        // `holds[k]`: das Ziel gilt an Position k — eine Eigenschaft ueber
        // Zustand und Eingaben des Ticks, eine Pruefstelle ueber dem
        // Uebergang k → k+1 (an Position 0 ueber dem Anfangszustand).
        let (label, holds): (String, Vec<String>) = match target {
            Target::Property(i) => {
                let prop = &model.properties[i];
                let word = if prop.assumption { "Annahme" } else { "Eigenschaft" };
                (format!("{word} `{}`", prop.name), (0..=steps).map(|k| p.name(&prop.formula, k, k)).collect())
            }
            Target::Check(i) => {
                let site = &model.checks[i];
                let mut holds = Vec::new();
                if kind == Query::Bmc {
                    let fires = p.name(&site.init, 0, 0);
                    holds.push(format!("(not {fires})"));
                }
                for k in 0..steps {
                    let fires = p.name(&site.fires, k, k + 1);
                    holds.push(format!("(not {fires})"));
                }
                (format!("Pruefstelle `{}` @{}", site.kind, site.start), holds)
            }
        };
        let _ = writeln!(p.out, "(push 1)");
        let _ = writeln!(p.out, "; {label}");
        if holds.is_empty() {
            let _ = writeln!(p.out, "(assert false)");
        } else if kind == Query::Induction {
            for h in &holds[..holds.len() - 1] {
                let _ = writeln!(p.out, "(assert {h})");
            }
            let _ = writeln!(p.out, "(assert (not {}))", holds[holds.len() - 1]);
        } else {
            let _ = writeln!(p.out, "(assert (not (and {})))", holds.join(" "));
        }
        let _ = writeln!(p.out, "(check-sat)");
        if values {
            let names: Vec<String> =
                (0..=steps).flat_map(|k| model.inputs.iter().map(move |(n, _)| at(n, k, tag))).collect();
            if !names.is_empty() {
                let _ = writeln!(p.out, "(get-value ({}))", names.join(" "));
            }
        }
        let _ = writeln!(p.out, "(pop 1)");
    }
    let _ = writeln!(out, "(pop 1)");
}

fn head(model: &Model) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; takt prove (Referenz 13.3, plan/m6.md 2.8)");
    let _ = writeln!(out, "; Zustand `s.…`, Eingaben `i.…`, Schritt hinter `@` (BMC) bzw. `#` (Induktion).");
    for n in &model.notes {
        let _ = writeln!(out, "; Reichweite: {n}");
    }
    let _ = writeln!(out, "(set-logic ALL)");
    out
}

/// Schreibt das Modell bis zur Tiefe `depth`: BMC und Induktionsschritt,
/// je Eigenschaft und Pruefstelle eine `check-sat`-Anfrage.
pub fn export(model: &Model, depth: u32) -> String {
    let mut out = head(model);
    let all: Vec<Target> =
        (0..model.properties.len()).map(Target::Property).chain((0..model.checks.len()).map(Target::Check)).collect();
    let _ = writeln!(out);
    block(&mut out, model, "@", Query::Bmc, depth, &all, false);
    let _ = writeln!(out);
    block(&mut out, model, "#", Query::Induction, depth, &all, false);
    out
}

/// Die Anfrage eines Ziels fuer den Solver; BMC fragt nach den Eingaben
/// des Gegenbeispiels.
pub fn query(model: &Model, depth: u32, target: Target, kind: Query) -> String {
    let mut out = head(model);
    let (tag, values) = match kind {
        Query::Bmc => ("@", true),
        Query::Induction => ("#", false),
    };
    block(&mut out, model, tag, kind, depth, &[target], values);
    out
}
