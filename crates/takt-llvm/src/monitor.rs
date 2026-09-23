//! Laufzeitmonitore (13.3, `with monitor = true`): je Eigenschaft eine
//! Funktion `takt_monitor_<i>(state, image, params, latch, tick)`, die die
//! Runtime nach dem Commit jedes Ticks ruft. Sie schreibt die Atome des
//! Tick-Rand-Snapshots in einen Ring und entscheidet die Position
//! `tick - F` (F: Zukunftstiefe der Formel) — im selben Tick wie der
//! Interpreter (`takt-interp/src/property.rs`). Eine Verletzung meldet
//! `takt_property(i, position)`, danach schweigt der Monitor wie der
//! Interpreter. Die Formel hat aussen `always`/`never` und innen nur
//! beschraenkte Operatoren (Pruefung 56); ihre Fenster sind Schleifen ueber
//! den Ring. Die Kosten je Tick sind statisch: Atome plus Fenster (9.4.3).

use takt_mir::expr::{Expr, TProp, TemporalOp};
use takt_mir::program::{Program, Property};

use crate::abi::Abi;
use crate::emit::{Module, Reg};
use crate::expr::{Lowered, NotYet, Vars, lower};
use crate::ty::{self, LlvmType};

/// Die Formel; Atome zeigen in den Ring, Fenster in die Zaehler.
enum Node {
    Atom(usize),
    Not(Box<Node>),
    And(Box<Node>, Box<Node>),
    Or(Box<Node>, Box<Node>),
    Bounded { id: usize, op: TemporalOp, ticks: i64, inner: Box<Node> },
}

/// `always(φ)` oder `never(φ)` aussen: `φ` und ob sie zu verneinen ist.
fn outer(f: &TProp) -> Option<(&TProp, bool)> {
    match f {
        TProp::Temporal { op: TemporalOp::Always, inner, .. } => Some((inner, false)),
        TProp::Temporal { op: TemporalOp::Never, inner, .. } => Some((inner, true)),
        _ => None,
    }
}

struct Build {
    atoms: Vec<Expr>,
    bounded: usize,
}

fn build(f: &TProp, tick: i64, b: &mut Build) -> Option<Node> {
    Some(match f {
        TProp::Atom(e) => {
            b.atoms.push(e.clone());
            Node::Atom(b.atoms.len() - 1)
        }
        TProp::Not(a) => Node::Not(Box::new(build(a, tick, b)?)),
        TProp::And(x, y) => Node::And(Box::new(build(x, tick, b)?), Box::new(build(y, tick, b)?)),
        TProp::Or(x, y) => Node::Or(Box::new(build(x, tick, b)?), Box::new(build(y, tick, b)?)),
        TProp::Implies(x, y) => {
            Node::Or(Box::new(Node::Not(Box::new(build(x, tick, b)?))), Box::new(build(y, tick, b)?))
        }
        TProp::Temporal { op: TemporalOp::Always | TemporalOp::Never, .. } => return None,
        TProp::Temporal { op, window, inner } => {
            let id = b.bounded;
            b.bounded += 1;
            Node::Bounded {
                id,
                op: *op,
                ticks: window.unwrap_or(0) / tick.max(1),
                inner: Box::new(build(inner, tick, b)?),
            }
        }
    })
}

/// Zukunfts- (`past = false`) oder Vergangenheitstiefe in Ticks.
fn depth(node: &Node, past: bool) -> i64 {
    match node {
        Node::Atom(_) => 0,
        Node::Not(a) => depth(a, past),
        Node::And(a, b) | Node::Or(a, b) => depth(a, past).max(depth(b, past)),
        Node::Bounded { op, ticks, inner, .. } => {
            let own = if (*op == TemporalOp::Once) == past { *ticks } else { 0 };
            own + depth(inner, past)
        }
    }
}

struct Shape {
    node: Node,
    atoms: Vec<Expr>,
    bounded: usize,
    rows: u64,
    future: i64,
    negate: bool,
}

fn shape(prop: &Property, p: &Program) -> Result<Shape, NotYet> {
    let (inner, negate) = outer(&prop.formula).ok_or(NotYet { what: "Monitor ohne `always`/`never` aussen" })?;
    let mut b = Build { atoms: Vec::new(), bounded: 0 };
    let node =
        build(inner, p.config.tick, &mut b).ok_or(NotYet { what: "`always` unter einem beschraenkten Operator" })?;
    let (future, past) = (depth(&node, false), depth(&node, true));
    Ok(Shape { node, atoms: b.atoms, bounded: b.bounded, rows: (future + past + 1) as u64, future, negate })
}

/// Bytes des Monitorzustands: ein Byte „verletzt" und der Ring der Atome.
pub fn state_size(prop: &Property, p: &Program) -> Option<u64> {
    let s = shape(prop, p).ok()?;
    Some(1 + s.rows * s.atoms.len().max(1) as u64)
}

