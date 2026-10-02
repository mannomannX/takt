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
use takt_llvm::toolchain::{Clang, find};
use takt_mir::program::Program;

mod common;

/// Die Korpusprogramme, die der Codegen vollstaendig senkt.
const KORPUS: [&str; 90] = [
    "01_minimal.takt",
    "20_native.takt",
    "19_faults.takt",
    "02_units_and_data.takt",
    "03_sequences_and_faults.takt",
    "12_bitfields.takt",
    "13_framing.takt",
    "13_protocol_analysis.takt",
    "14_latency.takt",
    "15_quality.takt",
    "16_timing.takt",
    "17_nested.takt",
    "18_blocks.takt",
    "21_fault_targets.takt",
    "22_faulted_outputs.takt",
    "23_patterns.takt",
    "24_send_has.takt",
    "25_format.takt",
    "26_samples.takt",
    "27_every.takt",
    "28_scheduled.takt",
    "32_next_run_after.takt",
    "33_enum_param.takt",
    "34_next_run_on_start.takt",
    "35_persist.takt",
    "36_int_units.takt",
    "37_follows.takt",
    "39_sha256.takt",
    "40_jobs.takt",
    "41_tunables.takt",
    "42_map.takt",
    "43_sent.takt",
    "46_matrices.takt",
    "47_monitors.takt",
    "49_record_streams.takt",
    "50_clause_words.takt",
    "51_text_into_bytes.takt",
    "52_padding_fields.takt",
    "53_stream_kinds.takt",
    "54_inout.takt",
    "55_frames_with_bytes.takt",
    "56_idle_timer.takt",
    "57_persist_often.takt",
    "58_persist_alert.takt",
    "59_persist_idle.takt",
    "60_resume.takt",
    "62_type_generics.takt",
    "63_scoped_instances.takt",
    "64_scoped_exit.takt",
    "68_uart_port.takt",
    "69_qp_box.takt",
    "70_padded_record.takt",
    "71_places.takt",
    "72_handler_levels.takt",
    "73_after_levels.takt",
    "74_instance_index.takt",
    "75_implicit_checks.takt",
    "76_stream_views.takt",
    "77_float_faults.takt",
    "78_length_guards.takt",
    "79_byte_literals.takt",
    "80_payload_variants.takt",
    "45_journal_cut.takt",
    "81_persist_variants.takt",
    "82_scheduled_sleep.takt",
    "83_durations.takt",
    "84_defaults.takt",
    "85_observe_invalid.takt",
    "86_units.takt",
    "87_fault_kinds.takt",
    "88_capture_segments.takt",
    "89_fault_paths.takt",
    "90_abort.takt",
    "91_subnormals.takt",
    "92_idle_streams.takt",
    "93_confirmations.takt",
    "94_float_ranges.takt",
    "95_boundary_ranges.takt",
    "96_record_outputs.takt",
    "97_fast_math.takt",
    "98_last_fault.takt",
    "99_exit_fault.takt",
    "100_dispatch.takt",
    "101_correct_math.takt",
    "102_correct_math_f32.takt",
    "103_math_domains.takt",
    "104_linear_has.takt",
    "11_foc_drive.takt",
    "sim/12_7/program.takt",
    "sim/14_7/program.takt",
];

/// Wie viele Ticks verglichen werden.
///
/// Genug, dass jede `after`-Frist des Korpus feuert (die laengste ist
/// 500 ms bei 10 ms Tick), und wenig genug, dass ein Fehlschlag noch zu
/// lesen ist.
const TICKS: u64 = 60;

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
    let options = takt_interp::RunOptions { ticks: TICKS, profile: None, order_seed: None, ..Default::default() };
    match takt_interp::run(p, &takt_interp::Trace::default(), &options) {
        Ok(r) => r.trace.render(),
        Err(e) => panic!("Interpreter: {e:?}"),
    }
}

/// **Der virtuelle Schlaf ist unsichtbar** (Satz 9.9.1): Jedes
/// Korpusprogramm mit einem `idle`-Zustand liefert schlafend denselben
/// Trace wie der Interpreter, der nie schlaeft — und der Rahmen hat
/// dabei tatsaechlich geschlafen. Auf dem Wirt, damit `_advance` (9.9)
/// nicht erst auf dem Board geprueft wird (FB-268, FB-273).
#[test]
fn virtual_sleep_is_invisible() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut failed = Vec::new();
    let mut slept = 0;
    for name in KORPUS {
        let path = format!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/{}"), name);
        let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        if !src.contains(" idle:") {
            continue;
        }
        let p = corpus(name);
        let native = match common::run_native_sleeping(&clang, &p, name, TICKS) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        slept += native.lines().filter(|l| l.contains(" slept=")).count();
        let widened = takt_conformance::run::widen_f32(&run_interpreted(&p), &takt_conformance::run::f32_outputs(&p));
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
    assert!(slept > 0, "kein Programm hat geschlafen");
}

/// **Die Abnahme.** Interpreter und erzeugter Code liefern dieselben
/// Outputs (Satz 9.4.4).
#[test]
fn the_interpreter_and_the_generated_code_agree() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let mut failed = Vec::new();
    for name in KORPUS {
        let p = corpus(name);
        // Alle Maschinen, in Schrittordnung (7.2): Ψ-Lesevorgaenge und
        // `follows` gibt es nur zwischen Maschinen.
        let native = match common::run_native_all(&clang, &p, name, TICKS) {
            Ok(t) => t,
            Err(e) => {
                failed.push(format!("{name}: kein nativer Lauf:\n{e}"));
                continue;
            }
        };
        let interpreted = run_interpreted(&p);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
/// `{x:04}`.
///
/// Sie laufen ueber dieselbe Ziffernrechnung, unterscheiden sich aber in
/// Basis, Vorzeichen und Fuellung — und jede davon hat einen Rand: die
/// Null ohne Ziffer, das Minus vor der Zahl, die Fuellung, die kuerzer
/// ist als die Zahl.
#[test]
fn the_format_specs_produce_the_expected_text() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
    let p = corpus("25_format.takt");
    let machine = p.machines.first().map(|m| m.name.clone()).expect("Maschine");
    let native = common::run_native_with(&clang, &p, "format", &machine, 8, &[]).unwrap_or_else(|e| panic!("{e}"));
    // Der Treiber meldet die Bytes als `out tx [..]` (8.8), also prueft
    // `compare` sie gegen den Interpreter. Hier steht, *was* dort stehen
    // muss — sonst waere ein Lauf gruen, in dem beide Seiten dasselbe
    // Falsche schreiben.
    for (was, bytes) in [
        // `d=10 h=a p=0010`
        ("`{x:hex}` von 10 ist `a`", "0x64, 0x3d, 0x31, 0x30, 0x20, 0x68, 0x3d, 0x61"),
        // `d=-2 h=fffffffffffffffe …`: hex zaehlt das Bitmuster (3.9).
        ("`{x:hex}` von -2 ist vorzeichenlos", "0x64, 0x3d, 0x2d, 0x32, 0x20, 0x68, 0x3d, 0x66, 0x66"),
    ] {
        assert!(native.contains(bytes), "{was} — fehlt:\n{native}");
    }
}

/// Tunables (8.4): Der Parametervektor aendert sich an der Tick-Grenze —
/// beide Seiten lesen denselben Wert im selben Tick, und ein Wert
/// ausserhalb der Range bleibt auf beiden Seiten ohne Wirkung.
#[test]
fn a_tunable_changes_the_parameter_vector_at_its_tick() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
    let inputs = Stimulus::from_trace(&stimulus);
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
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path);
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
