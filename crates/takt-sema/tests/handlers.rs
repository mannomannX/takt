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
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn errors(body: &str) -> String {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
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
    // bleiben im Puffer und werden vom Folgezustand gelesen. In Tick 1 ist
    // SECOND im Eintritts-Tick (Fenster leer), in Tick 2 sieht es die zwei
    // Reste aus Tick 0 und die drei aus Tick 1: 1 + 5 = 6. Ohne Rest waeren
    // es 4.
    assert!(trace.contains("t=1 out seen 1\n"), "ein Element, dann Uebergang: {trace}");
    assert!(trace.contains("t=2 out seen 6\n"), "der Rest bleibt erhalten: {trace}");
}

#[test]
fn the_window_is_empty_in_the_entry_tick() {
    // 8.7 und 9.6: „`on`-Handler laufen im Entry-Modus nicht (das
    // Stream-Fenster ist dort leer)".
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
    // Im Eintritts-Tick 2 laeuft kein Handler; in Tick 3 werden die
    // aufgestauten Elemente verarbeitet: gesendet in den Ticks 0 bis 2.
    assert!(!with_go.contains("t=2 out seen"), "kein Handler im Entry-Tick: {with_go}");
    assert!(with_go.contains("t=3 out seen 3\n"), "{with_go}");
}

#[test]
fn a_for_loop_over_a_stream_sees_an_empty_window_in_the_entry_tick() {
    // 9.6: `windows(m)` ist im Modus ENTRY leer — fuer `for x in s` wie fuer
    // Handler. Tick 0 ist fuer jede Maschine ein Eintritts-Tick (9.3).
    let trace = driven(
        "\
input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 200 kHz, capacity = 256

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine m:
    var count : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            for x in rx:
                count = (count + 1) % 100
            n = count
",
        "t=0 in rx 120\nt=0 in rx 121\nt=0 in rx 122\n",
        3,
    );
    assert!(trace.contains("t=0 out n 0\n"), "das Fenster ist im Eintritts-Tick leer: {trace}");
    assert!(trace.contains("t=1 out n 3\n"), "jedes Element genau einmal: {trace}");
    assert!(!trace.contains("out n 6"), "kein Element doppelt: {trace}");
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
    // Ohne Handler konsumiert niemand, und ein Strom ohne Konsumenten
    // behaelt nichts, was in einem Tick sichtbar war (9.6, FB-426): Der
    // Zaehler sieht das Element, das in diesem Tick ankam, und nicht mehr.
    // `count` selbst konsumiert nichts, sonst stuende hier 0.
    assert!(trace.contains("t=3 out n 1\n"), "{trace}");
    assert!(!trace.contains("out n 2\n"), "nichts sammelt sich an: {trace}");
}

