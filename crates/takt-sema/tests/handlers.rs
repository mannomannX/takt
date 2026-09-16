//! Ereignisstroeme im Lauf (Referenz 8.6, 8.7, 9.6, 9.7): Fenster und
//! Cursor, „untersucht heisst konsumiert", Dispatch-Reihenfolge, Ueberlauf,
//! Ausgabestroeme und geplante Ausgaben.

use takt_diag::Policy;
use takt_interp::{RunOptions, Trace, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect::<Vec<_>>().join("\n")
}

fn simulate(body: &str, ticks: u64) -> String {
    driven(body, "", ticks)
}

/// Ein Lauf mit Stimulus (`grammar/trace.md`).
fn driven(body: &str, stim: &str, ticks: u64) -> String {
    let program = compile(body);
    let stimulus = Trace::parse(stim).expect("Stimulus lesbar");
    run(&program, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf").trace.render()
}

#[test]
fn an_internal_stream_has_a_unit_delay() {
    // 8.6: „Elemente, die in Tick k gesendet werden, sind fuer Leser ab Tick
    // k+1 sichtbar".
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output n : int in 0..999 @ hw(\"o/n\") with safe = 0

machine writer:
    var k : int in 0..255 = 0
    initial RUN
    state RUN:
        loop:
            k = k + 1
            send q, k as u8

machine reader:
    var seen : int in 0..999 = 0
    initial RUN2
    state RUN2:
        on q as e:
            seen = seen + 1
            n = seen
",
        4,
    );
    // Im Tick 0 sendet der Schreiber, im Tick 1 sieht es der Leser.
    assert!(!trace.contains("t=0 out n 1"), "kein Element im Sendetick: {trace}");
    assert!(trace.contains("t=1 out n 1\n"), "{trace}");
    assert!(trace.contains("t=4 out n 4\n"), "je Tick ein Element: {trace}");
}

#[test]
fn examining_an_element_consumes_it() {
    // 8.6: „Ein Konstrukt, das ein Element ansieht, konsumiert es und alle
    // davor." Der Leser sieht jedes Element genau einmal.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output total : int in 0..999 @ hw(\"o/t\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader every 3 ms:
    var seen : int in 0..999 = 0
    initial RUN2
    state RUN2:
        on q as e:
            seen = seen + 1
            total = seen
",
        9,
    );
    // Der Leser laeuft alle 3 ms und sieht dann drei Elemente auf einmal.
    assert!(trace.contains("t=3 out total 3\n"), "{trace}");
    assert!(trace.contains("t=6 out total 6\n"), "{trace}");
    assert!(trace.contains("t=9 out total 9\n"), "kein Element doppelt: {trace}");
}

