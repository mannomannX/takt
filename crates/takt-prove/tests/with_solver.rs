//! `takt prove` mit Solver (plan/m6.md 2.8): bewiesen, verletzt mit im
//! Interpreter bestaetigtem Gegenbeispiel, unbewiesen mit Grund. Die Tests
//! ueberspringen sich ohne Solver (`TAKT_SOLVER`, `z3`, `cvc5`).

use takt_mir::Program;
use takt_mir::analysis::proof::{Site, parse, render};
use takt_prove::{
    CheckVerdict, ContractVerdict, Solver, Verdict, classify, classify_compositional, encode, find, prove,
    verify_contracts,
};

fn compile(src: &str) -> Program {
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn corpus_with(name: &str, properties: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    compile(&format!("{src}\n{properties}\n"))
}

/// Der Solver; fehlt er, scheitert der Test, es sei denn,
/// `TAKT_ALLOW_MISSING` erlaubt das Fehlen (FB-392).
fn solver() -> Option<Solver> {
    let found = Some(find()).filter(|s| !matches!(s, Solver::Missing));
    takt_testkit::require("solver", found, "`TAKT_SOLVER` setzen oder z3/cvc5 installieren")
}

/// Ein Programm, das `n` Durchlaeufe einer Schleife in `outer` Durchlaeufen ausrollt.
fn nested_loops(outer: u32, n: u32) -> Program {
    compile(&format!(
        "system:\n    language = 1\n    tick     = 10 ms\n\n\
         output total : int @ hw(\"total\") with safe = 0\n\n\
         machine m:\n    var acc : int = 0\n    initial RUN\n\n    state RUN:\n        loop:\n\
         \x20           for i in range({outer}):\n                for j in range({n}):\n\
         \x20                   acc = (acc + 1) % 1000\n            total = acc\n"
    ))
}

/// **Die Grenze des Ausrollens.** Eine Schleife ueber der Grenze ist
/// ausser Reichweite, statt das Modell ohne Ende wachsen zu lassen:
/// `watchdog.takt` rollte 1000 × 3000 Durchlaeufe aus (FB-403). Darunter
/// kodiert er weiter; der Test braucht keinen Solver.
#[test]
fn a_loop_beyond_the_unroll_limit_is_out_of_reach() {
    let limit = u32::try_from(takt_prove::encode::UNROLL_LIMIT).expect("Grenze");
    for (outer, n) in [(64, limit / 64), (1, limit)] {
        let e = encode(&nested_loops(outer, n)).expect_err("ueber der Grenze");
        assert!(e.what.contains("Durchlaeufe"), "{}", e.what);
    }
    let r = encode(&nested_loops(1, limit - 1));
    assert!(r.is_ok(), "an der Grenze: {:?}", r.err());
}

/// **Tiefe Terme im Stack eines Tests.** Ein Pfad an der Grenze vertieft
/// den Term des Akkumulators um jeden Durchlauf; Kodieren, Auswerten,
/// Schreiben als SMT-LIB2 und Abbauen rekursieren nicht ueber die Tiefe.
/// Vorher sprengten 64 × 64 Durchlaeufe den Stack (FB-403).
#[test]
fn a_path_at_the_unroll_limit_fits_the_stack_of_a_test() {
    let limit = u32::try_from(takt_prove::encode::UNROLL_LIMIT).expect("Grenze");
    let model = encode(&nested_loops(1, limit - 1)).expect("an der Grenze");
    let states = model.simulate(3, &|_, _| None);
    let total = |k: usize| match states[k].get("s.out.total") {
        Some(takt_prove::Val::Int(t)) => *t,
        other => panic!("{other:?}"),
    };
    // Jeder Tick zaehlt die Durchlaeufe des Pfads modulo 1000.
    assert_eq!((total(3) - total(2)).rem_euclid(1000), i64::from(limit - 1) % 1000);
    let text = takt_prove::export(&model, 2);
    assert!(text.contains("(check-sat)"));
}

#[test]
fn an_inductive_invariant_is_proven() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property vent_on_safe: always(tank_guard.state == SAFE implies vent)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].verdict, Verdict::Proven { k: 2, assumptions: vec![] }, "{:?}", reports[0]);
}

