//! Golden-Traces der Beispiele 14.1 bis 14.5: jedes Szenario aus
//! `corpus-try/sim/manifest.csv` laeuft mit seinem Stimulus und muss den
//! aufgezeichneten Trace zeichengleich erzeugen. Jeder Lauf wird zusaetzlich
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
    let path = root().join(example).join("program.takt");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: profile.map(str::to_string) };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{example}: unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{example}: kein Programm"))
}

#[test]
fn golden_traces_match() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok();
    let cases = cases();
    assert!(cases.len() >= 12, "zu wenige Szenarien: {}", cases.len());
    for case in &cases {
        let dir = root().join(&case.example);
        let stim_path = dir.join(format!("{}.stim.trace", case.scenario));
        let golden_path = dir.join(format!("{}.golden.trace", case.scenario));
        let stim_text = std::fs::read_to_string(&stim_path).unwrap_or_default();
        let stimulus = Trace::parse(&stim_text).unwrap_or_else(|e| panic!("{}: {e}", stim_path.display()));

        let p = program(&case.example, case.profile.as_deref());
        let options =
            RunOptions { ticks: case.ticks, profile: case.profile.clone(), order_seed: None, ..Default::default() };
        let result =
            run(&p, &stimulus, &options).unwrap_or_else(|e| panic!("{}/{}: {e:?}", case.example, case.scenario));
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
    // Satz 9.4.1: der Trace ist eine Funktion der Inputs, nicht der Reihenfolge.
    for case in cases() {
        let dir = root().join(&case.example);
        let stim_text = std::fs::read_to_string(dir.join(format!("{}.stim.trace", case.scenario))).unwrap_or_default();
        let stimulus = Trace::parse(&stim_text).expect("Stimulus lesbar");
        let p = program(&case.example, case.profile.as_deref());
        let base = run(
            &p,
            &stimulus,
            &RunOptions { ticks: case.ticks, profile: case.profile.clone(), order_seed: None, ..Default::default() },
        )
        .expect("Lauf")
        .trace
        .render();
        for seed in [3u64, 17, 9001] {
            let permuted = run(
                &p,
                &stimulus,
                &RunOptions {
                    ticks: case.ticks,
                    profile: case.profile.clone(),
                    order_seed: Some(seed),
                    ..Default::default()
                },
            )
            .expect("Lauf")
            .trace
            .render();
            assert_eq!(permuted, base, "{}/{}: Reihenfolge {seed} aendert den Trace", case.example, case.scenario);
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
    // 13.5: ein Fault, der sein Ziel erreicht, macht den Lauf FAIL.
    let failing = cases().iter().filter(|c| c.verdict == "FAIL").count();
    assert!(failing >= 5, "zu wenige FAIL-Szenarien: {failing}");
    assert_ne!(Verdict::Fail.name(), Verdict::Pass.name());
}
