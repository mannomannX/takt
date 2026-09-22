//! `<maschine>_persist_snapshot` und `_persist_restore` (5.9, 12.3).
//!
//! Der Codegen ist die einzige Stelle, die Struct-Layout und Typen der
//! Variablen zugleich kennt; die Runtime sieht nur Bytes (`takt-rt-core`
//! darf die MIR nicht kennen). Beide Funktionen erzeugen dieselbe
//! kanonische Form wie `takt_interp::persist` — auf little-endian-Zielen
//! ist ein `store iN … align 1` genau diese Form.
//!
//! `_restore` prueft erst und schreibt dann: Ein Eintrag mit falscher
//! Laenge, fremder Diskriminante oder Wert ausserhalb der Range wird als
//! Ganzes uebergangen, nie halb uebernommen (5.9: Default plus
//! `PersistReset`; den Alert meldet der Rahmen aus dem Rueckgabewert).

use takt_mir::TypeId;
use takt_mir::bytes::max_size;
use takt_mir::machine::Machine;
use takt_mir::program::Program;
use takt_mir::types::{Const, FloatWidth, Range, Type};

use crate::emit::{Module, Reg};
use crate::expr::NotYet;
use crate::machine::{Role, StateStruct};
use crate::stmt::Ctx;
use crate::ty::LlvmType;

/// Die `persist`-Variablen einer Maschine, nach Typ-Hash sortiert — wie
/// der Interpreter sie kodiert.
fn entries(m: &Machine) -> Vec<(u64, usize, TypeId)> {
    let mut out: Vec<(u64, usize, TypeId)> =
        m.persist.iter().map(|pv| (pv.type_hash, pv.var.index(), m.vars[pv.var.index()].ty)).collect();
    out.sort_by_key(|(h, _, _)| *h);
    out
}

/// `i32 <m>_persist_snapshot(ptr state, ptr out, i32 cap)`: schreibt die
/// kanonische Form aller `persist`-Variablen und liefert die Laenge; 0,
/// wenn `cap` unter der statischen Schranke liegt.
pub fn snapshot_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    if m.persist.is_empty() {
        return Ok(());
    }
    let mut bound = 0u64;
    for (_, _, ty) in entries(m) {
        bound += 12 + u64::from(max_size(p, ty).map_err(|_| NotYet { what: "persist ohne Byte-Form" })?);
    }
    let mark = module.mark();
    let params = module.begin(
        &format!("{}_persist_snapshot", m.name),
        &LlvmType::Int(32),
        &[LlvmType::Ptr, LlvmType::Ptr, LlvmType::Int(32)],
    );
    let out = params[1];
    let cap = module.inst(&format!("zext i32 {} to i64", params[2]));
    let fits = module.inst(&format!("icmp uge i64 {cap}, {bound}"));
    module.void_inst(&format!("br i1 {fits}, label %ps_go, label %ps_small"));
    module.label("ps_small");
    module.void_inst("ret i32 0");
    module.label("ps_go");

    let ctx = Ctx::new(m, st, p);
    let mut w = Writer { p, out, module };
    let mut off = w.module.inst("add i64 0, 0");
    for (hash, i, ty) in entries(m) {
        let Some(src) = ctx.field(Role::Var, i, w.module) else {
            w.module.abort(mark);
            return Err(NotYet { what: "persist-Variable im Zustand" });
        };
        let head = off;
        w.store_at(head, "i64", &hash.to_string());
        off = w.module.inst(&format!("add i64 {off}, 12"));
        let body = off;
        off = match w.encode(ty, src, off) {
            Ok(o) => o,
            Err(e) => {
                w.module.abort(mark);
                return Err(e);
            }
        };
        let len = w.module.inst(&format!("sub i64 {off}, {body}"));
        let len32 = w.module.inst(&format!("trunc i64 {len} to i32"));
        let at = w.module.inst(&format!("add i64 {head}, 8"));
        w.store_at(at, "i32", &len32.to_string());
    }
    let total = module.inst(&format!("trunc i64 {off} to i32"));
    module.end(Some((&LlvmType::Int(32), total.to_string())));
    Ok(())
}

