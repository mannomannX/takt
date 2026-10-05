//! Schlaf (5.10, 9.9) im Interpreter: der Verwurf im `idle` und die
//! Konjunkte von `sleep_allowed` einzeln. Satz 9.9.1 selbst prueft
//! `theorems.rs` und, mit der Schleife aus 12.1, `takt-rt-linux`.

use takt_diag::Policy as Diagnostics;
use takt_interp::{Run, RunOptions, Trace};
use takt_mir::Program;
use takt_rt_core::{Clock, Policy, Profile, Runtime, Sink, Tick, Watchdog};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Diagnostics::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// `may_sleep` nach jedem Tick `1..=ticks`; Tick 0 fuehrt `Run::new` aus.
fn sleepy(p: &Program, stimulus: &str, ticks: u64) -> Vec<bool> {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let mut run = Run::new(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    (1..=ticks)
        .map(|k| {
            run.tick(k).expect("Tick");
            run.may_sleep()
        })
        .collect()
}

/// **Was eine Maschine vor dem Einschlafen untersucht hat, ist nicht
/// verworfen** (5.10, 9.6): Im Tick 5 liest der Handler das Element, und
/// die Maschine wechselt in den Schlaf. `dropped` zaehlt erst die Ticks 6
/// bis 14 — neun Elemente, nicht zehn.
#[test]
fn what_a_machine_read_before_sleeping_is_not_dropped() {
    let p = compile(
        "\
input  noise     : stream<u8> @ hw(\"bus/noise\") with capacity = 4, max_rate = 2000 Hz
output noise_sim : stream<u8> @ sim(\"bus/noise\")
output lost      : int in 0..999 @ hw(\"o/lost\") with safe = 0

machine feeder:
    initial GO
    state GO:
        loop:
            send noise_sim, 1

machine m:
    var seen : int in 0..999 = 0

    initial LISTEN

    state LISTEN:
        on noise as e:
            seen = min(seen + 1, 999)
        after 5 ms: -> SLEEP

    state SLEEP idle:
        after 10 ms: -> BACK

    state BACK:
        loop:
            lost = min(noise.dropped, 999)
",
    );
    let out = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 20, ..Default::default() }).expect("Lauf");
    let text = out.trace.render();
    assert!(text.contains("t=15 out lost 9"), "{text}");
}

/// **Jedes Aufwachen meldet `StreamPaused`** (5.6, 5.10, FB-342): Der Alert
/// der Runtime ist aktiv im Tick des Aufwachens und danach inaktiv — sonst
/// erschiene nach dem ersten Mal keiner mehr, weil nur Flanken zaehlen.
#[test]
fn every_wake_up_reports_stream_paused() {
    let p = compile(
        "\
input  noise     : stream<u8> @ hw(\"bus/noise\") with capacity = 4, max_rate = 2000 Hz
output noise_sim : stream<u8> @ sim(\"bus/noise\")
output led       : bool @ hw(\"ui/led\") with safe = false

machine feeder:
    initial GO
    state GO:
        loop:
            send noise_sim, 1

machine m:
    initial SLEEP

    state SLEEP idle:
        after 5 ms: -> AWAKE

    state AWAKE:
        on noise as e:
            led = true
        after 3 ms: -> SLEEP
",
    );
    let out = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 24, ..Default::default() }).expect("Lauf");
    let text = out.trace.render();
    let edges: Vec<&str> = text.lines().filter(|l| l.contains("StreamPaused")).collect();
    let want = [
        "t=5 alert m on",
        "t=6 alert m off",
        "t=13 alert m on",
        "t=14 alert m off",
        "t=21 alert m on",
        "t=22 alert m off",
    ];
    assert_eq!(edges.len(), want.len(), "{text}");
    for (line, prefix) in edges.iter().zip(want) {
        assert!(line.starts_with(prefix), "`{line}` statt `{prefix}`:\n{text}");
    }
}

/// Konjunkt 3: Solange im Fenster eines Wake-Stroms ein Element steht,
/// schlaeft das System nicht. Die Maschine laeuft alle 5 ms; was in Tick 3
/// eintrifft, sieht sie erst in Tick 5, und bis dahin bleibt das System
/// wach. Ab Tick 10 schlaeft sie wieder.
#[test]
fn a_full_wake_window_keeps_the_system_awake() {
    let p = compile(
        "\
input  bell : stream<u8> @ hw(\"bus/bell\") with capacity = 8, max_rate = 200 Hz, wake = true
output led  : bool @ hw(\"ui/led\") with safe = false

machine m every 5 ms:
    initial WAIT

    state WAIT idle:
        when bell as e: -> RUN

    state RUN:
        enter:
            led = true
        after 1 ms: -> WAIT
",
    );
    let awake = sleepy(&p, "t=3 in bell 7\n", 10);
    assert_eq!(awake, [true, true, false, false, false, false, false, false, false, true]);
}

/// Konjunkt 2: Eine geplante Ausgabe haelt das System wach, bis sie
/// faellig ist (9.8).
#[test]
fn a_scheduled_output_keeps_the_system_awake() {
    let p = compile(
        "\
output y : int in 0..9 @ hw(\"o/y\") with safe = 0

machine m:
    initial WAIT

    state WAIT idle:
        enter:
            at now + 3 ms:
                y = 1
",
    );
    let awake = sleepy(&p, "", 5);
    assert_eq!(awake, [false, false, true, true, true]);
}

/// Konjunkt 5: Ein laufender Job haelt das System wach (4.5, 9.9). Er
/// startet in Tick 0 und lebt in der Maschine weiter, als sie in den
/// `idle`-Zustand wechselt; seine Fertigstellung erscheint in Tick 3.
#[test]
fn a_running_job_keeps_the_system_awake() {
    let p = compile(
        "\
native job sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 640, duration = 3 ms, total

machine m:
    var msg : bytes<64> = default
    initial START

    state START:
        enter:
            job v = sha256(msg)
        when true: -> REST

    state REST idle:
        after 100 ms: -> START
",
    );
    let awake = sleepy(&p, "", 5);
    assert_eq!(awake, [false, false, true, true, true]);
}

/// Konjunkt 4: Ein Runtime-Fault, der hinter einem Abort wartet, bleibt
/// vorgemerkt (5.4) und haelt das System wach, bis er zugestellt ist.
#[test]
fn a_fault_behind_an_abort_keeps_the_system_awake() {
    let p = compile(
        "\
output led  : bool @ hw(\"ui/led\")  with safe = false
output lamp : bool @ hw(\"ui/lamp\") with safe = false

machine boss:
    fault -> DOWN
    initial RUN

    state RUN:
        loop:
            if time_in_state >= 2 ms:
                abort \"Halt\"

    state DOWN idle:
        enter:
            lamp = false

machine m every 5 ms:
    fault -> OFF
    initial WAIT

    state WAIT idle:
        enter:
            led = true

    state OFF idle:
        enter:
            led = false
",
    );
    // Tick 2: `boss` bricht ab, die inaktive `m` bekommt den Abort in der
    // Abort-Phase, der Treiberfehler aus demselben Tick wartet. Beide
    // stehen danach in `idle`-Zustaenden; nur er haelt das System wach.
    // Tick 3 stellt ihn zu, und `m` eskaliert nach `FAULTED` (5.4).
    let stimulus = "t=2 runtime Driver led\n";
    assert_eq!(sleepy(&p, stimulus, 3), [false, false, false]);
    let out = takt_interp::run(
        &p,
        &Trace::parse(stimulus).expect("Stimulus"),
        &RunOptions { ticks: 3, ..Default::default() },
    )
    .expect("Lauf");
    let text = out.trace.render();
    for line in ["t=2 state boss DOWN", "t=2 state m OFF", "t=3 state m FAULTED"] {
        assert!(text.contains(line), "`{line}` fehlt:\n{text}");
    }
}

/// Konjunkt 6: Was im Sendepuffer eines Ausgabestroms steht oder im
/// letzten Tick abgeholt wurde, haelt das System wach (8.8). Der Treiber
/// holt je Tick ein Byte, in Tick 0 das erste, in Tick 1 das zweite.
#[test]
fn a_filled_send_buffer_keeps_the_system_awake() {
    let p = compile(
        "\
output tx : stream<u8> @ hw(\"uart/tx\") with max_rate = 1000 Hz

machine m:
    initial WAIT

    state WAIT idle:
        enter:
            send tx, 1
            send tx, 2
",
    );
    let awake = sleepy(&p, "", 5);
    assert_eq!(awake, [false, true, true, true, true]);
}

/// Eine Uhr, die nur springt, wenn die Schleife wartet.
struct Virtual(i64);

impl Clock for Virtual {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Zaehlt die uebersprungenen Ticks.
#[derive(Default)]
struct Slept(u64);

impl Sink for Slept {
    fn record(&mut self, tick: &Tick) {
        self.0 += tick.slept;
    }
}

/// Der Trace eines Laufs in der Schleife aus 12.1 und die uebersprungenen Ticks.
fn looped(p: &Program, stimulus: &str, may_sleep: bool, ticks: u64) -> (String, u64) {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    let run = Run::new(p, &stimulus, &RunOptions { ticks, ..Default::default() }).expect("Lauf");
    let profile = Profile { may_sleep, ..Profile::LINUX_RT };
    let mut rt = Runtime::new(run, Virtual(0), Quiet, Slept::default(), profile, p.config.tick, Policy::default());
    while rt.tick_number() < ticks {
        rt.step();
    }
    let slept = rt.sink.0;
    (rt.program.finish().expect("Ergebnis").trace.render(), slept)
}

/// **Ein Operator-Abort weckt** (5.10): Das System schlaeft bis zu dem
/// Tick, in dem der Abort eintrifft, und die Maschine nimmt ihren
/// Fault-Pfad dort, wo sie es ohne Schlaf taete (Satz 9.9.1).
#[test]
fn an_operator_abort_wakes_the_sleeping_system() {
    let p = compile(
        "\
output led : bool @ hw(\"ui/led\") with safe = false

machine m:
    fault -> SAFE
    initial WAIT

    state WAIT idle:
        enter:
            led = true
        after 500 ms: -> WAIT

    state SAFE:
        enter:
            led = false
",
    );
    let stimulus = "t=40 abort\n";
    let (awake, none) = looped(&p, stimulus, false, 60);
    let (asleep, slept) = looped(&p, stimulus, true, 60);
    assert_eq!(none, 0);
    assert_eq!(awake, asleep, "Satz 9.9.1: der Schlaf ist im Trace sichtbar");
    assert!(awake.contains("t=40 state m SAFE"), "{awake}");
    assert_eq!(slept, 38, "geschlafen wird von Tick 1 bis vor den Abort:\n{asleep}");
}

/// Neben der schlafenden Maschine liest eine wache denselben Strom: Ihr
/// `dropped` bleibt 0, denn `dropped[s, m]` zaehlt je Maschine (9.6), und
/// was die schlafende verwirft, verliert nur sie.
#[test]
fn an_awake_reader_of_the_same_stream_drops_nothing() {
    let p = compile(
        "\
input  noise     : stream<u8> @ hw(\"bus/noise\") with capacity = 4, max_rate = 2000 Hz
output noise_sim : stream<u8> @ sim(\"bus/noise\")
output lost      : int in 0..999 @ hw(\"o/lost\") with safe = 0
output kept      : int in 0..999 @ hw(\"o/kept\") with safe = 0
output count     : int in 0..999 @ hw(\"o/count\") with safe = 0

machine feeder:
    initial GO
    state GO:
        loop:
            send noise_sim, 1

machine m:
    initial LISTEN

    state LISTEN:
        on noise as e:
            pass
        after 5 ms: -> SLEEP

    state SLEEP idle:
        after 10 ms: -> BACK

    state BACK:
        loop:
            lost = min(noise.dropped, 999)
        on noise as e:
            pass

machine reader:
    var seen : int in 0..999 = 0

    initial READ

    state READ:
        loop:
            kept = min(noise.dropped, 999)
            count = seen
        on noise as e:
            seen = min(seen + 1, 999)
",
    );
    let out = takt_interp::run(&p, &Trace::default(), &RunOptions { ticks: 20, ..Default::default() }).expect("Lauf");
    let text = out.trace.render();
    assert!(text.contains("t=15 out lost 9"), "{text}");
    let kept: Vec<&str> = text.lines().filter(|l| l.contains(" out kept ")).collect();
    assert_eq!(kept, ["t=0 out kept 0"], "{text}");
    // Der wache Leser sieht jedes Element: das in Tick k gesendete im
    // Handler von Tick k + 1, nach dem `loop:`, der `count` schreibt.
    assert!(text.contains("t=20 out count 19"), "{text}");
    // Die Zaehler am Puffer selbst (9.6) bleiben stehen.
    assert!(!text.contains("stream noise"), "{text}");
}

/// Ein Programm, das im `idle` auf einen Taster oder seine Frist wartet.
const NAP: &str = "\
input  wake     : bool @ hw(\"gpio/btn\") with wake = true
output wake_sim : bool @ sim(\"gpio/btn\") with safe = false
output led      : bool @ hw(\"ui/led\") with safe = false

machine m:
    initial SLEEP
    state SLEEP idle:
        enter:
            led = false
        when wake: -> RUN
        after 50 ms: -> RUN
    state RUN:
        enter:
            led = true
        after 20 ms: -> SLEEP
";

/// **Satz 9.9.1 je Weckgrund**: Die Frist von `after`, ein Wake-Input und
/// das Veralten einer Wake-Quelle (der Guard, der sie liest, faultet) wecken
/// das schlafende System genau dort, wo der Lauf ohne Schlaf weitergeht —
/// der Trace ist derselbe. Geschlafen wird in Stuecken bis zum moeglichen
/// Veralten der Quelle (`max_age` 2 ms, 3.5), also nie ueber einen Wecker.
#[test]
fn theorem_9_9_1_holds_for_each_wake_reason() {
    let p = compile(NAP);
    for (stimulus, woken) in [
        ("", "t=50 state m RUN"),
        ("t=10 in wake true\nt=11 in wake false\n", "t=10 state m RUN"),
        ("t=30 in wake stale age=100 ms\n", "t=30 fault m SensorFault \"`wake` ungueltig\" -> FAULTED"),
    ] {
        let (awake, none) = looped(&p, stimulus, false, 200);
        let (asleep, slept) = looped(&p, stimulus, true, 200);
        assert_eq!(none, 0, "{stimulus}");
        assert_eq!(awake, asleep, "Satz 9.9.1, `{stimulus}`");
        assert!(awake.contains(woken), "`{woken}`:\n{awake}");
        assert!(slept > 0, "`{stimulus}`: geschlafen wird");
    }
}

/// Konjunkt 1: Solange eine Maschine wach ist, schlaeft das System nicht,
/// auch wenn die andere im `idle` steht.
#[test]
fn one_awake_machine_keeps_the_system_awake() {
    let p = compile(&format!(
        "{NAP}
output n : int in 0..999 @ hw(\"o/n\") with safe = 0

machine busy:
    var k : int in 0..999 = 0
    initial RUN
    state RUN:
        loop:
            k = min(k + 1, 999)
            n = k
"
    ));
    let (awake, _) = looped(&p, "", false, 120);
    let (asleep, slept) = looped(&p, "", true, 120);
    assert_eq!(awake, asleep);
    assert_eq!(slept, 0, "{asleep}");
}

/// Ein `abort` merkt den Fault fuer die anderen Maschinen vor (`raised`,
/// 5.4); die Abort-Phase stellt ihn im selben Tick zu, auch einer
/// schlafenden Maschine, und danach schlaeft das System wie ohne Schlaf.
#[test]
fn a_raised_abort_reaches_the_sleeping_machine_in_its_tick() {
    let p = compile(
        "\
output led  : bool @ hw(\"ui/led\")  with safe = false
output lamp : bool @ hw(\"ui/lamp\") with safe = true

machine boss:
    fault -> DOWN
    initial RUN

    state RUN:
        loop:
            if time_in_state >= 2 ms:
                abort \"Halt\"

    state DOWN idle:
        enter:
            lamp = false
        after 30 ms: -> RUN

machine m:
    fault -> OFF
    initial WAIT

    state WAIT idle:
        enter:
            led = true
        after 500 ms: -> WAIT

    state OFF idle:
        enter:
            led = false
        after 10 ms: -> WAIT
",
    );
    let (awake, _) = looped(&p, "", false, 60);
    let (asleep, slept) = looped(&p, "", true, 60);
    assert_eq!(awake, asleep, "Satz 9.9.1");
    for line in ["t=2 state boss DOWN", "t=2 state m OFF", "t=12 state m WAIT", "t=32 state boss RUN"] {
        assert!(awake.contains(line), "`{line}` fehlt:\n{awake}");
    }
    // Geschlafen wird nur, wenn beide im `idle` stehen, bis zur naechsten
    // Frist: 3..11 (OFF -> WAIT in 12), 13..31 (DOWN -> RUN in 32), nach
    // dem zweiten Abort in 34 dann 35..43 und 45..59 bis zum Ende.
    assert_eq!(slept, 9 + 19 + 9 + 15, "{asleep}");
}
