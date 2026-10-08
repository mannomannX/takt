//! SMT-LIB2-Ausgabe (plan/m6.md 2.8): Zustand und Eingaben je Schritt als
//! Konstanten, jeder innere Term einmal als `define-fun` (der Graph bleibt
//! ein Graph), dazu je Eigenschaft die BMC-Anfrage bis zur Tiefe und der
//! Induktionsschritt ohne Anfangszustand (k-Induktion mit k = Tiefe). Eine
//! Pruefstelle (`check`, B3) ist ein Ziel ueber den Uebergaengen: Sie
//! feuert nie.
//!
//! Eine Anfrage traegt nur den Kegel ihrer Ziele (cone of influence): die
//! Variablen, von denen sie ueber Anfangswerte, Uebergaenge und Annahmen
//! abhaengen. Was draussen liegt, faellt mit seinen Bedingungen weg. Das
//! nimmt nur Bedingungen fort — ein `unsat` gilt darum fuer das ganze
//! Modell, ein `sat` bestaetigt der Interpreter. Ein Output, den das Ziel
//! nicht liest, kostete sonst seine ganze Rechnung, `sqrt` in `Float64`
//! etwa Minuten.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write;
use std::rc::Rc;

use crate::encode::{Goal, Model, StateVar};
use crate::term::{Fun, Node, Op, Rounding, Sort, Term};

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
    /// Die Funktionen aus `libtaktm`, die schon deklariert sind.
    declared: BTreeSet<String>,
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
                if v.starts_with("c.") {
                    format!("|{v}|")
                } else if v.starts_with("i.") {
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
                let sort = t.sort();
                let body = match op {
                    Op::Math(f) => {
                        let fun = format!("|math.{}.{}|", f.name(), sort_text(sort).replace(['(', ')', ' ', '_'], ""));
                        if self.declared.insert(fun.clone()) {
                            let domain = vec![sort_text(sort); parts.len()].join(" ");
                            let _ = writeln!(self.out, "(declare-fun {fun} ({domain}) {})", sort_text(sort));
                        }
                        format!("({fun} {})", parts.join(" "))
                    }
                    Op::Scale { num, den } => {
                        let (e, s) = if sort == Sort::F32 { (8, 24) } else { (11, 53) };
                        format!("((_ to_fp {e} {s}) RNE (/ (* (fp.to_real {}) {num}.0) {den}.0))", parts[0])
                    }
                    _ => app_text(*op, &parts),
                };
                let n = format!("|d{}{}|", self.tag, self.next);
                self.next += 1;
                let _ = writeln!(self.out, "(define-fun {n} () {} {body})", sort_text(sort));
                if let Op::Math(f) = op {
                    let _ = writeln!(self.out, "(assert {})", math_bound(*f, &n, sort));
                }
                self.defs.insert(key, n.clone());
                n
            }
        }
    }
}

