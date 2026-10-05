//! Golden-Traces der Beispiele: jedes Szenario aus
//! `corpus-try/sim/manifest.csv` laeuft mit seinem Stimulus und muss den
//! aufgezeichneten Trace zeichengleich erzeugen; die Zeilen aus
//! `<szenario>.expect` stehen von Hand daneben. Jeder Lauf wird zusaetzlich
//! mit permutierter Schrittreihenfolge wiederholt (Satz 9.4.1).
//!
//! `UPDATE_GOLDEN=1 cargo test -p takt-sema --test golden_sim` schreibt die
//! Traces neu.

use std::path::{Path, PathBuf};

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, Verdict, run};
use takt_sema::{Build, Options};

/// Eine Zeile des Manifests.
struct Case {
    example: String,
    scenario: String,
    ticks: u64,
    profile: Option<String>,
    verdict: String,
}

fn root() -> PathBuf {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/sim")).to_path_buf()
}

/// Liest das Manifest; Felder in Anfuehrungszeichen duerfen Kommas tragen.
fn cases() -> Vec<Case> {
    let text = std::fs::read_to_string(root().join("manifest.csv")).expect("manifest.csv lesbar");
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let f = split_csv(line);
            assert!(f.len() >= 5, "Zeile unvollstaendig: {line}");
            Case {
                example: f[0].clone(),
                scenario: f[1].clone(),
                ticks: f[2].parse().unwrap_or_else(|e| panic!("ticks in `{line}`: {e}")),
                profile: (!f[3].is_empty()).then(|| f[3].clone()),
                verdict: f[4].clone(),
            }
        })
        .collect()
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    out.push(field);
    out
}

/// Uebersetzt das Programm eines Beispiels.
fn program(example: &str, profile: Option<&str>) -> takt_mir::Program {
    // Ein Beispiel ohne eigenes `program.takt` ist ein Korpusprogramm
    // (`corpus-try/<beispiel>.takt`); sein Verzeichnis traegt nur die Faelle.
    let own = root().join(example).join("program.takt");
    let path = if own.exists() { own } else { root().join("..").join(format!("{example}.takt")) };
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = Options {
        policy: Policy::default(),
        build: Build::Sim,
        profile: profile.map(str::to_string),
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{example}: unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{example}: kein Programm"))
}

/// 13.6: Heisst eine Szenario-Maschine wie der Fall (Leerzeichen als `_`),
/// laeuft sie mit; sonst kommt alles aus dem Stimulus.
fn scenario_machine(p: &takt_mir::Program, case: &str) -> Option<String> {
    p.machines
        .iter()
        .filter(|m| m.kind == takt_mir::machine::MachineKind::Scenario)
        .find(|m| m.name.replace(' ', "_") == case || m.name == case)
        .map(|m| m.name.clone())
}

/// Die Laufeinstellungen eines Falls: dieselben fuer den Golden-Lauf und
/// fuer die permutierten Laeufe (Satz 9.4.1 gilt fuer *diesen* Lauf).
fn options_of(case: &Case, p: &takt_mir::Program, order_seed: Option<u64>) -> RunOptions {
    RunOptions {
        ticks: case.ticks,
        profile: case.profile.clone(),
        order_seed,
        scenario: scenario_machine(p, &case.scenario),
        ..Default::default()
    }
}

/// Der Stimulus eines Falls. Ein Lauf ohne Eingaben hat eine Datei mit
/// einem Kommentar statt keiner: Eine fehlende Datei ist ein Fehler, kein
/// leerer Stimulus (KOR-031).
fn stimulus_text(case: &Case) -> String {
    let path = root().join(&case.example).join(format!("{}.stim.trace", case.scenario));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (ohne Eingaben: eine Datei mit `# Leer mit Absicht: …`)", path.display()))
}

#[test]
fn golden_traces_match() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    let cases = cases();
    assert!(cases.len() >= 12, "zu wenige Szenarien: {}", cases.len());
    for case in &cases {
        let dir = root().join(&case.example);
        let golden_path = dir.join(format!("{}.golden.trace", case.scenario));
        let stimulus = Trace::parse(&stimulus_text(case))
            .unwrap_or_else(|e| panic!("{}/{}: Stimulus: {e}", case.example, case.scenario));

        let p = program(&case.example, case.profile.as_deref());
        let result = run(&p, &stimulus, &options_of(case, &p, None))
            .unwrap_or_else(|e| panic!("{}/{}: {e:?}", case.example, case.scenario));
        let text = result.trace.render();

        assert_eq!(result.verdict.name(), case.verdict, "{}/{}: Verdikt weicht ab", case.example, case.scenario);

        if update {
            std::fs::write(&golden_path, &text).expect("Golden schreibbar");
            continue;
        }
        let expected = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("{}: {e} (UPDATE_GOLDEN=1 erzeugt ihn)", golden_path.display()));
        assert_eq!(text, expected, "{}/{}: Trace weicht ab", case.example, case.scenario);
    }
}

