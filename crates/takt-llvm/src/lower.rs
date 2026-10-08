//! Ein ganzes Programm nach LLVM-IR (11.2).
//!
//! **Warum das erst jetzt eine Bibliotheksfunktion ist.** Bis hierher
//! stand diese Folge nur im Testrahmen der Konformitaetssuite: Der Codegen
//! wurde *geprueft*, aber nie *benutzt* — die Abnahme uebersetzte selbst
//! und verglich. Mit M5 aendert sich das, weil ein Bring-up-Programm den
//! erzeugten Code mitbinden muss und ein Testrahmen dafuer der falsche
//! Ort ist.
//!
//! Die Reihenfolge ist nicht beliebig: Erst die ABI-Deklarationen, dann
//! Bloecke und Funktionen, zuletzt die Maschinen — jede Stufe benutzt,
//! was die vorige erklaert hat.

use crate::emit::Module;
use crate::symbols::Prefix;
use takt_mir::Program;

/// Was beim Senken nicht ging.
///
/// **Ein Abbruchgrund gehoert in die Ausgabe, nicht in den Papierkorb.**
/// Ohne ihn fehlt eine Schrittfunktion still, und der Linker meldet ein
/// fehlendes Symbol statt des Konstrukts, das gefehlt hat (FB-104).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Die Maschine, deren Schritt fehlt, oder `fn <name>`.
    pub machine: String,
    /// Warum.
    pub reason: String,
}

impl Skipped {
    /// Was fehlt: `<m>_step` oder `fn <name>`.
    pub fn what(s: &Skipped) -> String {
        if s.machine.starts_with("fn ") { s.machine.clone() } else { format!("{}_step", s.machine) }
    }
}

/// Das Ergebnis einer Uebersetzung.
#[derive(Clone, Debug)]
pub struct Lowered {
    /// Die IR als Text.
    pub ir: String,
    /// Maschinen, deren Schrittfunktion nicht entstand.
    ///
    /// Leer heisst: vollstaendig. Ist sie das nicht, laesst sich das
    /// Ergebnis zwar binden, aber nicht ausfuehren — und der Grund steht
    /// hier statt in einer Linker-Meldung.
    pub skipped: Vec<Skipped>,
    /// Maschinen mit `persist var`, fuer die kein Lesepfad entstand.
    ///
    /// Ihr Code laeuft, startet aber immer mit dem Default — der Typ ist
    /// im Codegen noch nicht abgebildet. Still waere das ein Bruch von
    /// 5.9, sobald der Speicher gefuellt ist.
    pub without_persist: Vec<String>,
}

impl Lowered {
    /// Ist die Uebersetzung vollstaendig?
    pub fn complete(&self) -> bool {
        self.skipped.is_empty()
    }
}

/// Senkt ein Programm nach LLVM-IR (11.2).
///
/// `triple` bestimmt den Kopf der Ausgabe und die Rumpfe der
/// Port-Helfer (12.10, [`crate::mmio`]) — sonst ist die IR fuer jedes Ziel
/// dieselbe. Genau darauf beruht Satz 9.4.4: Bit-Gleichheit ist damit eine
/// Aussage ueber *eine* Uebersetzung, nicht ueber mehrere Programme.
///
/// `prefix` steht vor jedem externen Namen (12.11) und benennt das Modul.
pub fn program(p: &Program, triple: &str, prefix: &Prefix) -> Lowered {
    program_with(p, triple, prefix, crate::target::Instrument::Off)
}

/// Wie [`program`], mit Instrumentierung (11.2): `pc` im Zustand traegt
/// je Anweisung oder je Zustandswechsel, wo die Maschine steht.
pub fn program_with(p: &Program, triple: &str, prefix: &Prefix, instrument: crate::target::Instrument) -> Lowered {
    program_with_diagnostics(p, triple, prefix, instrument, crate::target::Diagnostics::Ids)
}

/// Wie [`program_with`], mit Diagnosestufe (plan/codegen-hebel.md C).
///
/// Auf eigenem Stapel ([`takt_diag::stack`]): An den Grenzen aus 2.1
/// braucht das Senken mehr, als ein Aufrufer haben muss (FB-433).
pub fn program_with_diagnostics(
    p: &Program,
    triple: &str,
    prefix: &Prefix,
    instrument: crate::target::Instrument,
    diagnostics: crate::target::Diagnostics,
) -> Lowered {
    let m = Module::new(prefix.as_str(), triple)
        .with_prefix(prefix)
        .with_instrument(instrument)
        .with_diagnostics(diagnostics);
    takt_diag::stack::with_deep_stack(|| program_into(p, m))
}

/// Wie [`program`], fuer einen Lauf unter AddressSanitizer
/// ([`Module::with_address_sanitizer`]); nur fuer Tests auf dem Wirt.
pub fn program_sanitized(p: &Program, triple: &str, prefix: &Prefix) -> Lowered {
    let m = Module::new(prefix.as_str(), triple).with_prefix(prefix).with_address_sanitizer();
    takt_diag::stack::with_deep_stack(|| program_into(p, m))
}

