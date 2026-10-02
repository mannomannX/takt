//! Die Treiber eines Programms (8.10, 12.6, 12.11): eine Funktion je
//! gebundener Adresse und je Geraet mit Heartbeat.
//!
//! **Eine Liste, vier Formen.** Aus derselben Liste entstehen die
//! Prototypen in C, der Trait `Drivers` fuer Rust, der Kleber vom C-Aufruf
//! zur Methode und fuer einen Pruefstand die Verdrahtung aus einer Tabelle.
//! Die Bindung ist stark: Der Rahmen deklariert jeden Treiber nur. Fehlt
//! einer, linkt das Abbild nicht (C) oder die Firmware uebersetzt nicht
//! (Rust); einen Vorgabewert gibt es nicht.
//!
//! **Stummel nur ausdruecklich.** Wer einen Kanal ohne Treiber laesst —
//! ein Test, ein Pruefstand ohne das Geraet —, verlangt dafuer einen
//! Stummel ([`c_stubs`], die Stummel in [`rust_rig`]). Er steht im
//! erzeugten Text, mit dem Kanal benannt.
//!
//! **Die Schnittstelle hat feste Breiten** (12.11): `uint8_t` fuer
//! Wahrheitswerte und Qualitaet, `int64_t` fuer Zeit, `int32_t` fuer
//! Laengen; jeder Treiber bekommt vorn den Zeiger des Wirts und die Zeit
//! des Ticks.

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;
use takt_llvm::ty::LlvmType;
use takt_mir::program::{Binding, Direction, Program, RecordedValue};
use takt_mir::types::{FloatWidth, Type};

use crate::layout::Layout;

/// Was ein Treiber tut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Liefert die Abtastung eines skalaren Eingangs.
    Input,
    /// Liefert ein Element eines Eingabestroms.
    Poll,
    /// Stellt einen skalaren Ausgang.
    Output,
    /// Nennt den freien Platz eines Ausgabestroms.
    Free,
    /// Meldet den Heartbeat eines Geraets.
    Alive,
}

impl Kind {
    /// Das Wort im Namen: `P_in_<adr>`, `P_alive_<geraet>`.
    fn word(self) -> &'static str {
        match self {
            Kind::Input => "in",
            Kind::Poll => "poll",
            Kind::Output => "out",
            Kind::Free => "free",
            Kind::Alive => "alive",
        }
    }
}

/// Der Typ eines Werts an der Schnittstelle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value {
    /// In C, feste Breite.
    pub c: &'static str,
    /// In Rust, im Trait.
    pub rust: &'static str,
}

impl Value {
    /// Ein Wahrheitswert: in C ein `uint8_t`.
    fn is_bool(self) -> bool {
        self.rust == "bool"
    }

    /// Der Typ im Kleber, wie C ihn uebergibt.
    fn abi(self) -> &'static str {
        if self.is_bool() { "u8" } else { self.rust }
    }
}

/// Der Typ eines LLVM-Werts an der Schnittstelle.
pub fn value_of(t: &LlvmType, signed: bool) -> Option<Value> {
    let v = |c, rust| Some(Value { c, rust });
    match (t, signed) {
        (LlvmType::Int(1), _) => v("uint8_t", "bool"),
        (LlvmType::Int(8), true) => v("int8_t", "i8"),
        (LlvmType::Int(8), false) => v("uint8_t", "u8"),
        (LlvmType::Int(16), true) => v("int16_t", "i16"),
        (LlvmType::Int(16), false) => v("uint16_t", "u16"),
        (LlvmType::Int(32), true) => v("int32_t", "i32"),
        (LlvmType::Int(32), false) => v("uint32_t", "u32"),
        (LlvmType::Int(64), true) => v("int64_t", "i64"),
        (LlvmType::Int(64), false) => v("uint64_t", "u64"),
        (LlvmType::F32, _) => v("float", "f32"),
        (LlvmType::F64, _) => v("double", "f64"),
        _ => None,
    }
}

