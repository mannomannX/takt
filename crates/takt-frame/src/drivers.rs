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
//! **Das Geraet `sys` hat keine Treiber** (12.7, 12.11). `previous_run` und
//! die Wanduhr stellt der Wirt, nicht ein Treiber-Crate: Sie stehen als
//! [`Kind::Sys`] in derselben Liste, aber mit eigenem Namen `P_sys_<kanal>`,
//! im eigenen Trait `Sys` statt in `Drivers` und ohne Heartbeat;
//! `sys/next_run` liest der Wirt ueber den Einstieg `P_next_run`.
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
use takt_mir::TypeId;
use takt_mir::expr::{Expr, ExprKind};
use takt_mir::program::{Binding, Channel, Direction, Program, RecordedValue};
use takt_mir::types::{Const, FloatWidth, Range, Type};

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
    /// Meldet, ob der Sendepuffer eines Ausgabestroms leer und der Sender
    /// fertig ist (`tx.idle`, 8.8) — nur, wo das Programm es liest (12.11).
    Idle,
    /// Meldet den Heartbeat eines Geraets.
    Alive,
    /// Liefert einen Eingang des eingebauten Geraets `sys` (12.7, 7.4):
    /// kein Treiber, sondern ein Eintrag, den der Wirt stellt (12.11).
    Sys,
}