/// Was der Wertebereich einer Funktion aus `libtaktm` zusichert: Sie ist
/// eine Funktion (der Solver sieht sie uninterpretiert), und ihr Wert liegt
/// in diesen Schranken — kein NaN, Sinus und Kosinus in [-1, 1], die
/// Umkehrfunktionen in einem Intervall um ihren Bildbereich, `exp` nicht
/// negativ.
fn math_bound(f: Fun, n: &str, sort: Sort) -> String {
    let lit = |x: f64| match sort {
        Sort::F32 => fp_literal(u64::from((x as f32).to_bits()), 8, 23),
        _ => fp_literal(x.to_bits(), 11, 52),
    };
    let within = |lo: f64, hi: f64| format!("(and (fp.leq {} {n}) (fp.leq {n} {}))", lit(lo), lit(hi));
    match f {
        Fun::Sin | Fun::Cos => within(-1.0, 1.0),
        Fun::Asin | Fun::Atan => within(-2.0, 2.0),
        Fun::Acos => within(0.0, 4.0),
        Fun::Atan2 => within(-4.0, 4.0),
        Fun::Exp => format!("(fp.leq {} {n})", lit(0.0)),
        Fun::Tan | Fun::Log | Fun::Pow => format!("(not (fp.isNaN {n}))"),
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
        Op::Wrap { bits, .. } if bits >= 64 => a[0].clone(),
        Op::Wrap { bits, signed } => {
            let extend = if signed { "sign_extend" } else { "zero_extend" };
            format!("((_ {extend} {}) ((_ extract {} 0) {}))", 64 - bits, bits - 1, a[0])
        }
        // Gleiche Vorzeichen der Summanden, und die Summe hat das andere.
        Op::AddOverflows => format!(
            "(and (= (bvslt {0} (_ bv0 64)) (bvslt {1} (_ bv0 64))) \
             (not (= (bvslt (bvadd {0} {1}) (_ bv0 64)) (bvslt {0} (_ bv0 64)))))",
            a[0], a[1]
        ),
        Op::SubOverflows => format!(
            "(and (not (= (bvslt {0} (_ bv0 64)) (bvslt {1} (_ bv0 64)))) \
             (not (= (bvslt (bvsub {0} {1}) (_ bv0 64)) (bvslt {0} (_ bv0 64)))))",
            a[0], a[1]
        ),
        // In 128 Bit ist das Produkt exakt; passt es, ist es die Erweiterung
        // des Produkts in 64 Bit.
        Op::MulOverflows => format!(
            "(not (= ((_ sign_extend 64) (bvmul {0} {1})) (bvmul ((_ sign_extend 64) {0}) ((_ sign_extend 64) {1}))))",
            a[0], a[1]
        ),
        Op::Round(r) => {
            let mode = match r {
                Rounding::HalfAway => "RNA",
                Rounding::Down => "RTN",
                Rounding::Up => "RTP",
                Rounding::TowardZero => "RTZ",
            };
            format!("(fp.roundToIntegral {mode} {})", a[0])
        }
        Op::FloatToInt => format!("((_ fp.to_sbv 64) RTZ {})", a[0]),
        // Beide schreibt der Drucker selbst: Sie brauchen die Sorte.
        Op::Scale { .. } | Op::Math(_) => unreachable!("im Drucker behandelt"),
    }
}

/// Sammelt die Variablen eines Terms; geteilte Knoten einmal.
fn vars_of(t: &Term, out: &mut BTreeSet<String>, seen: &mut HashSet<usize>) {
    match &*t.0 {
        Node::Var(v, _) => {
            out.insert(v.clone());
        }
        Node::App(_, args) if seen.insert(Rc::as_ptr(&t.0) as usize) => {
            for a in args {
                vars_of(a, out, seen);
            }
        }
        _ => {}
    }
}

fn vars(t: &Term) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    vars_of(t, &mut out, &mut HashSet::new());
    out
}

