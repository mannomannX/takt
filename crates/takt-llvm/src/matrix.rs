//! Matrizen fester Groesse (3.11) im erzeugten Code.
//!
//! Die Rechenwege sind die von `libtaktm::mat`, Operation fuer Operation
//! in derselben Reihenfolge — `fma`-Ketten ab null, LU mit
//! Spaltenpivotisierung, Cholesky —, damit Interpreter und nativer Code
//! bitidentisch rechnen (4.2, Satz 9.4.4). Die Form steht zur
//! Uebersetzungszeit fest, also sind die Schleifen ausgerollt; nur die
//! Pivotzeile ist ein Laufzeitwert. Was einen Platz braucht — die LU, die
//! Loesung, die rechte Seite —, liegt im statischen Scratch (11.2), den
//! die Sema in `Layout::scratch_bytes` ausweist. Ein Pivot null springt
//! bei `inv` und `solve` in den Fault-Pfad (`ArithmeticFault(Singular)`);
//! `det` liefert dann null und `cholesky` `none`, wie der Interpreter.

use takt_mir::expr::{BinaryOp, Expr, MatOp};
use takt_mir::program::Program;

use crate::emit::{Module, Reg, float_literal};
use crate::expr::{Lowered, NotYet, Vars, lower};
use crate::ty::LlvmType;

/// Zeilen, Spalten und Elementtyp, wenn `ty` eine Matrix ist.
pub fn shape(ty: &LlvmType) -> Option<(usize, usize, LlvmType)> {
    let LlvmType::Array(row, rows) = ty else { return None };
    let LlvmType::Array(elem, cols) = row.as_ref() else { return None };
    elem.is_float().then(|| (*rows as usize, *cols as usize, (**elem).clone()))
}

pub(crate) fn suffix(t: &LlvmType) -> &'static str {
    if *t == LlvmType::F32 { "f32" } else { "f64" }
}

/// Ein einstelliges Intrinsic (`fabs`, `sqrt`) auf dem Elementtyp.
fn unary(m: &mut Module, name: &str, t: &LlvmType, x: &str) -> String {
    let s = suffix(t);
    m.needs_intrinsic(&format!("{t} @llvm.{name}.{s}({t})"));
    m.inst(&format!("call {t} @llvm.{name}.{s}({t} {x})")).to_string()
}

/// `fma(a, b, c)`, korrekt gerundet (4.2).
fn fma(m: &mut Module, t: &LlvmType, a: &str, b: &str, c: &str) -> String {
    let s = suffix(t);
    m.needs_intrinsic(&format!("{t} @llvm.fma.{s}({t}, {t}, {t})"));
    m.inst(&format!("call {t} @llvm.fma.{s}({t} {a}, {t} {b}, {t} {c})")).to_string()
}

fn neg(m: &mut Module, t: &LlvmType, x: &str) -> String {
    m.inst(&format!("fneg {t} {x}")).to_string()
}

/// Alle Elemente eines Matrixwerts, zeilenweise.
fn elements(m: &mut Module, v: &Lowered, rows: usize, cols: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(rows * cols);
    for i in 0..rows {
        for j in 0..cols {
            out.push(m.inst(&format!("extractvalue {} {}, {i}, {j}", v.ty, v.value)).to_string());
        }
    }
    out
}

/// Ein Matrixwert aus Elementen, zeilenweise.
fn assemble(m: &mut Module, ty: &LlvmType, t: &LlvmType, cols: usize, elems: &[String]) -> Lowered {
    let mut cur = "undef".to_string();
    for (k, e) in elems.iter().enumerate() {
        cur = m.inst(&format!("insertvalue {ty} {cur}, {t} {e}, {}, {}", k / cols, k % cols)).to_string();
    }
    Lowered { value: cur, ty: ty.clone() }
}