/// Schreibt `takt_monitor_<index>`.
pub fn monitor_function(index: usize, prop: &Property, p: &Program, m: &mut Module) -> Result<(), NotYet> {
    let s = shape(prop, p)?;
    let st = format!("{{ i8, [{} x [{} x i8]] }}", s.rows, s.atoms.len().max(1));
    let mark = m.mark();
    let ptr = LlvmType::Ptr;
    m.begin(
        &format!("takt_monitor_{index}"),
        &LlvmType::Void,
        &[ptr.clone(), ptr.clone(), ptr.clone(), ptr, LlvmType::Int(64)],
    );
    // Die Zaehler der Fenster im Eintrittsblock (11.2: statischer Scratch).
    let slots: Vec<(Reg, Reg)> = (0..s.bounded).map(|_| (m.alloca("i1"), m.alloca("i64"))).collect();
    let flag = m.inst(&format!("getelementptr inbounds {st}, ptr %0, i32 0, i32 0"));
    let seen = m.inst(&format!("load i8, ptr {flag}"));
    let was = m.inst(&format!("icmp ne i8 {seen}, 0"));
    let (start, end) = (format!("monitor{index}_start"), format!("monitor{index}_end"));
    m.void_inst(&format!("br i1 {was}, label %{end}, label %{start}"));
    m.label(&start);
    let row = m.inst(&format!("urem i64 %4, {}", s.rows));
    for (k, atom) in s.atoms.iter().enumerate() {
        let vars = MonitorVars { program: p, fault: format!("monitor{index}_atom{k}_fault") };
        let next = format!("monitor{index}_atom{k}_next");
        let cell = m.inst(&format!("getelementptr inbounds {st}, ptr %0, i32 0, i32 1, i64 {row}, i32 {k}"));
        let v = match lower(atom, p, m, &vars) {
            Ok(v) => v,
            Err(e) => {
                m.abort(mark);
                return Err(e);
            }
        };
        let byte = m.inst(&format!("zext i1 {} to i8", v.value));
        m.void_inst(&format!("store i8 {byte}, ptr {cell}"));
        m.void_inst(&format!("br label %{next}"));
        // Ein Fault im Atom — ungueltiger Input, Range — macht es falsch (13.5).
        m.label(&vars.fault);
        m.void_inst(&format!("store i8 0, ptr {cell}"));
        m.label(&next);
    }
    let ready = m.inst(&format!("icmp sge i64 %4, {}", s.future));
    let decide = format!("monitor{index}_decide");
    m.void_inst(&format!("br i1 {ready}, label %{decide}, label %{end}"));
    m.label(&decide);
    let pos = m.inst(&format!("sub i64 %4, {}", s.future));
    let cx = Cx { st: &st, rows: s.rows, slots: &slots, index };
    let v = eval(&s.node, &pos.to_string(), &cx, m);
    let holds = if s.negate { m.inst(&format!("xor i1 {v}, true")).to_string() } else { v };
    let report = format!("monitor{index}_report");
    m.void_inst(&format!("br i1 {holds}, label %{end}, label %{report}"));
    m.label(&report);
    m.void_inst(&format!("store i8 1, ptr {flag}"));
    m.void_inst(&format!("call void @{}(i32 {index}, i64 {pos})", Abi::PROPERTY));
    m.label(&end);
    m.end(None);
    Ok(())
}

struct Cx<'a> {
    st: &'a str,
    rows: u64,
    slots: &'a [(Reg, Reg)],
    index: usize,
}