impl Kind {
    /// Das Wort im Namen: `P_in_<adr>`, `P_alive_<geraet>`.
    fn word(self) -> &'static str {
        match self {
            Kind::Input => "in",
            Kind::Poll => "poll",
            Kind::Output => "out",
            Kind::Free => "free",
            Kind::Idle => "idle",
            Kind::Alive => "alive",
            Kind::Sys => "sys",
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
    /// Der Kanal oder das Geraet mit seinem Vertrag, fuer die Dokumentation.
    pub doc: String,
}

impl Driver {
    /// Der Name im Abbild: `P_<methode>`, fuer `sys` `P_sys_<methode>`.
    pub fn symbol(&self, x: &Prefix) -> String {
        match self.kind {
            Kind::Sys => x.name(&format!("sys_{}", self.method)),
            _ => x.name(&self.method),
        }
    }
}

/// Der Name, unter dem der Rahmen einen skalaren Eingang abtastet:
/// `P_in_<adresse>`, fuer das Geraet `sys` `P_sys_<kanal>`.
pub fn input_symbol(address: &takt_mir::pattern::Address, x: &Prefix) -> String {
    match sys_method(address) {
        Some(m) => x.name(&format!("sys_{m}")),
        None => x.name(&method(Kind::Input, &address.ident())),
    }
}

/// Die Methode eines `sys`-Kanals im Trait `Sys`: der Kanal ohne `sys/`.
fn sys_method(address: &takt_mir::pattern::Address) -> Option<String> {
    let text = address.text();
    takt_mir::sys::is_sys(&text).then(|| address.ident().trim_start_matches("sys_").to_string())
}

/// Der Name eines Treibers ohne Praefix.
pub fn method(kind: Kind, ident: &str) -> String {
    format!("{}_{ident}", kind.word())
}

/// Die Treiber eines Programms: die gebundenen Eingaenge und Eingabestroeme
/// ohne die, die ein Modell speist (8.3), die aufgezeichneten Inputs (8.2),
/// die gebundenen Ausgaenge und Ausgabestroeme und je Geraet eines
/// Ausgangs der Heartbeat (12.4). Am Ende stehen die Eingaenge des Geraets
/// `sys` als [`Kind::Sys`]: keine Treiber, aber Eintraege derselben Gestalt.
pub fn of(p: &Program, layout: &Layout) -> Vec<Driver> {
    let mut out = Vec::new();
    let mut sys = Vec::new();
    let fed = crate::parts::sim_fed_inputs(p);
    for slot in &layout.inputs {
        let Some(channel) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let (Some(addr), Some(value)) = (slot.address.as_ref(), value_of(&slot.ty, slot.signed)) else { continue };
        if fed.contains(&channel) {
            continue;
        }
        if let Some(method) = sys_method(addr) {
            sys.push(Driver {
                kind: Kind::Sys,
                method,
                address: addr.text(),
                value: Some(value),
                doc: format!(
                    "Eingang `{}` des Geraets `sys`, vom Wirt gestellt{}",
                    slot.name,
                    contract(p, &p.channels[channel])
                ),
            });
            continue;
        }
        out.push(Driver {
            kind: Kind::Input,
            method: method(Kind::Input, &addr.ident()),
            address: addr.text(),
            value: Some(value),
            doc: format!("Eingang `{}`{}", slot.name, contract(p, &p.channels[channel])),
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
                doc: format!("Eingabestrom `{}`{}", c.name, contract(p, c)),
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
        if sys_method(addr).is_some() {
            continue;
        }
        out.push(Driver {
            kind: Kind::Output,
            method: method(Kind::Output, &addr.ident()),
            address: addr.text(),
            value: Some(value),
            doc: format!(
                "Ausgang `{}`{}",
                slot.name,
                p.channels.iter().find(|c| c.name == slot.name).map_or_else(String::new, |c| contract(p, c))
            ),
        });
        if let Some(c) = p.channels.iter().find(|c| c.name == slot.name) {
            devices.push(takt_hal::edge::driver_of(c));
        }
    }
    let idle = idle_read(p);
    for (i, c) in p.channels.iter().enumerate() {
        let Binding::Hw(addr) = &c.binding else { continue };
        if c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))) {
            out.push(Driver {
                kind: Kind::Free,
                method: method(Kind::Free, &addr.ident()),
                address: addr.text(),
                value: None,
                doc: format!("Ausgabestrom `{}` (8.8){}", c.name, contract(p, c)),
            });
            if idle.contains(&i) {
                out.push(Driver {
                    kind: Kind::Idle,
                    method: method(Kind::Idle, &addr.ident()),
                    address: addr.text(),
                    value: None,
                    doc: format!("`{}.idle`: Puffer leer und Sender fertig (8.8)", c.name),
                });
            }
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
    out.append(&mut sys);
    out
}

/// Die Ausgabestroeme, deren `idle` das Programm liest (8.8), als Index in
/// `Program::channels`: Nur sie brauchen einen Treiber `P_idle_<adr>` — ein
/// Treiber wird verlangt, wenn das Programm ihn nutzt (12.11).
pub fn idle_read(p: &Program) -> Vec<usize> {
    let mut out = Vec::new();
    let mut look = |e: &Expr| {
        if let ExprKind::Accessor { base, accessor: takt_mir::expr::Accessor::Idle, .. } = &e.kind
            && let ExprKind::Input { channel, .. } = base.kind
            && !out.contains(&channel.index())
        {
            out.push(channel.index());
        }
    };
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut look);
    }
    for f in &p.fns {
        takt_mir::visit::for_each_expr_block(&f.body, &mut look);
    }
    out.sort_unstable();
    out
}