/// `+`, `-`, `*`, `/` mit einer Matrix; die Formen hat die Sema geprueft.
pub fn binary(op: BinaryOp, a: &Lowered, b: &Lowered, want: &LlvmType, m: &mut Module) -> Result<Lowered, NotYet> {
    let (_, cols, t) = shape(want).ok_or(NotYet { what: "Matrixtyp" })?;
    let out = match (op, shape(&a.ty), shape(&b.ty)) {
        (BinaryOp::Add | BinaryOp::Sub, Some((r, c, _)), Some(_)) => {
            let (x, y) = (elements(m, a, r, c), elements(m, b, r, c));
            let ins = if op == BinaryOp::Add { "fadd" } else { "fsub" };
            let mut out = Vec::with_capacity(x.len());
            for (p, q) in x.iter().zip(&y) {
                out.push(m.inst(&format!("{ins} {t} {p}, {q}")).to_string());
            }
            out
        }
        (BinaryOp::Mul, Some((rows, k, _)), Some((_, n, _))) => {
            let (x, y) = (elements(m, a, rows, k), elements(m, b, k, n));
            let mut out = Vec::with_capacity(rows * n);
            for i in 0..rows {
                for j in 0..n {
                    let mut acc = float_literal(0.0, &t);
                    for l in 0..k {
                        acc = fma(m, &t, &x[i * k + l], &y[l * n + j], &acc);
                    }
                    out.push(acc);
                }
            }
            out
        }
        (BinaryOp::Mul | BinaryOp::Div, Some((r, c, _)), None) => {
            let ins = if op == BinaryOp::Mul { "fmul" } else { "fdiv" };
            let x = elements(m, a, r, c);
            let mut out = Vec::with_capacity(x.len());
            for e in &x {
                out.push(m.inst(&format!("{ins} {t} {e}, {}", b.value)).to_string());
            }
            out
        }
        (BinaryOp::Mul, None, Some((r, c, _))) => {
            let y = elements(m, b, r, c);
            let mut out = Vec::with_capacity(y.len());
            for e in &y {
                out.push(m.inst(&format!("fmul {t} {e}, {}", a.value)).to_string());
            }
            out
        }
        _ => return Err(NotYet { what: "Matrixoperator" }),
    };
    Ok(assemble(m, want, &t, cols, &out))
}

/// `transpose`, `inv`, `det`, `solve`, `cholesky` (3.11).
pub fn op(
    op: MatOp,
    args: &[Expr],
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let a = lower(args.first().ok_or(NotYet { what: "Matrixoperand" })?, p, m, vars)?;
    let (rows, cols, t) = shape(&a.ty).ok_or(NotYet { what: "Matrixoperand" })?;
    match op {
        MatOp::Transpose => {
            let x = elements(m, &a, rows, cols);
            let mut out = vec![String::new(); rows * cols];
            for i in 0..rows {
                for j in 0..cols {
                    out[j * rows + i] = x[i * cols + j].clone();
                }
            }
            Ok(assemble(m, want, &t, rows, &out))
        }
        MatOp::Det => det(&a, rows, &t, m),
        MatOp::Inv => inv(&a, rows, want, &t, m, vars),
        MatOp::Solve => {
            let b = lower(args.get(1).ok_or(NotYet { what: "rechte Seite von `solve`" })?, p, m, vars)?;
            solve(&a, &b, rows, want, &t, m, vars)
        }
        MatOp::Cholesky => cholesky(&a, rows, want, &t, m),
    }
}

