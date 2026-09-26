//! Registerports im Wirtsrahmen (12.10, FB-261).
//!
//! Auf dem Wirt ruft der erzeugte Code fuer jeden Portzugriff die Runtime
//! (`takt_llvm::mmio`); hier ist der Rahmen die Runtime. Er bildet die
//! Adresse ab wie der Interpreter (`port_read`, `port_write`): Gelesen wird
//! der `sim`-Output `mmio/ADR/r`, den ein Modell stellt, mit Unit-Delay wie
//! jeder Modellwert (8.3), ohne Modell der Default des Records. Geschrieben
//! wird ein Element in den Eingabestrom `mmio/ADR/w`, und zwar der ganze
//! Record in kanonischer Form: Ein Feld darunter wird in den gelesenen
//! Record eingesetzt, wie `Eval::assign` es tut. Der Ring des Stroms zeigt
//! das Element ab dem naechsten Tick.

use std::fmt::Write;

use takt_llvm::ty::LlvmType;
use takt_mir::pattern::Address;
use takt_mir::program::{Binding, Direction, Port, Program};
use takt_mir::types::Type;
use takt_mir::{ChannelId, TypeId};

/// `(Port, Eingang, Elementtyp)` je Port mit Schreibstrom `mmio/ADR/w`.
pub(crate) fn write_streams(p: &Program) -> Vec<(usize, usize, TypeId)> {
    p.ports
        .iter()
        .enumerate()
        .filter_map(|(i, port)| {
            let (c, ch) = channel(p, port.address, "w", Direction::Input)?;
            match p.types.list.get(ch.ty.index()) {
                Some(Type::Stream(elem)) => Some((i, c, *elem)),
                _ => None,
            }
        })
        .collect()
}

/// Der Kanal `mmio/ADR/{side}` in Richtung `dir`.
fn channel<'a>(
    p: &'a Program,
    address: u64,
    side: &str,
    dir: Direction,
) -> Option<(usize, &'a takt_mir::program::Channel)> {
    let want = Address::simple(&format!("mmio/{address:#x}/{side}"));
    p.channels.iter().enumerate().find(|(_, c)| c.dir == dir && matches!(&c.binding, Binding::Sim(a) if *a == want))
}

/// Das Modell eines Ports: der Versatz seines Lesekanals im Latch, wenn ein
/// Modell ihn als Pegel stellt und er gebaut ist wie der Port. `Err` nennt,
/// warum der Rahmen ihn nicht abbilden kann.
fn model(p: &Program, port: &Port, ty: &LlvmType) -> Option<Result<u64, String>> {
    let (c, model) = channel(p, port.address, "r", Direction::Output)?;
    if matches!(p.types.list.get(model.ty.index()), Some(Type::Stream(_))) {
        return Some(Err(format!("Port {}: ein Strom an mmio/ADR/r fehlt im Wirtsrahmen", port.name)));
    }
    match takt_llvm::image::latch_offset(ChannelId(c as u32), p) {
        Some(offset) if takt_llvm::ty::lower(model.ty, p).as_ref() == Some(ty) => Some(Ok(offset)),
        _ => Some(Err(format!("Port {}: das Modell {} liegt anders als der Port", port.name, model.name))),
    }
}

/// Die kanonische Form (`takt_mir::bytes`) eines Werts im Speicher: je
/// Skalar `(Versatz, Laenge)`, in Feldordnung und ohne Padding. `None` fuer
/// Typen, die in einem Registerrecord nicht vorkommen.
fn canonical(p: &Program, ty: TypeId, llvm: &LlvmType, at: u64, out: &mut Vec<(u64, u64)>) -> Option<()> {
    match (p.types.list.get(ty.index())?, llvm) {
        (Type::Bool, LlvmType::Int(1)) => out.push((at, 1)),
        (Type::Int { width, .. }, LlvmType::Int(bits)) if width.bits() == *bits => {
            out.push((at, u64::from(bits / 8)));
        }
        (Type::Float { .. }, LlvmType::F32 | LlvmType::F64) => out.push((at, llvm.size())),
        (Type::Record(r), LlvmType::Struct(fields)) => {
            for (i, f) in p.records.get(r.index())?.fields.iter().enumerate() {
                canonical(p, f.ty, fields.get(i)?, at + llvm.field_offset(i), out)?;
            }
        }
        (Type::Array { elem, len }, LlvmType::Array(e, _)) => {
            for k in 0..u64::from(*len) {
                canonical(p, *elem, e, at + k * e.aligned_size(), out)?;
            }
        }
        _ => return None,
    }
    Some(())
}

/// Nimmt den Wert der Modelle in dem Stand, in dem er committet ist: am
/// Ende eines Ticks und vor Tick 0, dort, wo auch die `sim`-Bindungen
/// laufen. Gelesen wird er im naechsten Tick (8.3).
pub(crate) fn sample(s: &mut String, p: &Program, indent: &str) {
    for (i, port) in p.ports.iter().enumerate() {
        let Some(ty) = takt_llvm::ty::lower(port.ty, p) else { continue };
        if let Some(Ok(offset)) = model(p, port, &ty) {
            let _ = writeln!(s, "{indent}memcpy(g_port_model_{i}, latch + {offset}, {});", ty.aligned_size());
        }
    }
}