/// Was ein Kanal zusagt, fuer Prototyp und Trait (12.11): Typ mit Range
/// und Einheit, Abtastalter, Rate, Sprung, Entprellung, Kapazitaet und der
/// Doc-Kommentar der Quelle; leer, wenn er nichts davon traegt.
fn contract(p: &Program, c: &Channel) -> String {
    let a = &c.attrs;
    let parts: Vec<String> = [
        type_text(p, c.ty),
        a.max_age.map(|ns| format!("max_age {}", duration(ns))),
        a.rate.as_ref().and_then(literal).map(|r| format!("rate {r} Hz")),
        a.max_slew.as_ref().and_then(literal).map(|s| format!("max_slew {s}")),
        a.debounce.map(|n| format!("debounce {n}")),
        a.capacity.map(|n| format!("capacity {n}")),
        c.meta.doc.clone(),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() { String::new() } else { format!(": {}", parts.join(", ")) }
}

/// Ein skalarer Typ, wie die Quelle ihn schreibt: `int in 0..99`,
/// `float in 0..400 bar`, der Name eines Enums.
fn type_text(p: &Program, ty: TypeId) -> Option<String> {
    let unit = |u: Option<takt_mir::UnitId>| {
        u.and_then(|u| p.units.get(u.index())).map_or_else(String::new, |u| format!(" {}", u.name))
    };
    let range =
        |r: &Option<Range>| r.as_ref().map_or_else(String::new, |r| format!(" in {}..{}", konst(&r.lo), konst(&r.hi)));
    Some(match p.types.list.get(ty.index())? {
        Type::Bool => "bool".to_string(),
        Type::Int { range: r, unit: u, .. } => format!("int{}{}", range(r), unit(*u)),
        Type::Float { range: r, unit: u, .. } => format!("float{}{}", range(r), unit(*u)),
        Type::Duration { range: r } => format!("duration{}", range(r)),
        Type::Enum(e) => p.enums.get(e.index())?.name.clone(),
        _ => return None,
    })
}

fn konst(c: &Const) -> String {
    match c {
        Const::Int(n) => n.to_string(),
        Const::Float(f) => format!("{f}"),
        Const::Duration(ns) => duration(*ns),
        Const::Bool(b) => b.to_string(),
    }
}

/// Eine Dauer in der groessten Einheit, die sie ganz teilt.
fn duration(ns: i64) -> String {
    [(1_000_000_000, "s"), (1_000_000, "ms"), (1_000, "us")]
        .into_iter()
        .find(|(d, _)| ns != 0 && ns % d == 0)
        .map_or_else(|| format!("{ns} ns"), |(d, u)| format!("{} {u}", ns / d))
}

/// Ein Literal als Zahl; `None` fuer einen Ausdruck.
fn literal(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Int(n) => Some(n.to_string()),
        ExprKind::Float(f) => Some(format!("{f}")),
        _ => None,
    }
}

/// Die Parameter eines Treibers in C, nach dem Zeiger des Wirts und `now`.
fn c_params(d: &Driver) -> String {
    let value = d.value.map_or("uint8_t", |v| v.c);
    match d.kind {
        Kind::Input | Kind::Sys => format!("{value} *value, uint8_t *quality, int64_t *t"),
        Kind::Poll => "uint8_t *buf, int32_t cap, int32_t *len, int64_t *t, int64_t *seq".to_string(),
        Kind::Output => format!("{value} value"),
        Kind::Free | Kind::Idle | Kind::Alive => String::new(),
    }
}

/// Die Rueckgabe eines Treibers in C; `idle` ist eins, null oder -1 fuer
/// „kann ich nicht beantworten“.
fn c_return(kind: Kind) -> &'static str {
    match kind {
        Kind::Free => "int32_t",
        Kind::Idle => "int8_t",
        Kind::Input | Kind::Poll | Kind::Output | Kind::Alive | Kind::Sys => "uint8_t",
    }
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
/// Sendepuffer als unbekannt. Ein Sender ist fertig: Er nahm jedes Byte
/// sofort an, und `tx.idle` haengt wie in der Simulation (8.8) nur am
/// eigenen Puffer (FB-439). Fuer Tests, die einen Rahmen ohne Treiber
/// binden.
pub fn c_stubs(drivers: &[Driver], x: &Prefix) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Stummel, ausdruecklich verlangt: kein Treiber an diesen Kanaelen. */");
    for d in drivers {
        let names: Vec<&str> = match d.kind {
            Kind::Input | Kind::Sys => vec!["value", "quality", "t"],
            Kind::Poll => vec!["buf", "cap", "len", "t", "seq"],
            Kind::Output => vec!["value"],
            Kind::Free | Kind::Idle | Kind::Alive => vec![],
        };
        let unused: String = std::iter::once("user")
            .chain(std::iter::once("now"))
            .chain(names)
            .map(|n| format!("(void){n}; "))
            .collect();
        let result = match d.kind {
            Kind::Input | Kind::Poll | Kind::Sys => "0",
            Kind::Output | Kind::Idle | Kind::Alive => "1",
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
        Kind::Input | Kind::Sys => {
            format!("fn {}(&mut self, now: i64) -> Option<takt_embed::Sample<{value}>>", d.method)
        }
        Kind::Poll => format!("fn {}(&mut self, buf: &mut [u8], now: i64) -> Option<takt_embed::Piece>", d.method),
        Kind::Output => format!("fn {}(&mut self, value: {value}, now: i64) -> bool", d.method),
        Kind::Free => format!("fn {}(&mut self, now: i64) -> Option<u32>", d.method),
        Kind::Idle => format!("fn {}(&mut self, now: i64) -> Option<bool>", d.method),
        Kind::Alive => format!("fn {}(&mut self, now: i64) -> bool", d.method),
    }
}

