//! SMT-LIB2-Ausgabe (plan/m6.md 2.8): Zustand und Eingaben je Schritt als
//! Konstanten, jeder innere Term einmal als `define-fun` (der Graph bleibt
//! ein Graph), dazu zwei Anfragen je Eigenschaft — BMC bis zur Tiefe und
//! der Induktionsschritt ohne Anfangszustand.

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
fn at(name: &str, step: u32, tag: &str) -> String {
    format!("|{name}{tag}{step}|")
}

/// Bitmuster eines `f64` als `(fp …)`.
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

/// Schreibt das Modell bis zur Tiefe `depth`: BMC (mit Anfangszustand) und
/// den Induktionsschritt (ohne), je Eigenschaft eine `check-sat`-Anfrage.
pub fn export(model: &Model, depth: u32) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; takt prove --export (Referenz 13.3, plan/m6.md 2.8)");
    let _ = writeln!(out, "; Zustand `s.…`, Eingaben `i.…`, Schritt hinter `@` (BMC) bzw. `#` (Induktion).");
    for n in &model.notes {
        let _ = writeln!(out, "; Reichweite: {n}");
    }
    let _ = writeln!(out, "(set-logic ALL)");
    for (tag, induction) in [("@", false), ("#", true)] {
        let _ = writeln!(out, "\n(push 1)");
        let _ = writeln!(out, "; {}", if induction { "Induktionsschritt" } else { "BMC" });
        let steps = if induction { depth + 1 } else { depth };
        for k in 0..=steps {
            for v in &model.state {
                let _ = writeln!(out, "(declare-const {} {})", at(&v.name, k, tag), sort_text(v.sort));
            }
            for (name, sort) in &model.inputs {
                let _ = writeln!(out, "(declare-const {} {})", at(name, k, tag), sort_text(*sort));
            }
        }
        let mut p = Printer { out: &mut out, tag, defs: HashMap::new(), next: 0 };
        if !induction {
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
            for a in &model.assumptions {
                let t = p.name(a, k, k);
                let _ = writeln!(p.out, "(assert {t})");
            }
        }
        for prop in &model.properties {
            let holds: Vec<String> = (0..=steps).map(|k| p.name(&prop.formula, k, k)).collect();
            let _ = writeln!(p.out, "(push 1)");
            let _ = writeln!(p.out, "; {} `{}`", if prop.assumption { "Annahme" } else { "Eigenschaft" }, prop.name);
            if induction {
                for h in &holds[..steps as usize] {
                    let _ = writeln!(p.out, "(assert {h})");
                }
                let _ = writeln!(p.out, "(assert (not {}))", holds[steps as usize]);
            } else {
                let _ = writeln!(p.out, "(assert (not (and {})))", holds.join(" "));
            }
            let _ = writeln!(p.out, "(check-sat)");
            let _ = writeln!(p.out, "(pop 1)");
        }
        let _ = writeln!(out, "(pop 1)");
    }
    out
}