/// Schreibt `takt_mmio_read` und `takt_mmio_write`; ohne Ports nichts.
///
/// Was der Rahmen nicht abbilden kann, bricht den Bau des Rahmens mit
/// `#error` ab, statt still einen Default zu lesen: ein Modell, das den
/// Lesekanal als Strom stellt (ein Register, das beim Lesen weiterschaltet),
/// und ein Modellrecord, der anders im Speicher liegt als der Port.
pub(crate) fn emit(s: &mut String, p: &Program) {
    if p.ports.is_empty() {
        return;
    }
    let writes = write_streams(p);
    let mut sizes = Vec::new();
    let mut current = String::new();
    let mut send = String::new();
    let mut widest = 1;
    for (i, port) in p.ports.iter().enumerate() {
        let Some(ty) = takt_llvm::ty::lower(port.ty, p) else {
            sizes.push(1);
            let _ = writeln!(s, "#error \"Port {}: Record ohne Speicherform\"", port.name);
            continue;
        };
        let size = ty.aligned_size();
        sizes.push(size);
        match model(p, port, &ty) {
            Some(Ok(_)) => {
                let _ = writeln!(s, "static _Alignas(8) unsigned char g_port_model_{i}[{size}]; /* {} */", port.name);
                let _ = writeln!(current, "    case {i}: memcpy(whole, g_port_model_{i}, {size}); return;");
            }
            Some(Err(e)) => {
                let _ = writeln!(s, "#error \"{e}\"");
            }
            None => {}
        }
        if let Some(&(_, c, _)) = writes.iter().find(|(k, _, _)| *k == i) {
            let mut chunks = Vec::new();
            if canonical(p, port.ty, &ty, 0, &mut chunks).is_none() {
                let _ = writeln!(s, "#error \"Port {}: Record ohne kanonische Form\"", port.name);
                continue;
            }
            let mut len = 0;
            let _ = writeln!(send, "    case {i}: /* {} -> {} */", port.name, p.channels[c].name);
            for (at, n) in chunks {
                let _ = writeln!(send, "        memcpy(out + {len}, whole + {at}, {n});");
                len += n;
            }
            let _ = writeln!(send, "        (void)takt_int_send(takt_int_slot({c}), (const char *)out, {len});");
            let _ = writeln!(send, "        return;");
            widest = widest.max(len);
        }
    }
    let n = p.ports.len();
    let most = sizes.iter().copied().max().unwrap_or(1).max(1);
    let bases: Vec<String> = p.ports.iter().map(|port| format!("{:#x}LL", port.address)).collect();
    let sizes: Vec<String> = sizes.iter().map(u64::to_string).collect();
    let _ = writeln!(s, "/* Registerports (12.10): Adresse -> Port, Lesen vom Modell, Schreiben als Element. */");
    let _ = writeln!(s, "static const long long g_port_base[{n}] = {{ {} }};", bases.join(", "));
    let _ = writeln!(s, "static const int g_port_size[{n}] = {{ {} }};", sizes.join(", "));
    let _ = writeln!(s, "static int takt_port_of(long long addr, int n, int *off) {{");
    let _ = writeln!(s, "    for (int i = 0; i < {n}; i++)");
    let _ = writeln!(s, "        if (addr >= g_port_base[i] && addr + n <= g_port_base[i] + g_port_size[i]) {{");
    let _ = writeln!(s, "            *off = (int)(addr - g_port_base[i]);");
    let _ = writeln!(s, "            return i;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    return -1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_port_current(int i, unsigned char *whole) {{");
    let _ = writeln!(s, "    switch (i) {{");
    let _ = write!(s, "{current}");
    let _ = writeln!(s, "    default: memset(whole, 0, (size_t)g_port_size[i]); return;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_mmio_read(long long addr, void *dst, int n) {{");
    let _ = writeln!(s, "    unsigned char whole[{most}];");
    let _ = writeln!(s, "    int off = 0, i = takt_port_of(addr, n, &off);");
    let _ = writeln!(s, "    if (i < 0) {{ memset(dst, 0, (size_t)n); return; }}");
    let _ = writeln!(s, "    takt_port_current(i, whole);");
    let _ = writeln!(s, "    memcpy(dst, whole + off, (size_t)n);");
    let _ = writeln!(s, "}}");
    // Ein Feld unter dem Port hat keinen eigenen Speicher: Der ganze Record
    // wird gelesen, veraendert und als ein Element geschrieben.
    let _ = writeln!(s, "void takt_mmio_write(long long addr, const void *src, int n) {{");
    let _ = writeln!(s, "    unsigned char whole[{most}], out[{widest}];");
    let _ = writeln!(s, "    int off = 0, i = takt_port_of(addr, n, &off);");
    let _ = writeln!(s, "    if (i < 0) return;");
    let _ = writeln!(s, "    if (off != 0 || n != g_port_size[i]) takt_port_current(i, whole);");
    let _ = writeln!(s, "    memcpy(whole + off, src, (size_t)n);");
    let _ = writeln!(s, "    switch (i) {{");
    let _ = write!(s, "{send}");
    let _ = writeln!(s, "    default: (void)out; return;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
}
