//! Die Beispiele der Referenz auf beiden Wegen (M4-Exit, 13.8).
//!
//! `plan.md` verlangt fuer M4 „14.1–14.6 in Echtzeit auf der Box". Die
//! Programme liegen in `corpus-try/sim/<beispiel>/`, laufen im Interpreter
//! gegen ihre Golden-Traces (`takt-sema/tests/golden_sim.rs`) — und liefen
//! bis hierher *nur* dort: Der differentielle Test kannte sie nicht, und der
//! Exit war an dieser Stelle behauptet statt belegt (FB-102).
//!
//! **Dieselbe Liste wie die Golden-Traces** (KOR-032): Jede Zeile von
//! `corpus-try/sim/manifest.csv` — Beispiel, Szenario, Ticks, Profil — laeuft
//! hier mit ihrem Stimulus (`<szenario>.stim.trace`) im Interpreter, im
//! erzeugten Code und im Modell von `takt prove` (M11 Schritt 29b): Die
//! vollstaendigen Programme der Referenz sind Korpus wie jedes andere, mit
//! dem Verhalten, das ihr Text nennt, als Golden-Trace. Eine Handliste liess `sim/11_4` (die Bloecke der
//! Standardbibliothek) und `sim/12_6` (`max_slew`, `debounce` am Rand) aus,
//! und ohne Stimulus sah der Vergleich nur den Anfangszustand.
//!
//! **Warum sie einen eigenen Test brauchen.** Die meisten haben ein
//! Plant-Modell (8.3) — eine zweite Maschine, die `sim`-Outputs auf
//! dieselben Adressen schreibt, von denen das Programm liest.
//! `harness::build_scenario` fuehrt darum alle Maschinen samt dem Szenario,
//! jede nach ihrer Periode, mit der Bindung `sim` -> `hw` am Ende des Ticks.
//!
//! **Unter AddressSanitizer** laufen dieselben Faelle ein zweites Mal
//! (FB-461): Ein Zugriff neben einen Platz faellt dem Vergleich erst auf,
//! wenn er einen Wert trifft, den ein Ausgang zeigt.

use takt_conformance::harness;
use takt_conformance::run::compare;
use takt_conformance::stimulus::Stimulus;
use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::machine::MachineKind;
use takt_mir::{MachineId, Program};
use takt_sema::{Build, Options};

mod common;

/// Bekannte Luecken des Wirtsrahmens: Beispiel, Ausgang, Grund. Jede muss
/// noch auftreten — schliesst sich eine, scheitert der Test, bis sie hier
/// verschwindet; eine neue Abweichung faellt nie hinein.
const GAPS: &[(&str, &str, &str)] = &[(
    "14_8",
    "keys_sim",
    "FB-402: Der Wirtsrahmen schreibt keinen Record-Ausgang mit `bytes`-Feld. \
     TODO(M11 Schritt 9): mit dem Produktrahmen",
)];

/// Was das Modell an einem Beispiel nicht kodiert (FB-497): Beispiel, die
/// Meldung von `takt_prove::encode` oder `Model::run` und was dahinter
/// steht. Eine Ratsche wie [`GAPS`]: Jede Luecke muss noch auftreten, eine
/// neue faellt nie hinein.
const MODEL_GAPS: &[(&str, &str, &str)] = &[
    (
        "14_8",
        "Eingabestrom aus dem `sim`-Ausgang einer anderen Maschine",
        "der Kodierer ordnet den Sendestrom des Flash-Modells (`flash_model`, 8.10) in `fallback` und `no_image` keiner kodierten Maschine zu",
    ),
    ("14_8", "ein Zustand aus mehr als 32768 Blaettern", "das Flash-Modell, ein Zustand ueber `STATE_LIMIT` (FB-485)"),
];

/// Eine Zeile von `corpus-try/sim/manifest.csv`.
struct Case {
    example: String,
    scenario: String,
    ticks: u64,
    profile: Option<String>,
}

impl Case {
    fn label(&self) -> String {
        format!("{}/{}", self.example, self.scenario)
    }

    /// Der Stimulus aus `<szenario>.stim.trace`: als Trace fuer den
    /// Interpreter, als Eingaben fuer den Rahmen.
    fn stimulus(&self) -> (Trace, Vec<Stimulus>) {
        let label = self.label();
        let path = sim_dir().join(&self.example).join(format!("{}.stim.trace", self.scenario));
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let stimulus = Trace::parse(&text).unwrap_or_else(|e| panic!("{label}: {e}"));
        let inputs = Stimulus::from_trace(&stimulus).unwrap_or_else(|e| panic!("{label}: {e}"));
        (stimulus, inputs)
    }