/// Der Typ eines aufgezeichneten Inputs (8.2) an der Schnittstelle.
pub fn recorded_value(value: RecordedValue) -> Value {
    match value {
        RecordedValue::Bool => Value { c: "uint8_t", rust: "bool" },
        RecordedValue::Int(w) => {
            value_of(&LlvmType::Int(w.bits()), w.signed()).unwrap_or(Value { c: "int64_t", rust: "i64" })
        }
        RecordedValue::Float(FloatWidth::F32) => Value { c: "float", rust: "f32" },
        RecordedValue::Float(FloatWidth::F64) => Value { c: "double", rust: "f64" },
        RecordedValue::Enum(_) => Value { c: "uint32_t", rust: "u32" },
        RecordedValue::Wire => Value { c: "uint8_t", rust: "u8" },
    }
}

/// Ein Treiber.
#[derive(Clone, Debug)]
pub struct Driver {
    /// Was er tut.
    pub kind: Kind,
    /// Sein Name ohne Praefix, zugleich die Methode im Trait: `in_ui_button`.
    pub method: String,
    /// Wo er sitzt: die Adresse eines Kanals, fuer `alive` das Geraet.
    pub address: String,
    /// Der Wert eines Skalars.
    pub value: Option<Value>,
    /// Der Kanal oder das Geraet, fuer die Dokumentation.
    pub doc: String,
}

impl Driver {
    /// Der Name im Abbild: `P_<methode>`.
    pub fn symbol(&self, x: &Prefix) -> String {
        x.name(&self.method)
    }
}

/// Der Name eines Treibers ohne Praefix.
pub fn method(kind: Kind, ident: &str) -> String {
    format!("{}_{ident}", kind.word())
}

/// Die Treiber eines Programms: die gebundenen Eingaenge und Eingabestroeme
/// ohne die, die ein Modell speist (8.3), die aufgezeichneten Inputs (8.2),
/// die gebundenen Ausgaenge und Ausgabestroeme und je Geraet eines
/// Ausgangs der Heartbeat (12.4).
pub fn of(p: &Program, layout: &Layout) -> Vec<Driver> {
    let mut out = Vec::new();
    let fed = crate::parts::sim_fed_inputs(p);
    for slot in &layout.inputs {
        let Some(channel) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let (Some(addr), Some(value)) = (slot.address.as_ref(), value_of(&slot.ty, slot.signed)) else { continue };
        if fed.contains(&channel) {
            continue;
        }
        out.push(Driver {
            kind: Kind::Input,
            method: method(Kind::Input, &addr.ident()),
            address: addr.text(),
            value: Some(value),
            doc: format!("Eingang `{}`", slot.name),
        });
    }
    for (channel, c) in p.channels.iter().enumerate() {
        let Binding::Hw(addr) = &c.binding else { continue };
        let stream = matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_)));
        if stream && c.dir == Direction::Input && !crate::streams::coupled_input(p, channel) {
            out.push(Driver {
                kind: Kind::Poll,
                method: method(Kind::Poll, &addr.ident()),
                address: addr.text(),
                value: None,
                doc: format!("Eingabestrom `{}`", c.name),
            });
        }
    }
    for r in &p.recorded {
        let kind = if r.stream.is_some() { Kind::Poll } else { Kind::Input };
        out.push(Driver {
            kind,
            method: method(kind, &r.address.ident()),
            address: r.address.text(),
            value: r.stream.is_none().then(|| recorded_value(r.value)),
            doc: format!("aufgezeichneter Input `{}` (8.2)", r.name),
        });
    }
    let mut devices: Vec<String> = Vec::new();
    for slot in &layout.outputs {
        let (Some(addr), Some(value)) = (slot.address.as_ref(), value_of(&slot.ty, slot.signed)) else { continue };
        out.push(Driver {
            kind: Kind::Output,
            method: method(Kind::Output, &addr.ident()),
            address: addr.text(),
            value: Some(value),
            doc: format!("Ausgang `{}`", slot.name),
        });
        if let Some(c) = p.channels.iter().find(|c| c.name == slot.name) {
            devices.push(takt_hal::edge::driver_of(c));
        }
    }
    for c in &p.channels {
        let Binding::Hw(addr) = &c.binding else { continue };
        if c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))) {
            out.push(Driver {
                kind: Kind::Free,
                method: method(Kind::Free, &addr.ident()),
                address: addr.text(),
                value: None,
                doc: format!("Ausgabestrom `{}` (8.8)", c.name),
            });
            devices.push(takt_hal::edge::driver_of(c));
        }
    }
    devices.sort_unstable();
    devices.dedup();
    for d in devices {
        let ident = takt_mir::pattern::Address::simple(&d).ident();
        out.push(Driver {
            kind: Kind::Alive,
            method: method(Kind::Alive, &ident),
            address: d.clone(),
            value: None,
            doc: format!("Heartbeat des Geraets `{d}` (12.4)"),
        });
    }
    out
}