/// Der Kegel der Ziele `roots`: die Variablen, von denen sie abhaengen, und
/// die Bedingungen, die ihn beruehren. Eine Annahme mit einer Variablen im
/// Kegel zieht ihre anderen hinein, denn sie schraenkt ihn ueber sie ein.
/// Eine Invariante folgt aus den Uebergaengen; sie bleibt nur, wenn sie
/// ganz im Kegel liegt, und fehlt sie, ist das Modell nur freier.
fn cone<'a>(
    model: &Model,
    roots: &[&Term],
    constraints: &[&'a Term],
    invariants: &[&'a Term],
) -> (BTreeSet<String>, Vec<&'a Term>, Vec<&'a Term>) {
    let mut inside: BTreeSet<String> = roots.iter().flat_map(|t| vars(t)).collect();
    let state: Vec<(&str, BTreeSet<String>)> = model
        .state
        .iter()
        .map(|v| (v.name.as_str(), vars(&v.init).into_iter().chain(vars(&v.next)).collect()))
        .collect();
    let bound: Vec<BTreeSet<String>> = constraints.iter().map(|t| vars(t)).collect();
    loop {
        let before = inside.len();
        for (name, deps) in &state {
            if inside.contains(*name) {
                inside.extend(deps.iter().cloned());
            }
        }
        for deps in &bound {
            if deps.iter().any(|d| inside.contains(d)) {
                inside.extend(deps.iter().cloned());
            }
        }
        if inside.len() == before {
            break;
        }
    }
    let kept: Vec<&Term> = constraints
        .iter()
        .zip(&bound)
        .filter(|(_, deps)| deps.is_empty() || deps.iter().any(|d| inside.contains(d)))
        .map(|(t, _)| *t)
        .collect();
    let invariants = invariants.iter().filter(|t| vars(t).iter().all(|v| inside.contains(v))).copied().collect();
    (inside, kept, invariants)
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

/// Was eine Anfrage traegt: Zustand, Eingaben und Bedingungen, dazu die
/// `assumption`-Formeln darunter.
struct Scope<'a> {
    state: Vec<&'a StateVar>,
    inputs: Vec<&'a (String, Sort)>,
    /// Annahmen ueber die Eingaben, `assumption`-Formeln darunter.
    constraints: Vec<&'a Term>,
    /// Invarianten des Zustands; sie folgen aus den Uebergaengen.
    invariants: Vec<&'a Term>,
    assumed: Vec<&'a Goal>,
}

/// Der Umfang einer Anfrage ueber `targets`; `slice` schneidet auf den
/// Kegel der Ziele. `assumption`-Formeln gelten nur, wo ausschliesslich
/// Eigenschaften gefragt sind (13.3, 3.4) — eine Annahme selbst steht ohne
/// sie, sonst bewiese sie sich.
fn scope<'a>(model: &'a Model, targets: &[Target], slice: bool) -> Scope<'a> {
    let properties_only = targets.iter().all(|t| matches!(*t, Target::Property(i) if !model.properties[i].assumption));
    let assumed: Vec<&Goal> = model.properties.iter().filter(|g| g.assumption && properties_only).collect();
    let constraints: Vec<&Term> = model.assumptions.iter().chain(assumed.iter().map(|g| &g.formula)).collect();
    let invariants: Vec<&Term> = model.invariants.iter().collect();
    if !slice {
        return Scope {
            state: model.state.iter().collect(),
            inputs: model.inputs.iter().collect(),
            constraints,
            invariants,
            assumed,
        };
    }
    let roots: Vec<&Term> = targets
        .iter()
        .flat_map(|t| match *t {
            Target::Property(i) => vec![&model.properties[i].formula],
            Target::Check(i) => vec![&model.checks[i].init, &model.checks[i].fires],
        })
        .collect();
    let (inside, constraints, invariants) = cone(model, &roots, &constraints, &invariants);
    Scope {
        state: model.state.iter().filter(|v| inside.contains(&v.name)).collect(),
        inputs: model.inputs.iter().filter(|(n, _)| inside.contains(n)).collect(),
        assumed: assumed.into_iter().filter(|g| constraints.iter().any(|t| std::ptr::eq(*t, &g.formula))).collect(),
        constraints,
        invariants,
    }
}

/// Die `assumption`-Formeln, auf denen ein Urteil ueber `target` ruht: die
/// im Kegel seiner Anfrage (13.3, „kein Urteil ohne seine Annahmen").
pub fn assumptions_of(model: &Model, target: Target) -> Vec<String> {
    scope(model, &[target], true).assumed.iter().map(|g| g.name.clone()).collect()
}