#[test]
fn a_full_internal_stream_faults_its_writer() {
    // 8.6: Der Ueberlauf eines internen Stroms trifft den Schreiber beim
    // `send`, nicht die Leser (FB-326). Pruefung 17 laesst nur Kapazitaeten
    // zu, die ein mithaltender Konsument schafft; der Ueberlauf entsteht
    // hier, weil der Leser in `IDLE` nichts untersucht und der Puffer
    // volllaeuft — ein Element je Tick, vier Plaetze.
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
    assert!(trace.contains("t=4 fault writer StreamOverflow"), "{trace}");
    assert!(!trace.contains("fault reader"), "{trace}");
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
    // Die Bytes verlassen den Puffer im selben Tick (8.8): 100 kHz nehmen je
    // Tick 100 Bytes ab, beide Bytes stehen in Tick 0 im Trace. Ein Strom
    // traegt kein Latch: Jeder Tick nennt, was er gesendet hat.
    for t in 0..=2 {
        assert!(trace.contains(&format!("t={t} out tx [0x41, 0x42]\n")), "Tick {t}: {trace}");
    }
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
    // erscheinen, wenn sie sich aendern (8.6). Der Ueberlauf kommt von
    // einem Leser, der zurueckfaellt: Der Treiber haelt `MAXPT` ein (12.6),
    // der Puffer laeuft trotzdem voll, weil `IDLE` nichts abholt.
    let trace = driven(
        "\
input dut_log : stream<line<64>> @ hw(\"uart0/rx\") with capacity = 2, max_rate = 2000 Hz

output n : int in 0..99 @ hw(\"o/n\") with safe = 0

machine watch:
    var seen : int in 0..99 = 0
    initial IDLE
    state IDLE:
        loop:
            n = seen
    state READ:
        on dut_log as e:
            seen = seen + 1
",
        "t=1 in dut_log \"a\"\nt=1 in dut_log \"b\"\nt=2 in dut_log \"c\"\n",
        3,
    );
    assert!(trace.contains("stream dut_log dropped=0 overflowed=1 malformed=0"), "der Ueberlauf zaehlt: {trace}");
}

#[test]
fn a_text_element_may_stand_in_its_wire_form() {
    // `grammar/trace.md`: Ein Element von Text, Bytes oder einem Record darf
    // in seiner Drahtform stehen, wie die Zeile `rec` es aufzeichnet (8.2);
    // ein `u8` steht als Zahl.
    let trace = driven(
        "\
input dut_log : stream<line<64>> @ hw(\"uart0/rx\") with max_rate = 2000 Hz
input raw_rx  : stream<bytes<4>> @ hw(\"uart1/rx\") with max_rate = 2000 Hz

output hi  : bool        @ hw(\"o/hi\")  with safe = false
output got : int in 0..4 @ hw(\"o/got\") with safe = 0

machine watch:
    initial RUN
    state RUN:
        on dut_log matches \"hi\" as e:
            hi = true
        on raw_rx as r:
            got = r.data.len as int
",
        "t=1 in dut_log 0x6869\nt=1 in raw_rx 0x0102\n",
        3,
    );
    assert!(trace.contains("t=1 out hi true\n"), "`0x6869` ist der Text `hi`: {trace}");
    assert!(trace.contains("t=1 out got 2\n"), "`0x0102` sind zwei Bytes: {trace}");
}

/// Ein Leser an zweiter Stelle mit einem Handler auf Maschinenebene; `states`
/// sind die Zustaende hinter `initial`.
fn machine_level_reader(states: &str) -> String {
    format!(
        "\
stream<u8> q with capacity = 16

output n : int in 0..999 @ hw(\"o/n\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine reader:
    var seen : int in 0..999 = 0
    initial A
    on q as e:
        seen = seen + 1
        n = seen
{states}"
    )
}

/// **Ein Handler auf Maschinenebene gilt in jedem Zustand** (8.7, FB-411).
/// Die Sema legte ihn in den Zustand mit dem Index der Maschine: Der Leser
/// an zweiter Stelle hoerte nur im zweiten Zustand.
#[test]
fn a_machine_level_handler_listens_in_every_state() {
    let trace = simulate(
        &machine_level_reader("    state A:\n        after 2 ms: -> B\n    state B:\n        after 1 s: -> B\n"),
        5,
    );
    assert!(trace.contains("t=1 out n 1\n"), "im Zustand A: {trace}");
    assert!(trace.contains("t=5 out n 5\n"), "und im Zustand B: {trace}");
}

/// Hat die Maschine weniger Zustaende, als ihre Nummer zaehlt, stuerzte die
/// Sema mit einem Indexfehler ab (FB-411).
#[test]
fn a_machine_level_handler_in_a_machine_with_one_state_compiles() {
    let trace = simulate(&machine_level_reader("    state A:\n        after 1 s: -> A\n"), 2);
    assert!(trace.contains("t=2 out n 2\n"), "{trace}");
}

/// Ein Schreiber mit `sends` als Rumpf seines `loop:` und ein Leser mit
/// Periode `every` an einem internen Strom der Kapazitaet `cap`.
fn writer_and_reader(sends: &str, every: u32, cap: u32) -> String {
    format!(
        "\
input  rx : stream<u8> @ hw(\"u/rx\") with max_rate = 3 kHz, capacity = 3
output rx_sim : stream<u8> @ sim(\"u/rx\")
stream<u8> q with capacity = {cap}

output v : bool @ hw(\"o/v\") with safe = false

machine writer:
    initial RUN
    state RUN:
        loop:
{sends}
machine reader every {every} ms:
    initial RUN2
    state RUN2:
        on q as e:
            v = true
"
    )
}

/// Pruefungen 17 und 43 (8.6): `MAXPT * n_m <= CAP` mit MAXPT als
/// statischer Hoechstzahl der `send` je Aktivierung des Schreibers — eine
/// Schleife zaehlt mit ihrer Schranke, ein Handler je Element seines
/// Fensters.
#[test]
fn maxpt_counts_the_sends_of_one_activation() {
    let once = "            send q, 1\n";
    let three = "            send q, 1\n            send q, 2\n            send q, 3\n";
    let looped = "            for i in range(3):\n                send q, i as u8\n";
    let handled = "            pass\n        on rx as e:\n            send q, e.data\n";
    for (what, sends, every, cap, rejected) in [
        ("Grenze MAXPT * n_m == CAP", once, 4, 4, false),
        ("eins darueber", once, 5, 4, true),
        ("drei send hintereinander", three, 2, 4, true),
        ("drei send in einer Schleife", looped, 2, 4, true),
        ("drei send in einer Schleife, Platz genug", looped, 2, 6, false),
        ("ein send je Element eines Fensters aus drei", handled, 2, 4, true),
        ("ein send je Element, Platz genug", handled, 2, 6, false),
    ] {
        let e = errors(&writer_and_reader(sends, every, cap));
        assert_eq!(e.contains("[SC-17]"), rejected, "{what}: {e}");
    }
}

#[test]
fn a_stream_guard_leaves_the_elements_after_its_match() {
    // 8.7: Der Guard setzt `examined` auf das erste passende Element; die
    // zwei danach bleiben. GOT sieht sie in Tick 2 neben den drei aus
    // Tick 1: `q.count` ist 5. Haette der Guard alles konsumiert, waere es 3.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output st : int in 0..9 @ hw(\"o/st\") with safe = 0
output left : int in 0..99 @ hw(\"o/left\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1
            send q, 2
            send q, 3

machine reader:
    initial WAIT
    state WAIT:
        loop:
            st = 1
        when q as e: -> GOT
    state GOT:
        loop:
            st = 2
            left = q.count
        on q as e:
            st = 2
",
        3,
    );
    assert!(trace.contains("t=1 out st 2\n"), "der Guard trifft in Tick 1: {trace}");
    assert!(trace.contains("t=2 out left 5\n"), "zwei Reste und drei neue: {trace}");
}

#[test]
fn handlers_of_parent_and_child_see_the_same_window() {
    // 9.7: „Handler verschiedener Ebenen sehen dasselbe W"; jedes Element
    // erreicht den Handler des Elternzustands und den des Kindes.
    let trace = simulate(
        "\
stream<u8> q with capacity = 16

output outer : int in 0..99 @ hw(\"o/outer\") with safe = 0
output inner : int in 0..99 @ hw(\"o/inner\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1
            send q, 2

machine reader:
    var a : int in 0..99 = 0
    var b : int in 0..99 = 0
    initial PARENT
    state PARENT:
        initial CHILD
        on q as e:
            a = (a + 1) % 100
            outer = a
        state CHILD:
            on q as e:
                b = (b + 1) % 100
                inner = b
",
        3,
    );
    assert!(trace.contains("t=1 out outer 2\n") && trace.contains("t=1 out inner 2\n"), "{trace}");
    assert!(trace.contains("t=3 out outer 6\n") && trace.contains("t=3 out inner 6\n"), "{trace}");
}

#[test]
fn streams_dispatch_in_the_order_of_their_handlers() {
    // 9.7: `for s in streams(st) in Deklarationsreihenfolge`. Die Handler auf
    // `b` stehen zuerst im Zustand, `a` ist zuerst deklariert; der
    // Interpreter nimmt die Reihenfolge der Handler im Zustand. Welche
    // Deklaration 9.7 meint, ist offen (Bericht sema_a, SEM1-051).
    let trace = simulate(
        "\
stream<u8> a with capacity = 4
stream<u8> b with capacity = 4

output order : int in 0..99 @ hw(\"o/order\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send a, 1
            send b, 2

machine reader:
    var trail : int in 0..99 = 0
    initial RUN2
    state RUN2:
        on b as e:
            trail = (trail * 10 + 2) % 100
            order = trail
        on a as e:
            trail = (trail * 10 + 1) % 100
            order = trail
",
        2,
    );
    assert!(trace.contains("t=1 out order 21\n"), "erst `b`, dann `a`: {trace}");
}

#[test]
fn every_consumer_sees_every_element_at_its_own_period() {
    // 9.6: Jeder Konsument hat seinen Cursor; geraeumt wird hinter dem
    // kleinsten. Der schnelle Leser sieht je Tick ein Element, der langsame
    // alle drei Ticks drei, und keiner verliert eines.
    let trace = simulate(
        "\
stream<u8> q with capacity = 4

output fast : int in 0..99 @ hw(\"o/fast\") with safe = 0
output slow : int in 0..99 @ hw(\"o/slow\") with safe = 0

machine writer:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine quick:
    var n : int in 0..99 = 0
    initial RUN2
    state RUN2:
        on q as e:
            n = (n + 1) % 100
            fast = n

machine lazy every 3 ms:
    var n : int in 0..99 = 0
    initial RUN3
    state RUN3:
        on q as e:
            n = (n + 1) % 100
            slow = n
",
        9,
    );
    assert!(trace.contains("t=6 out fast 6\n") && trace.contains("t=9 out fast 9\n"), "{trace}");
    assert!(trace.contains("t=6 out slow 6\n") && trace.contains("t=9 out slow 9\n"), "{trace}");
    assert!(!trace.contains("fault"), "{trace}");
}

#[test]
fn the_consumers_of_a_stream_are_known_statically() {
    // Pruefung 17: Die Konsumentenmenge steht zur Uebersetzungszeit fest —
    // jede Maschine mit einem Konstrukt, das das Fenster untersucht. Wer nur
    // `count` liest, untersucht nichts und ist keiner (8.6).
    let p = compile(
        "\
stream<u8> q with capacity = 8

output a : int in 0..99 @ hw(\"o/a\") with safe = 0
output b : int in 0..99 @ hw(\"o/b\") with safe = 0
output c : int in 0..99 @ hw(\"o/c\") with safe = 0

machine source:
    initial RUN
    state RUN:
        loop:
            send q, 1

machine handled:
    initial RUN
    state RUN:
        on q as e:
            a = 1

machine looped every 2 ms:
    initial RUN
    state RUN:
        loop:
            for e in q:
                b = 1

machine counted:
    initial RUN
    state RUN:
        loop:
            c = q.count
",
    );
    let names: Vec<&str> = p.streams[0].readers.iter().map(|m| p.machines[m.index()].name.as_str()).collect();
    assert_eq!(names, ["handled", "looped"]);
}
