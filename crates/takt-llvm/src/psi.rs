//! Ψ im Prozessabbild (Referenz 9.4, 7.2, 5.8).
//!
//! Hinter den Commands liegen zwei gleich gebaute Baenke: Ψ_k, aus der
//! jede Maschine liest, und Ψ_{k+1}, in die `<m>_publish` nach dem
//! Schritt schreibt. Ein Follower liest die gefolgte Maschine aus der
//! zweiten Bank, sobald deren `fresh`-Byte gesetzt ist (7.2); die Runtime
//! kopiert am Tick-Ende die zweite Bank in die erste und loescht `fresh`
//! und die Signale (5.8: ein Signal ist einen Tick sichtbar).
//!
//! Die Felder liegen byteweise 8-ausgerichtet, ohne LLVM-Struct: Der
//! Rahmen rechnet dieselben Versaetze in C, und ein Struct-Layout haette
//! ein zweites Datenlayout daneben gestellt.

use takt_mir::machine::Machine;
use takt_mir::program::Program;
use takt_mir::{ChannelId, MachineId, SignalId, VarId};

use crate::emit::Module;
use crate::expr::{Lowered, NotYet};
use crate::machine::{self, Role, StateStruct};
use crate::ty::{self, LlvmType};

/// Ein Feld der Ψ-Region einer Maschine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// In dieser Schrittphase schon gelaufen (9.4, `fresh[m]`).
    Fresh,
    /// Blattzustand als Variante von `<m>.State`.
    State,
    /// Eine `pub var`.
    Var(VarId),
    /// Ein Signal.
    Signal(SignalId),
}

fn round8(x: u64) -> u64 {
    x.div_ceil(8) * 8
}

/// Versatz eines Feldes innerhalb der Region (8-ausgerichtet).
pub fn field_offset(machine: MachineId, field: Field, p: &Program) -> Option<u64> {
    let m = p.machines.get(machine.index())?;
    match field {
        Field::Fresh => return Some(0),
        Field::State => return Some(8),
        _ => {}
    }
    let mut off = 16;
    for (i, v) in m.vars.iter().enumerate() {
        if !v.public {
            continue;
        }
        if field == Field::Var(VarId(i as u32)) {
            return Some(off);
        }
        off += round8(ty::lower(v.ty, p)?.size());
    }
    for i in 0..m.signals.len() {
        if field == Field::Signal(SignalId(i as u32)) {
            return Some(off);
        }
        off += 8;
    }
    None
}

/// Groesse der Region einer Maschine.
pub fn region_size(machine: MachineId, p: &Program) -> u64 {
    let Some(m) = p.machines.get(machine.index()) else { return 0 };
    let vars: u64 =
        m.vars.iter().filter(|v| v.public).map(|v| round8(ty::lower(v.ty, p).map_or(8, |t| t.size()))).sum();
    16 + vars + 8 * m.signals.len() as u64
}

/// Anfang der ersten Bank: hinter den Commands, 8-ausgerichtet.
fn bank_base(p: &Program) -> u64 {
    let channels: u64 =
        (0..p.channels.len()).map(|i| crate::image::entry_type(ChannelId(i as u32), p).map_or(0, |t| t.size())).sum();
    round8(channels + p.commands.len() as u64)
}

/// Groesse einer Bank.
pub fn bank_size(p: &Program) -> u64 {
    (0..p.machines.len()).map(|i| region_size(MachineId(i as u32), p)).sum()
}

/// Versatz der Region einer Maschine im Abbild; `next` waehlt Ψ_{k+1}.
pub fn region_offset(machine: MachineId, next: bool, p: &Program) -> Option<u64> {
    p.machines.get(machine.index())?;
    let before: u64 = (0..machine.index()).map(|i| region_size(MachineId(i as u32), p)).sum();
    Some(bank_base(p) + if next { bank_size(p) } else { 0 } + before)
}

/// Groesse des Abbilds mit beiden Baenken.
pub fn image_size(p: &Program) -> u64 {
    bank_base(p) + 2 * bank_size(p)
}