/// Der Trait `Drivers` des Programms: eine Methode je Treiber. Nutzt das
/// Programm Eingaenge des Geraets `sys`, kommt der Trait `Sys` dazu.
pub fn rust_trait(drivers: &[Driver]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/// Die Treiber des Programms (8.10, 12.6): eine Methode je gebundener Adresse");
    let _ = writeln!(s, "/// und je Geraet mit Heartbeat. `now` ist die Tickgrenze in Nanosekunden.");
    let _ = writeln!(s, "pub trait Drivers {{");
    for d in drivers.iter().filter(|d| d.kind != Kind::Sys) {
        let _ = writeln!(s, "    /// {}, `hw(\"{}\")`.", d.doc, d.address);
        let _ = writeln!(s, "    {};", rust_signature(d));
    }
    let _ = writeln!(s, "}}");
    if drivers.iter().any(|d| d.kind == Kind::Sys) {
        let _ = writeln!(s, "\n/// Was der Wirt dem Programm ueber das eingebaute Geraet `sys` gibt (12.7, 12.11):");
        let _ = writeln!(s, "/// keine Treiber, sondern Eintraege des Wirts. `now` ist die Tickgrenze.");
        let _ = writeln!(s, "pub trait Sys {{");
        for d in drivers.iter().filter(|d| d.kind == Kind::Sys) {
            let _ = writeln!(s, "    /// {}, `hw(\"{}\")`.", d.doc, d.address);
            let _ = writeln!(s, "    {};", rust_signature(d));
        }
        let _ = writeln!(s, "}}");
    }
    s
}