struct Writer<'a> {
    p: &'a Program,
    out: Reg,
    module: &'a mut Module,
}

impl Writer<'_> {
    fn store_at(&mut self, off: Reg, ty: &str, value: &str) {
        let dst = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off}", self.out));
        self.module.void_inst(&format!("store {ty} {value}, ptr {dst}, align 1"));
    }

    fn encode(&mut self, ty: TypeId, src: Reg, off: Reg) -> Result<Reg, NotYet> {
        let t = self.p.types.list.get(ty.index()).ok_or(NotYet { what: "persist-Typ" })?;
        let llvm = crate::ty::lower(ty, self.p).ok_or(NotYet { what: "persist-Typ im Codegen" })?;
        Ok(match t {
            Type::Bool => {
                let v = self.module.inst(&format!("load i1, ptr {src}"));
                let b = self.module.inst(&format!("zext i1 {v} to i8"));
                self.store_at(off, "i8", &b.to_string());
                self.module.inst(&format!("add i64 {off}, 1"))
            }
            Type::Int { width, .. } => {
                let v = self.module.inst(&format!("load {llvm}, ptr {src}"));
                self.store_at(off, &llvm.to_string(), &v.to_string());
                self.module.inst(&format!("add i64 {off}, {}", width.bits() / 8))
            }
            Type::Float { width, .. } => {
                let (bits, n) = match width {
                    FloatWidth::F32 => ("i32", 4),
                    FloatWidth::F64 => ("i64", 8),
                };
                let v = self.module.inst(&format!("load {llvm}, ptr {src}"));
                let raw = self.module.inst(&format!("bitcast {llvm} {v} to {bits}"));
                self.store_at(off, bits, &raw.to_string());
                self.module.inst(&format!("add i64 {off}, {n}"))
            }
            Type::Duration { .. } => {
                let v = self.module.inst(&format!("load i64, ptr {src}"));
                self.store_at(off, "i64", &v.to_string());
                self.module.inst(&format!("add i64 {off}, 8"))
            }
            Type::Enum(_) => {
                let v = self.module.inst(&format!("load i32, ptr {src}"));
                let wide = self.module.inst(&format!("sext i32 {v} to i64"));
                self.store_at(off, "i64", &wide.to_string());
                self.module.inst(&format!("add i64 {off}, 8"))
            }
            Type::Record(r) => {
                let fields: Vec<TypeId> = self.p.records[r.index()].fields.iter().map(|f| f.ty).collect();
                let mut off = off;
                for (i, fty) in fields.into_iter().enumerate() {
                    let fp = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {src}, i32 0, i32 {i}"));
                    off = self.encode(fty, fp, off)?;
                }
                off
            }
            Type::Array { elem, len } => {
                let (elem, len) = (*elem, *len);
                let mut off = off;
                for i in 0..len {
                    let ep = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {src}, i32 0, i32 {i}"));
                    off = self.encode(elem, ep, off)?;
                }
                off
            }
            // Die Slots einer `map` sind ihre Byteform (3.9, 5.9): kopieren.
            Type::Map { .. } => {
                let n = u64::from(takt_mir::bytes::max_size(self.p, ty).map_err(|_| NotYet { what: "map-Typ" })?);
                let dst = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off}", self.out));
                self.module
                    .void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {src}, i64 {n}, i1 false)"));
                self.module.inst(&format!("add i64 {off}, {n}"))
            }
            Type::Bytes { .. } | Type::Str { .. } => {
                let lp = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {src}, i32 0, i32 0"));
                let len = self.module.inst(&format!("load i32, ptr {lp}"));
                self.store_at(off, "i32", &len.to_string());
                let off = self.module.inst(&format!("add i64 {off}, 4"));
                let ap = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {src}, i32 0, i32 1"));
                let dst = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off}", self.out));
                let n = self.module.inst(&format!("zext i32 {len} to i64"));
                self.module
                    .void_inst(&format!("call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {ap}, i64 {n}, i1 false)"));
                self.module.inst(&format!("add i64 {off}, {n}"))
            }
            _ => return Err(NotYet { what: "persist-Typ im Codegen" }),
        })
    }
}

