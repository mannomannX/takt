//! Sim-Build und HW-Build (11.3).
//!
//! 11.3 macht eine knappe, weitreichende Zusage: „Gleiche MIR für die
//! Logik; Bindungstabelle und Linkmenge (Plant-Modelle) unterscheiden
//! sich." Daran haengt mehr als Bequemlichkeit — die ganze Abnahme
//! setzt darauf auf. Ein Lauf in der Simulation belegt nur dann etwas
//! ueber die Hardware, wenn beide dieselbe Logik ausfuehren.
//!
//! **Der Logik-Hash ist das Messinstrument.** Er laeuft ueber die
//! Serialisierung *ohne* Bindungen, Metadaten und Positionen (`hash.rs`,
//! 11.3). Zwei Builds desselben Programms muessen ihn teilen; taeten sie
//! es nicht, waere der Unterschied mehr als eine Bindungstabelle.
//!
//! `roundtrip.rs` prueft denselben Hash an synthetischer MIR — hier
//! laeuft ein echtes Programm durch beide Pfade, weil die Zusage dem
//! *Compiler* gilt und nicht dem Hash.

use takt_mir::hash::{logic_hash, program_hash};
use takt_mir::program::{Binding, Direction, Program};

/// Ein Programm mit `sim`-Quelle und `hw`-Eingang an derselben Adresse
/// (8.3): der Fall, den 11.3 beschreibt.
const QUELLE: &str = "system:\n\
                      \x20   language = 1\n\
                      \x20   tick     = 10 ms\n\
                      \n\
                      input  p     : float[bar] in 0..10 bar @ hw(\"daq1/ai0\") with max_age = 50 ms\n\
                      output p_sim : float[bar] in 0..10 bar @ sim(\"daq1/ai0\")\n\
                      output v     : bool                    @ hw(\"do1/0\") with safe = false\n\
                      \n\
                      machine model:\n\
                      \x20   initial FEED\n\
                      \n\
                      \x20   state FEED:\n\
                      \x20       loop:\n\
                      \x20           p_sim = 3 bar\n\
                      \n\
                      machine ctrl:\n\
                      \x20   initial IDLE\n\
                      \n\
                      \x20   state IDLE:\n\
                      \x20       loop:\n\
                      \x20           v = p.valid and p > 2 bar\n";

fn build(kind: takt_sema::Build) -> Program {
    let options = takt_sema::Options { policy: takt_diag::Policy::default(), build: kind, profile: None };
    let out = takt_sema::compile(QUELLE, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{kind:?}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// **Die Zusage aus 11.3**: Beide Builds tragen dieselbe Logik.
#[test]
fn both_builds_share_the_same_logic() {
    let sim = build(takt_sema::Build::Sim);
    let hw = build(takt_sema::Build::Hw);
    assert_eq!(
        logic_hash(&sim),
        logic_hash(&hw),
        "Sim- und HW-Build tragen verschiedene Logik; 11.3 verlangt dieselbe"
    );
}

/// Die Bindungen sind das, was sich unterscheiden *darf* — und sie
/// stehen ausserhalb des Logik-Hashs (11.3, 8.3).
///
/// Der Test haelt fest, dass die Bindungstabelle ueberhaupt existiert:
/// Ein Programm, in dem jeder Kanal `Binding::None` traegt, erfuellte
/// die Zusage trivial und sagte nichts.
#[test]
fn the_binding_table_is_what_may_differ() {
    let sim = build(takt_sema::Build::Sim);
    let gebunden = sim.channels.iter().filter(|c| !matches!(c.binding, Binding::None)).count();
    assert_eq!(gebunden, 3, "alle drei Kanaele tragen eine Bindung");

    // Dieselbe Logik mit geloeschten Bindungen: Der Logik-Hash bleibt,
    // der Programm-Hash nicht. Das ist die Trennlinie, die 11.3 zieht.
    let mut ohne = sim.clone();
    for c in &mut ohne.channels {
        c.binding = Binding::None;
    }
    assert_eq!(logic_hash(&sim), logic_hash(&ohne), "die Bindung gehoert nicht zur Logik");
    assert_ne!(program_hash(&sim), program_hash(&ohne), "der Programm-Hash traegt sie sehr wohl");
}

/// Die Linkmenge unterscheidet sich: Ein Plant-Modell ist eine Maschine
/// wie jede andere (8.3), aber auf Hardware laeuft sie nicht mit.
///
/// Der Compiler entscheidet das heute nicht — beide Builds tragen alle
/// Maschinen, und *wer* tickt, legt die Runtime fest (12.1, `build_all`
/// gegen `build_with`). Der Test haelt den Stand fest, damit eine
/// spaetere Trennung im Compiler auffaellt statt still zu geschehen.
#[test]
fn the_plant_model_is_an_ordinary_machine() {
    let sim = build(takt_sema::Build::Sim);
    let hw = build(takt_sema::Build::Hw);
    let namen = |p: &Program| p.machines.iter().map(|m| m.name.clone()).collect::<Vec<_>>();
    assert_eq!(namen(&sim), namen(&hw), "die Maschinenliste ist in beiden Builds dieselbe");
    assert!(namen(&sim).contains(&"model".to_string()), "das Modell ist eine gewoehnliche Maschine");

    // Was es zum Modell macht, ist seine `sim`-Bindung — nicht seine
    // Art. Die Runtime liest sie, um zu entscheiden, wer auf Hardware
    // laeuft.
    let model_out = sim.channels.iter().find(|c| c.name == "p_sim").expect("`p_sim` steht im Programm");
    assert!(matches!(model_out.binding, Binding::Sim(_)), "`p_sim` traegt eine `sim`-Bindung");
    assert_eq!(model_out.dir, Direction::Output);
}