    /// Das Programm, wie der Rahmen es fuehrt: Den Parametervektor setzt
    /// er aus den Defaults; das Profil des Falls geht darum vorher in sie
    /// ein, wie im Interpreter (8.4). `None` ohne Profil.
    fn profiled(&self, p: &Program) -> Option<Program> {
        self.profile.as_deref().map(|name| {
            harness::with_profile(p, name).unwrap_or_else(|| panic!("{}: kein Profil {name}", self.label()))
        })
    }
}

fn sim_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/sim")
}

/// Die Faelle des Manifests; ein Feld in Anfuehrungszeichen darf Kommas
/// tragen (die Beschreibung).
fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(sim_dir().join("manifest.csv")).expect("sim/manifest.csv lesbar");
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<&str> = lines.next().expect("Kopf").split(',').collect();
    assert_eq!(head[..4], ["beispiel", "szenario", "ticks", "profil"], "die Spalten des Manifests");
    lines
        .map(|line| {
            let f: Vec<&str> = line.splitn(5, ',').collect();
            Case {
                example: f[0].to_string(),
                scenario: f[1].to_string(),
                ticks: f[2].parse().unwrap_or_else(|e| panic!("`{line}`: {e}")),
                profile: (!f[3].is_empty()).then(|| f[3].to_string()),
            }
        })
        .collect()
}