/// Die Parameter eines Treibers in C, nach dem Zeiger des Wirts und `now`.
fn c_params(d: &Driver) -> String {
    let value = d.value.map_or("uint8_t", |v| v.c);
    match d.kind {
        Kind::Input => format!("{value} *value, uint8_t *quality, int64_t *t"),
        Kind::Poll => "uint8_t *buf, int32_t cap, int32_t *len, int64_t *t, int64_t *seq".to_string(),
        Kind::Output => format!("{value} value"),
        Kind::Free | Kind::Alive => String::new(),
    }
}

/// Die Rueckgabe eines Treibers in C.
fn c_return(kind: Kind) -> &'static str {
    if kind == Kind::Free { "int32_t" } else { "uint8_t" }
}

/// Die Signatur eines Treibers in C.
fn c_signature(d: &Driver, x: &Prefix) -> String {
    let params = c_params(d);
    let rest = if params.is_empty() { String::new() } else { format!(", {params}") };
    format!("{} {}(void *user, int64_t now{rest})", c_return(d.kind), d.symbol(x))
}

/// Die Prototypen der Treiber.
pub fn c_prototypes(drivers: &[Driver], x: &Prefix) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "/* Die Treiber (8.10, 12.6): stark gebunden, je mit dem Zeiger des Wirts und der Zeit des Ticks. */"
    );
    if drivers.is_empty() {
        let _ = writeln!(s, "/*   keine — nichts ist an Hardware gebunden */");
    }
    for d in drivers {
        let _ = writeln!(s, "{}; /* {}, hw(\"{}\") */", c_signature(d, x), d.doc, d.address);
    }
    s
}

/// Ausdrueckliche Stummel fuer alle Treiber: ein Eingang liefert nichts,
/// ein Ausgang gilt als bestaetigt, ein Geraet als lebendig, ein
/// Sendepuffer als unbekannt. Fuer Tests, die einen Rahmen ohne Treiber
/// binden.
pub fn c_stubs(drivers: &[Driver], x: &Prefix) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Stummel, ausdruecklich verlangt: kein Treiber an diesen Kanaelen. */");
    for d in drivers {
        let names: Vec<&str> = match d.kind {
            Kind::Input => vec!["value", "quality", "t"],
            Kind::Poll => vec!["buf", "cap", "len", "t", "seq"],
            Kind::Output => vec!["value"],
            Kind::Free | Kind::Alive => vec![],
        };
        let unused: String = std::iter::once("user")
            .chain(std::iter::once("now"))
            .chain(names)
            .map(|n| format!("(void){n}; "))
            .collect();
        let result = match d.kind {
            Kind::Input | Kind::Poll => "0",
            Kind::Output | Kind::Alive => "1",
            Kind::Free => "-1",
        };
        let _ = writeln!(s, "{} {{ {unused}return {result}; }}", c_signature(d, x));
    }
    s
}

/// Die Methode eines Treibers im Trait.
fn rust_signature(d: &Driver) -> String {
    let value = d.value.map_or("u8", |v| v.rust);
    match d.kind {
        Kind::Input => format!("fn {}(&mut self, now: i64) -> Option<takt_embed::Sample<{value}>>", d.method),
        Kind::Poll => format!("fn {}(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece>", d.method),
        Kind::Output => format!("fn {}(&mut self, value: {value}, now: i64) -> bool", d.method),
        Kind::Free => format!("fn {}(&mut self, now: i64) -> Option<u32>", d.method),
        Kind::Alive => format!("fn {}(&mut self, now: i64) -> bool", d.method),
    }
}

/// Der Trait `Drivers` des Programms: eine Methode je Treiber.
pub fn rust_trait(drivers: &[Driver]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/// Die Treiber des Programms (8.10, 12.6): eine Methode je gebundener Adresse");
    let _ = writeln!(s, "/// und je Geraet mit Heartbeat. `now` ist die Tickgrenze in Nanosekunden.");
    let _ = writeln!(s, "pub trait Drivers {{");
    for d in drivers {
        let _ = writeln!(s, "    /// {}, `hw(\"{}\")`.", d.doc, d.address);
        let _ = writeln!(s, "    {};", rust_signature(d));
    }
    let _ = writeln!(s, "}}");
    s
}