/// Der Wert der Formel an der Position `pos` (ein `i64`-Operand).
fn eval(node: &Node, pos: &str, cx: &Cx<'_>, m: &mut Module) -> String {
    match node {
        Node::Atom(k) => {
            // Vor dem Start gab es nichts: Position < 0 ist falsch.
            let neg = m.inst(&format!("icmp slt i64 {pos}, 0"));
            let clamped = m.inst(&format!("select i1 {neg}, i64 0, i64 {pos}"));
            let row = m.inst(&format!("urem i64 {clamped}, {}", cx.rows));
            let cell = m.inst(&format!("getelementptr inbounds {}, ptr %0, i32 0, i32 1, i64 {row}, i32 {k}", cx.st));
            let raw = m.inst(&format!("load i8, ptr {cell}"));
            let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
            m.inst(&format!("select i1 {neg}, i1 false, i1 {b}")).to_string()
        }
        Node::Not(a) => {
            let v = eval(a, pos, cx, m);
            m.inst(&format!("xor i1 {v}, true")).to_string()
        }
        Node::And(a, b) | Node::Or(a, b) => {
            let x = eval(a, pos, cx, m);
            let y = eval(b, pos, cx, m);
            let op = if matches!(node, Node::And(..)) { "and" } else { "or" };
            m.inst(&format!("{op} i1 {x}, {y}")).to_string()
        }
        Node::Bounded { id, op, ticks, inner } => {
            let (acc, qv) = cx.slots[*id];
            let (lo, hi) = if *op == TemporalOp::Once {
                (m.inst(&format!("sub i64 {pos}, {ticks}")).to_string(), pos.to_string())
            } else {
                (pos.to_string(), m.inst(&format!("add i64 {pos}, {ticks}")).to_string())
            };
            let stable = *op == TemporalOp::Stable;
            m.void_inst(&format!("store i1 {}, ptr {acc}", if stable { "true" } else { "false" }));
            m.void_inst(&format!("store i64 {lo}, ptr {qv}"));
            let n = m.next_label();
            let (head, body, exit) = (
                format!("window{}_{n}_head", cx.index),
                format!("window{}_{n}_body", cx.index),
                format!("window{}_{n}_exit", cx.index),
            );
            m.void_inst(&format!("br label %{head}"));
            m.label(&head);
            let q = m.inst(&format!("load i64, ptr {qv}"));
            let over = m.inst(&format!("icmp sgt i64 {q}, {hi}"));
            m.void_inst(&format!("br i1 {over}, label %{exit}, label %{body}"));
            m.label(&body);
            let v = eval(inner, &q.to_string(), cx, m);
            let cur = m.inst(&format!("load i1, ptr {acc}"));
            let new = m.inst(&format!("{} i1 {cur}, {v}", if stable { "and" } else { "or" }));
            m.void_inst(&format!("store i1 {new}, ptr {acc}"));
            let q1 = m.inst(&format!("add i64 {q}, 1"));
            m.void_inst(&format!("store i64 {q1}, ptr {qv}"));
            m.void_inst(&format!("br label %{head}"));
            m.label(&exit);
            m.inst(&format!("load i1, ptr {acc}")).to_string()
        }
    }
}

/// Der Tick-Rand-Snapshot als Variablenquelle: Abbild `%1`, Parameter `%2`,
/// Latch `%3` (nach dem Commit der committete Wert), Ψ_k, Tick `%4`.
struct MonitorVars<'a> {
    program: &'a Program,
    fault: String,
}

impl Vars for MonitorVars<'_> {
    fn var(&self, _: takt_mir::VarId, _: &mut Module) -> Option<Lowered> {
        None
    }

    fn input(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        crate::stmt::image_slot(self.program, channel, crate::image::Slot::Value, m)
    }

    fn quality(&self, channel: takt_mir::ChannelId, slot: crate::image::Slot, m: &mut Module) -> Option<Lowered> {
        crate::stmt::image_slot(self.program, channel, slot, m)
    }

    fn param(&self, id: takt_mir::ParamId, m: &mut Module) -> Option<Lowered> {
        let p = self.program.params.get(id.index())?;
        let ty = ty::lower(p.ty, self.program)?;
        let off = crate::image::param_offset(id, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %2, i64 {off}"));
        let v = m.inst(&format!("load {ty}, ptr {at}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    fn output(&self, channel: takt_mir::ChannelId, m: &mut Module) -> Option<Lowered> {
        let c = self.program.channels.get(channel.index())?;
        let ty = ty::lower(c.ty, self.program)?;
        let off = crate::image::latch_offset(channel, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %3, i64 {off}"));
        let v = m.inst(&format!("load {ty}, ptr {at}"));
        Some(Lowered { value: v.to_string(), ty })
    }

    fn command(&self, id: takt_mir::CommandId, m: &mut Module) -> Option<Lowered> {
        let off = crate::image::command_offset(id, self.program)?;
        let at = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
        let raw = m.inst(&format!("load i8, ptr {at}"));
        let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
        Some(Lowered { value: b.to_string(), ty: LlvmType::Int(1) })
    }

    fn published(&self, target: takt_mir::MachineId, field: crate::psi::Field, m: &mut Module) -> Option<Lowered> {
        crate::psi::load_bank(target, field, self.program, m)
    }

    fn builtin(&self, b: takt_mir::expr::Builtin, p: &Program, m: &mut Module) -> Option<Lowered> {
        let dur = LlvmType::Int(64);
        match b {
            takt_mir::expr::Builtin::Tick => Some(Lowered { value: p.config.tick.to_string(), ty: dur }),
            takt_mir::expr::Builtin::Now => {
                let v = m.inst(&format!("mul i64 %4, {}", p.config.tick));
                Some(Lowered { value: v.to_string(), ty: dur })
            }
            _ => None,
        }
    }

    fn fault_label(&self) -> Option<String> {
        Some(self.fault.clone())
    }
}