/// `i32 <m>_persist_restore(ptr state, ptr in, i32 len)`: uebernimmt die
/// gueltigen Eintraege einer Journal-Nutzlast und liefert ihre Zahl.
///
/// Eintraege unter fremdem Typ-Hash werden uebersprungen; ein
/// verstuemmelter Rest beendet das Lesen. Beides ist kein Fehler — der
/// Rahmen vergleicht die Zahl mit der erwarteten und meldet den Rest als
/// `PersistReset`.
pub fn restore_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    if m.persist.is_empty() {
        return Ok(());
    }
    let mark = module.mark();
    let params = module.begin(
        &format!("{}_persist_restore", m.name),
        &LlvmType::Int(32),
        &[LlvmType::Ptr, LlvmType::Ptr, LlvmType::Int(32)],
    );
    let input = params[1];
    let off = module.inst("alloca i64");
    let applied = module.inst("alloca i32");
    module.void_inst(&format!("store i64 0, ptr {off}"));
    module.void_inst(&format!("store i32 0, ptr {applied}"));
    let total = module.inst(&format!("zext i32 {} to i64", params[2]));
    module.void_inst("br label %pr_head");

    module.label("pr_head");
    let o = module.inst(&format!("load i64, ptr {off}"));
    let rem = module.inst(&format!("sub i64 {total}, {o}"));
    let has = module.inst(&format!("icmp uge i64 {rem}, 12"));
    module.void_inst(&format!("br i1 {has}, label %pr_entry, label %pr_done"));

    module.label("pr_entry");
    let hp = module.inst(&format!("getelementptr i8, ptr {input}, i64 {o}"));
    let hash = module.inst(&format!("load i64, ptr {hp}, align 1"));
    let o8 = module.inst(&format!("add i64 {o}, 8"));
    let lp = module.inst(&format!("getelementptr i8, ptr {input}, i64 {o8}"));
    let len32 = module.inst(&format!("load i32, ptr {lp}, align 1"));
    let len = module.inst(&format!("zext i32 {len32} to i64"));
    let body = module.inst(&format!("add i64 {o}, 12"));
    let end = module.inst(&format!("add i64 {body}, {len}"));
    let fits = module.inst(&format!("icmp ule i64 {end}, {total}"));
    module.void_inst(&format!("br i1 {fits}, label %pr_dispatch, label %pr_done"));

    module.label("pr_dispatch");
    module.void_inst(&format!("store i64 {end}, ptr {off}"));
    let list = entries(m);
    let cases: Vec<String> = list.iter().enumerate().map(|(k, (h, _, _))| format!("i64 {h}, label %pr_v{k}")).collect();
    module.void_inst(&format!("switch i64 {hash}, label %pr_head [ {} ]", cases.join(" ")));

    let ctx = Ctx::new(m, st, p);
    for (k, (_, i, ty)) in list.into_iter().enumerate() {
        module.label(&format!("pr_v{k}"));
        let Some(dst) = ctx.field(Role::Var, i, module) else {
            module.abort(mark);
            return Err(NotYet { what: "persist-Variable im Zustand" });
        };
        let mut r = Reader { p, input, end, labels: 0, prefix: format!("pr_v{k}"), module };
        let after = match r.decode(ty, dst, body, false) {
            Ok(a) => a,
            Err(e) => {
                module.abort(mark);
                return Err(e);
            }
        };
        let exact = r.module.inst(&format!("icmp eq i64 {after}, {end}"));
        r.module.void_inst(&format!("br i1 {exact}, label %pr_v{k}_store, label %pr_head"));
        r.module.label(&format!("pr_v{k}_store"));
        if let Err(e) = r.decode(ty, dst, body, true) {
            module.abort(mark);
            return Err(e);
        }
        let n = module.inst(&format!("load i32, ptr {applied}"));
        let n1 = module.inst(&format!("add i32 {n}, 1"));
        module.void_inst(&format!("store i32 {n1}, ptr {applied}"));
        module.void_inst("br label %pr_head");
    }

    module.label("pr_done");
    let n = module.inst(&format!("load i32, ptr {applied}"));
    module.end(Some((&LlvmType::Int(32), n.to_string())));
    Ok(())
}

