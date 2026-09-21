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
use crate::TypeId;
use crate::expr::{BinaryOp, Expr, ExprKind, MatOp, Repr};
use crate::fns::{CostClass, CostVec};
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
            (activation(m, &types, &natives).total, fault_cost(m, &types, &natives))
        };
        program.machines[i].budget = Some(Budget { activation, fault_path });
    }
}

/// `B_m` mit seiner Herkunft je Zustand.
///
/// **Warum die Aufschluesselung hier entsteht und nicht im Bericht.** Die
/// Zustandswerte fallen beim Rechnen von `B_m` ohnehin an — `max` wirft sie
/// nur weg. Sie im Bericht erneut zu rechnen waere eine zweite Stelle mit
/// derselben Formel, und sie in `Budget` zu legen hiesse, sie in jede
/// MIR-Datei zu schreiben (Feld 20 des Formats), damit ein Werkzeug sie
/// gelegentlich anzeigen kann. Eine Rechnung, zwei Verbraucher: Wer nur
/// die Summe will, nimmt `.total`.
#[derive(Clone, Debug, Default)]
pub struct Activation {
    /// `B_m`: der maschinenweite Anteil plus das komponentenweise Maximum
    /// ueber die Zustaende.
    pub total: CostVec,
    /// Was unabhaengig vom Zustand anfaellt (`loop:` und Handler).
    pub base: CostVec,
    /// Je Zustand seine Kosten, indiziert wie `Machine::states`.
    pub states: Vec<CostVec>,
}

impl Activation {
    /// Der Zustand, der eine Klasse bestimmt — je Klasse ein anderer.
    ///
    /// **Es gibt keinen „teuersten Zustand".** Das Maximum ist
    /// komponentenweise (9.4.3: Kosten sind Vektoren), und die Schranke
    /// darf das sein: Zwei Zustaende schliessen einander aus, also ist
    /// fuer *jede* Klasse einzeln der groesste Wert erreichbar. Wer den
    /// Bericht liest, will darum je Klasse wissen, wo die Zahl herkommt —
    /// und das koennen verschiedene Zustaende sein.
    ///
    /// Bei Gleichstand gewinnt der erste: willkuerlich, aber stabil, damit
    /// zwei Laeufe desselben Programms denselben Bericht ergeben.
    pub fn driver(&self, class: CostClass) -> Option<usize> {
        let peak = self.states.iter().map(|c| c.of(class)).max()?;
        self.states.iter().position(|c| c.of(class) == peak)
    }
}

/// `B_m`: was eine Aktivierung im schlimmsten Fall kostet. Die Zustaende
/// schliessen einander aus, also zaehlt das Maximum, nicht die Summe.
pub fn activation(m: &Machine, types: &[Type], natives: &[CostVec]) -> Activation {
    let mut base = block_cost(&m.loop_block, types, natives);
    for h in &m.handlers {
        base = base + block_cost(&h.body, types, natives);
    }
    let states: Vec<CostVec> = m
        .states
        .iter()
        .map(|s| {
            let mut c = block_cost(&s.enter, types, natives)
                + block_cost(&s.loop_block, types, natives)
                + block_cost(&s.exit, types, natives);
            for h in &s.handlers {
                c = c + block_cost(&h.body, types, natives);
            }
            for t in &s.transitions {
                c = c + block_cost(&t.actions, types, natives);
            }
            c
        })
        .collect();

    // Komponentenweise, nicht „der teuerste Zustand": Zwei Zustaende
    // schliessen einander aus, also ist fuer jede Klasse einzeln ihr
    // groesster Wert erreichbar (9.4.3).
    let peak = states.iter().fold(CostVec::default(), |acc, c| acc.max(*c));
    Activation { total: base + peak, base, states }
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
                + block_cost(then, types, natives).max(block_cost(otherwise, types, natives))
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
            let worst = arms.iter().fold(CostVec::default(), |acc, a| acc.max(block_cost(&a.body, types, natives)));
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
        // 12.10: Ein Portzugriff ist ein Lade- oder Speichervorgang.
        Place::Port(_) => CostVec { mem: 1, ..CostVec::default() },
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
    let own = mat_cost(e, types).unwrap_or_else(|| match &e.kind {
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
    });
    // `children_mut` braucht `&mut`; hier reicht die lesende Entsprechung.
    e.children().iter().fold(own, |c, child| c + expr_cost(child, types, natives))
}

/// Kosten einer Matrixoperation (3.11, 9.4.3): elementweise R·C, Produkt
/// R·C·K, die LU-Verfahren n³ — in der Breite von `float` — und der
/// Scratch als Speicherzugriffe.
fn mat_cost(e: &Expr, types: &[Type]) -> Option<CostVec> {
    let dims = |ty: TypeId| match types.get(ty.index()) {
        Some(Type::Mat { rows, cols, .. }) => Some((u64::from(*rows), u64::from(*cols))),
        _ => None,
    };
    let float = types.iter().find_map(|t| if let Type::Float { width, .. } = t { Some(*width) } else { None });
    let unit = match float {
        Some(FloatWidth::F32) => CostVec { f32: 1, ..CostVec::default() },
        _ => CostVec { f64: 1, ..CostVec::default() },
    };
    let mem = |n: u64| CostVec { mem: n, ..CostVec::default() };
    Some(match &e.kind {
        ExprKind::Binary { op: BinaryOp::Mul, lhs, rhs } => match (dims(lhs.ty), dims(rhs.ty)) {
            (Some((r, k)), Some((_, c))) => times(unit, r * k * c),
            (Some((r, c)), None) | (None, Some((r, c))) => times(unit, r * c),
            _ => return None,
        },
        ExprKind::Binary { .. } => {
            let (r, c) = dims(e.ty)?;
            times(unit, r * c)
        }
        ExprKind::MatOp { op, args } => {
            let (n, k) = dims(args.first()?.ty)?;
            match op {
                MatOp::Transpose => mem(n * k),
                MatOp::Det => times(unit, n * n * n) + mem(n * n),
                MatOp::Inv => times(unit, 2 * n * n * n) + mem(2 * n * n),
                MatOp::Solve => {
                    let (_, c) = dims(args.get(1)?.ty)?;
                    times(unit, n * n * n + n * n * c) + mem(n * n + n * c)
                }
                MatOp::Cholesky => times(unit, n * n * n) + mem(n * n),
            }
        }
        _ => return None,
    })
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