/// Der Kleber vom C-Aufruf des Rahmens zur Methode von `Drivers` auf dem
/// Treiberobjekt des Wirts, Typ `host`. Monomorph, weil `extern "C"` nicht
/// generisch sein kann.
pub fn rust_glue(drivers: &[Driver], x: &Prefix, host: &str) -> String {
    let mut s = String::new();
    for d in drivers {
        let symbol = d.symbol(x);
        let value = d.value.map_or("u8", Value::abi);
        let params = match d.kind {
            Kind::Input => format!(", value: *mut {value}, quality: *mut u8, t: *mut i64"),
            Kind::Poll => ", buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64".to_string(),
            Kind::Output => format!(", value: {value}"),
            Kind::Free | Kind::Alive => String::new(),
        };
        let ret = if d.kind == Kind::Free { "i32" } else { "u8" };
        let _ = writeln!(s, "/// `{symbol}`: der Kleber zu [`Drivers::{}`].", d.method);
        let _ = writeln!(s, "///\n/// # Safety\n///");
        let _ = writeln!(s, "/// `user` ist das Treiberobjekt, das der Wirt bei `init` uebergab; die");
        let _ = writeln!(s, "/// uebrigen Zeiger sind Plaetze des Rahmens.");
        let _ = writeln!(s, "#[unsafe(no_mangle)]");
        let _ = writeln!(
            s,
            "pub unsafe extern \"C\" fn {symbol}(user: *mut core::ffi::c_void, now: i64{params}) -> {ret} {{"
        );
        let _ = writeln!(s, "    // SAFETY: siehe oben.");
        let _ = writeln!(s, "    let drivers = unsafe {{ &mut *user.cast::<{host}>() }};");
        let call = format!("<{host} as Drivers>::{}", d.method);
        match d.kind {
            Kind::Input => {
                let conv = if d.value.is_some_and(Value::is_bool) { "u8::from(sample.value)" } else { "sample.value" };
                let _ = writeln!(s, "    let Some(sample) = {call}(drivers, now) else {{ return 0 }};");
                let _ = writeln!(s, "    // SAFETY: siehe oben.");
                let _ = writeln!(s, "    unsafe {{");
                let _ = writeln!(s, "        *value = {conv};");
                let _ = writeln!(s, "        *quality = sample.quality as u8;");
                let _ = writeln!(s, "        *t = sample.t;");
                let _ = writeln!(s, "    }}");
                let _ = writeln!(s, "    1");
            }
            Kind::Poll => {
                let _ = writeln!(s, "    let cap = usize::try_from(cap).unwrap_or(0);");
                let _ = writeln!(s, "    // SAFETY: Der Rahmen reicht `cap` schreibbare Bytes.");
                let _ = writeln!(s, "    let buf = unsafe {{ core::slice::from_raw_parts_mut(buf, cap) }};");
                let _ = writeln!(s, "    let Some(piece) = {call}(drivers, buf, now) else {{ return 0 }};");
                let _ = writeln!(s, "    // SAFETY: siehe oben.");
                let _ = writeln!(s, "    unsafe {{");
                let _ = writeln!(s, "        *len = i32::try_from(piece.len.min(cap)).unwrap_or(0);");
                let _ = writeln!(s, "        *t = piece.t;");
                let _ = writeln!(s, "        if let Some(n) = piece.seq {{");
                let _ = writeln!(s, "            *seq = n;");
                let _ = writeln!(s, "        }}");
                let _ = writeln!(s, "    }}");
                let _ = writeln!(s, "    1");
            }
            Kind::Output => {
                let conv = if d.value.is_some_and(Value::is_bool) { "value != 0" } else { "value" };
                let _ = writeln!(s, "    u8::from({call}(drivers, {conv}, now))");
            }
            Kind::Free => {
                let _ = writeln!(s, "    {call}(drivers, now).map_or(-1, |n| i32::try_from(n).unwrap_or(i32::MAX))");
            }
            Kind::Alive => {
                let _ = writeln!(s, "    u8::from({call}(drivers, now))");
            }
        }
        let _ = writeln!(s, "}}\n");
    }
    s
}