fn example(name: &str, profile: Option<&str>) -> Program {
    // Ein Beispiel ohne eigenes `program.takt` ist ein Korpusprogramm
    // (`corpus-try/<beispiel>.takt`); sein Verzeichnis traegt nur die Faelle.
    let own = sim_dir().join(name).join("program.takt");
    let path = if own.exists() { own } else { sim_dir().join("..").join(format!("{name}.takt")) };
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = Options {
        policy: Policy::default(),
        build: Build::Sim,
        profile: profile.map(str::to_string),
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Die Szenario-Maschine eines Falls, wie `golden_sim.rs` sie waehlt.
fn scenario_machine(p: &Program, scenario: &str) -> Option<String> {
    p.machines
        .iter()
        .filter(|m| m.kind == MachineKind::Scenario)
        .find(|m| m.name.replace(' ', "_") == scenario || m.name == scenario)
        .map(|m| m.name.clone())
}

/// Der Tick einer Trace-Zeile.
fn tick_of(line: &str) -> u64 {
    line.strip_prefix("t=").and_then(|r| r.split_whitespace().next()).and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// **Die Abnahme.** Jedes Szenario jedes Beispiels liefert mit seinem
/// Stimulus im Interpreter, im erzeugten Code und im Modell dieselben
/// Outputs (Satz 9.4.4); zwischen Interpreter und Modell gilt der strengere
/// Vergleich Zeile fuer Zeile (`takt_prove::mismatches`).
#[test]
fn the_reference_examples_agree_on_both_paths() {
    let Some(clang) = common::clang() else { return };
    let cases = cases();
    let mut failed = Vec::new();
    let mut gaps_seen = Vec::new();
    let mut model_gaps = Vec::new();
    for case in &cases {
        let label = case.label();
        let p = example(&case.example, case.profile.as_deref());
        let (stimulus, inputs) = case.stimulus();
        let scenario = scenario_machine(&p, &case.scenario);
        let options = RunOptions {
            ticks: case.ticks,
            profile: case.profile.clone(),
            scenario: scenario.clone(),
            inputs: true,
            ..Default::default()
        };
        let result = match run(&p, &stimulus, &options) {
            Ok(r) => r,
            Err(e) => {
                failed.push(format!("{label}: der Interpreter bricht ab: {e:?}"));
                continue;
            }
        };
        let interpreted = result.trace.render();
        let name = format!("{}_{}", case.example, case.scenario);
        let profiled = case.profiled(&p);
        let p = profiled.as_ref().unwrap_or(&p);
        match modelled(p, scenario.as_deref(), &stimulus, &result, case.ticks) {
            Ok(diffs) if diffs.is_empty() => {}
            Ok(diffs) => {
                let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
                failed.push(format!("{label}: Interpreter und Modell weichen ab:\n{}", list.join("\n")));
            }
            Err(why) => model_gaps.push((case.example.clone(), why)),
        }
        let native = match &scenario {
            Some(s) => common::run_native_scenario_with(&clang, p, &name, s, case.ticks, &inputs),
            None => common::run_native_all_with(&clang, p, &name, case.ticks, &inputs),
        };
        let native = match native {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{label}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        // Der Interpreter endet mit dem Szenario (13.6); der Rahmen laeuft
        // die Ticks zu Ende, verglichen wird bis dorthin.
        let last = result.trace.lines.iter().map(|l| l.tick).max().unwrap_or(0);
        let native: String = native.lines().filter(|l| tick_of(l) <= last).map(|l| format!("{l}\n")).collect();
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(p));
        let (gaps, diffs): (Vec<_>, Vec<_>) = compare(&widened, &native).into_iter().partition(|d| {
            GAPS.iter()
                .any(|(example, output, _)| *example == case.example && d.output == *output && d.native == "fehlt")
        });
        gaps_seen.extend(gaps.iter().map(|d| (case.example.clone(), d.output.clone())));
        if !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{label}: {} Abweichungen\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    for example in ["11_4", "12_6", "12_7", "14_1", "14_7", "14_8"] {
        assert!(cases.iter().any(|c| c.example == example), "{example} fehlt im Manifest");
    }
    for (example, output, why) in GAPS {
        assert!(
            gaps_seen.iter().any(|(n, o)| n == example && o == output),
            "die Luecke {example} `{output}` ist geschlossen; den Eintrag in `GAPS` entfernen ({why})"
        );
    }
    let mut unexpected: Vec<String> = model_gaps
        .iter()
        .filter(|(n, why)| !MODEL_GAPS.iter().any(|(e, w, _)| e == n && why.contains(w)))
        .map(|(n, why)| format!("{n}: das Modell rechnet nicht, `MODEL_GAPS` nennt das nicht: {why}"))
        .collect();
    unexpected.dedup();
    for (example, message, why) in MODEL_GAPS {
        if !model_gaps.iter().any(|(n, w)| n == example && w.contains(message)) {
            unexpected
                .push(format!("{example}: das Modell rechnet wieder; `MODEL_GAPS` nennt noch `{message}` ({why})"));
        }
    }
    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
}

/// Der Lauf des Modells zum Lauf `result` des Interpreters, mit dem
/// Szenario, das mitlief: die Abweichungen nach `takt_prove::mismatches`;
/// `Err` mit dem Grund, wenn das Modell das Beispiel nicht kodiert.
fn modelled(
    p: &Program,
    scenario: Option<&str>,
    stimulus: &Trace,
    result: &takt_interp::RunResult,
    ticks: u64,
) -> Result<Vec<String>, String> {
    let model = match scenario {
        Some(s) => {
            let id = p.machines.iter().position(|m| m.name == s).ok_or_else(|| format!("kein Szenario `{s}`"))?;
            takt_prove::encode_scenario(p, MachineId(id as u32))
        }
        None => takt_prove::encode(p),
    };
    let model = model.map_err(|e| e.what)?;
    match model.run(p, stimulus, result, ticks) {
        Ok(traced) => Ok(takt_prove::mismatches(&result.trace, &traced)),
        Err(e @ takt_prove::Stop::Gap { .. }) => Err(e.to_string()),
        // Eine Invariante, die im Lauf nicht haelt, ist ein Fehler des Modells.
        Err(e) => Ok(vec![e.to_string()]),
    }
}

/// **Kein Zugriff daneben** (FB-461): Jeder Fall laeuft im erzeugten Code
/// unter AddressSanitizer bis zum letzten Tick. Ein Chunk eines
/// `stream<u8>` lief ueber den Platz seines Lesers hinaus, und der
/// Nachbarplatz fing ihn unbemerkt auf.
#[test]
fn the_reference_examples_run_clean_under_address_sanitizer() {
    let Some(clang) = common::clang() else { return };
    let hint = "die Laufzeit von AddressSanitizer (compiler-rt) zu clang installieren";
    let Some(search) = takt_testkit::require("asan", common::asan_path(&clang), hint) else { return };
    let mut failed = Vec::new();
    for case in &cases() {
        let p = example(&case.example, case.profile.as_deref());
        let (_, inputs) = case.stimulus();
        let scenario = scenario_machine(&p, &case.scenario);
        let profiled = case.profiled(&p);
        let p = profiled.as_ref().unwrap_or(&p);
        let h = match &scenario {
            Some(s) => harness::build_scenario(p, s, case.ticks, &inputs),
            None => harness::build_restoring(p, None, case.ticks, &inputs, &[]),
        };
        let name = format!("asan_{}_{}", case.example, case.scenario);
        if let Err(e) = common::run_native_sanitized(&clang, p, &name, case.ticks, h, &search) {
            failed.push(format!("{}:\n{e}", case.label()));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}
