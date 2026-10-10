//! Die Arena eines Programms (11.2, 12.11): der ganze veraenderliche
//! Zustand in einem Block, den der Wirt stellt.
//!
//! **Vorn der Programmbereich, dahinter die Runtime.** Den Programmbereich
//! adressiert der erzeugte Code: die Ablagen eines Faults, Prozessabbild,
//! Latch, Parameter und die Zustaende der Monitore und Maschinen. Seine
//! Versaetze rechnet nur diese Datei, und der Rahmen (`takt-frame`)
//! bestaetigt jeden mit `_Static_assert`. Dahinter legt der Rahmen den
//! Bereich der Runtime an — Ringe, Warteschlangen, Rand, Jobs —, den der
//! erzeugte Code nie beruehrt; er reicht der Runtime nur den Zeiger auf die
//! Arena weiter (`abi.rs`).
//!
//! **Ein Zeiger statt vier.** Was der Rahmen ruft, nimmt nur die Arena; der
//! Einstieg bildet daraus Zustand, Abbild, Parameter und Latch als feste
//! Abstaende ([`entries`]) und bettet den Rumpf ein. Im Maschinencode bleibt
//! ein Basisregister mit unmittelbaren Versaetzen.

use takt_mir::machine::{Machine, MachineKind};
use takt_mir::program::{Direction, Program};
use takt_mir::{ChannelId, CommandId, ParamId};

use crate::emit::Module;
use crate::symbols::Prefix;
use crate::ty::LlvmType;

/// Der Name des Arena-Parameters in jeder erzeugten Funktion. Benannt, damit
/// er die Nummern `%0` bis `%3` der Zeiger auf Zustand, Abbild, Parameter
/// und Latch nicht verschiebt.
pub const PARAM: &str = "%arena";

/// Jeder Bereich beginnt an einer durch acht teilbaren Stelle: So liegt
/// jeder `i64` und jedes `double` ausgerichtet, auf jedem Ziel.
const ALIGN: u64 = 8;

/// Die Ablagen eines Faults (5.3, `fault.rs`) am Anfang der Arena: Zeile der
/// Stelle (`i32`), Art eines Faults in einer reinen Funktion (`i32`), ob die
/// Nachricht gesetzt ist (`i8`), die Nachricht (`str<128>`: `i32` Laenge,
/// 128 Byte). Die Zeile steht auf null, weil der Fault-Stummel sie schreibt,
/// der kein Register anlegen kann: Ihre Adresse ist die Arena selbst.
///
/// Zeile und Nachricht schreibt der Codegen nur, wenn eine Maschine
/// `last_fault` liest; sonst endet der Block nach dem Flag ([`of`]).
pub mod fault {
    /// Die Zeile der letzten Fault-Stelle.
    pub const LINE: u64 = 0;
    /// Die Art eines Faults in einer reinen Funktion (4.1); null ist keine.
    /// Der Aufrufer liest sie nach dem Aufruf, loescht sie und nimmt seinen
    /// eigenen Fault-Pfad mit dieser Art; ebenso traegt sie die Art aus
    /// einer `loop:`-Funktion hinaus.
    pub const FLAG: u64 = 4;
    /// Steht in [`TEXT`] die Nachricht des Faults, der gerade genommen wird?
    pub const STATED: u64 = 8;
    /// Die Nachricht eines `check`, `expect` oder `abort`.
    pub const TEXT: u64 = 12;
    /// Die Groesse der Ablagen, wenn eine Maschine `last_fault` liest.
    pub const BYTES: u64 = TEXT + 4 + 128;
    /// Die Groesse ohne Nachricht: Zeile und Flag.
    pub const UNTRACKED: u64 = STATED;
}

/// Ein Bereich im Programmteil der Arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// Der Name, unter dem der Rahmen den Bereich fuehrt (`image`,
    /// `state_<maschine>`, `monitor_<i>`).
    pub name: String,
    /// Versatz ab dem Anfang der Arena.
    pub offset: u64,
    /// Groesse in Bytes, mindestens eins.
    pub bytes: u64,
}

