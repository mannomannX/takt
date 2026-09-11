//! Typisiertes Kostenmodell (Referenz 9.4.3, Satz 9.4.3).
//!
//! „Die Kosten sind Vektoren ueber den Operationsklassen c ∈ {i32, i64, f32,
//! f64, mem, call, native}: `N(s) ∈ ℕ^7`, `cost(e)` zaehlt jede Operation in
//! ihrer Klasse (eine Integer-Operation nach ihrer gewaehlten Darstellung in
//! `i32` oder `i64` (3.4) …)."
//!
//! Deshalb laeuft `narrow` vor `cost`: Ohne die Darstellung waere jede
//! Integer-Operation `i64` und das Budget auf 32-Bit-Kernen zu pessimistisch.

use crate::Program;
use crate::expr::{Expr, ExprKind, Repr};
use crate::fns::CostVec;
use crate::machine::{Budget, Machine};
use crate::stmt::{Block, Method, Place, Stmt, StmtKind};
use crate::types::{FloatWidth, Type};

/// Rechnet je Maschine `B_m` (Aktivierung) und `F_m` (Fault-Pfad) und traegt
/// beide in `Machine::budget` ein.
pub fn budgets(program: &mut Program) {
    let types = program.types.list.clone();
    let natives: Vec<CostVec> = program.natives.iter().map(|n| n.cost).collect();
    for i in 0..program.machines.len() {
        let (activation, fault_path) = {
            let m = &program.machines[i];
            (activation_cost(m, &types, &natives), fault_cost(m, &types, &natives))
        };
        program.machines[i].budget = Some(Budget { activation, fault_path });
    }
}

/// `B_m`: was eine Aktivierung im schlimmsten Fall kostet. Die Zustaende
/// schliessen einander aus, also zaehlt das Maximum, nicht die Summe.
fn activation_cost(m: &Machine, types: &[Type], natives: &[CostVec]) -> CostVec {
    let mut base = block_cost(&m.loop_block, types, natives);
    for h in &m.handlers {
        base = base + block_cost(&h.body, types, natives);
    }
    let mut worst = CostVec::default();
    for s in &m.states {
        let mut c = block_cost(&s.enter, types, natives)
            + block_cost(&s.loop_block, types, natives)
            + block_cost(&s.exit, types, natives);
        for h in &s.handlers {
            c = c + block_cost(&h.body, types, natives);
        }
        for t in &s.transitions {
            c = c + block_cost(&t.actions, types, natives);
        }
        worst = max(worst, c);
    }
    base + worst
}

/// `F_m`: der Fault-Pfad (5.3, 5.4). Er laeuft hoechstens einmal je Tick
/// und geht in die Abort-Phase ein (7.2).
fn fault_cost(m: &Machine, types: &[Type], natives: &[CostVec]) -> CostVec {
    let mut c = CostVec::default();
    for t in &m.faulted.transitions {
        c = c + block_cost(&t.actions, types, natives);
    }
    c
}

/// Komponentenweises Maximum: zwei einander ausschliessende Zweige.
fn max(a: CostVec, b: CostVec) -> CostVec {
    CostVec {
        i32: a.i32.max(b.i32),
        i64: a.i64.max(b.i64),
        f32: a.f32.max(b.f32),
        f64: a.f64.max(b.f64),
        mem: a.mem.max(b.mem),
        call: a.call.max(b.call),
        native: a.native.max(b.native),
    }
}

fn block_cost(b: &Block, types: &[Type], natives: &[CostVec]) -> CostVec {
    b.stmts.iter().fold(CostVec::default(), |acc, s| acc + stmt_cost(s, types, natives))
}