#[test]
fn step_order_does_not_change_any_golden_trace() {
    // Satz 9.4.1: der Trace ist eine Funktion der Inputs, nicht der
    // Reihenfolge — verglichen mit dem Golden-Trace, mit derselben
    // Szenario-Maschine wie dort.
    for case in cases() {
        let dir = root().join(&case.example);
        let stim_text = stimulus_text(&case);
        let stimulus = Trace::parse(&stim_text).expect("Stimulus lesbar");
        let golden_path = dir.join(format!("{}.golden.trace", case.scenario));
        let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| panic!("{}: {e}", golden_path.display()));
        let p = program(&case.example, case.profile.as_deref());
        for seed in [3u64, 17, 9001] {
            let permuted = run(&p, &stimulus, &options_of(&case, &p, Some(seed))).expect("Lauf").trace.render();
            assert_eq!(permuted, golden, "{}/{}: Reihenfolge {seed} aendert den Trace", case.example, case.scenario);
        }
    }
}

#[test]
fn every_golden_trace_round_trips() {
    // Der Golden-Trace ist im Format aus grammar/trace.md lesbar (T1 bis T5).
    for case in cases() {
        let path = root().join(&case.example).join(format!("{}.golden.trace", case.scenario));
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let trace = Trace::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(trace.render(), text, "{}: nicht zeichengleich", path.display());
    }
}

#[test]
fn a_failing_run_reports_fail() {
    // 13.5: FAIL heisst ein Fault hat sein Ziel erreicht, ein `verify` ist
    // verletzt, ein `verdict fail` oder eine Eigenschaft ist gebrochen. Jeder
    // FAIL-Lauf des Manifests nennt seinen Grund im Trace; kein anderer Lauf
    // endet mit FAIL.
    let cases = cases();
    let failing = cases.iter().filter(|c| c.verdict == "FAIL").count();
    assert!(failing >= 5, "zu wenige FAIL-Szenarien: {failing}");
    let reason = |line: &str| {
        line.contains(" fault ")
            || line.contains(" verify ") && line.contains(" fail ")
            || line.contains(" verdict ") && line.contains(" fail")
            || line.contains(" violated ")
    };
    for case in &cases {
        let stim_text = stimulus_text(case);
        let stimulus = Trace::parse(&stim_text).expect("Stimulus lesbar");
        let p = program(&case.example, case.profile.as_deref());
        let result = run(&p, &stimulus, &options_of(case, &p, None)).expect("Lauf");
        let text = result.trace.render();
        let fails = result.verdict == Verdict::Fail;
        assert_eq!(fails, case.verdict == "FAIL", "{}/{}: {:?}", case.example, case.scenario, result.verdict);
        if fails {
            assert!(text.lines().any(reason), "{}/{}: FAIL ohne Grund im Trace:\n{text}", case.example, case.scenario);
        }
    }
}

/// **Jedes Szenario zeigt, was sein Manifest beschreibt** (KOR-031): Die
/// Zeilen aus `<szenario>.expect` stehen in dieser Reihenfolge im
/// Golden-Trace. Den Trace schreibt `UPDATE_GOLDEN`; die Erwartung steht von
/// Hand daneben und faellt auf, wenn ein neuer Golden-Trace die
/// beschriebene Aussage verliert.
#[test]
fn every_scenario_shows_its_expected_lines() {
    for case in cases() {
        let dir = root().join(&case.example);
        let expect_path = dir.join(format!("{}.expect", case.scenario));
        let expected =
            std::fs::read_to_string(&expect_path).unwrap_or_else(|e| panic!("{}: {e}", expect_path.display()));
        let golden_path = dir.join(format!("{}.golden.trace", case.scenario));
        let golden = std::fs::read_to_string(&golden_path).unwrap_or_else(|e| panic!("{}: {e}", golden_path.display()));
        let mut lines = golden.lines();
        for want in expected.lines().filter(|l| !l.trim().is_empty()) {
            assert!(
                lines.any(|l| l == want),
                "{}/{}: `{want}` fehlt oder steht nicht in dieser Reihenfolge",
                case.example,
                case.scenario
            );
        }
    }
}