/// Der Trait, in dem die Methode eines Eintrags steht.
fn trait_of(d: &Driver) -> &'static str {
    if d.kind == Kind::Sys { "Sys" } else { "Drivers" }
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
            Kind::Input | Kind::Sys => format!(", value: *mut {value}, quality: *mut u8, t: *mut i64"),
            Kind::Poll => ", buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64".to_string(),
            Kind::Output => format!(", value: {value}"),
            Kind::Free | Kind::Idle | Kind::Alive => String::new(),
        };
        let ret = match d.kind {
            Kind::Free => "i32",
            Kind::Idle => "i8",
            Kind::Input | Kind::Poll | Kind::Output | Kind::Alive | Kind::Sys => "u8",
        };
        let _ = writeln!(s, "/// `{symbol}`: der Kleber zu [`{}::{}`].", trait_of(d), d.method);
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
        let call = format!("<{host} as {}>::{}", trait_of(d), d.method);
        match d.kind {
            Kind::Input | Kind::Sys => {
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
            Kind::Idle => {
                let _ = writeln!(s, "    {call}(drivers, now).map_or(-1, i8::from)");
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
        Kind::Input | Kind::Sys => format!("takt_embed::Input::sample(&mut self.{field}, now)"),
        Kind::Poll => format!("takt_embed::StreamInput::poll(&mut self.{field}, buf, now)"),
        Kind::Output => format!("takt_embed::Output::write(&mut self.{field}, value, now)"),
        Kind::Free => format!("takt_embed::StreamOutput::free(&mut self.{field}, now)"),
        Kind::Idle => format!("takt_embed::StreamOutput::idle(&mut self.{field}, now)"),
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
    let _ = writeln!(s, "}}");
    for name in ["Drivers", "Sys"] {
        let entries: Vec<&Driver> = drivers.iter().filter(|d| trait_of(d) == name).collect();
        if name == "Sys" && entries.is_empty() {
            continue;
        }
        let _ = writeln!(s, "\nimpl {name} for {rig} {{");
        for d in entries {
            rig_method(&mut s, d, wired(d).is_some());
        }
        let _ = writeln!(s, "}}");
    }
    s
}

/// Eine Methode des Pruefstands: das verdrahtete Geraet oder der Stummel.
fn rig_method(s: &mut String, d: &Driver, wired: bool) {
    let signature = rust_signature(d);
    if wired {
        let _ = writeln!(s, "    {signature} {{");
        let _ = writeln!(s, "        {}", device_call(d, &d.method));
    } else {
        // Die Parameter des Stummels sind unbenutzt.
        let signature =
            signature.replace("now: i64", "_now: i64").replace("buf: &mut", "_buf: &mut").replace("value:", "_value:");
        let result = match d.kind {
            Kind::Input | Kind::Poll | Kind::Free | Kind::Sys => "None",
            Kind::Idle => "Some(true)",
            Kind::Output | Kind::Alive => "true",
        };
        let _ = writeln!(s, "    {signature} {{");
        let _ = writeln!(s, "        // Stummel: kein Geraet fuer `{}` auf diesem Pruefstand.", d.address);
        let _ = writeln!(s, "        {result}");
    }
    let _ = writeln!(s, "    }}");
}

/// Liest eine Verdrahtung: je Zeile `"adresse" = "Rust-Typ"`, `#` ausserhalb
/// der Anfuehrungszeichen beginnt einen Kommentar — eine flache TOML-Tabelle
/// (`takt-drivers.toml`). Eine Adresse zweimal ist ein Fehler mit beiden
/// Zeilen, kein „der erste gilt“.
pub fn wiring(text: &str) -> Result<Vec<(String, String)>, String> {
    let quoted = |s: &str| s.trim().strip_prefix('"').and_then(|s| s.strip_suffix('"')).map(str::to_string);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut lines: Vec<usize> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = uncommented(line).trim();
        if line.is_empty() {
            continue;
        }
        let parsed = line.split_once('=').and_then(|(k, v)| Some((quoted(k)?, quoted(v)?)));
        let Some((addr, ty)) = parsed else {
            return Err(format!("Zeile {}: erwartet `\"adresse\" = \"Typ\"`", i + 1));
        };
        if let Some(first) = out.iter().position(|(a, _)| *a == addr) {
            return Err(format!("Zeile {}: `{addr}` ist schon in Zeile {} verdrahtet", i + 1, lines[first]));
        }
        out.push((addr, ty));
        lines.push(i + 1);
    }
    Ok(out)
}

/// Die Zeile bis zum ersten `#` ausserhalb von Anfuehrungszeichen.
fn uncommented(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Was eine Verdrahtung fuer einen Pruefstand ohne Stummel offen laesst
/// (12.6, 13.8): jede Adresse des Programms, die sie nicht nennt, und jede,
/// die sie mehr als einmal nennt — etwa Bring-up und Treiber-Crate
/// zugleich. Leer heisst vollstaendig und eindeutig.
pub fn wiring_errors(drivers: &[Driver], wiring: &[(String, String)]) -> Vec<String> {
    let mut errors = Vec::new();
    for d in drivers {
        match wiring.iter().filter(|(addr, _)| *addr == d.address).count() {
            0 => errors.push(format!("`{}` ist nicht verdrahtet ({})", d.address, d.doc)),
            1 => {}
            n => errors.push(format!("`{}` ist {n}-mal verdrahtet; genau ein Geraet bedient eine Adresse", d.address)),
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::{Driver, Kind, Value, rust_glue, rust_rig, rust_trait, wiring, wiring_errors};

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

    /// Ein `#` in der Adresse oder im Typ beginnt keinen Kommentar; eine
    /// leere Datei verdrahtet nichts.
    #[test]
    fn a_hash_inside_quotes_is_part_of_the_entry() {
        assert_eq!(
            wiring("\"bus#1/rx\" = \"crate::Rx\" # Kommentar"),
            Ok(vec![("bus#1/rx".to_string(), "crate::Rx".to_string())])
        );
        assert_eq!(wiring(""), Ok(vec![]));
        assert_eq!(wiring("# nur ein Kommentar\n\n"), Ok(vec![]));
    }

    /// **Zweimal dieselbe Adresse ist ein Fehler mit beiden Zeilen** (GEN-021),
    /// nicht „der erste gilt“.
    #[test]
    fn an_address_wired_twice_is_an_error() {
        let e = wiring("\"edge_a/p\" = \"A\"\n# x\n\"edge_a/p\" = \"B\"\n").expect_err("doppelt");
        assert!(e.contains("Zeile 3") && e.contains("Zeile 1") && e.contains("edge_a/p"), "{e}");
    }

    fn driver(kind: Kind, address: &str) -> Driver {
        Driver { kind, method: address.replace('/', "_"), address: address.into(), value: None, doc: String::new() }
    }

    /// **Ohne Stummel nennt die Pruefung jede offene Adresse** (GEN-021): Ein
    /// Tippfehler laesst die gemeinte Adresse offen, und eine Adresse, die
    /// Bring-up und Treiber-Crate beide verdrahten, ist ein Fehler.
    #[test]
    fn the_wiring_check_names_every_open_or_doubled_address() {
        let drivers =
            [driver(Kind::Input, "edge_a/p"), driver(Kind::Output, "edge_o/q"), driver(Kind::Alive, "edge_o")];
        let typo = [("edge_a/q".to_string(), "A".to_string()), ("edge_o".to_string(), "O".to_string())];
        let errors = wiring_errors(&drivers, &typo);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("`edge_a/p`") && errors[1].contains("`edge_o/q`"), "{errors:?}");
        let both = [
            ("edge_a/p".to_string(), "Bringup".to_string()),
            ("edge_o/q".to_string(), "Q".to_string()),
            ("edge_o".to_string(), "O".to_string()),
            ("edge_a/p".to_string(), "Crate".to_string()),
        ];
        let errors = wiring_errors(&drivers, &both);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("`edge_a/p`") && errors[0].contains("2-mal"), "{errors:?}");
        let complete = &both[..3];
        assert!(wiring_errors(&drivers, complete).is_empty());
    }

    /// **`sys` ist kein Treiber** (12.7, 12.11, GEN-037): `previous_run`
    /// steht im eigenen Trait `Sys` unter dem Namen `P_sys_previous_run`,
    /// nicht in `Drivers`; der Pruefstand bedient ihn aus der Verdrahtung.
    #[test]
    fn a_sys_entry_has_its_own_trait_and_name() {
        let x = takt_llvm::symbols::Prefix::default();
        let sys = Driver {
            kind: Kind::Sys,
            method: "previous_run".into(),
            address: "sys/previous_run".into(),
            value: Some(Value { c: "uint32_t", rust: "u32" }),
            doc: String::new(),
        };
        let drivers = [driver(Kind::Input, "u/b"), sys];
        assert_eq!(drivers[1].symbol(&x), x.name("sys_previous_run"));
        let text = rust_trait(&drivers);
        let (own, host) = text.split_once("pub trait Sys {").expect("eigener Trait");
        assert!(!own.contains("previous_run") && own.contains("fn u_b("), "{text}");
        assert!(host.contains("fn previous_run(&mut self, now: i64) -> Option<takt_embed::Sample<u32>>"), "{text}");
        let glue = rust_glue(&drivers, &x, "Host");
        assert!(glue.contains("<Host as Sys>::previous_run") && glue.contains("<Host as Drivers>::u_b"), "{glue}");
        let rig = rust_rig(&drivers, "Rig", &[("sys/previous_run".into(), "Dev".into())]);
        assert!(rig.contains("pub previous_run: Dev,") && rig.contains("impl Sys for Rig {"), "{rig}");
    }
}