fn stmt_cost(s: &Stmt, types: &[Type], natives: &[CostVec]) -> CostVec {
    match &s.kind {
        StmtKind::Assign { target, value } => place_cost(target, types, natives) + expr_cost(value, types, natives),
        StmtKind::Check { cond, .. } => expr_cost(cond, types, natives),
        StmtKind::If { cond, then, otherwise } => {
            // Nur ein Zweig laeuft.
            expr_cost(cond, types, natives)
                + max(block_cost(then, types, natives), block_cost(otherwise, types, natives))
        }
        StmtKind::ForRange { count, body, .. } => {
            let n = match count.kind {
                ExprKind::Int(v) if v > 0 => v as u64,
                _ => 1,
            };
            expr_cost(count, types, natives) + times(block_cost(body, types, natives), n)
        }
        StmtKind::ForEach { iter, body, .. } => {
            // Die Schranke ist die Kapazitaet des Behaelters; ohne sie eins.
            expr_cost(iter, types, natives) + times(block_cost(body, types, natives), capacity(iter, types))
        }
        StmtKind::Match { subject, arms } => {
            let worst = arms.iter().fold(CostVec::default(), |acc, a| max(acc, block_cost(&a.body, types, natives)));
            expr_cost(subject, types, natives) + worst
        }
        StmtKind::Every { period, body, .. } => expr_cost(period, types, natives) + block_cost(body, types, natives),
        StmtKind::At { time, body } => expr_cost(time, types, natives) + block_cost(body, types, natives),
        StmtKind::Send { value, .. } | StmtKind::Return(value) => expr_cost(value, types, natives),
        StmtKind::MethodCall { target, method, args, .. } => {
            let a = args.iter().fold(CostVec::default(), |acc, x| acc + expr_cost(x, types, natives));
            let t = target.as_ref().map_or(CostVec::default(), |p| place_cost(p, types, natives));
            // `append` kopiert; die obere Schranke ist die Kapazitaet der
            // Quelle (3.9), nicht ihre aktuelle Laenge. Alles andere ist O(1).
            let copy = match (method, args.first()) {
                (Method::Append, Some(src)) => CostVec { mem: capacity(src, types), ..CostVec::default() },
                _ => CostVec::default(),
            };
            a + t + copy + CostVec { call: 1, ..CostVec::default() }
        }
        StmtKind::Job { args, native, .. } => {
            let a = args.iter().fold(CostVec::default(), |acc, x| acc + expr_cost(x, types, natives));
            a + natives.get(native.index()).copied().unwrap_or_default()
        }
        StmtKind::Observe(o) => observe_cost(o, types, natives),
        _ => CostVec::default(),
    }
}

fn observe_cost(o: &crate::stmt::Observe, types: &[Type], natives: &[CostVec]) -> CostVec {
    use crate::stmt::Observe;
    match o {
        Observe::Alert { cond, .. } | Observe::Verify { cond, .. } => expr_cost(cond, types, natives),
        Observe::Measure { value, .. } => expr_cost(value, types, natives),
        _ => CostVec::default(),
    }
}

fn place_cost(p: &Place, types: &[Type], natives: &[CostVec]) -> CostVec {
    match p {
        Place::Var(_) | Place::Output(_) => CostVec::default(),
        Place::Field(b, _) => place_cost(b, types, natives),
        Place::Index(b, i) => {
            place_cost(b, types, natives) + expr_cost(i, types, natives) + CostVec { mem: 1, ..CostVec::default() }
        }
        Place::Index2(b, r, c) => {
            place_cost(b, types, natives)
                + expr_cost(r, types, natives)
                + expr_cost(c, types, natives)
                + CostVec { mem: 1, ..CostVec::default() }
        }
    }
}

/// Kosten eines Ausdrucks: die Operation selbst plus ihre Kinder.
fn expr_cost(e: &Expr, types: &[Type], natives: &[CostVec]) -> CostVec {
    let mut c = match &e.kind {
        ExprKind::Binary { .. } | ExprKind::Unary { .. } => class_of(e, types),
        ExprKind::Index { .. } | ExprKind::Index2 { .. } | ExprKind::Slice { .. } => {
            CostVec { mem: 1, ..CostVec::default() }
        }
        ExprKind::Call { .. } => CostVec { call: 1, ..CostVec::default() },
        ExprKind::NativeCall { native, .. } => natives.get(native.index()).copied().unwrap_or_default(),
        ExprKind::Intrinsic { .. } => class_of(e, types) + CostVec { call: 1, ..CostVec::default() },
        // Eine implizite Pruefung ist ein Vergleich und ein Sprung.
        ExprKind::Checked { .. } => class_of(e, types),
        _ => CostVec::default(),
    };
    // `children_mut` braucht `&mut`; hier reicht die lesende Entsprechung.
    for child in children(e) {
        c = c + expr_cost(child, types, natives);
    }
    c
}