#[test]
fn a_violation_yields_a_counterexample_the_interpreter_confirms() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property never_vents: never(vent)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 4, &solver, 60).expect("Solver laeuft");
    let Verdict::Violated { at, stimulus } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    // `start`, dann ein Tankdruck ueber LIMIT: der Fault-Pfad oeffnet das Ventil.
    assert!(*at <= 4, "{at}");
    assert!(stimulus.contains("cmd start") && stimulus.contains("in tank_p "), "{stimulus}");
}

/// B3: `check tank_p < LIMIT` in 01 ist erreichbar — der Pfad ist ein
/// Stimulus, den der Interpreter bestaetigt.
#[test]
fn a_reachable_check_gets_its_path() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "");
    let model = encode(&p).expect("kodierbar");
    // Dazu die Lesestellen von `tank_p` (3.5): Ein ungueltiger Druck faultet.
    assert_eq!(model.checks.iter().filter(|c| c.kind == "check").count(), 1);
    let reports = classify(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let check = reports.iter().find(|r| r.kind == "check").expect("die `check`-Stelle");
    let CheckVerdict::Reachable { at, stimulus } = &check.verdict else { panic!("{check:?}") };
    assert!(*at <= 3, "{at}");
    assert!(stimulus.contains("cmd start"), "{stimulus}");
    assert_eq!(check.machine, "tank_guard");
}

/// B3: eine Pruefung, die der Typ schon garantiert, ist bewiesen
/// unerreichbar — die Ranges des Zustands sind Invarianten der Kodierung.
#[test]
fn a_check_the_types_guarantee_is_proven_unreachable() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 1 ms

output r : int in 0..200 @ hw(\"o/r\") with safe = 0

machine f:
    var n : int in 0..200 = 0
    initial RUN
    state RUN:
        loop:
            n = (n + 1) % 200
            r = n
            check n <= 200, \"never\"
",
    );
    let model = encode(&p).expect("kodierbar");
    let reports = classify(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    assert_eq!(reports[0].verdict, CheckVerdict::Unreachable { k: 2 }, "{:?}", reports[0]);
}

/// B2: das `ensures` des Begrenzers haelt aus jedem typkonformen Zustand;
/// sein `requires` kann an der Aufrufstelle verletzt sein — der Pfad kommt
/// aus dem Modell, weil der Interpreter Vertraege nicht prueft.
#[test]
fn block_contracts_are_proven_and_their_call_sites_classified() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("48_contracts.takt", "");
    let model = encode(&p).expect("kodierbar");
    let contracts = verify_contracts(&model, &solver, 60).expect("Solver laeuft");
    assert_eq!(contracts.len(), 1);
    assert_eq!(contracts[0].verdict, ContractVerdict::Proven, "{:?}", contracts[0]);
    let sites = classify(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "requires").expect("requires-Stelle");
    let CheckVerdict::Reachable { at, stimulus } = &site.verdict else { panic!("{site:?}") };
    assert!(*at <= 2 && stimulus.contains("in level -"), "{at} {stimulus}");
    // Ein Vertrag, der nicht haelt: das Ergebnis ist nicht immer kleiner als `x`.
    let p = corpus_with("48_contracts.takt", "").clone();
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/48_contracts.takt"))
        .expect("Korpus")
        .replace("ensures result <= hi", "ensures result < x");
    let bad = compile(&src);
    let model = encode(&bad).expect("kodierbar");
    let contracts = verify_contracts(&model, &solver, 60).expect("Solver laeuft");
    let ContractVerdict::Violated { values } = &contracts[0].verdict else { panic!("{:?}", contracts[0]) };
    assert!(values.contains("limiter.step.x = "), "{values}");
    let _ = p;
}

#[test]
fn a_true_but_not_inductive_property_stays_unproven_with_a_reason() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("01_minimal.takt", "property never_both: never(vent and tank_guard.state == WATCH)");
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let Verdict::Unproven { reason } = &reports[0].verdict else { panic!("{:?}", reports[0]) };
    assert!(reason.contains("Induktionsschritt offen"), "{reason}");
}