/// Ein Block mit Deklarationen, Uebergaengen und den Anfragen der Ziele.
/// `ask`: eine Anfrage an den Solver — geschnitten auf den Kegel der Ziele,
/// bei BMC mit den Eingaben des Gegenbeispiels; sonst das ganze Modell.
fn block(out: &mut String, model: &Model, tag: &'static str, kind: Query, depth: u32, targets: &[Target], ask: bool) {
    let steps = depth;
    let Scope { state, inputs, constraints, invariants, .. } = scope(model, targets, ask);
    let constraints: Vec<&Term> = constraints.into_iter().chain(invariants).collect();
    let _ = writeln!(out, "(push 1)");
    let _ = writeln!(out, "; {}", if kind == Query::Induction { "Induktionsschritt" } else { "BMC" });
    for k in 0..=steps {
        for v in &state {
            let _ = writeln!(out, "(declare-const {} {})", at(&v.name, k, tag), sort_text(v.sort));
        }
        for (name, sort) in &inputs {
            let _ = writeln!(out, "(declare-const {} {})", at(name, k, tag), sort_text(*sort));
        }
    }
    let mut p = Printer { out, tag, defs: HashMap::new(), next: 0, declared: BTreeSet::new() };
    if kind == Query::Bmc {
        for v in &state {
            let init = p.name(&v.init, 0, 0);
            let _ = writeln!(p.out, "(assert (= {} {init}))", at(&v.name, 0, tag));
        }
    }
    for k in 0..steps {
        for v in &state {
            let next = p.name(&v.next, k, k + 1);
            let _ = writeln!(p.out, "(assert (= {} {next}))", at(&v.name, k + 1, tag));
        }
    }
    for k in 0..=steps {
        for a in &constraints {
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
        if ask && kind == Query::Bmc {
            let names: Vec<String> = (0..=steps).flat_map(|k| inputs.iter().map(move |(n, _)| at(n, k, tag))).collect();
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

/// Die Anfrage eines Block-Vertrags (5.7, B2): freie Variablen, die
/// Verletzung als Formel, die Belegung bei `sat`.
pub fn contract_query(goal: &crate::encode::ContractGoal) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "; takt prove — Vertrag von `{}` (5.7)", goal.block);
    let _ = writeln!(out, "(set-logic ALL)");
    for (name, sort) in &goal.vars {
        let _ = writeln!(out, "(declare-const |{name}| {})", sort_text(*sort));
    }
    let mut p = Printer { out: &mut out, tag: "@", defs: HashMap::new(), next: 0, declared: BTreeSet::new() };
    let v = p.name(&goal.violation, 0, 0);
    let _ = writeln!(p.out, "(assert {v})");
    let _ = writeln!(p.out, "(check-sat)");
    let names: Vec<String> = goal.vars.iter().map(|(n, _)| format!("|{n}|")).collect();
    if !names.is_empty() {
        let _ = writeln!(p.out, "(get-value ({}))", names.join(" "));
    }
    out
}

/// Die Grenzen von `i64` als ganze Zahlen.
const I64: (&str, &str) = ("(- 9223372036854775808)", "9223372036854775807");

/// Eine Operation ueber ganzen Zahlen (LIA) fuer die Invariantensuche;
/// `None` ausserhalb dieses Fragments. Jede Rechnung eines Programms ist
/// ueberlaufgeprueft, auf einem lebendigen Pfad rechnen ganze Zahlen also
/// wie 64 Bit; Division und Rest schneiden ab wie in Rust.
fn lia_text(op: Op, a: &[String]) -> Option<String> {
    let outside = |x: String| format!("(or (< {x} {}) (> {x} {}))", I64.0, I64.1);
    let trunc = |x: &str, y: &str| {
        format!(
            "(ite (>= {x} 0) (ite (>= {y} 0) (div {x} {y}) (- (div {x} (- {y})))) \
             (ite (>= {y} 0) (- (div (- {x}) {y})) (div (- {x}) (- {y}))))"
        )
    };
    Some(match op {
        Op::Not | Op::And | Op::Or | Op::Eq | Op::Ite => app_text(op, a),
        Op::Neg => format!("(- {})", a[0]),
        Op::Add => format!("(+ {} {})", a[0], a[1]),
        Op::Sub => format!("(- {} {})", a[0], a[1]),
        Op::Mul => format!("(* {} {})", a[0], a[1]),
        Op::Div => trunc(&a[0], &a[1]),
        Op::Rem => format!("(- {} (* {} {}))", a[0], a[1], trunc(&a[0], &a[1])),
        Op::Lt => format!("(< {} {})", a[0], a[1]),
        Op::Le => format!("(<= {} {})", a[0], a[1]),
        Op::Gt => format!("(> {} {})", a[0], a[1]),
        Op::Ge => format!("(>= {} {})", a[0], a[1]),
        Op::AddOverflows => outside(format!("(+ {} {})", a[0], a[1])),
        Op::SubOverflows => outside(format!("(- {} {})", a[0], a[1])),
        Op::MulOverflows => outside(format!("(* {} {})", a[0], a[1])),
        Op::BitAnd
        | Op::BitOr
        | Op::BitXor
        | Op::Shl
        | Op::Shr
        | Op::Wrap { .. }
        | Op::FNeg
        | Op::FAdd
        | Op::FSub
        | Op::FMul
        | Op::FDiv
        | Op::FLt
        | Op::FLe
        | Op::FGt
        | Op::FGe
        | Op::FEq
        | Op::FAbs
        | Op::FSqrt
        | Op::FFma
        | Op::ToF32
        | Op::ToF64
        | Op::IsFinite
        | Op::Scale { .. }
        | Op::Math(_)
        | Op::Round(_)
        | Op::FloatToInt => return None,
    })
}

/// Ein Term ueber ganzen Zahlen mit `let` statt `define-fun`: In einer
/// Horn-Klausel sind die Variablen gebunden, und eine Definition auf
/// oberster Ebene saehe sie nicht. Jeder geteilte Knoten bekommt eine
/// Bindung, der Text bleibt linear in der Groesse des Graphen; `None`, wenn
/// ein Knoten ausserhalb des Fragments liegt.
struct LetPrinter {
    tag: &'static str,
    binds: Vec<(String, String)>,
    names: HashMap<(usize, u32, u32), String>,
}

impl LetPrinter {
    fn new(tag: &'static str) -> LetPrinter {
        LetPrinter { tag, binds: Vec::new(), names: HashMap::new() }
    }

    fn name(&mut self, t: &Term, state: u32, input: u32) -> Option<String> {
        Some(match &*t.0 {
            Node::App(op, args) => {
                let key = (Rc::as_ptr(&t.0) as usize, state, input);
                if let Some(n) = self.names.get(&key) {
                    return Some(n.clone());
                }
                let parts: Vec<String> = args.iter().map(|a| self.name(a, state, input)).collect::<Option<_>>()?;
                let n = format!("|l{}{}|", self.tag, self.binds.len());
                self.binds.push((n.clone(), lia_text(*op, &parts)?));
                self.names.insert(key, n.clone());
                n
            }
            Node::Var(_, Sort::F32 | Sort::F64) | Node::F32(_) | Node::F64(_) => return None,
            Node::Var(v, _) if v.starts_with("i.") => at(v, input, self.tag),
            Node::Var(v, _) => at(v, state, self.tag),
            Node::Bool(b) => b.to_string(),
            Node::Int(i) if *i >= 0 => i.to_string(),
            Node::Int(i) => format!("(- {})", i.unsigned_abs()),
        })
    }

    /// Alle `terms` in einer Konjunktion.
    fn all(&mut self, terms: &[&Term], state: u32, input: u32) -> Option<Vec<String>> {
        terms.iter().map(|t| self.name(t, state, input)).collect()
    }

    /// `body` unter allen Bindungen.
    fn wrap(self, body: String) -> String {
        self.binds.into_iter().rev().fold(body, |acc, (n, e)| format!("(let (({n} {e}))\n  {acc})"))
    }
}

/// Die Anfrage der Invariantensuche (FB-375): das Transitionssystem als
/// Horn-Klauseln ueber ganzen Zahlen. `Inv(s, i)` heisst: Zustand `s` ist
/// erreichbar, mit den Eingaben `i` des Ticks, der ihn ergab. z3 sucht mit
/// Spacer (PDR) eine induktive Invariante — `sat` heisst, das Ziel gilt in
/// jeder Tiefe. `None`, wenn der Kegel des Ziels ausserhalb des Fragments
/// liegt (Fliesskomma, Bitoperationen, Schieben).
pub fn horn(model: &Model, target: Target) -> Option<String> {
    let Scope { state, inputs, constraints, invariants, .. } = scope(model, &[target], true);
    let before: Vec<&Term> = constraints.iter().chain(&invariants).copied().collect();
    let lia = |s: Sort| match s {
        Sort::Bool => Some("Bool"),
        Sort::Int => Some("Int"),
        Sort::F32 | Sort::F64 => None,
    };
    let sorts: Vec<&str> =
        state.iter().map(|v| lia(v.sort)).chain(inputs.iter().map(|(_, s)| lia(*s))).collect::<Option<_>>()?;
    let tag = "h";
    let args = |k: u32| -> String {
        state
            .iter()
            .map(|v| at(&v.name, k, tag))
            .chain(inputs.iter().map(|(n, _)| at(n, k, tag)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let binders = |steps: &[u32]| -> String {
        let mut out = Vec::new();
        for k in steps {
            out.extend(state.iter().map(|v| format!("({} {})", at(&v.name, *k, tag), lia(v.sort).unwrap_or("Int"))));
            out.extend(inputs.iter().map(|(n, s)| format!("({} {})", at(n, *k, tag), lia(*s).unwrap_or("Int"))));
        }
        out.join(" ")
    };
    let rule = |vars: String, body: String| format!("(assert (forall ({vars}) {body}))");
    let mut out = String::new();
    let _ = writeln!(out, "; takt prove — Invariantensuche (13.3, FB-375)");
    let _ = writeln!(out, "(set-logic HORN)");
    // Quantifizierte Verallgemeinerung: Am Zeitgeber-Programm aus FB-375
    // 1 s statt 42 s.
    let _ = writeln!(out, "(set-option :fp.spacer.q3.use_qgen true)");
    let _ = writeln!(out, "(declare-fun Inv ({}) Bool)", sorts.join(" "));
    // Der erste Tick: der Anfangszustand aus den Eingaben des Ticks 0.
    let start = |lp: &mut LetPrinter| -> Option<Vec<String>> {
        let mut body = Vec::new();
        for v in &state {
            body.push(format!("(= {} {})", at(&v.name, 1, tag), lp.name(&v.init, 1, 1)?));
        }
        body.extend(lp.all(&constraints, 1, 1)?);
        Some(body)
    };
    // Ein Tick aus einem erreichbaren Zustand mit neuen Eingaben.
    let step = |lp: &mut LetPrinter| -> Option<Vec<String>> {
        let mut body = vec![format!("(Inv {})", args(0))];
        body.extend(lp.all(&before, 0, 0)?);
        for v in &state {
            body.push(format!("(= {} {})", at(&v.name, 1, tag), lp.name(&v.next, 0, 1)?));
        }
        body.extend(lp.all(&constraints, 1, 1)?);
        Some(body)
    };
    let mut lp = LetPrinter::new(tag);
    let body = start(&mut lp)?;
    let _ = writeln!(
        out,
        "{}",
        rule(binders(&[1]), lp.wrap(format!("(=> (and true {}) (Inv {}))", body.join(" "), args(1))))
    );
    let mut lp = LetPrinter::new(tag);
    let body = step(&mut lp)?;
    let _ = writeln!(
        out,
        "{}",
        rule(binders(&[0, 1]), lp.wrap(format!("(=> (and {}) (Inv {}))", body.join(" "), args(1))))
    );
    match target {
        Target::Property(i) => {
            let mut lp = LetPrinter::new(tag);
            let mut body = vec![format!("(Inv {})", args(0))];
            body.extend(lp.all(&before, 0, 0)?);
            body.push(format!("(not {})", lp.name(&model.properties[i].formula, 0, 0)?));
            let _ = writeln!(out, "{}", rule(binders(&[0]), lp.wrap(format!("(=> (and {}) false)", body.join(" ")))));
        }
        Target::Check(i) => {
            let site = &model.checks[i];
            let mut lp = LetPrinter::new(tag);
            let mut body = start(&mut lp)?;
            body.push(lp.name(&site.init, 1, 1)?);
            let _ = writeln!(out, "{}", rule(binders(&[1]), lp.wrap(format!("(=> (and {}) false)", body.join(" ")))));
            let mut lp = LetPrinter::new(tag);
            let mut body = step(&mut lp)?;
            body.push(lp.name(&site.fires, 0, 1)?);
            let _ =
                writeln!(out, "{}", rule(binders(&[0, 1]), lp.wrap(format!("(=> (and {}) false)", body.join(" ")))));
        }
    }
    let _ = writeln!(out, "(check-sat)");
    Some(out)
}

/// Eine Pruefrunde nach Houdini (FB-375): `step = false` fragt je Kandidat,
/// ob er im Anfangszustand fallen kann; `step = true`, ob er nach einem
/// Tick aus einem Zustand fallen kann, in dem alle gelten. Je Kandidat eine
/// Antwort, in ihrer Reihenfolge.
pub fn houdini(model: &Model, candidates: &[&Term], step: bool) -> String {
    let tag = "@";
    let mut out = String::new();
    let _ = writeln!(out, "; takt prove — Hilfslemmata (Houdini, FB-375)");
    let _ = writeln!(out, "(set-logic ALL)");
    let steps: &[u32] = if step { &[0, 1] } else { &[0] };
    for &k in steps {
        for v in &model.state {
            let _ = writeln!(out, "(declare-const {} {})", at(&v.name, k, tag), sort_text(v.sort));
        }
        for (name, sort) in &model.inputs {
            let _ = writeln!(out, "(declare-const {} {})", at(name, k, tag), sort_text(*sort));
        }
    }
    let mut p = Printer { out: &mut out, tag, defs: HashMap::new(), next: 0, declared: BTreeSet::new() };
    if step {
        for c in candidates {
            let t = p.name(c, 0, 0);
            let _ = writeln!(p.out, "(assert {t})");
        }
        for v in &model.state {
            let next = p.name(&v.next, 0, 1);
            let _ = writeln!(p.out, "(assert (= {} {next}))", at(&v.name, 1, tag));
        }
    } else {
        for v in &model.state {
            let init = p.name(&v.init, 0, 0);
            let _ = writeln!(p.out, "(assert (= {} {init}))", at(&v.name, 0, tag));
        }
    }
    for &k in steps {
        for c in model.assumptions.iter().chain(&model.invariants) {
            let t = p.name(c, k, k);
            let _ = writeln!(p.out, "(assert {t})");
        }
    }
    let last = *steps.last().expect("Schritt");
    for c in candidates {
        let t = p.name(c, last, last);
        let _ = writeln!(p.out, "(push 1)\n(assert (not {t}))\n(check-sat)\n(pop 1)");
    }
    out
}

/// Die Anfrage eines Ziels fuer den Solver; BMC fragt nach den Eingaben
/// des Gegenbeispiels.
pub fn query(model: &Model, depth: u32, target: Target, kind: Query) -> String {
    let mut out = head(model);
    let tag = match kind {
        Query::Bmc => "@",
        Query::Induction => "#",
    };
    block(&mut out, model, tag, kind, depth, &[target], true);
    out
}