/// Der Geraete-Trait, den ein Treiber dieser Art erfuellt, und der Aufruf darauf.
fn device_call(d: &Driver, field: &str) -> String {
    match d.kind {
        Kind::Input => format!("takt_embed::Input::sample(&mut self.{field}, now)"),
        Kind::Poll => format!("takt_embed::StreamInput::poll(&mut self.{field}, buf, now)"),
        Kind::Output => format!("takt_embed::Output::write(&mut self.{field}, value, now)"),
        Kind::Free => format!("takt_embed::StreamOutput::free(&mut self.{field}, now)"),
        Kind::Alive => format!("takt_embed::Device::alive(&mut self.{field}, now)"),
    }
}

/// Ein Pruefstand `rig`: je verdrahteter Adresse ein Geraet aus `wiring`
/// (Adresse oder Geraetename auf den Rust-Typ), fuer jede andere ein
/// ausdruecklicher Stummel, den der Kopf des Textes nennt.
pub fn rust_rig(drivers: &[Driver], rig: &str, wiring: &[(String, String)]) -> String {
    let wired = |d: &Driver| wiring.iter().find(|(addr, _)| *addr == d.address).map(|(_, ty)| ty.as_str());
    let stubs: Vec<String> =
        drivers.iter().filter(|d| wired(d).is_none()).map(|d| format!("`{}`", d.address)).collect();
    let mut s = String::new();
    let _ = writeln!(s, "/// Der Pruefstand (12.6): je verdrahteter Adresse ein Geraet.");
    if !stubs.is_empty() {
        let _ = writeln!(s, "///");
        let _ = writeln!(s, "/// Stummel, ausdruecklich: {}.", stubs.join(", "));
    }
    let _ = writeln!(s, "#[derive(Default)]");
    let _ = writeln!(s, "pub struct {rig} {{");
    for d in drivers {
        if let Some(ty) = wired(d) {
            let _ = writeln!(s, "    /// {}, `hw(\"{}\")`.", d.doc, d.address);
            let _ = writeln!(s, "    pub {}: {ty},", d.method);
        }
    }
    let _ = writeln!(s, "}}\n");
    let _ = writeln!(s, "impl Drivers for {rig} {{");
    for d in drivers {
        let signature = rust_signature(d);
        match wired(d) {
            Some(_) => {
                let _ = writeln!(s, "    {signature} {{");
                let _ = writeln!(s, "        {}", device_call(d, &d.method));
            }
            None => {
                // Die Parameter des Stummels sind unbenutzt.
                let signature = signature
                    .replace("now: i64", "_now: i64")
                    .replace("buf: &mut", "_buf: &mut")
                    .replace("value:", "_value:");
                let result = match d.kind {
                    Kind::Input | Kind::Poll | Kind::Free => "None",
                    Kind::Output | Kind::Alive => "true",
                };
                let _ = writeln!(s, "    {signature} {{");
                let _ = writeln!(s, "        // Stummel: kein Geraet fuer `{}` auf diesem Pruefstand.", d.address);
                let _ = writeln!(s, "        {result}");
            }
        }
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}");
    s
}

/// Liest eine Verdrahtung: je Zeile `"adresse" = "Rust-Typ"`, `#` beginnt
/// einen Kommentar — eine flache TOML-Tabelle (`takt-drivers.toml`).
pub fn wiring(text: &str) -> Result<Vec<(String, String)>, String> {
    let quoted = |s: &str| s.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string);
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parsed = line.split_once('=').and_then(|(k, v)| Some((quoted(k)?, quoted(v)?)));
        let Some((addr, ty)) = parsed else {
            return Err(format!("Zeile {}: erwartet `\"adresse\" = \"Typ\"`", i + 1));
        };
        out.push((addr, ty));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::wiring;

    #[test]
    fn a_wiring_maps_addresses_to_types() {
        let text = "# Pruefgeraet\n\"edge_a/p\" = \"takt_driver_probe::EdgeAP\"\n\n\"edge_o\" = \"takt_driver_probe::EdgeO\" # Heartbeat\n";
        assert_eq!(
            wiring(text),
            Ok(vec![
                ("edge_a/p".to_string(), "takt_driver_probe::EdgeAP".to_string()),
                ("edge_o".to_string(), "takt_driver_probe::EdgeO".to_string()),
            ])
        );
        assert!(wiring("edge_a/p = Typ").is_err());
    }
}
