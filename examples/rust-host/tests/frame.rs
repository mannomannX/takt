//! Der Rahmen der Lieferform gegen den Interpreter (13.1, Satz 9.4.4): je
//! Regel ein kleines Programm in `takt/`, ohne Treiber, in logischer Zeit.

use rust_host::{
    Bits, Host, Overfull, Script, byte_ring, drop_oldest, exit_fault, faulted_pending, float_elements, idle_multirate,
    lifecycle, long_line, overflow_late, overfull_tx, sys_inputs, tuning,
};

fn agrees(name: &str, trace: &str, ticks: u64) {
    let source = format!("{}/takt/{name}.takt", env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = takt_embed::testing::same_as_interpreter(&source, ticks, trace) {
        panic!("{e}\nTrace:\n{trace}");
    }
}

/// **`send` an einen internen Strom mit `drop_oldest` verdraengt** (8.6,
/// FB-427): Der Leser liest sechs Ticks nicht; der Ring verdraengt das
/// aelteste Element, `dropped` zaehlt, der Leser bekommt den Alert, und der
/// Sender faultet nie.
#[test]
fn a_send_to_a_full_drop_oldest_stream_evicts_the_oldest() {
    let mut arena = drop_oldest::Arena::new();
    let trace = takt_embed::testing::run(drop_oldest::Program::init(&mut arena), drop_oldest::TICK_NS, 40);
    assert!(!trace.contains(" fault "), "der Sender faultet nicht:\n{trace}");
    assert!(trace.contains("stream q dropped=1 "), "verdraengt:\n{trace}");
    agrees("drop_oldest", &trace, 40);
}

/// **Ein Fault-Uebergang des Besitzers laesst die `exit:`-Bloecke der
/// Instanz aus** (5.11, KON2-024); ein gewoehnlicher Uebergang fuehrt sie
/// aus. Verglichen werden die `log`-Zeilen je Tick.
#[test]
fn a_fault_exit_of_the_owner_skips_the_exit_blocks_of_its_instance() {
    let mut arena = exit_fault::Arena::new();
    let trace = takt_embed::testing::run(exit_fault::Program::init(&mut arena), exit_fault::TICK_NS, 16);
    let logs: Vec<&str> = trace.lines().filter(|l| l.contains(" log ")).collect();
    assert_eq!(logs.len(), 1, "nur der regulaere Austritt:\n{trace}");
    agrees("exit_fault", &trace, 16);
}

/// Der Stimulus in Trace-Form zu den Elementen eines Drehbuchs.
fn stimulus(script: &Script) -> String {
    let mut s: String = script.rx.iter().map(|(t, b)| format!("t={t} in rx {b}\n")).collect();
    if let Some(t) = script.resume_at {
        s.push_str(&format!("t={t} in resume true\n"));
    }
    s
}

fn agrees_with(name: &str, stimulus: &str, trace: &str, ticks: u64) {
    let source = format!("{}/takt/{name}.takt", env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = takt_embed::testing::same_as_interpreter_with(&source, ticks, stimulus, trace) {
        panic!("{e}\nStimulus:\n{stimulus}\nTrace:\n{trace}");
    }
}

/// **Ein Ueberlauf in einem inaktiven Tick wirkt bei der naechsten
/// Aktivierung** (8.6, 9.4, KON1-016): Die Abort-Phase stellt nur Abort und
/// Runtime zu; `slow` schreitet alle 4 ms, und das dreizehnte Element laeuft
/// in Tick 5 ueber.
#[test]
fn an_overflow_in_an_inactive_tick_faults_at_the_next_activation() {
    let rx: Vec<(u64, u8)> = (1..=6).flat_map(|t| (0..3).map(move |i| (t, (t * 3 + i) as u8))).collect();
    let mut script = Script { rx, resume_at: None, tick_ns: overflow_late::TICK_NS };
    let stim = stimulus(&script);
    let mut arena = overflow_late::Arena::new();
    let trace =
        takt_embed::testing::run(overflow_late::Program::init(&mut arena, &mut script), overflow_late::TICK_NS, 20);
    assert!(trace.contains(" fault slow StreamOverflow"), "der Ueberlauf faultet:\n{trace}");
    agrees_with("overflow_late", &stim, &trace, 20);
}

/// **Ein Ueberlauf in FAULTED bleibt vorgemerkt** (9.3, 9.6, KON2-027):
/// Er trifft den Leser, sobald er FAULTED verlaesst, statt verworfen zu
/// werden.
#[test]
fn an_overflow_while_faulted_waits_until_the_machine_leaves_faulted() {
    let mut script =
        Script { rx: vec![(6, 6), (7, 7), (8, 8), (9, 9)], resume_at: Some(12), tick_ns: faulted_pending::TICK_NS };
    let stim = stimulus(&script);
    let mut arena = faulted_pending::Arena::new();
    let trace =
        takt_embed::testing::run(faulted_pending::Program::init(&mut arena, &mut script), faulted_pending::TICK_NS, 20);
    assert!(trace.contains(" fault reader StreamOverflow"), "nach FAULTED faultet der Ueberlauf:\n{trace}");
    agrees_with("faulted_pending", &stim, &trace, 20);
}

/// **Zwei Arenen im selben Faden teilen nichts** (12.11, GEN-032): Zwei
/// Instanzen desselben Programms schreiten abwechselnd Frist fuer Frist,
/// jede mit eigenem Drehbuch, und jede gleicht ihrem eigenen
/// Interpreterlauf. Ein geteilter veraenderlicher Zustand im Rahmen zeigte
/// sich als Abweichung der einen oder der anderen.
#[test]
fn two_arenas_stepped_alternately_each_match_their_own_interpreter_run() {
    let tick_ns = faulted_pending::TICK_NS;
    let mut a_script = Script { rx: vec![(6, 6), (7, 7), (8, 8), (9, 9)], resume_at: Some(12), tick_ns };
    let mut b_script = Script { rx: vec![(1, 41), (2, 42), (4, 44)], resume_at: Some(5), tick_ns };
    let (a_stim, b_stim) = (stimulus(&a_script), stimulus(&b_script));
    let (mut a_arena, mut b_arena) = (faulted_pending::Arena::new(), faulted_pending::Arena::new());
    let mut a =
        takt_embed::testing::Stepper::new(faulted_pending::Program::init(&mut a_arena, &mut a_script), tick_ns, 20);
    let mut b =
        takt_embed::testing::Stepper::new(faulted_pending::Program::init(&mut b_arena, &mut b_script), tick_ns, 20);
    let (mut a_trace, mut b_trace) = (String::new(), String::new());
    loop {
        let (x, y) = (a.step(), b.step());
        if x.is_none() && y.is_none() {
            break;
        }
        a_trace.push_str(&x.unwrap_or_default());
        b_trace.push_str(&y.unwrap_or_default());
    }
    assert_ne!(a_trace, b_trace, "verschiedene Eingaben, verschiedene Laeufe");
    agrees_with("faulted_pending", &a_stim, &a_trace, 20);
    agrees_with("faulted_pending", &b_stim, &b_trace, 20);
}

/// **Eine Zeile ueber `line<16>` kommt gekuerzt an** (3.9, GEN-033): Das
/// Geraet liefert zwanzig Bytes; der Handler sieht sechzehn, wie im
/// Interpreter, und kein Byte geht ueber den Platz des Elements hinaus.
#[test]
fn an_overlong_line_arrives_truncated_like_in_the_interpreter() {
    let mut line = rust_host::Line { at: 1, text: b"0123456789ABCDEFXXXX", tick_ns: long_line::TICK_NS };
    let mut arena = long_line::Arena::new();
    let trace = takt_embed::testing::run(long_line::Program::init(&mut arena, &mut line), long_line::TICK_NS, 4);
    assert!(trace.contains("out len 16"), "auf sechzehn gekuerzt:\n{trace}");
    assert!(trace.contains("t=1 out cut 1"), "`.truncated` meldet das Kuerzen (KON2-028):\n{trace}");
    agrees_with("long_line","t=1 in rx \"0123456789ABCDEFXXXX\"\n", &trace, 4);
}

/// **Die Eingaenge von `sys` stellt der Wirt** (12.7, 12.11, GEN-037): Sie
/// kommen aus dem Trait `Sys`, nicht aus `Drivers` (das Treiberobjekt
/// `Host` hat dort nur den Knopf, und `sys` hat keinen Heartbeat), und der
/// Lauf gleicht dem Interpreter mit demselben Stimulus.
#[test]
fn the_host_provides_the_sys_inputs_like_a_stimulus() {
    let tick_ns = sys_inputs::TICK_NS;
    let wall_at_zero = 4_999_000_000;
    let mut host = Host { previous_run: 1, wall_at_zero, pressed_at: Some(3), tick_ns };
    let mut arena = sys_inputs::Arena::new();
    let trace = takt_embed::testing::run(sys_inputs::Program::init(&mut arena, &mut host), tick_ns, 6);
    // Der Wirt meldet beide je Tick frisch; `max_age` ist zwei Perioden.
    let mut stim = String::new();
    for k in 0..6 {
        stim.push_str(&format!("t={k} in previous_run ENDED\nt={k} in wall {} ns\n", wall_at_zero + k * tick_ns));
        if k == 3 {
            stim.push_str("t=3 in button true\n");
        }
    }
    assert!(trace.contains("out after_end 1"), "previous_run = ENDED vom Wirt:\n{trace}");
    assert!(trace.contains("out late 1"), "die Wanduhr vom Wirt:\n{trace}");
    agrees_with("sys_inputs", &stim, &trace, 6);
}

/// **NaN und Inf in einem Stromelement sind `malformed`** (5.9, 8.6,
/// INT-025): Der Rand zaehlt sie und verwirft sie wie einen Range-Verstoss;
/// nur die beiden endlichen Elemente kommen an.
#[test]
fn nan_and_infinity_in_a_stream_element_are_malformed() {
    let tick_ns = float_elements::TICK_NS;
    let elements = vec![
        (1, 1.5f64.to_bits()),
        (2, f64::NAN.to_bits()),
        (3, f64::INFINITY.to_bits()),
        (4, f64::NEG_INFINITY.to_bits()),
        (5, 2.5f64.to_bits()),
    ];
    let mut bits = Bits { elements, tick_ns };
    let mut arena = float_elements::Arena::new();
    let trace = takt_embed::testing::run(float_elements::Program::init(&mut arena, &mut bits), tick_ns, 8);
    assert!(trace.contains("stream rx dropped=0 overflowed=0 malformed=3"), "drei verworfen:\n{trace}");
    let counts = trace.lines().filter(|l| l.contains(" out count ") && !l.starts_with("t=0 ")).count();
    assert_eq!(counts, 2, "zwei angekommen:\n{trace}");
    assert!(trace.contains("t=5 out last 2.5"), "das letzte endliche:\n{trace}");
    // Ein Stimulus kennt NaN und Inf nicht (4.1): Der Interpreter bekommt
    // solche Bytes nie vom Rand, und beide urteilen ueber die Byteform mit
    // demselben `takt_native::bytes::decodes`.
}

/// **Die Huelle durch einen ganzen Lauf** (12.11, GEN-028): ein Job, der
/// zwischen den Ticks rechnet (4.5), ein `idle`-Zustand, ueber den der Kern
/// schlaeft (`next_deadline`, `advance`; 9.9), und `next_run`, das den Lauf
/// mit den `safe`-Werten beendet (12.7) — wie im Interpreter.
#[test]
fn the_hull_runs_jobs_sleep_and_the_end_of_a_run_like_the_interpreter() {
    let mut arena = lifecycle::Arena::new();
    let trace = takt_embed::testing::run(lifecycle::Program::init(&mut arena), lifecycle::TICK_NS, 60);
    assert!(trace.contains("t=3 out word 38858"), "der Job rechnete:\n{trace}");
    assert!(trace.contains("t=34 end after"), "next_run beendet den Lauf:\n{trace}");
    assert!(trace.lines().any(|l| l.contains(" time ") && !l.ends_with("slept=0")) || !trace.contains(" time "));
    agrees("lifecycle", &trace, 60);
}

/// **`persist` durch die Huelle** (5.9, 12.11, GEN-028): Der Stand nach dem
/// Start (`persist_snapshot`) laedt in eine neue Arena vor Tick 0
/// (`persist_restore` vor `init`), und der zweite Lauf zaehlt weiter.
#[test]
fn the_hull_restores_persist_before_tick_zero() {
    use takt_embed::rt::Program as _;
    let mut first = lifecycle::Arena::new();
    let mut started = lifecycle::Program::init(&mut first);
    let mut payload = [0u8; 64];
    let n = started.persist_snapshot(&mut payload);
    assert!(n > 0, "der Start schreibt `starts`");

    let mut second = lifecycle::Arena::new();
    let mut restored = lifecycle::Program::new(&mut second);
    assert_eq!(restored.persist_restore(&payload[..n]), 1, "ein Eintrag uebernommen");
    let trace = takt_embed::testing::run(restored, lifecycle::TICK_NS, 2);
    assert!(trace.contains("t=0 out boots 2"), "der zweite Start zaehlt weiter:\n{trace}");
}

/// **Der Schlaf ueber Maschinen mit eigener Periode ist unsichtbar** (Satz
/// 9.9.1, 7.2, FB-429): Die Frist des Rahmens zaehlt ab jetzt bis zur
/// Aktivierung, an der `after` feuert, und das Vorruecken behaelt die
/// Phase. Der Kern schlaeft ueber beide `idle`-Zustaende; jeder Wechsel
/// liegt im Tick des Interpreters ohne Schlaf.
#[test]
fn sleep_over_machines_with_their_own_period_is_invisible() {
    let mut arena = idle_multirate::Arena::new();
    let trace = takt_embed::testing::run(idle_multirate::Program::init(&mut arena), idle_multirate::TICK_NS, 150);
    agrees("idle_multirate", &trace, 150);
}

/// Saetze von Tunables je Tick des Traces, in kanonischer Byteform (5.9).
struct Sets(Vec<(u64, Vec<u8>)>);

impl takt_embed::rt::Tunables for Sets {
    fn poll(&mut self, k: u64, apply: &mut dyn FnMut(u32, &[u8])) {
        // Grenze `k` der Schleife ist Tick `k + 1` des Traces.
        for (_, value) in self.0.iter().filter(|(t, _)| *t == k + 1) {
            apply(0, value);
        }
    }
}

/// **Tunables gehen ueber die Schleife in das Programm** (8.4, KON1-014,
/// RT-019): `GAIN = 7` wirkt ab Tick 3, `200` und `101` liegen ausserhalb
/// der Range und bleiben ohne Wirkung, `100` an der Grenze wirkt, ein Wert
/// in falscher Laenge ebenso wenig wie ein fremder; der Trace gleicht dem
/// Interpreter mit den `tune`-Zeilen.
#[test]
fn tunables_reach_the_program_through_the_loop() {
    let gain = |v: i64| v.to_le_bytes().to_vec();
    let mut sets = Sets(vec![
        (3, gain(7)),
        (5, gain(200)),
        (6, gain(100)),
        (7, gain(101)),
        (8, gain(0)),
        (9, 3i32.to_le_bytes().to_vec()),
    ]);
    let mut arena = tuning::Arena::new();
    let trace = takt_embed::testing::run_tuned(tuning::Program::init(&mut arena), tuning::TICK_NS, 12, &mut sets);
    for line in ["t=3 out level 14", "t=6 out level 200", "t=8 out level 0"] {
        assert!(trace.contains(line), "{line}:\n{trace}");
    }
    assert!(
        !trace.contains("level 400") && !trace.contains("level 202") && !trace.contains("t=9 out level"),
        "{trace}"
    );
    let stim = "t=3 tune GAIN 7\nt=5 tune GAIN 200\nt=6 tune GAIN 100\nt=7 tune GAIN 101\nt=8 tune GAIN 0\n";
    agrees_with("tuning", stim, &trace, 12);
}

/// **Der Byte-Ring laeuft um** (8.6, 9.6, KON2-026): Elemente von ein bis
/// acht Byte in Ringen, deren `capacity_bytes` Pruefung 17 gerade zulaesst,
/// und ein Leser, der pausiert. `tight` laeuft um und dann ueber (der Sender
/// faultet), `rolling` verdraengt ueber viele Umlaeufe; Inhalt, Laenge und
/// Nummer jedes gelesenen Elements gehen in die Pruefsummen.
#[test]
fn the_byte_ring_wraps_around_like_in_the_interpreter() {
    let mut arena = byte_ring::Arena::new();
    let trace = takt_embed::testing::run(byte_ring::Program::init(&mut arena), byte_ring::TICK_NS, 120);
    assert!(trace.contains(" fault feed_a StreamOverflow"), "tight laeuft ueber:\n{trace}");
    assert!(trace.contains("stream rolling dropped="), "rolling verdraengt:\n{trace}");
    agrees("byte_ring", &trace, 120);
}

/// **Ein ueberfahrener Sendepuffer ist `Runtime(Driver)`** (12.6 Zeile 6,
/// dritter Fall; KON1-012): Der Treiber meldet in Tick 5 einen Platz mehr,
/// als `capacity = 64` fasst; der Commit erkennt es, und der Besitzer faultet
/// im naechsten Tick. Vorher reichte der Rahmen fuer Ausgabestroeme keine
/// Kapazitaet an den Rand, und der Fall blieb unbemerkt.
#[test]
fn a_driver_that_reports_more_room_than_the_stream_has_breaks_the_contract() {
    let tick_ns = overfull_tx::TICK_NS;
    let mut device = Overfull { at: 5, capacity: 64, tick_ns };
    let mut arena = overfull_tx::Arena::new();
    let trace = takt_embed::testing::run(overfull_tx::Program::init(&mut arena, &mut device), tick_ns, 10);
    let driver: Vec<&str> = trace.lines().filter(|l| l.contains("runtime Driver")).collect();
    assert_eq!(driver, ["t=6 runtime Driver tx"], "{trace}");
    assert!(trace.contains("t=6 out up 0"), "der Besitzer geht auf DOWN:\n{trace}");
}