struct Reader<'a> {
    p: &'a Program,
    input: Reg,
    end: Reg,
    labels: u32,
    prefix: String,
    module: &'a mut Module,
}

impl Reader<'_> {
    fn load_at(&mut self, off: Reg, ty: &str) -> Reg {
        let src = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off}", self.input));
        self.module.inst(&format!("load {ty}, ptr {src}, align 1"))
    }

    /// Prueft eine Bedingung; bei `false` gilt der Eintrag nicht.
    fn require(&mut self, ok: Reg) {
        self.labels += 1;
        let go = format!("{}_c{}", self.prefix, self.labels);
        self.module.void_inst(&format!("br i1 {ok}, label %{go}, label %pr_head"));
        self.module.label(&go);
    }

    fn int_range(&mut self, v: Reg, range: &Option<Range>) {
        let Some(r) = range else { return };
        if let (Const::Int(lo), Const::Int(hi)) | (Const::Duration(lo), Const::Duration(hi)) = (r.lo, r.hi) {
            let ge = self.module.inst(&format!("icmp sge i64 {v}, {lo}"));
            self.require(ge);
            let le = self.module.inst(&format!("icmp sle i64 {v}, {hi}"));
            self.require(le);
        }
    }

    fn decode(&mut self, ty: TypeId, dst: Reg, off: Reg, store: bool) -> Result<Reg, NotYet> {
        let t = self.p.types.list.get(ty.index()).ok_or(NotYet { what: "persist-Typ" })?;
        let llvm = crate::ty::lower(ty, self.p).ok_or(NotYet { what: "persist-Typ im Codegen" })?;
        Ok(match t {
            Type::Bool => {
                let b = self.load_at(off, "i8");
                if store {
                    let v = self.module.inst(&format!("icmp ne i8 {b}, 0"));
                    self.module.void_inst(&format!("store i1 {v}, ptr {dst}"));
                } else {
                    let ok = self.module.inst(&format!("icmp ule i8 {b}, 1"));
                    self.require(ok);
                }
                self.module.inst(&format!("add i64 {off}, 1"))
            }
            Type::Int { width, range, .. } => {
                let v = self.load_at(off, &llvm.to_string());
                if store {
                    self.module.void_inst(&format!("store {llvm} {v}, ptr {dst}"));
                } else if range.is_some() {
                    let ext = if width.signed() { "sext" } else { "zext" };
                    let wide =
                        if width.bits() == 64 { v } else { self.module.inst(&format!("{ext} {llvm} {v} to i64")) };
                    self.int_range(wide, range);
                }
                self.module.inst(&format!("add i64 {off}, {}", width.bits() / 8))
            }
            Type::Float { width, range, .. } => {
                let (bits, n) = match width {
                    FloatWidth::F32 => ("i32", 4),
                    FloatWidth::F64 => ("i64", 8),
                };
                let raw = self.load_at(off, bits);
                let v = self.module.inst(&format!("bitcast {bits} {raw} to {llvm}"));
                if store {
                    self.module.void_inst(&format!("store {llvm} {v}, ptr {dst}"));
                } else if let Some(r) = range {
                    if let (Const::Float(lo), Const::Float(hi)) = (r.lo, r.hi) {
                        let lo = crate::emit::float_literal(lo, &llvm);
                        let hi = crate::emit::float_literal(hi, &llvm);
                        let ge = self.module.inst(&format!("fcmp oge {llvm} {v}, {lo}"));
                        self.require(ge);
                        let le = self.module.inst(&format!("fcmp ole {llvm} {v}, {hi}"));
                        self.require(le);
                    }
                }
                self.module.inst(&format!("add i64 {off}, {n}"))
            }
            Type::Duration { range } => {
                let v = self.load_at(off, "i64");
                if store {
                    self.module.void_inst(&format!("store i64 {v}, ptr {dst}"));
                } else {
                    self.int_range(v, range);
                }
                self.module.inst(&format!("add i64 {off}, 8"))
            }
            Type::Enum(e) => {
                let d = self.load_at(off, "i64");
                if store {
                    let v = self.module.inst(&format!("trunc i64 {d} to i32"));
                    self.module.void_inst(&format!("store i32 {v}, ptr {dst}"));
                } else {
                    let discs: Vec<i64> = self.p.enums[e.index()].variants.iter().map(|v| v.discriminant).collect();
                    let mut any = self.module.inst("add i1 0, 0");
                    for disc in discs {
                        let eq = self.module.inst(&format!("icmp eq i64 {d}, {disc}"));
                        any = self.module.inst(&format!("or i1 {any}, {eq}"));
                    }
                    self.require(any);
                }
                self.module.inst(&format!("add i64 {off}, 8"))
            }
            Type::Record(r) => {
                let fields: Vec<TypeId> = self.p.records[r.index()].fields.iter().map(|f| f.ty).collect();
                let mut off = off;
                for (i, fty) in fields.into_iter().enumerate() {
                    let fp = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 {i}"));
                    off = self.decode(fty, fp, off, store)?;
                }
                off
            }
            // 8.9: Kopf `t, pre, post, rate`, dann `N` Abtastwerte; der
            // Struct traegt dieselben fuenf Felder (`ty::lower`).
            Type::Capture { elem, len } => {
                let (elem, len) = (*elem, *len);
                let mut off = off;
                for (i, w, n) in [(0usize, "i64", 8), (1, "i32", 4), (2, "i32", 4), (3, "double", 8)] {
                    let v = self.load_at(off, w);
                    if store {
                        let fp = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 {i}"));
                        self.module.void_inst(&format!("store {w} {v}, ptr {fp}"));
                    }
                    off = self.module.inst(&format!("add i64 {off}, {n}"));
                }
                let samples = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 4"));
                let inner = crate::ty::lower(elem, self.p).ok_or(NotYet { what: "Capture-Elementtyp" })?;
                let arr = crate::ty::LlvmType::Array(Box::new(inner), len);
                for i in 0..len {
                    let ep = self.module.inst(&format!("getelementptr inbounds {arr}, ptr {samples}, i32 0, i32 {i}"));
                    off = self.decode(elem, ep, off, store)?;
                }
                off
            }
            Type::Array { elem, len } => {
                let (elem, len) = (*elem, *len);
                let mut off = off;
                for i in 0..len {
                    let ep = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 {i}"));
                    off = self.decode(elem, ep, off, store)?;
                }
                off
            }
            Type::Map { .. } => {
                let n = u64::from(takt_mir::bytes::max_size(self.p, ty).map_err(|_| NotYet { what: "map-Typ" })?);
                let after = self.module.inst(&format!("add i64 {off}, {n}"));
                if store {
                    let src = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off}", self.input));
                    self.module.void_inst(&format!(
                        "call void @llvm.memcpy.p0.p0.i64(ptr {dst}, ptr {src}, i64 {n}, i1 false)"
                    ));
                } else {
                    let inside = self.module.inst(&format!("icmp ule i64 {after}, {}", self.end));
                    self.require(inside);
                }
                after
            }
            Type::Bytes { cap } | Type::Str { cap } => {
                let cap = *cap;
                let len = self.load_at(off, "i32");
                let off4 = self.module.inst(&format!("add i64 {off}, 4"));
                let n = self.module.inst(&format!("zext i32 {len} to i64"));
                let after = self.module.inst(&format!("add i64 {off4}, {n}"));
                if store {
                    let lp = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 0"));
                    self.module.void_inst(&format!("store i32 {len}, ptr {lp}"));
                    let ap = self.module.inst(&format!("getelementptr inbounds {llvm}, ptr {dst}, i32 0, i32 1"));
                    let src = self.module.inst(&format!("getelementptr i8, ptr {}, i64 {off4}", self.input));
                    self.module.void_inst(&format!(
                        "call void @llvm.memcpy.p0.p0.i64(ptr {ap}, ptr {src}, i64 {n}, i1 false)"
                    ));
                } else {
                    let small = self.module.inst(&format!("icmp ule i32 {len}, {cap}"));
                    self.require(small);
                    let inside = self.module.inst(&format!("icmp ule i64 {after}, {}", self.end));
                    self.require(inside);
                }
                after
            }
            _ => return Err(NotYet { what: "persist-Typ im Codegen" }),
        })
    }
}

