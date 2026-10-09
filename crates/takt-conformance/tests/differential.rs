//! Die Abnahme von M4: Interpreter ≡ nativ (13.8, Satz 9.4.4).
//!
//! Jedes Korpusprogramm wird zweimal ausgefuehrt — einmal vom
//! Referenzinterpreter, einmal als uebersetztes Binaerprogramm — und die
//! Outputs muessen Zeichen fuer Zeichen gleich sein.
//!
//! **Die Tests ueberspringen sich ohne clang**, wie die uebrigen
//! LLVM-Tests: Der Compiler baut ueberall, die Abnahme laeuft dort, wo
//! die Werkzeugkette steht.

use takt_conformance::compare;
use takt_conformance::stimulus::Stimulus;
use takt_mir::program::Program;

mod common;

/// Die Programme der Abnahme: die Suite `vergleich` aus dem Manifest
/// (FB-378) und die Beispiele.
fn korpus() -> Vec<&'static str> {
    let mut out = takt_conformance::suites::programs("vergleich");
    out.extend(takt_conformance::suites::EXAMPLES);
    out
}

/// Wie viele Ticks die Einzeltests vergleichen: genug fuer ihre Fristen,
/// wenig genug, dass ein Fehlschlag noch zu lesen ist.
const TICKS: u64 = 60;

/// Die Obergrenze der Tickzahl eines Korpusprogramms (KON1-006): Fristen
/// darueber (`after 3 s` bei 50 us Tick, `after 7 d`) erreicht der
/// Vergleich nicht, und `UNFIRED` nennt ihre Transitionen.
const MAX_TICKS: u64 = 2000;

/// Wie viele Ticks ein Korpusprogramm laeuft (KON1-006): die Summe seiner
/// statischen Fristen (`after`, `timeout`, `within`, `every`, Dauern in
/// Ausdruecken) in Ticks und ein Rand, mindestens 60 und hoechstens
/// [`MAX_TICKS`]. Die Summe statt der laengsten Frist, weil Fristen
/// hintereinander liegen: `after 200 ms`, dann `until … timeout 500 ms`
/// erreicht den Timeout-Pfad erst nach 700 ms.
fn ticks_for(p: &Program) -> u64 {
    let mut sum = 0i64;
    for m in &p.machines {
        takt_mir::visit::for_each_expr_machine(m, &mut |e| {
            if let takt_mir::expr::ExprKind::Duration(d) = &e.kind {
                sum = sum.saturating_add((*d).max(0));
            }
        });
    }
    let ticks = u64::try_from(sum / p.config.tick.max(1)).unwrap_or(u64::MAX);
    ticks.saturating_add(10).clamp(60, MAX_TICKS)
}

/// Transitionen eines Korpusprogramms, die im Lauf ohne Eingaben nie feuern
/// (KON1-006, Coverage aus 13.2): Programm und Zahl. Die Ratsche verlangt die
/// Zahl genau; feuert eine weitere nicht mehr, ist das ein Befund, feuert
/// eine mehr, wird die Zahl gesenkt. Der Grund ist fast immer derselbe: Ohne
/// Stimulus fehlen die Commands und Lieferungen, die sie ausloesen (FB-376);
/// bei `11_foc_drive` (`after 3 s` bei 50 us) und `sim/14_7` (`after 2 min`,
/// `after 7 d`) liegen Fristen jenseits von [`MAX_TICKS`].
const UNFIRED: &[(&str, usize)] = &[
    ("01_minimal.takt", 3),
    ("03_sequences_and_faults.takt", 12),
    ("11_foc_drive.takt", 6),
    ("12_bitfields.takt", 1),
    ("14_latency.takt", 1),
    ("16_timing.takt", 2),
    ("17_nested.takt", 3),
    ("18_blocks.takt", 2),
    ("21_fault_targets.takt", 1),
    ("40_jobs.takt", 3),
    ("43_sent.takt", 2),
    ("45_journal_cut.takt", 14),
    ("47_monitors.takt", 2),
    ("49_record_streams.takt", 2),
    ("50_clause_words.takt", 2),
    ("53_stream_kinds.takt", 1),
    ("60_resume.takt", 2),
    ("62_type_generics.takt", 2),
    ("63_scoped_instances.takt", 8),
    ("64_scoped_exit.takt", 1),
    ("87_fault_kinds.takt", 3),
    ("88_capture_segments.takt", 3),
    ("92_idle_streams.takt", 1),
    ("04_blocks_and_multirate.takt", 3),
    ("05_streams_and_protocol.takt", 6),
    ("06_test_harness.takt", 5),
    ("07_embedded_field.takt", 17),
    ("30_idle.takt", 1),
    ("31_idle_multirate.takt", 1),
    ("38_scenarios.takt", 2),
    ("61_requirements.takt", 4),
    ("65_trigger.takt", 2),
    ("116_sequence_timeout.takt", 2),
    // Der Timeout ist der Zweck: `until go` wird nie wahr.
    ("119_timeout_cancels_schedule.takt", 1),
    // Schnitt, Timeouts und Flash-Fehler nur mit `CUT > 0`, also in der Kampagne.
    ("110_journal_log.takt", 17),
    ("sim/12_7/program.takt", 4),
    ("sim/14_7/program.takt", 10),
];

fn corpus(name: &str) -> Program {
    let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{name}:\n{}", errors.join("\n"));
    out.program.unwrap_or_else(|| panic!("{name}: kein Programm"))
}

/// Fuehrt dasselbe Programm im Interpreter aus.
fn run_interpreted(p: &Program) -> String {
    interpret(p, TICKS).trace.render()
}

/// Der Lauf des Interpreters ueber `ticks` Ticks ohne Eingaben.
fn interpret(p: &Program, ticks: u64) -> takt_interp::RunResult {
    let options = takt_interp::RunOptions { ticks, profile: None, order_seed: None, ..Default::default() };
    takt_interp::run(p, &takt_interp::Trace::default(), &options).unwrap_or_else(|e| panic!("Interpreter: {e:?}"))
}