/// Der Programmbereich der Arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arena {
    /// Die Ablagen eines Faults ([`fault`]), immer am Anfang.
    pub fault: Region,
    /// Das Prozessabbild.
    pub image: Region,
    /// Der Latch der Ausgaben.
    pub latch: Region,
    /// Der Parametervektor.
    pub params: Region,
    /// Je Laufzeitmonitor (13.3) sein Zustand, mit dem Index der Eigenschaft.
    pub monitors: Vec<(usize, Region)>,
    /// Je Maschine mit eigenem Schritt ihr Zustands-Struct (11.2).
    pub states: Vec<Region>,
    /// Das Ende des Programmbereichs, durch acht teilbar: Hier beginnt die
    /// Runtime.
    pub bytes: u64,
}

impl Arena {
    /// Der Zustand der Maschine `name`, wenn sie einen eigenen Schritt hat.
    pub fn state(&self, name: &str) -> Option<&Region> {
        self.states.iter().find(|r| r.name == state_name(name))
    }

    /// Der Zustand des Monitors der Eigenschaft `index`.
    pub fn monitor(&self, index: usize) -> Option<&Region> {
        self.monitors.iter().find(|(i, _)| *i == index).map(|(_, r)| r)
    }

    /// Alle Bereiche in der Reihenfolge ihrer Versaetze.
    pub fn regions(&self) -> Vec<&Region> {
        let mut out = vec![&self.fault, &self.image, &self.latch, &self.params];
        out.extend(self.monitors.iter().map(|(_, r)| r));
        out.extend(&self.states);
        out
    }
}

/// Der Name des Zustandsbereichs einer Maschine.
pub fn state_name(machine: &str) -> String {
    format!("state_{}", crate::fns::sanitized(machine))
}

/// Die Maschinen, die einen eigenen Schritt und damit einen Zustand in der
/// Arena haben: alle ausser Vorlagen (5.9), sofern ihr Zustands-Struct
/// entsteht — wie in `lower::program`.
pub fn machines(p: &Program) -> Vec<&Machine> {
    p.machines
        .iter()
        .enumerate()
        .filter(|(i, m)| {
            m.kind != MachineKind::Template
                && p.in_build(takt_mir::MachineId(*i as u32))
                && crate::machine::state_struct(m, p).is_some()
        })
        .map(|(_, m)| m)
        .collect()
}

/// Rechnet den Programmbereich der Arena aus.
pub fn of(p: &Program) -> Arena {
    let mut at = 0u64;
    let mut region = |name: String, bytes: u64| {
        let r = Region { name, offset: at, bytes: bytes.max(1) };
        at = (r.offset + r.bytes).div_ceil(ALIGN) * ALIGN;
        r
    };
    let tracked = p.machines.iter().any(takt_mir::visit::reads_last_fault);
    let fault = region("fault".into(), if tracked { fault::BYTES } else { fault::UNTRACKED });
    let image = region("image".into(), image_bytes(p));
    let latch = region("latch".into(), latch_bytes(p));
    let params = region("params".into(), params_bytes(p));
    let monitors = p
        .properties
        .iter()
        .enumerate()
        .filter(|(_, prop)| prop.monitor)
        .map(|(i, prop)| (i, region(format!("monitor_{i}"), crate::monitor::state_size(prop, p).unwrap_or(1))))
        .collect();
    let states = machines(p)
        .into_iter()
        .filter_map(|m| Some(region(state_name(&m.name), crate::machine::state_struct(m, p)?.aligned_size())))
        .collect();
    Arena { fault, image, latch, params, monitors, states, bytes: at }
}