/// Die kanonische Form eines Werts fuer die Grenze zu einer nativen
/// Funktion (4.5): schreibt den Wert unter `src` nach `out` und liefert
/// die Laenge.
pub(crate) fn encode_canonical(
    p: &Program,
    ty: TypeId,
    src: Reg,
    out: Reg,
    module: &mut Module,
) -> Result<Reg, NotYet> {
    let zero = module.inst("add i64 0, 0");
    let mut w = Writer { p, out, module };
    w.encode(ty, src, zero)
}

/// Liest die kanonische Form ab `input` in den Wert unter `dst`. Die
/// Native gehoert zur TCB; ihre Bytes gelten ohne Pruefung.
pub(crate) fn decode_canonical(
    p: &Program,
    ty: TypeId,
    input: Reg,
    dst: Reg,
    module: &mut Module,
) -> Result<(), NotYet> {
    let zero = module.inst("add i64 0, 0");
    let mut r = Reader { p, input, end: zero, labels: 0, prefix: "native".into(), module };
    r.decode(ty, dst, zero, true).map(|_| ())
}

/// Ein Wert in kanonischer Form, mit Nullen auf `len` Byte aufgefuellt —
/// Schluessel und Wert einer `map` (3.9). Liefert den Puffer.
pub(crate) fn encode_padded(
    e: &takt_mir::expr::Expr,
    len: u32,
    p: &Program,
    module: &mut Module,
    vars: &dyn crate::expr::Vars,
) -> Result<Reg, NotYet> {
    let v = crate::expr::lower(e, p, module, vars)?;
    let tmp = module.alloca(&v.ty);
    module.void_inst(&format!("store {} {}, ptr {tmp}", v.ty, v.value));
    let buf = module.inst(&format!("alloca [{len} x i8]"));
    module.write(&LlvmType::Array(Box::new(LlvmType::Int(8)), len), "zeroinitializer", &buf.to_string());
    encode_canonical(p, e.ty, tmp, buf, module)?;
    Ok(buf)
}

/// Schluessel- und Wertbreite einer `map` in Byte (`bytes::max_size`).
pub(crate) fn map_widths(p: &Program, key: TypeId, value: TypeId) -> Result<(u32, u32), NotYet> {
    let k = takt_mir::bytes::max_size(p, key).map_err(|_| NotYet { what: "map-Schluessel ohne Byteform" })?;
    let v = takt_mir::bytes::max_size(p, value).map_err(|_| NotYet { what: "map-Wert ohne Byteform" })?;
    Ok((k, v))
}