fn field_type(machine: MachineId, field: Field, p: &Program) -> Option<LlvmType> {
    Some(match field {
        Field::Fresh | Field::Signal(_) => LlvmType::Int(8),
        Field::State => LlvmType::Int(32),
        Field::Var(v) => ty::lower(p.machines.get(machine.index())?.vars.get(v.index())?.ty, p)?,
    })
}

/// Laedt `m.x`, `m.state` oder ein Signal fuer `reader`; `%1` ist das
/// Abbild. Ein Follower liest frisch, wenn die Maschine schon lief (7.2).
pub fn load(reader: &Machine, target: MachineId, field: Field, p: &Program, m: &mut Module) -> Option<Lowered> {
    let fty = field_type(target, field, p)?;
    let off = field_offset(target, field, p)?;
    let psi = region_offset(target, false, p)? + off;
    let ptr = if reader.follows.contains(&target) {
        let next = region_offset(target, true, p)?;
        let flag = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {next}"));
        let fresh = m.inst(&format!("load i8, ptr {flag}"));
        let sel = m.inst(&format!("icmp ne i8 {fresh}, 0"));
        let a = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {}", next + off));
        let b = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {psi}"));
        m.inst(&format!("select i1 {sel}, ptr {a}, ptr {b}"))
    } else {
        m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {psi}"))
    };
    let raw = m.inst(&format!("load {fty}, ptr {ptr}"));
    Some(match field {
        Field::Signal(_) => {
            let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
            Lowered { value: b.to_string(), ty: LlvmType::Int(1) }
        }
        _ => Lowered { value: raw.to_string(), ty: fty },
    })
}

/// Wie [`load`], aber mit einem Index in ein Instanz-Array (5.11).
///
/// Die Regionen der Instanzen liegen hintereinander und sind gleich gross
/// — sie entstehen aus derselben Vorlage. Der Index ist darum ein
/// Vielfaches der Regionsgroesse auf der Adresse des ersten Elements.
/// Die Frischepruefung entfaellt: `follows` nennt eine Maschine, kein
/// Array, ein indizierter Zugriff hat also nie eine Kante.
pub fn load_indexed(
    first: MachineId,
    field: Field,
    len: u32,
    index: &Lowered,
    p: &Program,
    m: &mut Module,
) -> Option<Lowered> {
    let fty = field_type(first, field, p)?;
    let off = field_offset(first, field, p)?;
    let base = region_offset(first, false, p)? + off;
    let stride = region_size(first, p);
    // 3.4: Der Index ist gegen die Laenge geprueft, bevor er hier
    // ankommt; `urem` haelt die Adresse auch dann im Array, wenn eine
    // spaetere Aenderung die Pruefung verloere.
    let i64_index = match index.ty {
        LlvmType::Int(64) => index.value.clone(),
        ref t => m.inst(&format!("sext {t} {} to i64", index.value)).to_string(),
    };
    let safe = m.inst(&format!("urem i64 {i64_index}, {}", u64::from(len.max(1))));
    let delta = m.inst(&format!("mul i64 {safe}, {stride}"));
    let at = m.inst(&format!("add i64 {delta}, {base}"));
    let ptr = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {at}"));
    let raw = m.inst(&format!("load {fty}, ptr {ptr}"));
    Some(match field {
        Field::Signal(_) => {
            let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
            Lowered { value: b.to_string(), ty: LlvmType::Int(1) }
        }
        _ => Lowered { value: raw.to_string(), ty: fty },
    })
}

