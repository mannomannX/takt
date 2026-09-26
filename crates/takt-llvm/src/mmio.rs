//! Registerports (12.10): ein Helfer je Zugriff und Typ, dessen Rumpf das
//! Ziel bestimmt (FB-261, plan/m10.md 2.6).
//!
//! Auf einem Ziel ohne Betriebssystem ist ein Portzugriff ein `load
//! volatile` oder `store volatile` an der Adresse; der Helfer ist
//! `alwaysinline`, und das Objekt ist dasselbe wie ohne ihn. Auf einem Ziel
//! mit Betriebssystem liegt an der Adresse nichts: Dort ruft der Helfer die
//! Runtime ([`Abi::MMIO_READ`], [`Abi::MMIO_WRITE`]), und die bildet die
//! Adresse ab — der Testrahmen auf das Geraetemodell, wie der Interpreter.
//!
//! Neben dem Triple ist das der einzige Unterschied der IR zwischen den
//! Zielen. Die Rumpfe stehen darum am Ende des Moduls hinter [`HEADER`],
//! und `mcu_codegen.rs` haelt fest, dass die IR davor fuer jedes Ziel
//! dieselbe ist.

use std::fmt::Write;

use crate::abi::Abi;
use crate::ty::LlvmType;

/// Die Zeile vor den Rumpfen der Helfer.
pub const HEADER: &str = "; Registerports (12.10): der Rumpf je Ziel";

/// Lesen oder Schreiben.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// `load`.
    Read,
    /// `store`.
    Write,
}

/// Der Name des `i`-ten Helfers.
pub fn name(access: Access, i: usize) -> String {
    match access {
        Access::Read => format!("takt_mmio_read.{i}"),
        Access::Write => format!("takt_mmio_write.{i}"),
    }
}

/// Die Rumpfe der Helfer, die ein Modul ruft; `bare_metal` waehlt die Form.
pub fn definitions(helpers: &[(Access, LlvmType)], bare_metal: bool) -> String {
    let mut s = String::new();
    if helpers.is_empty() {
        return s;
    }
    let _ = writeln!(s, "\n{HEADER}");
    for (i, (access, ty)) in helpers.iter().enumerate() {
        let f = name(*access, i);
        let size = ty.aligned_size();
        let _ = match (access, bare_metal) {
            (Access::Read, true) => writeln!(
                s,
                "define internal {ty} @{f}(ptr %at) alwaysinline nounwind {{\n  %v = load volatile {ty}, ptr %at\n  ret {ty} %v\n}}"
            ),
            (Access::Write, true) => writeln!(
                s,
                "define internal void @{f}(ptr %at, {ty} %v) alwaysinline nounwind {{\n  store volatile {ty} %v, ptr %at\n  ret void\n}}"
            ),
            (Access::Read, false) => writeln!(
                s,
                "define internal {ty} @{f}(ptr %at) nounwind {{\n  %buf = alloca {ty}\n  %addr = ptrtoint ptr %at to i64\n  \
                 call void @{}(i64 %addr, ptr %buf, i32 {size})\n  %v = load {ty}, ptr %buf\n  ret {ty} %v\n}}",
                Abi::MMIO_READ
            ),
            (Access::Write, false) => writeln!(
                s,
                "define internal void @{f}(ptr %at, {ty} %v) nounwind {{\n  %buf = alloca {ty}\n  store {ty} %v, ptr %buf\n  \
                 %addr = ptrtoint ptr %at to i64\n  call void @{}(i64 %addr, ptr %buf, i32 {size})\n  ret void\n}}",
                Abi::MMIO_WRITE
            ),
        };
    }
    s
}