/// `A[i, j]`: Die Sema hat die Indizes statisch in die Form gezwungen
/// (3.11, Pruefung 30); hier steht nur der Zugriff.
pub fn index(
    base: &Expr,
    row: &Expr,
    col: &Expr,
    want: &LlvmType,
    p: &Program,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let x = lower(base, p, m, vars)?;
    let i = lower(row, p, m, vars)?;
    let j = lower(col, p, m, vars)?;
    if let (Ok(ci), Ok(cj)) = (i.value.parse::<u32>(), j.value.parse::<u32>()) {
        let v = m.inst(&format!("extractvalue {} {}, {ci}, {cj}", x.ty, x.value));
        return Ok(Lowered { value: v.to_string(), ty: want.clone() });
    }
    let tmp = m.alloca(&x.ty);
    m.write(&x.ty, &x.value, &tmp.to_string());
    let at = m.inst(&format!(
        "getelementptr inbounds {}, ptr {tmp}, i32 0, {} {}, {} {}",
        x.ty, i.ty, i.value, j.ty, j.value
    ));
    let v = m.inst(&format!("load {want}, ptr {at}"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

/// Eine Matrix im Scratch: Zeiger und Typ `[n x [k x T]]`.
struct Mem {
    ptr: Reg,
    ty: LlvmType,
}

fn mem_of(m: &mut Module, ty: &LlvmType) -> Mem {
    Mem { ptr: m.alloca(ty), ty: ty.clone() }
}

fn at(m: &mut Module, mem: &Mem, i: usize, j: usize) -> Reg {
    m.inst(&format!("getelementptr inbounds {}, ptr {}, i32 0, i32 {i}, i32 {j}", mem.ty, mem.ptr))
}

fn load(m: &mut Module, t: &LlvmType, ptr: Reg) -> String {
    m.inst(&format!("load {t}, ptr {ptr}")).to_string()
}

fn store(m: &mut Module, t: &LlvmType, v: &str, ptr: Reg) {
    m.void_inst(&format!("store {t} {v}, ptr {ptr}"));
}

/// Was ein Pivot null ausloest.
enum OnSingular {
    /// Sprung in den Fault-Pfad (`inv`, `solve`).
    Fault(String),
    /// Nur merken; der Aufrufer waehlt sein Ergebnis danach (`det`).
    Flag,
}

/// Die LU-Zerlegung im Scratch: `mem` traegt L (unter der Diagonale,
/// Einsen implizit) und U, `perm` die Zeilen der Eingabe, `swaps` zaehlt
/// die Vertauschungen; `ok` ist bei `Flag` „kein Pivot war null".
struct Lu {
    mem: Mem,
    perm: Reg,
    perm_ty: String,
    n: usize,
    swaps: String,
    ok: String,
}

fn perm_at(m: &mut Module, lu: &Lu, i: usize) -> String {
    let p = m.inst(&format!("getelementptr inbounds {}, ptr {}, i32 0, i32 {i}", lu.perm_ty, lu.perm));
    m.inst(&format!("load i32, ptr {p}")).to_string()
}

fn lu(a: &Lowered, n: usize, t: &LlvmType, on_singular: OnSingular, m: &mut Module) -> Lu {
    let mem = mem_of(m, &a.ty);
    store(m, &a.ty, &a.value, mem.ptr);
    let perm_ty = LlvmType::Array(Box::new(LlvmType::Int(32)), n as u32);
    let perm = m.alloca(&perm_ty);
    for i in 0..n {
        let p = m.inst(&format!("getelementptr inbounds {perm_ty}, ptr {perm}, i32 0, i32 {i}"));
        m.void_inst(&format!("store i32 {i}, ptr {p}"));
    }
    let row_ty = format!("[{n} x {t}]");
    let zero = float_literal(0.0, t);
    let mut swaps = "0".to_string();
    let mut ok = "true".to_string();
    for k in 0..n {
        // Spaltenpivot: der betragsgroesste Eintrag, bei Gleichheit der erste.
        let mut p = k.to_string();
        let ptr = at(m, &mem, k, k);
        let v = load(m, t, ptr);
        let mut best = unary(m, "fabs", t, &v);
        for i in k + 1..n {
            let ptr = at(m, &mem, i, k);
            let v = load(m, t, ptr);
            let v = unary(m, "fabs", t, &v);
            let gt = m.inst(&format!("fcmp ogt {t} {v}, {best}"));
            best = m.inst(&format!("select i1 {gt}, {t} {v}, {t} {best}")).to_string();
            p = m.inst(&format!("select i1 {gt}, i32 {i}, i32 {p}")).to_string();
        }
        let singular = m.inst(&format!("fcmp oeq {t} {best}, {zero}"));
        match &on_singular {
            OnSingular::Fault(label) => {
                let go_on = format!("pivot{}", m.next_label());
                m.void_inst(&format!("br i1 {singular}, label %{label}, label %{go_on}"));
                m.label(&go_on);
            }
            OnSingular::Flag => {
                let fine = m.inst(&format!("xor i1 {singular}, true"));
                ok = m.inst(&format!("and i1 {ok}, {fine}")).to_string();
            }
        }
        // Zeilen k und p tauschen; bei p == k tauscht die Zeile mit sich selbst.
        let row_p = m.inst(&format!("getelementptr inbounds {}, ptr {}, i32 0, i32 {p}", mem.ty, mem.ptr));
        for j in 0..n {
            let ak = at(m, &mem, k, j);
            let ap = m.inst(&format!("getelementptr inbounds {row_ty}, ptr {row_p}, i32 0, i32 {j}"));
            let vk = load(m, t, ak);
            let vp = load(m, t, ap);
            store(m, t, &vp, ak);
            store(m, t, &vk, ap);
        }
        let pk = m.inst(&format!("getelementptr inbounds {perm_ty}, ptr {perm}, i32 0, i32 {k}"));
        let pp = m.inst(&format!("getelementptr inbounds {perm_ty}, ptr {perm}, i32 0, i32 {p}"));
        let ik = m.inst(&format!("load i32, ptr {pk}"));
        let ip = m.inst(&format!("load i32, ptr {pp}"));
        m.void_inst(&format!("store i32 {ip}, ptr {pk}"));
        m.void_inst(&format!("store i32 {ik}, ptr {pp}"));
        let moved = m.inst(&format!("icmp ne i32 {p}, {k}"));
        let inc = m.inst(&format!("zext i1 {moved} to i32"));
        swaps = m.inst(&format!("add i32 {swaps}, {inc}")).to_string();
        // Elimination.
        let ptr = at(m, &mem, k, k);
        let pivot = load(m, t, ptr);
        for i in k + 1..n {
            let aik = at(m, &mem, i, k);
            let v = load(m, t, aik);
            let l = m.inst(&format!("fdiv {t} {v}, {pivot}")).to_string();
            store(m, t, &l, aik);
            let nl = neg(m, t, &l);
            for j in k + 1..n {
                let akj = at(m, &mem, k, j);
                let aij = at(m, &mem, i, j);
                let u = load(m, t, akj);
                let w = load(m, t, aij);
                let v = fma(m, t, &nl, &u, &w);
                store(m, t, &v, aij);
            }
        }
    }
    Lu { mem, perm, perm_ty: perm_ty.to_string(), n, swaps, ok }
}

/// Loest `L·U·x = P·b` fuer die Spalte `col` von `out`; `rhs(i)` ist die
/// rechte Seite der Zeile `i` nach der Permutation.
fn lu_solve(m: &mut Module, t: &LlvmType, lu: &Lu, out: &Mem, col: usize, rhs: impl Fn(&mut Module, usize) -> String) {
    let n = lu.n;
    for i in 0..n {
        let mut acc = rhs(m, i);
        for j in 0..i {
            let ptr = at(m, &lu.mem, i, j);
            let l = load(m, t, ptr);
            let nl = neg(m, t, &l);
            let ptr = at(m, out, j, col);
            let x = load(m, t, ptr);
            acc = fma(m, t, &nl, &x, &acc);
        }
        let ptr = at(m, out, i, col);
        store(m, t, &acc, ptr);
    }
    for i in (0..n).rev() {
        let ptr = at(m, out, i, col);
        let mut acc = load(m, t, ptr);
        for j in i + 1..n {
            let ptr = at(m, &lu.mem, i, j);
            let u = load(m, t, ptr);
            let nu = neg(m, t, &u);
            let ptr = at(m, out, j, col);
            let x = load(m, t, ptr);
            acc = fma(m, t, &nu, &x, &acc);
        }
        let ptr = at(m, &lu.mem, i, i);
        let uii = load(m, t, ptr);
        let v = m.inst(&format!("fdiv {t} {acc}, {uii}")).to_string();
        let ptr = at(m, out, i, col);
        store(m, t, &v, ptr);
    }
}

fn det(a: &Lowered, n: usize, t: &LlvmType, m: &mut Module) -> Result<Lowered, NotYet> {
    let lu = lu(a, n, t, OnSingular::Flag, m);
    let odd = m.inst(&format!("and i32 {}, 1", lu.swaps));
    let is_odd = m.inst(&format!("icmp ne i32 {odd}, 0"));
    let (one, minus) = (float_literal(1.0, t), float_literal(-1.0, t));
    let mut d = m.inst(&format!("select i1 {is_odd}, {t} {minus}, {t} {one}")).to_string();
    for i in 0..n {
        let ptr = at(m, &lu.mem, i, i);
        let v = load(m, t, ptr);
        d = m.inst(&format!("fmul {t} {d}, {v}")).to_string();
    }
    let zero = float_literal(0.0, t);
    let v = m.inst(&format!("select i1 {}, {t} {d}, {t} {zero}", lu.ok));
    Ok(Lowered { value: v.to_string(), ty: t.clone() })
}

fn inv(
    a: &Lowered,
    n: usize,
    want: &LlvmType,
    t: &LlvmType,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let fault = vars.fault_label().ok_or(NotYet { what: "Matrixinversion ohne Fault-Pfad" })?;
    let lu = lu(a, n, t, OnSingular::Fault(fault), m);
    let out = mem_of(m, want);
    let (one, zero) = (float_literal(1.0, t), float_literal(0.0, t));
    for col in 0..n {
        lu_solve(m, t, &lu, &out, col, |m, i| {
            let pi = perm_at(m, &lu, i);
            let eq = m.inst(&format!("icmp eq i32 {pi}, {col}"));
            m.inst(&format!("select i1 {eq}, {t} {one}, {t} {zero}")).to_string()
        });
    }
    let v = m.inst(&format!("load {want}, ptr {}", out.ptr));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

fn solve(
    a: &Lowered,
    b: &Lowered,
    n: usize,
    want: &LlvmType,
    t: &LlvmType,
    m: &mut Module,
    vars: &dyn Vars,
) -> Result<Lowered, NotYet> {
    let fault = vars.fault_label().ok_or(NotYet { what: "`solve` ohne Fault-Pfad" })?;
    let (_, k, _) = shape(&b.ty).ok_or(NotYet { what: "rechte Seite von `solve`" })?;
    let lu = lu(a, n, t, OnSingular::Fault(fault), m);
    let bm = mem_of(m, &b.ty);
    store(m, &b.ty, &b.value, bm.ptr);
    let out = mem_of(m, want);
    for col in 0..k {
        lu_solve(m, t, &lu, &out, col, |m, i| {
            let pi = perm_at(m, &lu, i);
            let row = m.inst(&format!("getelementptr inbounds {}, ptr {}, i32 0, i32 {pi}", bm.ty, bm.ptr));
            let ptr = m.inst(&format!("getelementptr inbounds [{k} x {t}], ptr {row}, i32 0, i32 {col}"));
            load(m, t, ptr)
        });
    }
    let v = m.inst(&format!("load {want}, ptr {}", out.ptr));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}

fn cholesky(a: &Lowered, n: usize, want: &LlvmType, t: &LlvmType, m: &mut Module) -> Result<Lowered, NotYet> {
    let x = elements(m, a, n, n);
    let out = mem_of(m, &a.ty);
    m.write(&a.ty, "zeroinitializer", &out.ptr.to_string());
    let zero = float_literal(0.0, t);
    // Ohne Zweig: `ok` sammelt „jedes d > 0"; ein Wert mit `ok = false`
    // wird nicht gelesen, wie `none` im Interpreter.
    let mut ok = "true".to_string();
    for j in 0..n {
        let mut d = x[j * n + j].clone();
        for k in 0..j {
            let ptr = at(m, &out, j, k);
            let ljk = load(m, t, ptr);
            let nl = neg(m, t, &ljk);
            d = fma(m, t, &nl, &ljk, &d);
        }
        let pos = m.inst(&format!("fcmp ogt {t} {d}, {zero}"));
        ok = m.inst(&format!("and i1 {ok}, {pos}")).to_string();
        let l = unary(m, "sqrt", t, &d);
        let ptr = at(m, &out, j, j);
        store(m, t, &l, ptr);
        for i in j + 1..n {
            let mut s = x[i * n + j].clone();
            for k in 0..j {
                let ptr = at(m, &out, i, k);
                let lik = load(m, t, ptr);
                let nl = neg(m, t, &lik);
                let ptr = at(m, &out, j, k);
                let ljk = load(m, t, ptr);
                s = fma(m, t, &nl, &ljk, &s);
            }
            let v = m.inst(&format!("fdiv {t} {s}, {l}")).to_string();
            let ptr = at(m, &out, i, j);
            store(m, t, &v, ptr);
        }
    }
    let val = m.inst(&format!("load {}, ptr {}", a.ty, out.ptr));
    let with = m.inst(&format!("insertvalue {want} undef, {} {val}, 0", a.ty));
    let v = m.inst(&format!("insertvalue {want} {with}, i1 {ok}, 1"));
    Ok(Lowered { value: v.to_string(), ty: want.clone() })
}
