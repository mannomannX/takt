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
use takt_mir::Program;
use takt_mir::machine::MachineKind;

/// Was beim Senken nicht ging.
///
/// **Ein Abbruchgrund gehoert in die Ausgabe, nicht in den Papierkorb.**
/// Ohne ihn fehlt eine Schrittfunktion still, und der Linker meldet ein
/// fehlendes Symbol statt des Konstrukts, das gefehlt hat (FB-104).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// Die Maschine, deren Schritt fehlt.
    pub machine: String,
    /// Warum.
    pub reason: String,
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
/// `triple` bestimmt allein den Kopf der Ausgabe — der Rumpf ist fuer
/// jedes Ziel derselbe. Genau darauf beruht Satz 9.4.4: Bit-Gleichheit
/// ist damit eine Aussage ueber *eine* Uebersetzung, nicht ueber mehrere
/// Programme.
pub fn program(p: &Program, triple: &str, module_name: &str) -> Lowered {
    program_with(p, triple, module_name, crate::target::Instrument::Off)
}

/// Wie [`program`], mit Instrumentierung (11.2): `pc` im Zustand traegt
/// je Anweisung oder je Zustandswechsel, wo die Maschine steht.
pub fn program_with(p: &Program, triple: &str, module_name: &str, instrument: crate::target::Instrument) -> Lowered {
    program_with_diagnostics(p, triple, module_name, instrument, crate::target::Diagnostics::Ids)
}

/// Wie [`program_with`], mit Diagnosestufe (plan/codegen-hebel.md C).
pub fn program_with_diagnostics(
    p: &Program,
    triple: &str,
    module_name: &str,
    instrument: crate::target::Instrument,
    diagnostics: crate::target::Diagnostics,
) -> Lowered {
    let mut m = Module::new(module_name, triple).with_instrument(instrument).with_diagnostics(diagnostics);
    let mut skipped = Vec::new();
    let mut without_persist = Vec::new();

    crate::abi::Abi::declare(&mut m);
    crate::stream::Streams::declare(&mut m);

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

    for (i, f) in p.fns.iter().enumerate() {
        if !methods.contains(&takt_mir::FnId(i as u32)) {
            let _ = crate::fns::function(f, p, &mut m);
        }
    }

    for machine in &p.machines {
        // Eine Vorlage hat keine eigene Schrittfunktion — nur ihre
        // Instanzen laufen (5.9). Sie zu senken meldete „Maschine ohne
        // Blattzustand", und das laese sich wie ein Mangel (FB-118).
        if machine.kind == MachineKind::Template {
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
        let _ = crate::step::init_function(machine, &st, p, &mut m, vars.is_err() || enter.is_err());
        if !machine.persist.is_empty() {
            let snapshot = crate::persist::snapshot_function(machine, &st, p, &mut m);
            let restore = crate::persist::restore_function(machine, &st, p, &mut m);
            if vars.is_err() || enter.is_err() || snapshot.is_err() || restore.is_err() {
                without_persist.push(machine.name.clone());
            }
        }
        let _ = crate::step::idle_function(machine, &st, &mut m);
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
        let _ = crate::step::advance_function(machine, &st, &mut m);
        let _ = crate::step::deadline_function(machine, &st, p, &mut m);
        if let Err(e) = crate::psi::publish_function(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.to_string() });
        }
        if let Err(e) = crate::step::step_function(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.to_string() });
        }
        // Zuletzt: Was die Schritte an Entry-Tick-Funktionen angefordert haben.
        if let Err(e) = crate::step::entry_functions(machine, &st, p, &mut m) {
            skipped.push(Skipped { machine: machine.name.clone(), reason: e.what.into() });
        }
    }

    // 13.3: Laufzeitmonitore hinter den Maschinen; sie lesen nur das Abbild.
    for (i, prop) in p.properties.iter().enumerate().filter(|(_, prop)| prop.monitor) {
        if let Err(e) = crate::monitor::monitor_function(i, prop, p, &mut m) {
            skipped.push(Skipped { machine: format!("monitor {}", prop.name), reason: e.what.to_string() });
        }
    }

    Lowered { ir: m.finish(), skipped, without_persist }
}