/// Ψ_k eines Feldes ohne Frischepruefung: fuer die Monitore nach dem
/// Commit des Ticks (13.3), wenn die erste Bank das Veroeffentlichte traegt.
pub fn load_bank(target: MachineId, field: Field, p: &Program, m: &mut Module) -> Option<Lowered> {
    let fty = field_type(target, field, p)?;
    let off = field_offset(target, field, p)?;
    let psi = region_offset(target, false, p)? + off;
    let ptr = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {psi}"));
    let raw = m.inst(&format!("load {fty}, ptr {ptr}"));
    Some(match field {
        Field::Signal(_) => {
            let b = m.inst(&format!("icmp ne i8 {raw}, 0"));
            Lowered { value: b.to_string(), ty: LlvmType::Int(1) }
        }
        _ => Lowered { value: raw.to_string(), ty: fty },
    })
}

/// `raise s` (5.8): das Signal steht ab jetzt in Ψ_{k+1} der eigenen Maschine.
pub fn raise(machine: MachineId, signal: SignalId, p: &Program, m: &mut Module) -> Option<()> {
    let off = region_offset(machine, true, p)? + field_offset(machine, Field::Signal(signal), p)?;
    let ptr = m.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {off}"));
    m.void_inst(&format!("store i8 1, ptr {ptr}"));
    Some(())
}

/// `void <m>_publish(ptr st, ptr image)`: `publish_m(v_m)` (9.4) — Zustand
/// und `pub var` nach Ψ_{k+1}, dazu `fresh`. Signale schreibt `raise`.
pub fn publish_function(m: &Machine, st: &StateStruct, p: &Program, module: &mut Module) -> Result<(), NotYet> {
    let id = MachineId(p.machines.iter().position(|x| x.name == m.name).ok_or(NotYet { what: "Maschine" })? as u32);
    let next = region_offset(id, true, p).ok_or(NotYet { what: "Psi-Region" })?;
    let leaves = machine::leaves(m);
    let variants = p.enums.iter().find(|e| e.name == format!("{}.State", m.name)).map(|e| &e.variants);
    let variant_of = |name: &str| variants.and_then(|v| v.iter().position(|x| x.name == name)).unwrap_or(0);
    let mark = module.mark();
    module.begin(&format!("{}_publish", m.name), &LlvmType::Void, &[LlvmType::Ptr, LlvmType::Ptr]);
    let flag = module.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {next}"));
    module.void_inst(&format!("store i8 1, ptr {flag}"));

    let state_ty = format!("%{}_state", crate::fns::sanitized(&m.name));
    let Some(conf_i) = st.index_of(Role::Conf, 0) else {
        module.abort(mark);
        return Err(NotYet { what: "conf im Zustand" });
    };
    let conf = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {conf_i}"));
    let slot = module.inst(&format!("getelementptr inbounds [{} x i8], ptr {conf}, i32 0, i32 0", st.depth));
    let cur = module.inst(&format!("load i8, ptr {slot}"));
    // Hinter dem letzten Blatt steht `FAULTED` (step.rs, `leave_configuration`).
    let mut acc = variant_of("FAULTED").to_string();
    for (i, leaf) in leaves.iter().enumerate() {
        let v = variant_of(&m.states[leaf.index()].name);
        let eq = module.inst(&format!("icmp eq i8 {cur}, {i}"));
        acc = module.inst(&format!("select i1 {eq}, i32 {v}, i32 {acc}")).to_string();
    }
    let state = module.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {}", next + 8));
    module.void_inst(&format!("store i32 {acc}, ptr {state}"));

    for (i, v) in m.vars.iter().enumerate() {
        if !v.public {
            continue;
        }
        let (Some(ty), Some(field), Some(off)) =
            (ty::lower(v.ty, p), st.index_of(Role::Var, i), field_offset(id, Field::Var(VarId(i as u32)), p))
        else {
            module.abort(mark);
            return Err(NotYet { what: "pub var in Psi" });
        };
        let src = module.inst(&format!("getelementptr inbounds {state_ty}, ptr %0, i32 0, i32 {field}"));
        let val = module.inst(&format!("load {ty}, ptr {src}"));
        let dst = module.inst(&format!("getelementptr inbounds i8, ptr %1, i64 {}", next + off));
        module.void_inst(&format!("store {ty} {val}, ptr {dst}"));
    }
    module.end(None);
    Ok(())
}