/// Die Groesse des Prozessabbilds: Eintraege der Eingaenge und Commands,
/// dahinter die zwei Ψ-Baenke (9.4, 7.2) und die Job-Slots (4.5).
pub fn image_bytes(p: &Program) -> u64 {
    let mut end = 0u64;
    for (i, c) in p.channels.iter().enumerate() {
        let id = ChannelId(i as u32);
        if c.dir == Direction::Input
            && let (Some(offset), Some(entry)) = (crate::image::offset_of(id, p), crate::image::entry_type(id, p))
        {
            end = end.max(offset + entry.aligned_size());
        }
    }
    for i in 0..p.commands.len() {
        if let Some(offset) = crate::image::command_offset(CommandId(i as u32), p) {
            end = end.max(offset + 1);
        }
    }
    end.max(crate::psi::image_size(p)).max(crate::image::jobs_end(p))
}

/// Die Groesse des Latches der Ausgaben.
pub fn latch_bytes(p: &Program) -> u64 {
    let mut end = 0u64;
    for (i, c) in p.channels.iter().enumerate() {
        let id = ChannelId(i as u32);
        if c.dir != Direction::Input
            && let (Some(offset), Some(ty)) = (crate::image::latch_offset(id, p), crate::ty::lower(c.ty, p))
        {
            end = end.max(offset + ty.aligned_size());
        }
    }
    end
}

/// Die Groesse des Parametervektors (8.4).
pub fn params_bytes(p: &Program) -> u64 {
    let mut end = 0u64;
    for (i, param) in p.params.iter().enumerate() {
        if let (Some(offset), Some(ty)) =
            (crate::image::param_offset(ParamId(i as u32), p), crate::ty::lower(param.ty, p))
        {
            end = end.max(offset + ty.aligned_size());
        }
    }
    end
}

/// Ein Zeiger auf `offset` in der Arena der laufenden Funktion.
pub fn at(offset: u64, m: &mut Module) -> crate::emit::Reg {
    m.inst(&format!("getelementptr inbounds i8, ptr {PARAM}, i64 {offset}"))
}

/// Die Gestalt eines Einstiegs: welche Zeiger der Rumpf nimmt, was er
/// zurueckgibt und welche Parameter danach folgen.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    /// Welche der vier Zeiger der Rumpf nimmt.
    pub pointers: Pointers,
    /// Die Rueckgabe.
    pub ret: &'static str,
    /// Die Parameter nach den Zeigern, als LLVM-Typen.
    pub extra: &'static [&'static str],
}

/// Welche der vier Zeiger ein Rumpf nimmt, in dieser Reihenfolge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointers {
    /// Nur den Zustand.
    State,
    /// Zustand und Abbild.
    StateImage,
    /// Zustand, Abbild, Parameter und Latch.
    All,
}

/// Der Name eines Einstiegs, wie der Rahmen ihn ruft: `P_<maschine>_<suffix>`.
pub fn entry_symbol(prefix: &Prefix, machine: &str, suffix: &str) -> String {
    prefix.name(&format!("{}_{suffix}", crate::fns::sanitized(machine)))
}

/// Der Name des Einstiegs eines Laufzeitmonitors: `P_monitor_<i>`.
pub fn monitor_symbol(prefix: &Prefix, index: usize) -> String {
    prefix.name(&format!("monitor_{index}"))
}