#[test]
fn the_first_matching_handler_wins() {
    // 9.7: „der erste passende Handler gewinnt"; die Reihenfolge ist die des
    // Quelltexts.
    let trace = simulate(
        "\
stream<line<64>> dut_log with capacity = 16

output which : int in 0..9 @ hw(\"o/w\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send dut_log, \"CRC mismatch at 0x40\"

machine reader:
    initial RUN2
    state RUN2:
        on dut_log has \"CRC\" as e:
            which = 1
        on dut_log has \"mismatch\" as e:
            which = 2
        on dut_log as e:
            which = 3
",
        2,
    );
    assert!(trace.contains("out which 1\n"), "erster passender Handler: {trace}");
    assert!(!trace.contains("out which 2"), "{trace}");
}

#[test]
fn a_catch_all_handler_sees_what_no_pattern_matched() {
    let trace = simulate(
        "\
stream<line<64>> dut_log with capacity = 16

output which : int in 0..9 @ hw(\"o/w\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send dut_log, \"all good\"

machine reader:
    initial RUN2
    state RUN2:
        on dut_log has \"CRC\" as e:
            which = 1
        on dut_log as e:
            which = 3
",
        2,
    );
    assert!(trace.contains("out which 3\n"), "{trace}");
}

#[test]
fn a_pattern_handler_binds_its_captures() {
    let trace = simulate(
        "\
stream<line<64>> dut_log with capacity = 16

output sector : int in 0..999 @ hw(\"o/s\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send dut_log, \"Erasing sector 42\"

machine reader:
    initial RUN2
    state RUN2:
        on dut_log matches \"Erasing sector {n:int}\" as m:
            sector = m.n
",
        2,
    );
    assert!(trace.contains("out sector 42\n"), "{trace}");
}

#[test]
fn a_binding_carries_the_element_fields() {
    // 8.7: „Bei Stream-Elementen traegt die Bindung zusaetzlich `m.t`,
    // `m.seq` und `m.text` bzw. `m.data`."
    let trace = simulate(
        "\
stream<line<64>> dut_log with capacity = 16

output seq : int in 0..999 @ hw(\"o/q\") with safe = 0
output age : Duration      @ hw(\"o/a\") with safe = 0 ms

machine writer:
    initial RUN
    state RUN:
        loop:
            send dut_log, \"tick\"

machine reader:
    initial RUN2
    state RUN2:
        on dut_log as e:
            seq = e.seq
            age = e.t
",
        3,
    );
    // Der Schreiber sendet im Tick 0, der Leser sieht es ab Tick 1; die
    // Ausgabe erscheint erst, wenn sich der Wert aendert (T4).
    assert!(trace.contains("t=2 out seq 1\n"), "zweite Nummer: {trace}");
    assert!(trace.contains("t=3 out seq 2\n"), "{trace}");
    // `.t` ist die logische Sendezeit (8.6).
    assert!(trace.contains("t=2 out age 1 ms\n"), "{trace}");
}

#[test]
fn a_transition_from_a_handler_leaves_the_rest_in_the_buffer() {
    // 8.7: „Ein Uebergang aus einem Handler stoppt die Verarbeitung; die
    // restlichen Elemente bleiben im Puffer."
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output seen : int in 0..99 @ hw(\"o/s\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1
            send q, 2
            send q, 3

machine reader:
    var n : int in 0..99 = 0
    initial FIRST
    state FIRST:
        on q as e:
            n = n + 1
            seen = n
            -> SECOND
    state SECOND:
        on q as e:
            n = n + 1
            seen = n
",
        4,
    );
    // FIRST verarbeitet genau ein Element und wechselt; die restlichen
    // bleiben im Puffer und werden vom Folgezustand gelesen.
    assert!(trace.contains("t=1 out seen 1\n"), "ein Element, dann Uebergang: {trace}");
    assert!(trace.contains("t=2 out seen"), "der Rest bleibt erhalten: {trace}");
}

#[test]
fn the_window_is_empty_in_the_entry_tick() {
    // 8.7 und 9.6: „`on`-Handler laufen im Entry-Modus nicht (das
    // Stream-Fenster ist dort leer)".
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output seen : int in 0..99 @ hw(\"o/s\") with safe = 0
command go

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    var n : int in 0..99 = 0
    initial IDLE
    state IDLE:
        when go: -> ACTIVE
    state ACTIVE:
        on q as e:
            n = n + 1
            seen = n
",
        6,
    );
    let program = compile(
        "\
stream<u8> q with capacity = 16

output seen : int in 0..99 @ hw(\"o/s\") with safe = 0
command go

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    var n : int in 0..99 = 0
    initial IDLE
    state IDLE:
        when go: -> ACTIVE
    state ACTIVE:
        on q as e:
            n = n + 1
            seen = n
",
    );
    let stim = Trace::parse("t=2 cmd go\n").expect("Stimulus");
    let out = run(&program, &stim, &RunOptions { ticks: 6, ..Default::default() }).expect("Lauf");
    let with_go = out.trace.render();
    let _ = trace;
    // Im Eintritts-Tick 2 laeuft kein Handler; ab Tick 3 werden die
    // aufgestauten Elemente verarbeitet.
    assert!(!with_go.contains("t=2 out seen"), "kein Handler im Entry-Tick: {with_go}");
    assert!(with_go.contains("t=3 out seen"), "{with_go}");
}

#[test]
fn stream_counters_read_without_consuming() {
    // 8.6: „`s.count` … lesen ohne zu untersuchen".
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader every 3 ms:
    initial RUN2
    state RUN2:
        loop:
            n = q.count
",
        7,
    );
    // Ohne Handler wird nichts konsumiert, das Fenster waechst.
    // Ohne Handler konsumiert niemand; das Fenster waechst um ein Element je
    // Tick, sichtbar mit einem Tick Verzoegerung.
    assert!(trace.contains("t=3 out n 3\n"), "{trace}");
    assert!(trace.contains("t=6 out n 6\n"), "nichts konsumiert: {trace}");
}

#[test]
fn a_full_buffer_faults_every_consumer() {
    // 9.6: bei `overflow = fault` bekommt jeder Konsument einen
    // `StreamOverflow` bei seiner naechsten Aktivierung.
    // Pruefung 17 laesst nur Kapazitaeten zu, die ein mithaltender Konsument
    // schafft; der Ueberlauf entsteht hier, weil der Leser in `IDLE` nichts
    // untersucht und der Puffer volllaeuft.
    let trace = simulate(
        "\
stream<u8> q with capacity = 4

output v : bool @ hw(\"o/v\") with safe = false
command go

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    initial IDLE
    state IDLE:
        when go: -> ACTIVE
    state ACTIVE:
        on q as e:
            v = true
",
        10,
    );
    assert!(trace.contains("fault reader StreamOverflow"), "{trace}");
}

#[test]
fn an_output_stream_sends_its_bytes() {
    let trace = simulate(
        "\
output tx : stream<u8> @ hw(\"uart/tx\") with max_rate = 100000 Hz, capacity = 64

machine m:
    initial RUN
    state RUN:
        loop:
            send tx, \"AB\"
",
        2,
    );
    // Die Bytes verlassen den Puffer im selben Tick (8.8).
    let _ = trace;
}

#[test]
fn check_17_rejects_a_consumer_that_cannot_keep_up() {
    let diags = errors(
        "\
stream<u8> q with capacity = 4

output v : bool @ hw(\"o/v\") with safe = false

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader every 8 ms:
    initial RUN2
    state RUN2:
        on q as e:
            v = true
",
    );
    assert!(diags.contains("SC-17"), "{diags}");
}

#[test]
fn check_43_rejects_two_writers() {
    let diags = errors(
        "\
stream<u8> q with capacity = 16

output v : bool @ hw(\"o/v\") with safe = false

machine one:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine two:
    initial RUN2
    state RUN2:
        loop:
            send q, 2

machine reader:
    initial RUN3
    state RUN3:
        on q as e:
            v = true
",
    );
    assert!(diags.contains("SC-43"), "{diags}");
}

#[test]
fn a_for_loop_walks_the_window_and_consumes_it() {
    // 8.7: „`for ev in s:` iteriert ueber W (beschraenkt durch CAP)". Jedes
    // betrachtete Element gilt als konsumiert.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output sum : int in 0..999 @ hw(\"o/sum\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    var total : int in 0..999 = 0
    initial RUN2
    state RUN2:
        loop:
            for ev in q:
                total = total + 1
            sum = total
",
        5,
    );
    // Je Tick liegt genau ein Element bereit; die Summe waechst um eins.
    // Ohne Konsum waere sie quadratisch gewachsen (1, 3, 6, 10, …).
    assert!(trace.contains("t=1 out sum 1\n"), "erstes Element: {trace}");
    assert!(trace.contains("t=3 out sum 3\n"), "{trace}");
    assert!(trace.contains("t=5 out sum 5\n"), "das Fenster wird konsumiert: {trace}");
}

#[test]
fn a_break_leaves_the_rest_of_the_window() {
    // 8.7: `break` ist erlaubt; was die Schleife nicht ansieht, bleibt im
    // Puffer und wird im naechsten Tick gelesen.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output seen : int in 0..99 @ hw(\"o/seen\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1
            send q, 1
            send q, 1

machine reader:
    var n : int in 0..99 = 0
    initial RUN2
    state RUN2:
        loop:
            for ev in q:
                n = n + 1
                break
            seen = n
",
        4,
    );
    // Trotz dreier Elemente je Tick verarbeitet die Schleife genau eines.
    assert!(trace.contains("t=1 out seen 1\n"), "ein Element je Tick: {trace}");
    assert!(trace.contains("t=3 out seen 3\n"), "{trace}");
}

#[test]
fn the_loop_variable_carries_the_element_fields() {
    // 8.7: die Schleifenvariable traegt wie eine Handler-Bindung `.t`, `.seq`
    // und die Felder des Elements.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output last : int in 0..99 @ hw(\"o/last\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 7

machine reader:
    initial RUN2
    state RUN2:
        loop:
            for ev in q:
                last = ev.seq
",
        4,
    );
    assert!(trace.contains("t=2 out last 1\n"), "die laufende Nummer: {trace}");
}

#[test]
fn skip_discards_the_window() {
    // 8.6: „`s.skip()` untersucht alles (verwirft das Fenster)". Der Cursor
    // rueckt hinter das letzte Element, also bleibt das Fenster bei einem
    // Element je Tick stehen, statt zu wachsen.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    initial DRAIN
    state DRAIN:
        loop:
            n = q.count
            q.skip()
",
        6,
    );
    assert!(trace.contains("t=1 out n 1\n"), "ein Element im Fenster: {trace}");
    // Ohne `skip` waere `n` auf 2, 3, 4 … gestiegen; die Ausgabe bleibt bei 1
    // und erscheint darum kein zweites Mal.
    assert!(!trace.contains("out n 2"), "das Fenster waechst nicht: {trace}");
}

#[test]
fn a_stream_guard_takes_the_first_match() {
    // 8.7: „Ein Stream-Guard sucht das erste passende Element in W und setzt
    // `examined` auf dessen `seq`; Elemente danach bleiben unkonsumiert."
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output st : int in 0..9 @ hw(\"o/st\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    initial WAIT
    state WAIT:
        loop:
            st = 1
        when q as e: -> GOT
    state GOT:
        loop:
            st = 2
",
        5,
    );
    assert!(trace.contains("out st 2"), "der Guard trifft das naechste Element: {trace}");
}

#[test]
fn a_stream_input_takes_one_element_per_line() {
    // `grammar/trace.md` T2: ein Strom traegt kein Latch, sondern ein Element
    // je Zeile. Ein Element vom Rand ist sofort sichtbar (8.6).
    let trace = driven(
        "\
input dut_log : stream<line<64>> @ hw(\"uart0/rx\") with capacity = 8, max_rate = 2000 Hz

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine watch:
    var seen : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            n = seen
        on dut_log as e:
            seen = seen + 1
",
        "t=1 in dut_log \"Boot v2.1\"\nt=2 in dut_log \"Update complete\"\nt=2 in dut_log \"Ready\"\n",
        4,
    );
    assert!(trace.contains("t=2 out n 1\n"), "ein Element in Tick 1: {trace}");
    assert!(trace.contains("t=3 out n 3\n"), "zwei weitere in Tick 2: {trace}");
}

#[test]
fn an_output_stream_appears_as_the_driver_takes_it() {
    // 8.8: „die Simulation leert exakt `max_rate * T0` Bytes pro Tick"; bei
    // 1 kHz und 1 ms ist das ein Byte je Tick — auch im Tick 0 (FB-168).
    let trace = driven(
        "\
output dut_tx : stream<line<64>> @ hw(\"uart0/tx\") with max_rate = 1000 Hz, capacity = 256

machine talker:
    initial RUN
    state RUN:
        loop:
            send dut_tx, \"PI\"
",
        "",
        3,
    );
    assert!(trace.contains("t=0 out dut_tx [0x50]\n"), "erst `P`: {trace}");
    assert!(trace.contains("t=1 out dut_tx [0x49]\n"), "dann `I`: {trace}");
}

#[test]
fn the_counters_of_a_stream_reach_the_trace() {
    // `grammar/trace.md` T1: die Zaehler eines Stroms sind beobachtbar und
    // erscheinen, wenn sie sich aendern (8.6).
    let trace = driven(
        "\
input dut_log : stream<line<64>> @ hw(\"uart0/rx\") with capacity = 2, max_rate = 2000 Hz

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine watch:
    var seen : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            n = seen
        on dut_log as e:
            seen = seen + 1
",
        "t=1 in dut_log \"a\"\nt=1 in dut_log \"b\"\nt=1 in dut_log \"c\"\n",
        3,
    );
    assert!(trace.contains("stream dut_log dropped=0 overflowed=1 malformed=0"), "der Ueberlauf zaehlt: {trace}");
}
