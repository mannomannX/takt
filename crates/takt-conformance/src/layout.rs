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
    /// Ist der Wert vorzeichenbehaftet?
    ///
    /// LLVM kennt das nicht im Typ (dort steht nur die Breite), der
    /// Rahmen braucht es aber: Ein `u32` als `int` gelesen gibt
    /// 2286445522 als -2008521774 aus.
    pub signed: bool,
    /// Versatz in Bytes.
    pub offset: u64,
    /// Groesse in Bytes.
    pub size: u64,
    /// Der LLVM-Typ, der dort steht.
    pub ty: LlvmType,
    /// Name des Channels oder Parameters, fuer Meldungen.
    pub name: String,
    /// Die Hardware-Adresse, falls der Channel eine hat (8.1, 8.10).
    ///
    /// Sie ist der symbolische Schluessel auf ein Geraet — der Rahmen
    /// baut daraus den Namen der Treiberfunktion. `None` heisst `sim(...)`
    /// oder `none`: Dann gibt es nichts zu stellen.
    pub address: Option<takt_mir::pattern::Address>,
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
                out.inputs.push(Slot {
                    offset,
                    size: value_ty.size(),
                    ty: value_ty.clone(),
                    signed: is_signed(c.ty, p),
                    name: c.name.clone(),
                    address: hw_address(c),
                });
                out.image = out.image.max(offset + entry.aligned_size());
            }
        } else if let Some(offset) = image::latch_offset(id, p) {
            out.outputs.push(Slot {
                offset,
                size: value_ty.size(),
                ty: value_ty.clone(),
                signed: is_signed(c.ty, p),
                name: c.name.clone(),
                address: hw_address(c),
            });
            out.latch = out.latch.max(offset + value_ty.aligned_size());
        }
    }
    for (i, cmd) in p.commands.iter().enumerate() {
        if let Some(offset) = image::command_offset(takt_mir::CommandId(i as u32), p) {
            out.commands.push(Slot {
                offset,
                size: 1,
                ty: LlvmType::Int(8),
                signed: false,
                name: cmd.name.clone(),
                address: None,
            });
            out.image = out.image.max(offset + 1);
        }
    }
    for (i, param) in p.params.iter().enumerate() {
        let Some(t) = ty::lower(param.ty, p) else { continue };
        if let Some(offset) = image::param_offset(ParamId(i as u32), p) {
            out.parameters.push(Slot {
                offset,
                size: t.size(),
                ty: t.clone(),
                signed: is_signed(param.ty, p),
                name: param.name.clone(),
                address: None,
            });
            out.params = out.params.max(offset + t.aligned_size());
        }
    }
    // Hinter Channels und Commands liegen die zwei Ψ-Baenke (9.4, 7.2).
    out.image = out.image.max(takt_llvm::psi::image_size(p));
    // Hinter den Baenken die Job-Slots (4.5).
    out.image = out.image.max(image::jobs_end(p));
    // Ein leerer Vektor bekommt trotzdem ein Byte: `alloca 0` ist
    // gueltig, aber ein Zeiger darauf waere keiner, den man weiterreichen
    // moechte.
    out.image = out.image.max(1);
    out.latch = out.latch.max(1);
    out.params = out.params.max(1);
    out
}

/// Die Hardware-Adresse eines Channels, falls er eine hat.
///
/// `sim(...)` liefert `None`: Ein simulierter Channel hat kein Geraet, das
/// ihn stellt — 8.3 nennt die Bindung als die eine Stelle, an der sich
/// Sim- und HW-Bau unterscheiden duerfen.
fn hw_address(c: &takt_mir::program::Channel) -> Option<takt_mir::pattern::Address> {
    match &c.binding {
        takt_mir::program::Binding::Hw(a) => Some(a.clone()),
        takt_mir::program::Binding::Sim(_) | takt_mir::program::Binding::None => None,
    }
}

/// Ist der Typ vorzeichenbehaftet (3.2)?
///
/// Dauern sind Nanosekunden in `i64` und duerfen negativ sein; `bool` und
/// die Aufzaehlungen sind vorzeichenlos.
fn is_signed(ty: takt_mir::TypeId, p: &Program) -> bool {
    match p.types.list.get(ty.index()) {
        Some(takt_mir::types::Type::Int { width, .. }) => takt_llvm::ty::signed(*width),
        Some(takt_mir::types::Type::Duration { .. } | takt_mir::types::Type::Float { .. }) => true,
        _ => false,
    }
}

/// Deklariert einen ABI-Puffer mit der Ausrichtung, die der Codegen annimmt.
///
/// **Ein `unsigned char[]` ist die falsche Deklaration, und sie war lange
/// unauffaellig.** Der erzeugte Code sieht denselben Speicher als Struktur
/// mit `i64`-Feldern und greift mit `LDRD` darauf zu; ein Byte-Array hat in
/// C aber Ausrichtung 1, und der Linker legt es dahin, wo Platz ist. Auf
/// x86-64 kostet das nichts — die Architektur traegt unausgerichtete
/// Zugriffe. Auf ARMv7-M ist `LDRD` ohne Wortausrichtung ein UsageFault,
/// und weil `USGFAULTENA` nicht gesetzt ist, eskaliert er zum HardFault:
/// Das Programm steht, ohne ein Zeichen zu geben.
///
/// Gefunden auf dem STM32F401, wo `state_blink` auf 0x2000_0036 landete.
/// 12.8 nennt die Zielklassen aus genau diesem Grund verschieden: Was der
/// Wirt verzeiht, entscheidet nicht, was richtig ist.
///
/// Acht Byte, weil `i64` und `f64` die breitesten Felder der ABI sind
/// (3.2: Dauern sind `i64`-Nanosekunden).
pub fn c_buffer(name: &str, bytes: u64) -> String {
    format!("static _Alignas(8) unsigned char {name}[{}];", bytes.max(1))
}

/// Der C-Typ zu einem LLVM-Typ, fuer den Testrahmen.
///
/// Das Vorzeichen steht nicht im LLVM-Typ (dort ist nur die Breite), also
/// nimmt es der Aufrufer aus dem Slot mit.
pub fn c_type(t: &LlvmType, signed: bool) -> Option<&'static str> {
    Some(match (t, signed) {
        (LlvmType::Int(1), _) => "unsigned char",
        (LlvmType::Int(8), true) => "signed char",
        (LlvmType::Int(8), false) => "unsigned char",
        (LlvmType::Int(16), true) => "short",
        (LlvmType::Int(16), false) => "unsigned short",
        (LlvmType::Int(32), true) => "int",
        (LlvmType::Int(32), false) => "unsigned int",
        (LlvmType::Int(64), true) => "long long",
        (LlvmType::Int(64), false) => "unsigned long long",
        (LlvmType::F32, _) => "float",
        (LlvmType::F64, _) => "double",
        _ => return None,
    })
}