/// P3 (11.3): Eine implizite Range-Pruefung, die ein Uebergang unerreichbar
/// macht, ist bewiesen; die Beweisdatei laesst sie im Codegen aus.
#[test]
fn an_unreachable_implicit_check_is_proven_and_its_proof_drops_it() {
    let Some(solver) = solver() else { return };
    let src = "system:
    language = 1
    tick     = 10 ms

output y : int @ hw(\"o/y\") with safe = 0

machine counter:
    fault -> SAFE
    var x : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            x = x + 1
            y = x
        when x >= 50:
            -> RESET
    state RESET:
        loop:
            x = 0
            -> RUN
    state SAFE:
        enter:
            y = 7
";
    let p = compile(src);
    let model = encode(&p).expect("kodierbar");
    let sites = classify(&model, &p, 5, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").expect("Range-Stelle");
    assert_eq!(site.verdict, CheckVerdict::Unreachable { k: 5 }, "{site:?}");

    let hash = takt_mir::review::hash_of(src.as_bytes());
    let text =
        render(&hash, "z3 4.13.4", &[Site { start: site.start, end: site.span.end, kind: site.kind.clone(), k: 5 }]);
    let proof = parse(&text).expect("Beweisdatei");
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile_with(src, &options, Some(&proof));
    assert!(!out.has_errors(), "{:?}", out.diagnostics);
    assert_eq!(out.report.checks.get("Declared").copied().unwrap_or(0), 0, "{:?}", out.report.checks);
}

/// P3: Eine implizite Range-Pruefung mit Pfad — der Interpreter bestaetigt
/// den Fault.
#[test]
fn a_reachable_implicit_check_gets_its_path() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("19_faults.takt", "");
    let model = encode(&p).expect("kodierbar");
    let sites = classify(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").expect("Range-Stelle");
    let CheckVerdict::Reachable { at, .. } = &site.verdict else { panic!("{site:?}") };
    assert_eq!((*at, site.machine.as_str()), (1, "f"));
}

/// 13.3: Je Maschine, mit Ψ als freier Eingabe in seiner Range. Der
/// Erzeuger rechnet in `u64` und ist nicht kodierbar — die Kodierung
/// rechnet in 64 Bit mit Vorzeichen —; der Verbraucher wird trotzdem
/// bewiesen.
#[test]
fn a_machine_is_proven_alone_when_the_whole_is_not_encodable() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 10 ms

output y  : int @ hw(\"o/y\") with safe = 0

machine producer:
    pub var level : int in 0..100 = 0
    var wide : u64 = 0
    initial RUN
    state RUN:
        loop:
            wide = wide.wrap_u64() + 1
            level = (level + 1) % 101

machine consumer:
    var x : int in 0..100 = 0
    initial RUN
    state RUN:
        loop:
            x = x + (producer.level % 2)
            y = x
        when x >= 50:
            -> RESET
    state RESET:
        loop:
            x = 0
            -> RUN
",
    );
    assert!(encode(&p).is_err(), "das Ganze ist nicht kodierbar");
    let (sites, notes) = classify_compositional(&p, None, 5, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").unwrap_or_else(|| panic!("Range-Stelle: {notes:?}"));
    assert_eq!((site.machine.as_str(), &site.verdict), ("consumer", &CheckVerdict::Unreachable { k: 5 }), "{site:?}");
}

/// Ein Pfad im Maschinenmodell wird am Gesamtmodell gesucht und dort
/// vom Interpreter bestaetigt.
#[test]
fn a_path_found_alone_is_confirmed_on_the_whole() {
    let Some(solver) = solver() else { return };
    let p = corpus_with("19_faults.takt", "");
    let whole = encode(&p).expect("kodierbar");
    let (sites, _) = classify_compositional(&p, Some(&whole), 3, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").expect("Range-Stelle");
    let CheckVerdict::Reachable { at, .. } = &site.verdict else { panic!("{site:?}") };
    assert_eq!(*at, 1);
}

/// 13.3: `max_slew` gilt als Annahme — die Aenderung je Tick ist durch
/// die Rate beschraenkt, wie der Rand sie erzwingt (12.6).
#[test]
fn max_slew_bounds_the_change_per_tick() {
    let Some(solver) = solver() else { return };
    let body = |attr: &str| {
        format!(
            "system:
    language = 1
    tick     = 1 ms

input  x : float in -1000.0..1000.0 @ hw(\"i/x\"){attr}
output y : float @ hw(\"o/y\") with safe = 0.0

machine m:
    var last  : float = 0.0
    var armed : bool = false
    initial RUN
    state RUN:
        loop:
            if armed:
                check abs(x - last) <= 0.02, \"Sprung\"
            last = x
            armed = true
            y = last
"
        )
    };
    let check = |sites: Vec<takt_prove::CheckReport>| sites.into_iter().find(|s| s.kind == "check").expect("Stelle");
    let p = compile(&body(" with max_slew = 10.0"));
    let model = encode(&p).expect("kodierbar");
    assert!(model.state.iter().any(|v| v.name == "s.q.x.good.prev"), "der letzte gute Wert liegt im Zustand");
    let site = check(classify(&model, &p, 2, &solver, 60).expect("Solver laeuft"));
    assert_eq!(site.verdict, CheckVerdict::Unreachable { k: 2 }, "{site:?}");

    let p = compile(&body(""));
    let model = encode(&p).expect("kodierbar");
    let site = check(classify(&model, &p, 2, &solver, 60).expect("Solver laeuft"));
    assert!(matches!(site.verdict, CheckVerdict::Reachable { .. }), "ohne Rate springt x: {site:?}");
}

/// Ein Eingang bis 100, eine Variable bis 60, die ihn uebernimmt; die
/// Annahme beschraenkt ihn auf unter 50.
fn assumed(with_assumption: bool) -> Program {
    let assumption = if with_assumption { "assumption small: always(x < 50)\n" } else { "" };
    compile(&format!(
        "system:
    language = 1
    tick     = 10 ms

input x : int in 0..100 @ hw(\"i/x\")
output y : int @ hw(\"o/y\") with safe = 0

machine m:
    var v : int in 0..60 = 0
    initial RUN
    state RUN:
        loop:
            v = x
            y = v

{assumption}property below: always(x < 50)
"
    ))
}

/// INT-021: 13.3 — eine Annahme beschraenkt die Beweisverpflichtung einer
/// Eigenschaft, nicht die Typsicherheit (3.4 lehnt `assume` ab). Unter
/// `always(x < 50)` waere die Range-Pruefung von `v` unerreichbar; ohne
/// sie ist sie es nicht, und nur das darf in eine Beweisdatei.
#[test]
fn an_assumption_does_not_prove_a_check_away() {
    let Some(solver) = solver() else { return };
    let p = assumed(true);
    let model = encode(&p).expect("kodierbar");
    let sites = classify(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").expect("Range-Stelle");
    assert!(!matches!(site.verdict, CheckVerdict::Unreachable { .. }), "unter der Annahme bewiesen: {site:?}");
    let (sites, _) = classify_compositional(&p, Some(&model), 3, &solver, 60).expect("Solver laeuft");
    let site = sites.iter().find(|s| s.kind == "range").expect("Range-Stelle");
    assert!(!matches!(site.verdict, CheckVerdict::Unreachable { .. }), "unter der Annahme bewiesen: {site:?}");
}

/// INT-021: Dieselbe Eigenschaft ist mit der Annahme bewiesen, ohne sie
/// verletzt; das Urteil nennt die Annahme (13.3, „kein Urteil ohne seine
/// Annahmen"). Die Annahme selbst steht ohne sich: Frueher setzte ihre
/// Anfrage sie voraus, und sie hiess „bewiesen". Das Modell garantiert sie
/// nicht, also bleibt sie Annahme.
#[test]
fn an_assumption_proves_what_it_assumes_and_nothing_without_it() {
    let Some(solver) = solver() else { return };
    let p = assumed(true);
    let reports = prove(&encode(&p).expect("kodierbar"), &p, 3, &solver, 60).expect("Solver laeuft");
    let below = reports.iter().find(|r| r.name == "below").expect("below");
    assert_eq!(below.verdict, Verdict::Proven { k: 3, assumptions: vec!["small".into()] }, "{below:?}");
    assert!(below.text().ends_with("unter der Annahme `small`"), "{}", below.text());
    let small = reports.iter().find(|r| r.name == "small").expect("die Annahme steht im Bericht");
    assert!(small.assumption, "{small:?}");
    assert!(matches!(small.verdict, Verdict::Violated { .. }), "sie bewies sich selbst: {small:?}");
    assert!(small.text().contains("bleibt Annahme"), "{}", small.text());
    let p = assumed(false);
    let reports = prove(&encode(&p).expect("kodierbar"), &p, 3, &solver, 60).expect("Solver laeuft");
    let below = reports.iter().find(|r| r.name == "below").expect("below");
    assert!(matches!(below.verdict, Verdict::Violated { .. }), "{below:?}");
}

/// INT-020: Ein Pfad zu einer impliziten Pruefung gilt erst als
/// bestaetigt, wenn der Interpreter an *dieser* Stelle faultet — nicht an
/// irgendeiner Stelle derselben Art in derselben Maschine.
///
/// Eine abweichende Kodierung steht hier als Modell eines anderen
/// Programms mit denselben Stellen: Darin faultet `a` im zweiten Tick, im
/// Lauf nie. `c` faultet in beiden nach sechs Ticks; dieser Fault darf den
/// Pfad von `a` nicht bestaetigen.
#[test]
fn a_path_is_confirmed_only_by_its_own_site() {
    let Some(solver) = solver() else { return };
    let src = |cond: &str| {
        format!(
            "system:
    language = 1
    tick     = 10 ms

output y : int @ hw(\"o/y\") with safe = 0

machine m:
    var a : int in 0..14 = 0
    var c : int in 0..5 = 0
    initial RUN
    state RUN:
        loop:
            a = c + 14 if {cond} else 0
            c = c + 1
            y = a + c
"
        )
    };
    let (modelled, run) = (src("c < 9"), src("c > 9"));
    let model = encode(&compile(&modelled)).expect("kodierbar");
    let sites = classify(&model, &compile(&run), 8, &solver, 60).expect("Solver laeuft");
    let line = run.find("a = c + 14").expect("Zeile von a");
    let next = line + run[line..].find('\n').expect("Zeilenende");
    let a = sites
        .iter()
        .find(|s| s.kind == "range" && (line..next).contains(&(s.start as usize)))
        .unwrap_or_else(|| panic!("keine Range-Stelle an `a`: {sites:?}"));
    assert!(
        matches!(&a.verdict, CheckVerdict::Undecided { reason } if reason.contains("bestaetigt ihn nicht")),
        "der Fault von `c` bestaetigte `a`: {a:?}"
    );
}

/// Ein Urteil nennt nur die Annahmen, von denen es abhaengen kann: die im
/// Kegel seines Ziels. `calm` beschraenkt einen Input, den `below` nicht
/// liest.
#[test]
fn a_proof_names_only_the_assumptions_it_rests_on() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 10 ms

input x : int in 0..100 @ hw(\"i/x\")
input z : int in 0..100 @ hw(\"i/z\")
output y : int @ hw(\"o/y\") with safe = 0
output w : int @ hw(\"o/w\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            y = x.or(0)
            w = z.or(0)

assumption small: always(x < 50)
assumption calm: always(z < 5)
property below: always(y < 50)
property quiet: always(w <= 100)
",
    );
    let reports = prove(&encode(&p).expect("kodierbar"), &p, 2, &solver, 60).expect("Solver laeuft");
    let verdict = |name: &str| reports.iter().find(|r| r.name == name).expect("Bericht").verdict.clone();
    assert_eq!(verdict("below"), Verdict::Proven { k: 2, assumptions: vec!["small".into()] });
    assert_eq!(verdict("quiet"), Verdict::Proven { k: 2, assumptions: vec!["calm".into()] });
}

/// Ein Solver, der auf jede Frage `word` antwortet: `unknown` oder
/// `timeout`, wie z3 und cvc5 es tun, wenn sie aufgeben.
fn answering(word: &str) -> Solver {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-prove-{word}"));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let path = if cfg!(windows) {
        let p = dir.join("solver.cmd");
        std::fs::write(&p, format!("@echo off\r\necho {word}\r\n")).expect("Skript");
        p
    } else {
        let p = dir.join("solver.sh");
        std::fs::write(&p, format!("#!/bin/sh\necho {word}\n")).expect("Skript");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("ausfuehrbar");
        }
        p
    };
    Solver::At(path)
}

/// INT-020: Was der Solver nicht entscheidet, bleibt offen — eine
/// Pruefstelle unentschieden, eine Eigenschaft unbewiesen, und der Grund
/// nennt sein Wort.
#[test]
fn an_undecided_answer_leaves_sites_and_properties_open() {
    let p = corpus_with("01_minimal.takt", "property never_vents: never(vent)");
    let model = encode(&p).expect("kodierbar");
    for word in ["unknown", "timeout"] {
        let solver = answering(word);
        assert!(solver.works(), "das Skript antwortet auf `--version`");
        let sites = classify(&model, &p, 2, &solver, 5).expect("das Skript laeuft");
        assert!(
            matches!(&sites[0].verdict, CheckVerdict::Undecided { reason } if reason.contains(word)),
            "{word}: {:?}",
            sites[0]
        );
        let reports = prove(&model, &p, 2, &solver, 5).expect("das Skript laeuft");
        assert!(
            matches!(&reports[0].verdict, Verdict::Unproven { reason } if reason.contains(word)),
            "{word}: {:?}",
            reports[0]
        );
    }
}

/// Zwei Indexpruefungen am selben Anfang: `g[i][j]` prueft `j` am
/// aeusseren und `i` am inneren Zugriff, und beide beginnen bei `g`.
const SHARED_START: &str = "system:
    language = 1
    tick     = 10 ms

input i : int in 0..5 @ hw(\"i/i\")
input j : int in 0..3 @ hw(\"i/j\")
output y : int @ hw(\"o/y\") with safe = 0

machine m:
    var g : [3] [3] int = default
    initial RUN
    state RUN:
        loop:
            y = g[i][j]
";

/// Wie viele Indexpruefungen die MIR noch traegt.
fn index_nodes(p: &Program) -> usize {
    let mut n = 0;
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut |e| {
            n += usize::from(matches!(
                e.kind,
                takt_mir::expr::ExprKind::Checked { kind: takt_mir::expr::CheckedKind::Index { .. }, .. }
            ));
        });
    }
    n
}

/// **FB-393, SYN-025.** Der Schluessel einer Stelle ist Anfang, Ende und
/// Art. `g[i][j]` traegt zwei Indexpruefungen mit demselben Anfang, die
/// innere ueber `g[i]` und die aeussere ueber das Ganze: zwei Stellen im
/// Bericht (`takt check --checks`), und eine Beweisdatei, die nur die
/// aeussere nennt, laesst nur sie aus. Mit dem Anfang allein liess ein
/// Beweis fuer die eine beide fallen.
#[test]
fn a_site_is_its_start_its_end_and_its_kind() {
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let before = takt_sema::compile(SHARED_START, &options);
    let p = before.program.as_ref().expect("Programm");
    assert_eq!(index_nodes(p), 2, "zwei Indexpruefungen in der MIR");
    assert_eq!(
        before.report.checks.get("Index").copied(),
        Some(2),
        "zwei Stellen im Bericht: {:?}",
        before.report.checks
    );
    let start = SHARED_START.find("g[i][j]").expect("Stelle") as u32;
    let end = start + "g[i][j]".len() as u32;
    let hash = takt_mir::review::hash_of(SHARED_START.as_bytes());
    let proof =
        parse(&render(&hash, "z3 4.13.4", &[Site { start, end, kind: "index".into(), k: 1 }])).expect("Beweisdatei");
    let after = takt_sema::compile_with(SHARED_START, &options, Some(&proof));
    assert!(!after.has_errors(), "{:?}", after.diagnostics);
    assert_eq!(
        index_nodes(after.program.as_ref().expect("Programm")),
        1,
        "nur die aeussere Pruefung ueber {start}..{end} faellt weg"
    );
}

/// Ein Strom im Modell (M11 Schritt 27c): Das Gegenbeispiel liefert ein
/// Element, auf das der Handler passt, und der Interpreter bestaetigt es;
/// was der Eintritt zusichert, ist bewiesen.
#[test]
fn a_stream_element_is_part_of_the_counterexample() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 10 ms

record Frame:
    id : int in 0..9

input rx : stream<Frame> @ hw(\"bus/rx\") with max_rate = 200 Hz, capacity = 4

output alarm : bool @ hw(\"o/alarm\") with safe = false

machine watch:
    initial QUIET

    state QUIET:
        enter:
            alarm = false
        on rx matches Frame(id = 7) as e:
            -> LOUD

    state LOUD:
        enter:
            alarm = true
        after 20 ms: -> QUIET

property never_loud: never(alarm)
property quiet_is_silent: always(watch.state == QUIET implies not alarm)
",
    );
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 3, &solver, 60).expect("Solver laeuft");
    let loud = reports.iter().find(|r| r.name == "never_loud").expect("never_loud");
    let Verdict::Violated { stimulus, .. } = &loud.verdict else { panic!("{loud:?}") };
    assert!(stimulus.contains("in rx Frame(7) t="), "{stimulus}");
    let quiet = reports.iter().find(|r| r.name == "quiet_is_silent").expect("quiet_is_silent");
    assert!(matches!(quiet.verdict, Verdict::Proven { .. }), "{quiet:?}");
}

/// Text im Gegenbeispiel (M11 Schritt 27c-2): Eine Zeile, auf die das
/// Muster mit seinem Guard passt, und eine, die der Rand kuerzt; beide
/// bestaetigt der Interpreter.
#[test]
fn a_text_line_is_part_of_the_counterexample() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 10 ms

input rx : stream<line<8>> @ hw(\"u/rx\") with max_rate = 100 Hz, capacity = 1

output level : int in 0..2000 @ hw(\"o/level\") with safe = 0

machine m:
    initial RUN

    state RUN:
        on rx matches \"set {n:int}\" as e when e.n >= 0 and e.n < 1000:
            level = e.n
        on rx as e when e.text.truncated:
            level = 2000

property never_high: never(level > 500 and level < 1000)
property never_cut: never(level == 2000)
",
    );
    let model = encode(&p).expect("kodierbar");
    let reports = prove(&model, &p, 1, &solver, 120).expect("Solver laeuft");
    let high = reports.iter().find(|r| r.name == "never_high").expect("never_high");
    let Verdict::Violated { stimulus, .. } = &high.verdict else { panic!("{high:?}") };
    assert!(stimulus.contains("in rx \"set "), "{stimulus}");
    let cut = reports.iter().find(|r| r.name == "never_cut").expect("never_cut");
    assert!(matches!(cut.verdict, Verdict::Violated { .. }), "{cut:?}");
}

/// Funktionen aus `libtaktm` (4.2) sieht der Solver uninterpretiert, mit den
/// Schranken ihres Wertebereichs: Was die Schranke traegt, ist bewiesen; ein
/// Pfad, der einen anderen Wert braucht, als die Funktion annimmt, bleibt
/// offen und sagt warum.
#[test]
fn a_bounded_function_proves_its_range_and_keeps_its_paths_open() {
    let Some(solver) = solver() else { return };
    let p = compile(
        "system:
    language = 1
    tick     = 10 ms

input  x : float in -10.0..10.0 @ hw(\"i/x\")
output y : float @ hw(\"o/y\") with safe = 0.0

machine m:
    initial RUN

    state RUN:
        loop:
            y = sin(x)

property bounded: always(y <= 1.0 and y >= -1.0)
property small: always(x != 0.0 or y < 0.5)
",
    );
    let model = encode(&p).expect("kodierbar");
    assert_eq!(model.uninterpreted, vec!["sin".to_string()]);
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let bounded = reports.iter().find(|r| r.name == "bounded").expect("bounded");
    assert!(matches!(bounded.verdict, Verdict::Proven { .. }), "{bounded:?}");
    let small = reports.iter().find(|r| r.name == "small").expect("small");
    let Verdict::Unproven { reason } = &small.verdict else { panic!("{small:?}") };
    assert!(reason.contains("`sin` uninterpretiert"), "{reason}");
}

/// Eine Native der kuratierten Menge (4.5) sieht der Solver wie eine
/// Funktion aus `libtaktm`: Was ihre Breite traegt, ist bewiesen; ein Pfad
/// ueber einen Wert, den die Pruefsumme nie annimmt, bleibt offen.
#[test]
fn a_native_proves_its_width_and_keeps_its_paths_open() {
    let Some(solver) = solver() else { return };
    let p = corpus_with(
        "20_native.takt",
        "property width: always(pruefsumme <= 4294967295)\nproperty magic: never(pruefsumme == 7)",
    );
    let model = encode(&p).expect("kodierbar");
    assert_eq!(model.uninterpreted, vec!["crc32".to_string()]);
    let reports = prove(&model, &p, 2, &solver, 60).expect("Solver laeuft");
    let width = reports.iter().find(|r| r.name == "width").expect("width");
    assert!(matches!(width.verdict, Verdict::Proven { .. }), "{width:?}");
    let magic = reports.iter().find(|r| r.name == "magic").expect("magic");
    let Verdict::Unproven { reason } = &magic.verdict else { panic!("{magic:?}") };
    assert!(reason.contains("`crc32` uninterpretiert"), "{reason}");
}