/// Schreibt den Einstieg `P_<maschine>_<suffix>(ptr %arena, …)` vor den
/// Rumpf `body`: Er bildet die Zeiger als feste Abstaende in der Arena und
/// ruft den Rumpf, den LLVM als einzige Aufrufstelle einbettet.
pub fn entry(symbol: &str, body: &str, e: Shape, state: u64, arena: &Arena, m: &mut Module) {
    let mut params: Vec<String> = vec![format!("ptr {PARAM}")];
    let mut args: Vec<String> = Vec::new();
    let mut lines = String::new();
    let ptr = |name: &str, offset: u64, lines: &mut String, args: &mut Vec<String>| {
        lines.push_str(&format!("  %{name} = getelementptr inbounds i8, ptr {PARAM}, i64 {offset}\n"));
        args.push(format!("ptr %{name}"));
    };
    ptr("st", state, &mut lines, &mut args);
    if matches!(e.pointers, Pointers::StateImage | Pointers::All) {
        ptr("in", arena.image.offset, &mut lines, &mut args);
    }
    if e.pointers == Pointers::All {
        ptr("par", arena.params.offset, &mut lines, &mut args);
        ptr("out", arena.latch.offset, &mut lines, &mut args);
    }
    for (i, t) in e.extra.iter().enumerate() {
        params.push(format!("{t} %a{i}"));
        args.push(format!("{t} %a{i}"));
    }
    args.push(format!("ptr {PARAM}"));
    let call = if e.ret == "void" {
        format!("  call void @{body}({})\n  ret void", args.join(", "))
    } else {
        format!("  %r = call {} @{body}({})\n  ret {} %r", e.ret, args.join(", "), e.ret)
    };
    let attrs = m.fn_attrs();
    m.declare(&format!("\ndefine {} @{symbol}({}) {attrs} {{\n{lines}{call}\n}}", e.ret, params.join(", ")));
}

/// Die Einstiege einer Maschine, die der Rahmen ruft, mit ihrer Endung:
/// `step` fuer `<maschine>_step`; je gescopter Instanz (5.11) ihr Praedikat
/// `scope_<i>`.
pub fn entries(machine: &Machine, p: &Program) -> Vec<(String, Shape)> {
    const ALL: Pointers = Pointers::All;
    let shape = |pointers, ret, extra| Shape { pointers, ret, extra };
    let mut out: Vec<(String, Shape)> = vec![
        ("init_vars".into(), shape(ALL, "void", &[])),
        ("enter".into(), shape(ALL, "void", &[])),
        ("step".into(), shape(ALL, "void", &[])),
        ("publish".into(), shape(Pointers::StateImage, "void", &[])),
        ("deliver".into(), shape(ALL, "void", &["i32", "i1"])),
        ("pend".into(), shape(Pointers::State, "void", &["i32"])),
        ("idle".into(), shape(Pointers::State, "i1", &[])),
        ("deadline".into(), shape(Pointers::State, "i64", &[])),
        ("advance".into(), shape(Pointers::State, "void", &["i64"])),
        ("exit_all".into(), shape(ALL, "void", &[])),
    ];
    if crate::step::drops(machine, p) {
        out.push(("drop".into(), shape(Pointers::State, "void", &[])));
    }
    if !machine.persist.is_empty() {
        out.push(("persist_snapshot".into(), shape(Pointers::State, "i32", &["ptr", "i32"])));
        // Die Eingabe des Journals wird nur gelesen; der Rahmen reicht sie `const`.
        out.push(("persist_restore".into(), shape(Pointers::State, "i32", &["ptr readonly", "i32"])));
    }
    if !machine.layout.trigger_flags.is_empty() {
        out.push(("triggers".into(), shape(ALL, "void", &[])));
    }
    let scoped = machine.states.iter().map(|s| s.instances.len()).sum::<usize>();
    out.extend((0..scoped).map(|i| (format!("scope_{i}"), shape(Pointers::State, "i1", &[]))));
    if crate::psi::is_scoped(machine, p) {
        out.push(("publish_inactive".into(), shape(Pointers::StateImage, "void", &[])));
    }
    out
}

/// Die Gestalt des Einstiegs eines Laufzeitmonitors (13.3): alle vier Zeiger
/// und der Tick.
pub const MONITOR: Shape = Shape { pointers: Pointers::All, ret: "void", extra: &["i64"] };

/// Der LLVM-Typ der Fault-Nachricht in den Ablagen.
pub fn text_type() -> LlvmType {
    LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), 128)])
}