/// **Der virtuelle Schlaf ist unsichtbar** (Satz 9.9.1): Jedes
/// Korpusprogramm mit einem `idle`-Zustand liefert schlafend denselben
/// Trace wie der Interpreter, der nie schlaeft — und der Rahmen hat
/// dabei tatsaechlich geschlafen, jedes Programm fuer sich (KON1-036).
/// Auf dem Wirt, damit `_advance` (9.9) nicht erst auf dem Board geprueft
/// wird (FB-268, FB-273).
#[test]
fn virtual_sleep_is_invisible() {
    let Some(clang) = common::clang() else { return };
    let mut failed = Vec::new();
    let mut sleepy_programs = 0;
    for name in korpus() {
        let p = corpus(name);
        if !p.machines.iter().any(|m| m.states.iter().any(|s| s.idle)) {
            continue;
        }
        let ticks = ticks_for(&p);
        let native = match common::run_native_sleeping(&clang, &p, name, ticks) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        // 9.9: Geschlafen wird nur, wenn alle Maschinen `idle` sind; eine
        // ohne solchen Zustand (ein Modell, ein Zubringer) haelt das System
        // wach, und `_advance` laeuft dort zu Recht nie.
        let sleepy = p.machines.iter().all(|m| m.states.iter().any(|s| s.idle));
        sleepy_programs += usize::from(sleepy);
        if sleepy && !native.lines().any(|l| l.contains(" slept=")) {
            failed.push(format!("{name}: jede Maschine kann schlafen, und der Rahmen schlief nie"));
        }
        let interpreted = interpret(&p, ticks).trace.render();
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &native);
        if !diffs.is_empty() {
            failed.push(format!(
                "{name}: {} Abweichungen mit Schlaf, etwa {:?}",
                diffs.len(),
                &diffs[..diffs.len().min(4)]
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    assert!(sleepy_programs > 0, "kein Programm, in dem jede Maschine schlafen kann");
}

/// **Zwei Rahmen desselben Programms bauen nebeneinander** (KON1-007):
/// `virtual_sleep_is_invisible` und die Abnahme laufen im selben Testbinary
/// parallel ueber dieselben Programme, der eine schlafend, der andere nicht.
/// Jeder Lauf baut in seinem eigenen Verzeichnis; teilten sie eines, raeumte
/// der eine dem anderen die Dateien weg oder ueberschriebe sein Binary.
#[test]
fn two_frames_of_the_same_program_build_side_by_side() {
    let Some(clang) = common::clang() else { return };
    let name = "56_idle_timer.takt";
    let p = corpus(name);
    let start = std::sync::Barrier::new(4);
    let traces: Vec<(bool, Result<String, String>)> = std::thread::scope(|s| {
        let runs: Vec<_> = [true, false, true, false]
            .into_iter()
            .map(|sleeping| {
                let (clang, p, start) = (&clang, &p, &start);
                s.spawn(move || {
                    start.wait();
                    let trace = if sleeping {
                        common::run_native_sleeping(clang, p, name, TICKS)
                    } else {
                        common::run_native_all(clang, p, name, TICKS)
                    };
                    (sleeping, trace)
                })
            })
            .collect();
        runs.into_iter().map(|r| r.join().expect("Faden")).collect()
    });
    let widened = takt_conformance::run::widen_f32(&run_interpreted(&p), &takt_conformance::run::f32_outputs(&p));
    for (sleeping, trace) in traces {
        let trace = trace.unwrap_or_else(|e| panic!("schlafend {sleeping}: kein Lauf:\n{e}"));
        assert_eq!(trace.contains(" slept="), sleeping, "der Lauf nahm den Rahmen des anderen:\n{trace}");
        let diffs = compare(&widened, &trace);
        assert!(diffs.is_empty(), "schlafend {sleeping}: {diffs:?}");
    }
}

/// **Ein voller Wake-Strom haelt auch den erzeugten Code wach** (9.9
/// Konjunkt 3, FB-334): Die Maschine laeuft alle 5 ms und schlaeft bis zu
/// ihrer Frist in Tick 20. Die Glocke laeutet in Tick 3; der Rahmen
/// schlaeft bis davor, bleibt dann wach, solange das Element im Fenster
/// steht, und die Maschine sieht es in Tick 5 wie im Interpreter. Schliefe
/// er trotz vollem Fenster, saehe sie es erst nach der Frist.
#[test]
fn a_full_wake_window_keeps_the_native_system_awake() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick = 1 ms\n\n\
               input  bell : stream<u8> @ hw(\"bus/bell\") with capacity = 8, max_rate = 200 Hz, wake = true\n\
               output led  : bool @ hw(\"ui/led\") with safe = false\n\n\
               machine m every 5 ms:\n    initial WAIT\n\n    state WAIT idle:\n        when bell as e: -> RUN\n\
               \x20       after 20 ms: -> RUN\n\n    state RUN:\n        enter:\n            led = true\n\
               \x20       after 1 ms: -> WAIT\n";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let stimulus = takt_interp::Trace::parse("t=3 in bell 7\n").expect("Stimulus");
    let native = common::run_native_sleeping_with(
        &clang,
        &p,
        "wake_window",
        30,
        &Stimulus::from_trace(&stimulus).expect("Stimulus"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 30, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
    assert!(interpreted.contains("t=5 out led true"), "{interpreted}");
    assert!(native.contains("t=1 time took=0 drift=0 slept=1"), "bis vor die Lieferung geschlafen:\n{native}");
    for tick in [3, 4] {
        assert!(
            !native.contains(&format!("t={tick} time took=0 drift=0 slept=")),
            "wach mit vollem Fenster:\n{native}"
        );
    }
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **Ein laufender Job haelt auch den erzeugten Code wach** (9.9 Konjunkt
/// 5, 4.5): Der Job startet in Tick 0 und laeuft weiter, als die Maschine
/// in den `idle`-Zustand wechselt; fertig ist er in Tick 3. Sein Handle ist
/// dort nicht mehr sichtbar (SC-22 verbietet eine Sequenz im `idle`), also
/// zeigt sich das Konjunkt nur am Schlaf: keiner bis Tick 3, danach bis
/// zur Frist.
#[test]
fn a_running_job_keeps_the_native_system_awake() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick = 1 ms\n\n\
               native job sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 640, duration = 3 ms, total\n\n\
               output ready : bool @ hw(\"o/ready\") with safe = false\n\n\
               machine m:\n    var msg : bytes<64> = default\n    initial START\n\n\
               \x20   state START:\n        enter:\n            job v = sha256(msg)\n        when true: -> REST\n\n\
               \x20   state REST idle:\n        after 20 ms: -> DONE\n\n\
               \x20   state DONE:\n        enter:\n            ready = true\n";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let native = common::run_native_sleeping_with(&clang, &p, "running_job", 30, &[]).unwrap_or_else(|e| panic!("{e}"));
    for tick in [1, 2] {
        assert!(!native.contains(&format!("t={tick} time took=0 drift=0 slept=")), "wach mit laufendem Job:\n{native}");
    }
    assert!(native.contains("t=3 time took=0 drift=0 slept="), "nach dem Job geschlafen:\n{native}");
    let options = takt_interp::RunOptions { ticks: 30, ..Default::default() };
    let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **Die Abnahme.** Interpreter und erzeugter Code liefern dieselben
/// Outputs (Satz 9.4.4), jedes Programm so lange, wie seine Fristen
/// verlangen ([`ticks_for`]); jede Transition feuert dabei, ausser denen,
/// die [`UNFIRED`] zaehlt.
#[test]
fn the_interpreter_and_the_generated_code_agree() {
    let Some(clang) = common::clang() else { return };
    let mut failed = Vec::new();
    for name in korpus() {
        let p = corpus(name);
        let ticks = ticks_for(&p);
        // Alle Maschinen, in Schrittordnung (7.2): Ψ-Lesevorgaenge und
        // `follows` gibt es nur zwischen Maschinen.
        let native = match common::run_native_all(&clang, &p, name, ticks) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        let result = interpret(&p, ticks);
        let items = takt_interp::coverage::items(&p);
        let unfired: Vec<String> = result
            .coverage
            .missing(&items)
            .into_iter()
            .filter(|i| i.kind == takt_interp::CoverKind::Transition)
            .map(|i| format!("{} {}", i.machine, i.key))
            .collect();
        let allowed = UNFIRED.iter().find(|(n, _)| *n == name).map_or(0, |(_, k)| *k);
        if unfired.len() != allowed {
            failed.push(format!(
                "{name}: {} Transitionen feuern in {ticks} Ticks nie, `UNFIRED` erwartet {allowed}:\n  {}",
                unfired.len(),
                unfired.join("\n  ")
            ));
        }
        let interpreted = result.trace.render();
        // Der Vergleich prueft nur Ausgaenge, die beide Seiten melden; einer,
        // den der Rahmen nie schreibt, fiele sonst durch (FB-305).
        let missing: Vec<String> = common::board::output_names(&interpreted)
            .difference(&common::board::output_names(&native))
            .cloned()
            .collect();
        // Ein `f32` schreibt der Rahmen als seinen Wert in `f64` (4.2).
        let widened = takt_conformance::run::widen_f32(&interpreted, &takt_conformance::run::f32_outputs(&p));
        let diffs = compare(&widened, &native);
        if !missing.is_empty() || !diffs.is_empty() {
            let list: Vec<String> = diffs.iter().take(8).map(|d| format!("  {d}")).collect();
            failed.push(format!(
                "{name}: {} Abweichungen, fehlende Ausgaenge {missing:?}\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
                diffs.len(),
                list.join("\n"),
                interpreted.lines().take(12).collect::<Vec<_>>().join("\n"),
                native.lines().take(12).collect::<Vec<_>>().join("\n")
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Die Grenzen der Abnahme stehen im Code, nicht nur im Plan.
///
/// Der Test gibt sie aus, damit ein gruener Lauf sie mitliefert — und er
/// prueft, dass die Liste nicht leer laeuft. Eine Grenze ohne Termin
/// waere eine Ausrede; jede traegt einen.
#[test]
fn the_limits_of_the_acceptance_are_written_down() {
    let limits = takt_conformance::LIMITS;
    assert!(limits.len() >= 5, "die Liste ist zu kurz, um vollstaendig zu sein: {}", limits.len());
    for l in limits {
        assert!(!l.was.is_empty() && !l.warum.is_empty(), "eine Grenze ohne Begruendung");
        assert!(!l.wann.is_empty(), "`{}` hat keinen Termin", l.was);
    }
    eprintln!("{}", takt_conformance::limits::report());
}

/// **Die Abnahme mit Eingaben** (12.5): Beide Seiten sehen denselben
/// Stimulus, und ihre Outputs stimmen ueberein.
///
/// Das ist die Haelfte, die bis Schritt 10 fehlte: Ein Lauf ohne
/// Eingaben prueft den Anfangszustand und seine Fortschreibung, nicht die
/// *Reaktion* auf Lieferungen. Ein Command ist die einfachste Form davon
/// (ein Puls, ein Byte, 8.5) — und die, die der Korpus benutzt.
#[test]
fn the_two_implementations_agree_on_recorded_inputs() {
    let Some(clang) = common::clang() else { return };
    // `16_timing` wartet auf `go` und faellt nach 200 ms zurueck; damit
    // laeuft jeder Uebergang mindestens einmal.
    let p = corpus("16_timing.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let stimulus = takt_interp::Trace::parse(
        "t=3 cmd go
t=40 cmd go
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Command { name } => Some(Stimulus::cmd(l.tick, name)),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 2, "der Stimulus traegt zwei Commands");

    let native =
        common::run_native_with(&clang, &p, "eingaben", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Der Stimulus wirkt (KON1-015): Jedes `go` schaltet ein, und nach
    // 200 ms faellt die Lampe zurueck; ohne Stimulus bliebe sie aus. Ein
    // Rahmen, der Commands nicht setzte, liefe sonst gruen neben einem
    // Interpreter, der sie ebenso uebersaehe.
    for line in ["t=3 out led true", "t=23 out led false", "t=40 out led true"] {
        assert!(
            interpreted.contains(line),
            "`{line}` fehlt:
{interpreted}"
        );
    }
    let quiet = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    assert!(
        !quiet.contains("out led true"),
        "ohne Stimulus schaltet nichts:
{quiet}"
    );

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Eingaben:
{}
--- Interpreter ---
{}
--- nativ ---
{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join(
            "
"
        ),
        interpreted,
        native
    );
}

/// **Die Abnahme mit Stromelementen** (8.6, 8.7): Ein Handler laeuft
/// nativ ueber ein Fenster mit Daten.
///
/// Bis hierher war das nicht geprueft, und zwar unbemerkt: `takt_stream_count`
/// lieferte null, also lief der Handler-Dispatch des erzeugten Codes
/// zwar, aber nie ueber ein Element (FB-115). Ein leeres Fenster ist
/// gruen wie ein richtiges Ergebnis.
///
/// `23_patterns` ist der Fall, der es zeigt: ein Handler mit `matches`
/// ueber dem Produkt-DFA (8.7, 11.2). Trifft das Muster, steht `y = 7` im
/// Trace; trifft es nicht, bleibt der Anfangswert. Beide Seiten muessen
/// dasselbe sagen — und der Unterschied zwischen „trifft" und „trifft
/// nicht" ist im Trace sichtbar, sonst prueft der Test nichts.
#[test]
fn the_two_implementations_agree_on_stream_elements() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("23_patterns.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    // Vier Elemente: eines trifft `"READY"`, eines gar kein Muster, und
    // zwei tragen einen Platzhalter mit verschiedenen Werten. Ohne den
    // zweiten Fall pruefte der Test nur, dass etwas ankommt; ohne die
    // letzten beiden nicht, dass die Extraktion den *Wert* trifft.
    let stimulus = takt_interp::Trace::parse(
        "t=2 in rx_log READY
t=5 in rx_log BUSY
t=8 in rx_log Erasing sector 42
t=11 in rx_log Erasing sector 7
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 4, "der Stimulus traegt vier Elemente");

    let native =
        common::run_native_with(&clang, &p, "stroeme", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Der Handler muss gelaufen sein: `y = 7` steht nur im Trace, wenn
    // das Muster getroffen hat. Ohne diese Zusicherung waere ein Lauf,
    // in dem beide Seiten nichts tun, ebenfalls gruen.
    assert!(
        interpreted.contains("out y 7"),
        "der Interpreter hat den Handler nicht ausgefuehrt; der Test pruefte sonst ein leeres Fenster:\n{interpreted}"
    );

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Stromelementen:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **`has` und `send` mit Daten** (8.7, 8.8): Das Muster darf an jeder
/// Stelle beginnen, und der Rumpf sendet.
///
/// Der Produkt-DFA beginnt fuer `has` an jeder Stelle einen neuen Faden
/// (8.7, 11.2); dieser Test misst, dass er dieselben Zeilen findet wie
/// der Interpreter.
#[test]
fn the_two_implementations_agree_on_has_and_send() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("24_send_has.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    // Vier Zeilen: `ERR` am Anfang, in der Mitte, am Ende, und gar nicht.
    // Ein Muster, das nur am Anfang traefe, kaeme auf eine andere Zahl.
    let stimulus = takt_interp::Trace::parse(
        "t=2 in rx ERR init failed
t=4 in rx warn: ERR on bus
t=6 in rx sensor ERR
t=8 in rx all good
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 4, "der Stimulus traegt vier Zeilen");

    let native =
        common::run_native_with(&clang, &p, "has_send", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Drei der vier Zeilen tragen `ERR`; stuende hier eine andere Zahl,
    // haette `has` nicht an jeder Stelle gesucht.
    assert!(
        interpreted.contains("out hits 3"),
        "`has` hat nicht drei Vorkommen gefunden; der Test pruefte sonst nichts:\n{interpreted}"
    );
    // Der Treiber holt das Gesendete ab und meldet es als `out tx
    // [bytes]` (8.8) — `compare` prueft die Zeile also gegen den
    // Interpreter. Die Zusicherung hier belegt, dass ueberhaupt gesendet
    // wurde: Ein Lauf ohne `send` waere sonst ebenfalls gruen.
    assert!(
        native.contains("out tx [0x61, 0x63, 0x6b, 0x20, 0x31, 0x0a]"),
        "`ack 1` fehlt im nativen Trace:\n{native}"
    );

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen bei `has`/`send`:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **Dispatch ueber den Produkt-DFA** (8.7, 11.2, FB-282): Ein Durchlauf
/// je Element entscheidet, welche der vier Textmuster treffen; der Guard,
/// der Wertebereich und die Reihenfolge entscheiden, welcher Handler
/// laeuft. Jede Zeile des Stimulus trifft einen anderen Fall.
#[test]
fn the_product_automaton_dispatches_like_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("100_dispatch.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    // 1: Guard haelt. 5: Guard faellt. 5: zwanzig Ziffern sind kein `int`.
    // 5: neunzehn Ziffern ueber `i64::MAX` — der Automat trifft, die
    // Extraktion nicht. 2: `has` an der fruehesten Stelle, und an einer
    // spaeteren, wenn die erste nicht traegt. 3: `has` vor `matches`.
    // 5: `{_}` endet am ersten `:`. 4: `{_}:{k:word}` trifft. 5: `has` mit
    // Ueberlauf an jeder Stelle.
    let stimulus = takt_interp::Trace::parse(
        "t=2 in rx code 42
t=4 in rx code 700
t=6 in rx code 99999999999999999999
t=8 in rx code 9999999999999999999
t=10 in rx x ERR 12 ERR 7
t=12 in rx ERR x ERR 5
t=14 in rx WARN: disk
t=16 in rx a:b:c
t=18 in rx id:abc
t=20 in rx ERR 9999999999999999999
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 10, "der Stimulus traegt zehn Zeilen");

    let native =
        common::run_native_with(&clang, &p, "dispatch", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Jeder Handler lief mindestens einmal; sonst pruefte der Vergleich
    // einen Fall weniger, als der Stimulus verspricht.
    for which in 1..=5 {
        assert!(interpreted.contains(&format!("out which {which}")), "Handler {which} lief nie:\n{interpreted}");
    }
    assert!(interpreted.contains("out value 5"), "`has` fand die spaetere Stelle nicht:\n{interpreted}");

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen im Dispatch:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(8).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// Die Formatangaben aus 3.9 im erzeugten Code: `{x}`, `{x:hex}`,
/// `{x:04}` (KON1-030).
///
/// Sie laufen ueber dieselbe Ziffernrechnung, unterscheiden sich aber in
/// Basis, Vorzeichen und Fuellung — und jede davon hat einen Rand: die
/// Null, das Minus vor der Zahl und vor der Fuellung, `i64::MIN` (dessen
/// Betrag kein `i64` ist), `i64::MAX` und die Fuellung, die kuerzer ist als
/// die Zahl. `hex` zaehlt das Bitmuster (3.9). Der Text je Tick steht hier,
/// sonst bestuende ein Lauf, in dem beide Seiten dasselbe Falsche schreiben;
/// `compare` haelt ihn zudem gegen den Interpreter.
#[test]
fn the_format_specs_produce_the_expected_text() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick     = 10 ms\n\n\
               output tx : stream<u8> @ hw(\"u/tx\") with max_rate = 100000 Hz, capacity = 256\n\n\
               machine m:\n    var k : int = 0\n    var i : int in 0..5 = 0\n    initial RUN\n\n\
               \x20   state RUN:\n        loop:\n\
               \x20           k = [0, -1, -9223372036854775807 - 1, 123456, -5, 9223372036854775807][i]\n\
               \x20           send tx, \"d={k} h={k:hex} p={k:04};\"\n\
               \x20           i = (i + 1) % 6\n";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let native = common::run_native_all(&clang, &p, "format", 6).unwrap_or_else(|e| panic!("{e}"));
    let interpreted = interpret(&p, 6).trace.render();
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
    // Der Treiber meldet die Bytes als `out tx [..]` (8.8).
    let text = |tick: u64| -> String {
        let head = format!("t={tick} out tx [");
        let line = native.lines().find(|l| l.starts_with(&head)).unwrap_or_else(|| panic!("t={tick}:\n{native}"));
        let bytes = line[head.len()..].trim_end_matches(']').split(", ");
        bytes.filter_map(|b| u8::from_str_radix(b.trim_start_matches("0x"), 16).ok()).map(char::from).collect()
    };
    for (tick, want) in [
        (0, "d=0 h=0 p=0000;"),
        (1, "d=-1 h=ffffffffffffffff p=-001;"),
        (2, "d=-9223372036854775808 h=8000000000000000 p=-9223372036854775808;"),
        (3, "d=123456 h=1e240 p=123456;"),
        (4, "d=-5 h=fffffffffffffffb p=-005;"),
        (5, "d=9223372036854775807 h=7fffffffffffffff p=9223372036854775807;"),
    ] {
        assert_eq!(text(tick), want, "t={tick}");
    }
}

/// Tunables (8.4): Der Parametervektor aendert sich an der Tick-Grenze —
/// beide Seiten lesen denselben Wert im selben Tick, und ein Wert
/// ausserhalb der Range bleibt auf beiden Seiten ohne Wirkung.
#[test]
fn a_tunable_changes_the_parameter_vector_at_its_tick() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("41_tunables.takt");
    let stimulus = takt_interp::Trace::parse(
        "t=3 tune GAIN 5
t=6 tune GAIN 200
t=8 tune GAIN 7
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Tune { name, value, .. } => {
                Some(Stimulus::Tune { tick: l.tick, name: name.clone(), text: value.clone() })
            }
            _ => None,
        })
        .collect();
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
    // Der Stimulus wirkt (KON1-015): `y = k * GAIN` mit `k` gleich Tick + 1
    // springt an jeder angenommenen Grenze auf den neuen Faktor, der Wert
    // 200 ausserhalb der Range bleibt ohne Wirkung. Ohne Stimulus rechnet
    // der Lauf durchgehend mit 2.
    for line in ["t=2 out y 6", "t=3 out y 20", "t=6 out y 35", "t=8 out y 63"] {
        assert!(
            interpreted.contains(&format!(
                "{line}
"
            )),
            "`{line}` fehlt:
{interpreted}"
        );
    }
    let quiet = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    assert!(
        quiet.contains(
            "t=8 out y 18
"
        ),
        "ohne Stimulus bleibt GAIN 2:
{quiet}"
    );
    let native = common::run_native_with(&clang, &p, "41_tunables.takt", "m", TICKS, &inputs)
        .unwrap_or_else(|e| panic!("41_tunables.takt: {e}"));
    let diffs = compare(&interpreted, &native);
    let list: Vec<String> = diffs.iter().map(|d| format!("  {d}")).collect();
    assert!(
        diffs.is_empty(),
        "{} Abweichungen:
{}",
        diffs.len(),
        list.join(
            "
"
        )
    );
}

/// Record-Elemente auf Stroemen (8.6, 8.7, plan/m6.md 2.14) und `peek`
/// (FB-15) nativ: Der Rahmen liefert `Pulse(...)` als kanonische
/// Byteform, Handler und Guard vergleichen die Felder, die Bindung
/// traegt `t`, `seq` und `data`/`text` — und ein `peek` laesst den
/// Handler desselben Ticks das Element noch sehen.
#[test]
fn the_two_implementations_agree_on_record_elements_and_peek() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("49_record_streams.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let stimulus = takt_interp::Trace::parse(
        "t=2 in edges Pulse(true, 3)
t=4 in edges Pulse(false, 5)
t=6 in edges Pulse(true, 9)
t=8 in edges Pulse(false, 7)
t=10 in rx go 42
t=12 in rx stop
t=14 in edges Pulse(true, 1)
",
    )
    .expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 7, "der Stimulus traegt sieben Elemente");

    let native =
        common::run_native_with(&clang, &p, "records", &machine, TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Jede Zusicherung belegt ein Konstrukt: das Record-Muster im Handler
    // (`rises`, `pin` aus `data`), der Guard mit zwei Feldern (`armed`),
    // `peek` (`peeked`), und `seq`/`t`/`text` der Textbindung.
    for line in ["out rises 3", "out pin 9", "out armed true", "out peeked true", "out late true", "out width 5"] {
        assert!(interpreted.contains(line), "`{line}` fehlt im Interpreter-Trace:\n{interpreted}");
    }

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Record-Elementen:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **Trigger auf beiden Seiten** (7.5, Satz 9.4.4).
///
/// Der Hauptlauf treibt keine Stroeme, ein Trigger feuerte dort also
/// nie — und ein Lauf, in dem beide Seiten nichts tun, waere gruen. Der
/// Test schickt darum ein passendes Element und prueft, dass die
/// geplante Ausgabe auf beiden Seiten zur selben Zeit steht.
#[test]
fn the_two_implementations_agree_on_triggers() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("65_trigger.takt");
    let stimulus = takt_interp::Trace::parse("t=2 in dut_log Erasing sector 7\n").expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 1);

    let native =
        common::run_native_all_with(&clang, &p, "65_trigger", TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Ohne diese Zusicherung pruefte der Test einen Trigger, der nie feuert.
    assert!(interpreted.contains("out vbus false"), "der Trigger hat nicht gefeuert:\n{interpreted}");

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Trigger:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **Eine Dauer in einem Record schreibt der Rahmen wie der Interpreter**
/// (T2): in ihrer groessten ganzzahligen Einheit, als Feld von
/// `Name(f1, f2)`. Den MCU-Rahmen betrifft das noch nicht, er schreibt
/// keine Record-Ausgaenge (FB-312).
#[test]
fn a_duration_in_a_record_is_written_alike() {
    let Some(clang) = common::clang() else { return };
    let src = "\
system:
    language = 1
    tick     = 10 ms

record Timing:
    period : Duration
    count  : int

output timing : Timing @ sim(\"o/timing\")

machine m:
    var k : int in 0..2 = 0

    initial RUN

    state RUN:
        loop:
            if k == 0:
                timing = Timing(period = 90 min, count = k)
            else:
                timing = Timing(period = 250 us, count = k)
            k = (k + 1) % 2
";
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let native = common::run_native_all(&clang, &p, "record_duration", 4).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 4, ..Default::default() };
    let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    assert!(native.contains("out timing Timing(90 min, 0)"), "{native}");
    assert!(native.contains("out timing Timing(250 us, 1)"), "{native}");
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **`o.jitter` ist in beiden Implementierungen null** (7.5): Der Wert
/// gehoert der Bindung, und ohne Hardware-Konfiguration schreiben beide
/// Seiten exakt; der Codegen fragt die Runtime (`takt_jitter`).
#[test]
fn the_jitter_of_an_output_is_zero_without_a_configuration() {
    let Some(clang) = common::clang() else { return };
    let src = "system:
    language = 1
    tick     = 1 ms

output strobe : bool     @ hw(\"gpio/strobe\") with safe = false
output spread : Duration @ hw(\"o/spread\") with safe = 0 s

machine m:
    initial RUN

    state RUN:
        loop:
            spread = strobe.jitter
            strobe = strobe.jitter < 5 us
";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(src, &options).program.expect("Programm");
    let native = common::run_native_all(&clang, &p, "jitter", 3).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: 3, ..Default::default() };
    let interpreted = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();
    assert!(interpreted.contains("t=0 out strobe true"), "{interpreted}");
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **Der QP-Loeser trifft die bekannte Loesung** (11.4, Satz 9.4.4).
///
/// `H = diag(2, 4)`, `g = (-2, -8)`: unbeschraenkt liegt das Minimum bei
/// `(1, 2)`. Mit `ub = (0.5, 1.5)` liegen beide Komponenten am Rand, und
/// die Projektion trifft ihn genau — nicht ungefaehr.
#[test]
fn qp_box_solves_a_two_by_two_problem() {
    let p = corpus("69_qp_box.takt");
    let options = takt_interp::RunOptions { ticks: 2, ..Default::default() };
    let trace = takt_interp::run(&p, &takt_interp::Trace::default(), &options).expect("Lauf").trace.render();

    for (channel, want) in [("x_tight", "0.5"), ("y_tight", "1.5"), ("both_ok", "true")] {
        let line = format!("out {channel} {want}");
        assert!(
            trace.contains(&line),
            "`{line}` fehlt:
{trace}"
        );
    }
    // Der unbeschraenkte Fall konvergiert gegen (1, 2); nach 200
    // Iterationen steht `y` exakt, `x` bis auf die Schrittweite.
    assert!(trace.contains("out y_free 2"), "{trace}");
    let x = trace
        .lines()
        .find_map(|l| l.strip_prefix("t=0 out x_free ").map(|v| v.parse::<f64>().expect("Zahl")))
        .expect("x_free");
    assert!((x - 1.0).abs() < 1e-6, "x_free = {x}");
}

/// **Capture-Fenster auf beiden Seiten** (8.9, Satz 9.4.4).
#[test]
fn the_two_implementations_agree_on_capture_windows() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("66_capture.takt");
    let stimulus =
        takt_interp::Trace::parse("t=2 in wave 20000000;2;2;1000.0;[1.0, 2.0, 0.5, 3.0]\n").expect("Stimulus");
    let inputs: Vec<Stimulus> = stimulus
        .lines
        .iter()
        .filter_map(|l| match &l.kind {
            takt_interp::trace::LineKind::Input { channel, sample } => {
                Some(Stimulus::element(l.tick, channel, sample.value.as_deref().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 1);

    let native =
        common::run_native_all_with(&clang, &p, "66_capture", TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();

    // Ohne diese Zusicherung pruefte der Test ein leeres Fenster.
    assert!(interpreted.contains("out dip 0.5 V"), "der Handler lief nicht:\n{interpreted}");

    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen mit Capture:\n{}\n--- Interpreter ---\n{}\n--- nativ ---\n{}",
        diffs.len(),
        diffs.iter().take(6).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n"),
        interpreted,
        native
    );
}

/// **Faults von aussen kommen auf beiden Wegen gleich an** (5.3, 5.4,
/// FB-332): ein Operator-Abort, ein Ueberlauf fuer alle Maschinen und ein
/// Treiberfehler fuer den Besitzer eines Outputs, als Stimulus — die
/// Zeilen, mit denen eine Aufzeichnung sie nachspielt (12.5). Korpus 90 hat
/// eine inaktive Maschine (`slow`), eine mit Latch und eine ohne Fault-Ziel;
/// ein Runtime-Fault ist nicht idempotent und eskaliert.
#[test]
fn faults_from_outside_arrive_alike() {
    let Some(clang) = common::clang() else { return };
    let p = corpus("90_abort.takt");
    let stimulus = takt_interp::Trace::parse(
        "t=12 abort
t=20 runtime Overrun
t=25 runtime Driver b_out
t=26 runtime Driver b_out
t=31 runtime Hardware
",
    )
    .expect("Stimulus");
    let inputs = Stimulus::from_trace(&stimulus).expect("Stimulus");
    assert_eq!(inputs.len(), 5, "Abort und vier Runtime-Faults");
    let native =
        common::run_native_all_with(&clang, &p, "von_aussen", TICKS, &inputs).unwrap_or_else(|e| panic!("{e}"));
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    let interpreted = takt_interp::run(&p, &stimulus, &options).expect("Lauf").trace.render();
    for kind in ["Abort", "Runtime(Overrun)", "Runtime(Driver)", "Runtime(Hardware)"] {
        assert!(interpreted.contains(kind), "`{kind}` fehlt im Interpreter:\n{interpreted}");
    }
    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        diffs.len(),
        diffs.iter().take(8).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n")
    );
}

/// Ein Byteblock als Takt-Funktion aus einer Hexfolge.
fn bytes_fn(name: &str, cap: usize, hex: &str) -> String {
    let items: Vec<String> =
        hex.as_bytes().chunks(2).map(|p| format!("0x{}", std::str::from_utf8(p).expect("ascii"))).collect();
    format!(
        "fn {name}() -> bytes<{cap}>:\n    var b : bytes<{cap}> = default\n    for x in [{}]:\n        b.push(x as u8)\n    return b\n\n",
        items.join(", ")
    )
}

/// Die Eingaben der ersten Zeile einer Funktion aus den Krypto-Bloecken.
fn crypto_line(fun: &str, nth: usize) -> Vec<String> {
    let spec = include_str!("../../../grammar/takt-native.md");
    let line = spec.lines().filter_map(|l| l.trim().strip_prefix(fun)?.split_once(':')).nth(nth).expect("Zeile");
    line.0.split_whitespace().map(str::to_string).collect()
}

/// **Die Natives aus M10 Schritt 21 im erzeugten Code** (4.5, Satz
/// 9.4.4): `fft256` als Funktion ueber `[256] float` (Feld als kanonischer
/// Puffer), `rsa3072_verify` als Job, `aes_gcm_decrypt` mit passendem und
/// mit gekipptem Tag (`Err(FAILED)`). Interpreter und Wirtsrahmen rufen
/// dieselbe Implementierung (FB-293); verglichen wird, was dazwischen liegt.
#[test]
fn the_new_natives_agree_with_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    let (rsa, good, bad) =
        (crypto_line("rsa3072_verify", 0), crypto_line("aes_gcm_decrypt", 1), crypto_line("aes_gcm_decrypt", 4));
    let src = format!(
        "system:\n    language = 1\n    tick = 1 ms\n
native fn fft256(x: [256] float) -> [256] float with cost = 8400, stack = 9000, total
native job rsa3072_verify(key: bytes<384>, digest: bytes<32>, sig: bytes<384>) -> bool with cost = 900, stack = 10464, duration = 5 ms, total
native job aes_gcm_decrypt(key: bytes<32>, nonce: bytes<12>, aad: bytes<16>, data: bytes<64>, tag: bytes<16>) -> bytes<64> with cost = 300, stack = 2752, duration = 3 ms, total

output spectrum : float      @ sim(\"o/spectrum\")
output verified : bool       @ sim(\"o/verified\")
output plain    : int in 0..64 @ sim(\"o/plain\")
output first    : u8         @ sim(\"o/first\")
output failed   : bool       @ sim(\"o/failed\")

{}{}{}{}{}{}{}{}{}
machine m:
    var x : [256] float = default
    var y : [256] float = default
    initial RUN
    state RUN:
        loop:
            for i in range(256):
                x[i] = ((i * 37) % 101) as float / 50.0 - 1.0
            y = fft256(x)
            spectrum = y[2] + y[7] * 0.5 - y[255]
        sequence:
            job r = rsa3072_verify(key = key(), digest = digest(), sig = sig())
            until r.done timeout 1 s -> STUCK
            verified = r.result.or(false)
            job a = aes_gcm_decrypt(key = k(), nonce = nonce(), aad = aad(), data = data(), tag = good_tag())
            until a.done timeout 1 s -> STUCK
            plain = a.result.or(default).len
            first = a.result.or(default)[0]
            job a = aes_gcm_decrypt(key = k(), nonce = nonce(), aad = aad(), data = data(), tag = bad_tag())
            until a.done timeout 1 s -> STUCK
            failed = a.result.err.or(PENDING) == FAILED
            -> DONE
    state DONE:
        when false: -> RUN
    state STUCK:
        when false: -> RUN
",
        bytes_fn("key", 384, &rsa[0]),
        bytes_fn("digest", 32, &rsa[1]),
        bytes_fn("sig", 384, &rsa[2]),
        bytes_fn("k", 32, &good[0]),
        bytes_fn("nonce", 12, &good[1]),
        bytes_fn("aad", 16, &good[2]),
        bytes_fn("data", 64, &good[3]),
        bytes_fn("good_tag", 16, &good[4]),
        bytes_fn("bad_tag", 16, &bad[4]),
    );
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Sim,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let p = out.program.expect("Programm");
    let interpreted = run_interpreted(&p);
    for want in ["out verified true", "out plain 57", "out failed true"] {
        assert!(interpreted.contains(want), "`{want}` fehlt im Interpreter:\n{interpreted}");
    }
    let native = common::run_native_all(&clang, &p, "natives_21", TICKS).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interpreted, &native);
    assert!(
        diffs.is_empty(),
        "{} Abweichungen:\n{}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}",
        diffs.len(),
        diffs.iter().take(8).map(|d| format!("  {d}")).collect::<Vec<_>>().join("\n")
    );
}

/// **Eine Signaturpruefung mit Argumenten falscher Laenge ist `false`**
/// (4.5, FB-487): Der Schluessel aus `default` ist leer. Vorher endete der
/// Interpreter mit einem Bug und der Wirtsrahmen mit `Err(FAILED)`.
#[test]
fn a_signature_check_with_a_short_key_is_false() {
    let Some(clang) = common::clang() else { return };
    let src = "system:\n    language = 1\n    tick = 1 ms\n\n\
               native job ecdsa_p256_verify(key: bytes<64>, digest: bytes<32>, sig: bytes<64>) -> bool \
               with cost = 300, stack = 5600, duration = 3 ms, total\n\n\
               output verified : bool @ sim(\"o/verified\")\n\
               output failed   : bool @ sim(\"o/failed\")\n\n\
               machine m:\n    var key    : bytes<64> = default\n    var digest : bytes<32> = default\n\
               \x20   var sig    : bytes<64> = default\n    initial RUN\n\n    state RUN:\n        sequence:\n\
               \x20           job e = ecdsa_p256_verify(key = key, digest = digest, sig = sig)\n\
               \x20           until e.done timeout 1 s -> STUCK\n\
               \x20           verified = e.result.or(true)\n\
               \x20           failed = e.result.err.or(PENDING) == FAILED\n\
               \x20           -> DONE\n\
               \x20   state DONE:\n        when false: -> RUN\n\
               \x20   state STUCK:\n        when false: -> RUN\n";
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let p = out.program.unwrap_or_else(|| panic!("{:?}", out.diagnostics));
    let interpreted = run_interpreted(&p);
    for want in ["out verified false", "out failed false"] {
        assert!(interpreted.contains(want), "`{want}` fehlt im Interpreter:\n{interpreted}");
    }
    let native = common::run_native_all(&clang, &p, "short_key", TICKS).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- Interpreter ---\n{interpreted}\n--- nativ ---\n{native}");
}

/// **Ueber 64 Text-Handler: ohne Produkt-DFA dasselbe Urteil** (8.7, 11.2;
/// SYN-035): `117_many_text_handlers` hat 66 Muster in einem Zustand, der
/// Codegen prueft jedes mit eigenem Durchlauf. Der erste passende Handler
/// nimmt das Element, nativ wie im Interpreter: Tick 1 bis 64 die Handler 0
/// bis 63, dann `w5x` das allgemeine `w{_}` (65) statt `w5`, `n65` den
/// Platzhalter (64, Wert 65), `w99` wieder 65 und `zz` den Catch-all (66,
/// ein Fehlgriff).
#[test]
fn more_than_64_text_handlers_judge_like_the_interpreter() {
    let Some(clang) = common::clang() else { return };
    let name = "117_many_text_handlers.takt";
    let p = corpus(name);
    let interpreted = interpret(&p, 70).trace.render();
    let at = |tick: u64, output: &str| -> Option<String> {
        let head = format!("t={tick} out {output} ");
        interpreted.lines().find_map(|l| l.strip_prefix(&head).map(str::to_string))
    };
    for tick in 1..=64u64 {
        assert_eq!(at(tick, "hit"), Some((tick - 1).to_string()), "Tick {tick}:\n{interpreted}");
    }
    for (tick, output, want) in [
        (65, "hit", "65"),
        (66, "hit", "64"),
        (66, "value", "65"),
        (67, "hit", "65"),
        (68, "hit", "66"),
        (68, "misses", "1"),
    ] {
        assert_eq!(at(tick, output).as_deref(), Some(want), "t={tick} {output}:\n{interpreted}");
    }
    let native = common::run_native_all(&clang, &p, name, 70).unwrap_or_else(|e| panic!("{e}"));
    let diffs = compare(&interpreted, &native);
    assert!(diffs.is_empty(), "{diffs:?}\n--- nativ ---\n{native}");
}