/// Die Operationsklasse eines Ausdrucks nach Typ und gewaehlter Darstellung.
fn class_of(e: &Expr, types: &[Type]) -> CostVec {
    match types.get(e.ty.index()) {
        Some(Type::Float { width: FloatWidth::F32, .. }) => CostVec { f32: 1, ..CostVec::default() },
        Some(Type::Float { width: FloatWidth::F64, .. }) => CostVec { f64: 1, ..CostVec::default() },
        Some(Type::Int { .. } | Type::Duration { .. }) => match e.repr {
            // 9.4.3: „eine Integer-Operation nach ihrer gewaehlten
            // Darstellung in `i32` oder `i64` (3.4)".
            Some(Repr::I32) => CostVec { i32: 1, ..CostVec::default() },
            _ => CostVec { i64: 1, ..CostVec::default() },
        },
        _ => CostVec { i32: 1, ..CostVec::default() },
    }
}

/// Vielfaches eines Vektors.
fn times(c: CostVec, n: u64) -> CostVec {
    CostVec {
        i32: c.i32.saturating_mul(n),
        i64: c.i64.saturating_mul(n),
        f32: c.f32.saturating_mul(n),
        f64: c.f64.saturating_mul(n),
        mem: c.mem.saturating_mul(n),
        call: c.call.saturating_mul(n),
        native: c.native.saturating_mul(n),
    }
}

/// Statische Schranke einer `for`-Schleife ueber einen Behaelter.
fn capacity(iter: &Expr, types: &[Type]) -> u64 {
    match types.get(iter.ty.index()) {
        Some(Type::Array { len, .. } | Type::Samples { len, .. }) => u64::from(*len),
        Some(Type::Vec { cap, .. } | Type::Bytes { cap } | Type::Map { cap, .. }) => u64::from(*cap),
        _ => 1,
    }
}

/// Die unmittelbaren Teilausdruecke, lesend.
fn children(e: &Expr) -> Vec<&Expr> {
    match &e.kind {
        ExprKind::Variant { fields, .. } | ExprKind::Record { fields, .. } => fields.iter().collect(),
        ExprKind::Array(items) => items.iter().collect(),
        ExprKind::Tuple(a, b) => vec![a, b],
        ExprKind::BlockInit { args, .. }
        | ExprKind::Call { args, .. }
        | ExprKind::NativeCall { args, .. }
        | ExprKind::MatOp { args, .. }
        | ExprKind::Intrinsic { args, .. } => args.iter().collect(),
        ExprKind::Field { base, .. } => vec![base],
        ExprKind::Index { base, index } => vec![base, index],
        ExprKind::Index2 { base, row, col } => vec![base, row, col],
        ExprKind::Slice { base, from, to } => vec![base, from, to],
        ExprKind::Accessor { base, args, .. } => {
            let mut v: Vec<&Expr> = vec![base];
            v.extend(args.iter());
            v
        }
        ExprKind::Unary { expr, .. }
        | ExprKind::Cast { expr, .. }
        | ExprKind::Convert { expr, .. }
        | ExprKind::Checked { expr, .. } => vec![expr],
        ExprKind::Lift(x) | ExprKind::Ok(x) | ExprKind::Err(x) => vec![x],
        ExprKind::Binary { lhs, rhs, .. } => vec![lhs, rhs],
        ExprKind::Cond { cond, then, otherwise } => vec![cond, then, otherwise],
        ExprKind::Matches { subject, .. } => vec![subject],
        ExprKind::Decode { bytes, .. } => vec![bytes],
        _ => Vec::new(),
    }
}