/// Senkt `p` in das vorbereitete Modul, auf dem Stapel des Aufrufers.
fn program_into(p: &Program, mut m: Module) -> Lowered {
    let mut skipped = Vec::new();
    let mut without_persist = Vec::new();

    crate::abi::Abi::declare(&mut m);
    crate::stream::Streams::declare(&mut m);
    let arena = crate::arena::of(p);
    if p.machines.iter().any(takt_mir::visit::reads_last_fault) {
        m.track_faults(&p.sources);
    }

    // Blockmethoden zuerst: Die Funktionen darunter rufen sie.
    let methods: Vec<_> = p.blocks.iter().flat_map(|b| b.step.iter().chain(&b.methods).copied()).collect();
    for b in &p.blocks {
        let Some(inst) = crate::block::instance_of(b, p) else { continue };
        crate::block::declare(b, &inst, &mut m);
        for fid in b.step.iter().chain(&b.methods) {
            let Some(f) = p.fns.get(fid.index()) else { continue };
            if let Err(e) = crate::fns::block_method(b, f, p, &mut m) {
                skipped.push(Skipped { machine: f.name.clone(), reason: e.what.to_string() });
            }
        }
    }

    let reachable = takt_mir::analysis::reachable_fns(p);
    for (i, f) in p.fns.iter().enumerate() {
        let id = takt_mir::FnId(i as u32);
        if methods.contains(&id) {
            continue;
        }
        if let Err(e) = crate::fns::function(f, p, &mut m)
            && reachable.contains(&id)
        {
            skipped.push(Skipped { machine: format!("fn {}", f.name), reason: e.what.to_string() });
        }
    }

    for (i, machine) in p.machines.iter().enumerate() {
        // Eine Vorlage hat keine eigene Schrittfunktion — nur ihre
        // Instanzen laufen (5.9). Sie zu senken meldete „Maschine ohne
        // Blattzustand", und das laese sich wie ein Mangel (FB-118). Ein
        // Plant-Modell wird im Hardware-Build nicht gelinkt (8.3).
        if !p.has_code(takt_mir::MachineId(i as u32)) {
            continue;
        }
        let Some(st) = crate::machine::state_struct(machine, p) else {
            skipped.push(Skipped { machine: machine.name.clone(), reason: "kein Zustands-Struct".into() });
            continue;
        };
        crate::machine::declare_state(machine, &st, &mut m);
        let vars = crate::step::init_vars_function(machine, &st, p, &mut m);
        let enter = crate::step::enter_function(machine, &st, p, &mut m);
        if let Err(e) = &enter {
            skipped.push(Skipped { machine: machine.name.clone(), reason: format!("enter: {}", e.what) });
        }
        if !machine.persist.is_empty() {
            let snapshot = crate::persist::snapshot_function(machine, &st, p, &mut m);
            let restore = crate::persist::restore_function(machine, &st, p, &mut m);
            if vars.is_err() || enter.is_err() || snapshot.is_err() || restore.is_err() {
                without_persist.push(machine.name.clone());
            }
        }
        let _ = crate::step::idle_function(machine, &st, p, &mut m);
        if crate::step::drops(machine, p)
            && let Err(e) = crate::step::drop_function(machine, &st, p, &mut m)
        {
            skipped.push(Skipped { machine: machine.name.clone(), reason: format!("Verwurf im `idle`: {}", e.what) });
        }
        // 5.11: je gescopter Instanz ein Praedikat auf der Konfiguration.
        for (i, si) in machine.states.iter().flat_map(|s| s.instances.iter()).enumerate() {
            let _ = crate::step::scope_function(machine, &st, i, si.scope, &mut m);
        }
        // 5.11: die `exit:`-Bloecke, wenn der Besitzer den Scope verlaesst.
        let _ = crate::step::exit_all_function(machine, &st, p, &mut m);
        // 7.5: die Trigger-Phase der Maschine, die sie armiert.
        if let Err(e) = crate::step::trigger_function(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.to_string() });
        }
        let _ = crate::step::advance_function(machine, &st, p, &mut m);
        let _ = crate::step::deadline_function(machine, &st, p, &mut m);
        if let Err(e) = crate::psi::publish_function(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.to_string() });
        }
        if let Err(e) = crate::step::step_function(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.to_string() });
        }
        if let Err(e) = crate::step::deliver_function(machine, &st, &mut m)
            .and_then(|()| crate::step::pend_function(machine, &st, &mut m))
        {
            skipped.push(Skipped { machine: machine.name.clone(), reason: format!("Abort-Phase: {}", e.what) });
        }
        // Zuletzt: Was die Schritte an Entry-Tick-Funktionen angefordert haben.
        if let Err(e) = crate::step::entry_functions(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.into() });
        }
        entries(machine, p, &arena, &mut m);
    }

    // 13.3: Laufzeitmonitore hinter den Maschinen; sie lesen nur das Abbild.
    for (i, prop) in p.properties.iter().enumerate().filter(|(_, prop)| prop.monitor) {
        if let Err(e) = crate::monitor::monitor_function(i, prop, p, &mut m) {
            skipped.push(Skipped { machine: format!("monitor {}", prop.name), reason: e.what.to_string() });
        } else if let Some(state) = arena.monitor(i) {
            let (symbol, body) = (crate::arena::monitor_symbol(m.prefix(), i), format!("monitor_{i}"));
            crate::arena::entry(&symbol, &body, crate::arena::MONITOR, state.offset, &arena, &mut m);
        }
    }

    Lowered { ir: m.finish(), skipped, without_persist }
}

/// Die Einstiege einer Maschine (12.11): je Rumpf, den der Codegen
/// vollstaendig geschrieben hat, `P_<maschine>_<endung>(ptr %arena, …)`.
fn entries(machine: &takt_mir::machine::Machine, p: &Program, arena: &crate::arena::Arena, m: &mut Module) {
    let Some(state) = arena.state(&machine.name) else { return };
    for (suffix, shape) in crate::arena::entries(machine, p) {
        let body = format!("{}_{suffix}", machine.name);
        if m.defines(&body) {
            let symbol = crate::arena::entry_symbol(m.prefix(), &machine.name, &suffix);
            crate::arena::entry(&symbol, &body, shape, state.offset, arena, m);
        }
    }
}
