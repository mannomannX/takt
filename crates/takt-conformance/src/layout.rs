//! Die Speicherform der ABI-Vektoren, wie der erzeugte Code sie erwartet.
//!
//! **Die Versaetze kommen aus `takt-llvm`, nicht aus einer zweiten
//! Rechnung.** Das ist der Kern: Ein Testrahmen, der die Adressen selbst
//! ausrechnet, prueft seine eigene Rechnung gegen die des Codegens — und
//! findet genau dann nichts, wenn beide denselben Fehler machen. Er ruft
//! darum `image::offset_of`, `latch_offset` und `param_offset` auf.
//!
//! Was er *nicht* aus dem Codegen nimmt, ist die Bedeutung: Welcher Wert
//! an einem Versatz stehen soll, sagt der Interpreter.

use takt_llvm::image;
use takt_llvm::ty::{self, LlvmType};
use takt_mir::program::{Direction, Program};
use takt_mir::{ChannelId, ParamId};

/// Ein Feld im Prozessabbild, Latch oder Parametervektor.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// Versatz in Bytes.
    pub offset: u64,
    /// Groesse in Bytes.
    pub size: u64,
    /// Der LLVM-Typ, der dort steht.
    pub ty: LlvmType,
    /// Name des Channels oder Parameters, fuer Meldungen.
    pub name: String,
}

/// Die Speicherform eines Programms.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    /// Groesse des Prozessabbilds in Bytes.
    pub image: u64,
    /// Groesse des Latches in Bytes.
    pub latch: u64,
    /// Groesse des Parametervektors in Bytes.
    pub params: u64,
    /// Die Eingaben mit ihrem Platz im Abbild.
    pub inputs: Vec<Slot>,
    /// Die Ausgaben mit ihrem Platz im Latch.
    pub outputs: Vec<Slot>,
    /// Die Parameter mit ihrem Platz im Vektor.
    pub parameters: Vec<Slot>,
    /// Die Commands mit ihrem Platz im Abbild (je ein Byte, 8.5).
    pub commands: Vec<Slot>,
}

/// Rechnet die Speicherform eines Programms aus.
pub fn of(p: &Program) -> Layout {
    let mut out = Layout::default();
    for (i, c) in p.channels.iter().enumerate() {
        let id = ChannelId(i as u32);
        let Some(value_ty) = ty::lower(c.ty, p) else { continue };
        if c.dir == Direction::Input {
            if let (Some(offset), Some(entry)) = (image::offset_of(id, p), image::entry_type(id, p)) {
                out.inputs.push(Slot { offset, size: value_ty.size(), ty: value_ty.clone(), name: c.name.clone() });
                out.image = out.image.max(offset + entry.size());
            }
        } else if let Some(offset) = image::latch_offset(id, p) {
            out.outputs.push(Slot { offset, size: value_ty.size(), ty: value_ty.clone(), name: c.name.clone() });
            out.latch = out.latch.max(offset + value_ty.size());
        }
    }
    for (i, cmd) in p.commands.iter().enumerate() {
        if let Some(offset) = image::command_offset(takt_mir::CommandId(i as u32), p) {
            out.commands.push(Slot { offset, size: 1, ty: LlvmType::Int(8), name: cmd.name.clone() });
            out.image = out.image.max(offset + 1);
        }
    }
    for (i, param) in p.params.iter().enumerate() {
        let Some(t) = ty::lower(param.ty, p) else { continue };
        if let Some(offset) = image::param_offset(ParamId(i as u32), p) {
            out.parameters.push(Slot { offset, size: t.size(), ty: t.clone(), name: param.name.clone() });
            out.params = out.params.max(offset + t.size());
        }
    }
    // Ein leerer Vektor bekommt trotzdem ein Byte: `alloca 0` ist
    // gueltig, aber ein Zeiger darauf waere keiner, den man weiterreichen
    // moechte.
    out.image = out.image.max(1);
    out.latch = out.latch.max(1);
    out.params = out.params.max(1);
    out
}

/// Der C-Typ zu einem LLVM-Typ, fuer den Testrahmen.
pub fn c_type(t: &LlvmType) -> Option<&'static str> {
    Some(match t {
        LlvmType::Int(1) => "unsigned char",
        LlvmType::Int(8) => "signed char",
        LlvmType::Int(16) => "short",
        LlvmType::Int(32) => "int",
        LlvmType::Int(64) => "long long",
        LlvmType::F32 => "float",
        LlvmType::F64 => "double",
        _ => return None,
    })
}
