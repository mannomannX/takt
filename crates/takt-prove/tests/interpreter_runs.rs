//! Laeufe des Interpreters aus Quelltext, fuer Regeln, die erst ein ganzes
//! Programm zeigt: Jobs (4.5), gescopte Instanzen (5.11, 5.12), `persist`
//! (5.9) und der Stimulus (`grammar/trace.md`).
//!
//! Sie stehen hier, weil `takt-prove` neben dem Interpreter auch das Sema
//! kennt; der Interpreter selbst darf es nicht, ohne den Abhaengigkeitsgraphen
//! umzudrehen. Was sie pruefen, ist Semantik des Interpreters, nicht des
//! Beweisers.

use takt_diag::Policy;
use takt_interp::nvm::Nvm;
use takt_interp::{RunOptions, Trace, Value, run};
use takt_mir::Program;
use takt_sema::{Build, Options};

fn compile(src: &str) -> Program {
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None, ..Default::default() };
    let out = takt_sema::compile(src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

fn trace_with(p: &Program, stimulus: &str, options: RunOptions) -> String {
    let stimulus = Trace::parse(stimulus).expect("Stimulus");
    run(p, &stimulus, &options).unwrap_or_else(|e| panic!("Lauf: {e:?}")).trace.render()
}

fn trace(p: &Program, stimulus: &str, ticks: u64) -> String {
    trace_with(p, stimulus, RunOptions { ticks, ..Default::default() })
}

/// `mode` in jedem Tick bis `ticks`, eins in `off`, sonst null: Ein Input
/// ohne Lieferung veraltet und faultet den Leser.
fn modes(ticks: u64, off: std::ops::Range<u64>) -> String {
    (0..ticks).map(|k| format!("t={k} in mode {}\n", u8::from(off.contains(&k)))).collect()
}

/// Zwei Jobs nacheinander auf einem Handle, 25 ms bei 10 ms Tick: Jeder
/// ist fruehestens drei Ticks nach seinem Start fertig (4.5).
const JOBS: &str = "system:
    language = 1
    tick = 10 ms

native job sha256(b: bytes<64>) -> bytes<32> with cost = 60000, stack = 640, duration = 25 ms, total

output first : bool @ hw(\"o/first\") with safe = false
output second : bool @ hw(\"o/second\") with safe = false

machine m:
    var msg : bytes<64> = default
    initial RUN
    state RUN:
        sequence:
            job v = sha256(msg)
            until v.done timeout 1 s -> STUCK
            first = true
            job v = sha256(msg)
            until v.done timeout 1 s -> STUCK
            second = true
            -> DONE
    state DONE:
        when false: -> RUN
    state STUCK:
        when false: -> RUN
";

fn done_ticks(trace: &str) -> Vec<&str> {
    trace.lines().filter(|l| l.ends_with(" job m v done")).map(|l| l.split(' ').next().unwrap_or("")).collect()
}

/// SEM2-005: Eine Aufzeichnung ersetzt den Tick des Modells nur, wenn der
/// Job seine Dauer ueberschritt (4.5: „fruehestens im Tick
/// `start + ceil(duration/T0)`"). Eine fruehere Zeile haelt das Ergebnis
/// nicht frueher frei.
#[test]
fn a_recorded_completion_never_comes_before_the_duration() {
    let p = compile(JOBS);
    let t = trace(&p, "t=1 job m v done\n", 10);
    assert_eq!(done_ticks(&t).first(), Some(&"t=3"), "{t}");
}

/// SEM2-005: Die Aufzeichnung des ersten Jobs gehoert nicht dem zweiten,
/// der auf demselben Handle danach startet.
#[test]
fn a_second_job_does_not_take_the_record_of_the_first() {
    let p = compile(JOBS);
    // Der erste ueberschreitet seine Dauer und wird in Tick 4 fertig; der
    // zweite startet danach und haelt seine eigenen drei Ticks.
    let t = trace(&p, "t=4 job m v done\n", 12);
    let done = done_ticks(&t);
    assert_eq!(done.first(), Some(&"t=4"), "{t}");
    let second: u64 = done.get(1).and_then(|d| d.strip_prefix("t=")).and_then(|d| d.parse().ok()).expect("zweiter");
    assert!(second >= 7, "der zweite Job endet vor seiner Dauer:\n{t}");
}

/// Ein Template mit einem geplanten Schreibvorgang und einer `persist`-
/// Variablen; der Besitzer betritt und verlaesst seinen Zustand nach
/// `mode`.
const SCOPED: &str = "system:
    language = 1
    tick = 10 ms

input mode : int in 0..1 @ sim(\"i/mode\")

output a : bool @ hw(\"o/a\") with safe = false
output n : int in 0..100 @ hw(\"o/n\") with safe = 0

machine late(o: output bool, k: output int in 0..100):
    persist var count : int in 0..100 = 0
    initial WAIT
    state WAIT:
        enter:
            if count < 100:
                count = count + 1
            k = count
            at now + 50 ms:
                o = true
        loop:
            k = count

machine owner:
    initial ON
    state ON:
        instance x = late(o = a, k = n)
        when mode == 1: -> OFF
    state OFF:
        when mode == 0: -> ON
";

/// SEM2-070: 5.11 „sched, Jobs und Trigger von inst werden verworfen".
/// Was die Instanz vor ihrem Austritt plante, darf `safe` danach nicht
/// ueberschreiben.
#[test]
fn leaving_a_scope_drops_the_scheduled_writes_of_its_instance() {
    let p = compile(SCOPED);
    let t = trace(&p, &modes(12, 1..12), 12);
    assert!(t.contains("out n 1"), "die Instanz lief nicht:\n{t}");
    assert!(!t.lines().any(|l| l.contains(" out a true")), "der geplante Wert kam nach dem Austritt:\n{t}");
}

/// SEM2-010: `persist`-Variablen liegen in Sigma und ueberdauern jeden
/// Zustandswechsel (11.5, 5.9) — auch den Austritt einer gescopten
/// Instanz; der Wiedereintritt initialisiert nur die uebrigen Variablen.
#[test]
fn a_persist_variable_outlives_the_scope_of_its_instance() {
    let p = compile(SCOPED);
    let t = trace(&p, &modes(8, 2..4), 8);
    assert!(t.contains("out n 2"), "der Zaehler begann beim Wiedereintritt von vorn:\n{t}");
}

/// KON2-023: `instance x resume` behaelt die Konfiguration ueber
/// Deaktivierungen (5.11, 5.12).
#[test]
fn a_resumed_instance_continues_in_its_last_leaf() {
    let p = compile(
        "system:
    language = 1
    tick = 10 ms

input mode : int in 0..1 @ sim(\"i/mode\")

output stage : int in 0..2 @ hw(\"o/stage\") with safe = 0

machine walker(o: output int in 0..2):
    initial ONE
    state ONE:
        enter:
            o = 1
        after 20 ms: -> TWO
    state TWO:
        enter:
            o = 2

machine owner:
    initial ON
    state ON:
        instance x resume = walker(o = stage)
        when mode == 1: -> OFF
    state OFF:
        when mode == 0: -> ON
",
    );
    // TWO nach 20 ms, dann Austritt und Wiedereintritt des Besitzers.
    let t = trace(&p, &modes(12, 5..7), 12);
    let after: Vec<&str> =
        t.lines().filter(|l| l.contains(" out stage ")).skip_while(|l| !l.ends_with(" out stage 0")).collect();
    assert!(after.len() >= 2, "kein Austritt und Wiedereintritt:\n{t}");
    assert!(after[1].ends_with(" out stage 2"), "die Instanz begann wieder bei `initial`:\n{t}");
}

/// KON2-021: Ein Eintrag, dessen Bytes sich nicht dekodieren lassen (hier
/// das `bool`-Byte 2), ist ungueltig und kein fehlender: Default plus
/// `PersistReset` (5.9), wie der erzeugte Restore es meldet.
#[test]
fn an_undecodable_persist_entry_resets_with_an_alert() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/35_persist.takt");
    let p = compile(&std::fs::read_to_string(path).expect("lesbar"));
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let pv = m.persist.iter().find(|pv| m.vars[pv.var.index()].name == "health").expect("health");
    let health = Value::Record(vec![Value::Bool(true), Value::Int(7)]);
    let mut payload = Nvm::payload(&p, &[(pv.type_hash, &health, m.vars[pv.var.index()].ty)]).expect("kodierbar");
    // Hash (8 Byte), Laenge (4 Byte), dann das `bool` des Records.
    payload[12] = 2;
    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let t = trace_with(&p, "", RunOptions { ticks: 2, nvm, ..Default::default() });
    assert!(t.contains("PersistReset"), "kein Alert:\n{t}");
    assert!(t.contains("t=0 out code 42"), "nicht der Default:\n{t}");
}

/// SEM2-056, SEM2-057: Ein Stimulus oder Parameter in fremder Einheit ist
/// kein Wert des Typs; der Lauf lehnt ihn ab, statt `psi` als `bar` zu
/// lesen.
#[test]
fn a_value_in_another_unit_is_refused() {
    let p = compile(
        "system:
    language = 1
    tick = 10 ms

param GAIN : float[bar] = 1.0 bar

input p : float[bar] @ sim(\"i/p\")

output q : float[bar] @ hw(\"o/q\") with safe = 0.0 bar

machine m:
    initial RUN
    state RUN:
        loop:
            q = p.or(GAIN)
",
    );
    let stimulus = Trace::parse("t=0 in p 5 psi\n").expect("Zeile");
    assert!(run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }).is_err(), "5 psi als bar gelesen");
    let options = RunOptions { ticks: 2, overrides: vec![("GAIN".into(), "5 psi".into())], ..Default::default() };
    assert!(run(&p, &Trace::default(), &options).is_err(), "GAIN = 5 psi als bar gelesen");
    let options = RunOptions { ticks: 2, overrides: vec![("GAIN".into(), "NaN".into())], ..Default::default() };
    assert!(run(&p, &Trace::default(), &options).is_err(), "GAIN = NaN angenommen");
}

/// SEM2-029: NaN gibt es nicht (4.1). Ohne Range liess der Rand ihn durch,
/// und `.max()` endete in einem internen Fehler statt in einem Urteil.
#[test]
fn a_nan_sample_never_reaches_a_reduction() {
    let p = compile(
        "system:
    language = 1
    tick = 1 ms

input i_dut : samples<float[A], 4> @ hw(\"daq1/ai2\") with rate = 4 kHz

output peak : float[A] @ hw(\"o/peak\") with safe = 0.0 A

machine watch:
    initial RUN
    state RUN:
        loop:
            peak = i_dut.max()
",
    );
    let stimulus = Trace::parse("t=0 in i_dut [NaN A, 1.0 A]\n").expect("Zeile");
    match run(&p, &stimulus, &RunOptions { ticks: 2, ..Default::default() }) {
        Err(takt_interp::Trap::Bug(e)) => assert!(e.contains("4.1"), "nicht als Stimulus abgelehnt: {e}"),
        other => panic!("NaN angenommen: {other:?}"),
    }
}

/// INT-004: Ein `{d:float}` traegt den Typ `float` in Programmbreite; in
/// einem `f32`-Programm las der Interpreter ihn als `f64`, und jede
/// Rechnung mit ihm endete in einem internen Fehler.
#[test]
fn a_float_capture_has_the_width_of_the_program() {
    let p = compile(
        "system:
    language = 1
    tick = 10 ms
    float = f32

input rx : stream<line<32>> @ hw(\"uart0/rx\") with max_rate = 100 Hz, framing = lines, capacity = 4

output out : float @ hw(\"o/out\") with safe = 0.0

machine m:
    initial RUN
    state RUN:
        on rx matches \"d={d:float}\" as ev:
            out = ev.d + 1.0
",
    );
    let t = trace(&p, "t=1 in rx \"d=0.1\"\n", 3);
    let want = 0.1f32 + 1.0f32;
    assert!(t.contains(&format!("t=1 out out {}", takt_interp::trace::float32_text(want))), "{t}");
}

/// Ein Strom von Records in kanonischer Byteform (8.6, 5.9) und ein
/// Handler, der zaehlt, was er sieht.
const PAIRS: &str = "system:
    language = 1
    tick = 10 ms

record Pair:
    a : u8
    b : bool

input pairs : stream<Pair> @ hw(\"bus/rx\") with max_rate = 100 Hz, capacity = 4

output seen : int in 0..100 @ hw(\"o/seen\") with safe = 0
output last : u8 @ hw(\"o/last\") with safe = 0

machine m:
    var n : int in 0..100 = 0
    initial RUN
    state RUN:
        on pairs as e:
            if n < 100:
                n = n + 1
            seen = n
            last = e.data.a
";

/// INT-014: Ein Element, dessen Bytes sich nicht dekodieren lassen (das
/// `bool`-Byte 2), wird verworfen und zaehlt `malformed` (12.6, Zeile 5);
/// es verbraucht keine Folgenummer, das naechste kommt an.
#[test]
fn an_undecodable_element_counts_as_malformed() {
    let p = compile(PAIRS);
    let t = trace(&p, "t=1 in pairs 0x0102\nt=2 in pairs 0x0701\n", 4);
    assert!(t.contains("t=1 stream pairs dropped=0 overflowed=0 malformed=1"), "{t}");
    assert!(t.contains("t=2 out seen 1") && t.contains("t=2 out last 7"), "{t}");
}

/// INT-014: Im Tick 0 laeuft jede Maschine im Modus ENTRY, und ihr Fenster
/// ist leer (9.6); das Element bleibt liegen und erscheint im naechsten
/// Tick.
#[test]
fn the_entry_tick_sees_an_empty_window() {
    let p = compile(PAIRS);
    let t = trace(&p, "t=0 in pairs 0x0501\n", 3);
    assert!(!t.contains("t=0 out seen 1"), "der Handler lief im Eintrittstick:\n{t}");
    assert!(t.contains("t=1 out seen 1") && t.contains("t=1 out last 5"), "{t}");
}

/// Ein Zaehler, der mit dem gespeicherten `start` beginnt.
const STORED: &str = "system:
    language = 1
    tick = 10 ms

output n : int in 0..9999 @ hw(\"o/n\") with safe = 0

machine count:
    persist var start : int in 0..99 = 0
    var k : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            k = min(k + 1, 99)
            n = start * 100 + k
";

/// SEM2-045: s0 der `persist`-Variablen steht im Kopf der Aufzeichnung
/// (12.5), und der Lauf aus ihr allein gibt einen Lauf wieder, der mit
/// gefuelltem Speicher begann — auch mit einem Eintrag, dessen Bytes keine
/// kanonische Form sind und der darum als Default mit Alert laedt.
#[test]
fn a_recording_carries_the_store_the_run_began_with() {
    use takt_interp::record::{Header, Recording};
    let p = compile(STORED);
    let key = p.machines[0].persist[0].type_hash;
    let mut filled = Nvm::new();
    filled.put(key, Value::Int(7));
    let mut broken = Nvm::new();
    let payload: Vec<u8> = key.to_le_bytes().into_iter().chain(1u32.to_le_bytes()).chain([0xFF]).collect();
    broken.from_program_payload(&p, &payload);
    for (nvm, want) in [(filled, "t=0 out n 701\n"), (broken, "t=0 out n 1\n")] {
        let first =
            run(&p, &Trace::default(), &RunOptions { ticks: 3, nvm: nvm.clone(), ..Default::default() }).expect("Lauf");
        let first = first.trace.render();
        assert!(first.contains(want), "{first}");
        let header = Header::of(&p, None, &[], 3).with_store(&p, &nvm);
        let text = Recording { header, inputs: Trace::default() }.render();
        let rec = Recording::parse(&text).expect("lesbar");
        let options =
            RunOptions { ticks: rec.header.ticks, nvm: rec.header.store(&p).expect("Speicher"), ..Default::default() };
        let again = run(&p, &rec.inputs, &options).expect("Wiedergabe").trace.render();
        assert_eq!(again, first, "{text}");
    }
}

/// SEM1-049 (8.6, Vierte Runde): `s.count` zaehlt das Fenster und ist im
/// Modus ENTRY null — in `enter:` und im `loop:` des Eintritts-Ticks —, im
/// naechsten Tick die Elemente, die das Fenster sieht.
#[test]
fn the_count_of_a_stream_is_zero_in_entry_mode() {
    let p = compile(
        "system:
    language = 1
    tick = 1 ms

input rx : stream<u8> @ hw(\"u/rx\") with max_rate = 200 kHz, capacity = 256
output n : int in 0..99 @ hw(\"o/n\") with safe = 0
output k : int in 0..99 @ hw(\"o/k\") with safe = 0

machine m:
    initial RUN
    state RUN:
        enter:
            n = min(rx.count, 99) + 1
        loop:
            k = min(rx.count, 99) + 1
",
    );
    let t = trace(&p, "t=0 in rx 1\nt=0 in rx 2\nt=0 in rx 3\nt=1 in rx 4\nt=1 in rx 5\n", 2);
    assert!(t.contains("t=0 out n 1\n") && t.contains("t=0 out k 1\n"), "im Eintritts-Tick leer:\n{t}");
    // Im Eintritts-Tick sah niemand die drei Elemente, also blieben sie
    // (9.6, Eviction); in Tick 1 zaehlt das Fenster sie mit den zwei neuen.
    assert!(t.contains("t=1 out k 6\n"), "fuenf Elemente in Tick 1:\n{t}");
}

/// INT-025 (3.7, 5.9, 8.6): Ein NaN- oder Inf-Bitmuster in einem
/// Gleitkommafeld ist ungueltig — `decode` eines `layout`-Records liefert
/// `none`, ein Stromelement in kanonischer Form ist `malformed`, ein
/// `persist`-Eintrag laedt als Default mit `PersistReset`.
#[test]
fn a_non_finite_float_field_is_no_value() {
    let p = compile(
        "system:
    language = 1
    tick = 10 ms

record Sample layout little:
    a : f32
    b : f64

record Point:
    v : float

input rx : stream<Point> @ hw(\"u/rx\") with max_rate = 1000 Hz, capacity = 16
output ok : int in 0..99 @ hw(\"o/ok\") with safe = 0
output got : int in 0..99 @ hw(\"o/got\") with safe = 0

machine m:
    persist var level : float = 1.5
    var n : int in 0..99 = 0
    initial RUN
    state RUN:
        loop:
            var good : bytes<12> = [0, 0, 0x80, 0x3F, 0, 0, 0, 0, 0, 0, 0, 0x40]
            var nan : bytes<12> = [0, 0, 0x80, 0x3F, 0, 0, 0, 0, 0, 0, 0xF8, 0x7F]
            var inf : bytes<12> = [0, 0, 0x80, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0x40]
            var k = 0
            if Sample.decode(good).valid:
                k = k + 1
            if Sample.decode(nan).valid:
                k = k + 10
            if Sample.decode(inf).valid:
                k = k + 20
            ok = k
        on rx as e:
            n = min(n + 1, 99)
            got = n
",
    );
    let key = p.machines[0].persist[0].type_hash;
    let payload: Vec<u8> =
        key.to_le_bytes().into_iter().chain(8u32.to_le_bytes()).chain(f64::NAN.to_bits().to_le_bytes()).collect();
    let mut nvm = Nvm::new();
    nvm.from_program_payload(&p, &payload);
    let stimulus = format!(
        "t=1 in rx 0x{}\nt=1 in rx 0x{}\nt=1 in rx 0x{}\n",
        hex(&1.5f64.to_bits().to_le_bytes()),
        hex(&f64::NAN.to_bits().to_le_bytes()),
        hex(&f64::NEG_INFINITY.to_bits().to_le_bytes())
    );
    let t = trace_with(&p, &stimulus, RunOptions { ticks: 3, nvm, ..Default::default() });
    assert!(t.contains("t=0 out ok 1\n"), "nur der endliche Record:\n{t}");
    assert!(t.contains("PersistReset") || t.contains("persist"), "NaN im Speicher wird verworfen:\n{t}");
    assert!(t.contains("malformed=2"), "zwei Elemente verworfen:\n{t}");
    assert!(t.contains("out got 1\n") && !t.contains("out got 2\n"), "eines kommt an:\n{t}");
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// KON2-017 (13.3): Ein Atom ueber einen Input, der ungueltig wird, zaehlt
/// als falsch, auch wenn ein alter Wert daneben liegt — die implizite
/// Pruefung `Checked{Valid}` um den Input faultet, und das Atom ist nicht
/// wahr. Eine Eigenschaft meldet ihre erste Verletzung; danach ist sie
/// entschieden und meldet keine weitere.
#[test]
fn an_atom_over_an_invalid_input_is_false() {
    let p = compile(
        "system:
    language = 1
    tick = 10 ms

input level : int in 0..100 @ hw(\"i/level\") with max_age = 1 s
output o : bool @ hw(\"o/o\") with safe = false

machine m:
    initial A
    state A:
        loop:
            o = true

property sensor: always(level > 10) with monitor = true
",
    );
    let t = trace(&p, "t=0 in level 50\nt=5 in level bad reason=Driver\nt=7 in level 50\nt=9 in level 5\n", 12);
    assert!(t.contains("t=5 property sensor violated 5\n"), "{t}");
    assert_eq!(t.matches("property sensor violated").count(), 1, "nur die erste Verletzung:\n{t}");
}

/// SEM2-008: Ein Stimulus, der nicht zum Programm passt, ist eine Eingabe
/// mit Zeilennummer (`read_stimulus`, `StimulusError`), kein interner
/// Fehler des Interpreters. Kommentare und Leerzeilen zaehlen mit.
#[test]
fn an_invalid_stimulus_names_its_line() {
    use takt_interp::{StimulusError, read_stimulus};
    let p = compile(JOBS);
    let error = |text: &str| read_stimulus(&p, text, None).err().map(|StimulusError { line, message }| (line, message));
    let (line, message) = error("t=1 job nope v done\n").expect("abgelehnt");
    assert_eq!(line, 1, "{message}");
    assert!(message.contains("nope"), "{message}");
    let (line, message) = error("# Kopf\n\nt=1 job m v done\nt=2 in gibtsnicht 3\n").expect("abgelehnt");
    assert_eq!((line, message.contains("gibtsnicht")), (4, true), "{message}");
    assert!(error("t=1 job m w done\n").is_some_and(|(l, m)| l == 1 && m.contains("`w`")), "fremdes Handle");
    assert!(error("t=1 cmd los\n").is_some(), "unbekanntes Command");
    assert!(error("t=1 runtime Blitz\n").is_some(), "unbekannter Runtime-Fault");
    assert!(error("t=1 job m v\n").is_some(), "Zeile ohne `done`");
    let ok = read_stimulus(&p, "t=1 job m v done\nt=4 abort\n", None).expect("gueltig");
    assert_eq!(ok.lines.len(), 2);
}

/// KOR-011 (8.4): Ein unbekanntes Profil und eine Ueberlagerung daneben
/// sind Fehler der Eingabe (`check_params`), mit dem Namen dessen, was nicht
/// passt. Einen gefalteten Profilwert ausserhalb der Range lehnt schon die
/// Sema ab (SC-3, takt-sema tests/profile_values.rs).
#[test]
fn a_profile_that_does_not_fit_is_an_input_error() {
    use takt_interp::system::check_params;
    let p = compile(
        "system:
    language = 1
    tick = 10 ms

const K : int = 6
param P : int in 0..10 = 1

profile FINE:
    P = K + 1

output o : int @ hw(\"o/o\") with safe = 0

machine m:
    initial A
    state A:
        loop:
            o = P
",
    );
    assert_eq!(check_params(&p, Some("FINE"), &[]), Ok(()));
    assert!(check_params(&p, Some("NOPE"), &[]).is_err_and(|e| e.contains("NOPE")));
    assert!(check_params(&p, None, &[("P".into(), "11".into())]).is_err_and(|e| e.contains("P = 11")));
    assert!(check_params(&p, None, &[("Q".into(), "1".into())]).is_err_and(|e| e.contains("`Q`")));
    assert!(check_params(&p, None, &[("P".into(), "zehn".into())]).is_err());
}

/// SEM1-027 (13.7 neu): Ein leerer Laufraum ist ein Fehler, nie eine
/// bestandene Kampagne. Die Pruefung SC-3 weist `lo > hi` schon im
/// Quelltext ab; der Laufraum prueft dennoch selbst, hier an einer MIR, in
/// der die Grenzen nachtraeglich getauscht sind.
#[test]
fn an_empty_run_space_is_an_error() {
    let mut p = compile(
        "system:
    language = 1
    tick = 10 ms

param GAIN : int in 0..100 = 1
output o : int @ hw(\"o/o\") with safe = 0

machine m:
    initial A
    state A:
        loop:
            o = GAIN

campaign c:
    sweep GAIN = 1..5 step 2
    stop_on never
",
    );
    assert_eq!(takt_interp::campaign::runs(&p, &p.campaigns[0]).map(|r| r.len()), Ok(3));
    let Some(takt_mir::program::Sweep::Range { from, to, .. }) = p.campaigns[0].sweeps.first_mut() else {
        panic!("ein Bereichs-Sweep")
    };
    std::mem::swap(from, to);
    let e = takt_interp::campaign::runs(&p, &p.campaigns[0]).expect_err("leerer Laufraum");
    assert!(e.to_string().contains("`c`") && e.to_string().contains("`GAIN`") && e.to_string().contains("leer"), "{e}");
}
