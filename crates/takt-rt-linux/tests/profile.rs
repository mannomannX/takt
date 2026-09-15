//! Das Profil `linux_rt` (12.2, 12.8).
//!
//! Die Tests laufen auf jedem Rechner: Was Linux-spezifisch ist, prueft
//! den Befund (`Guarantee`), nicht die Zusage — auf einem Nicht-Linux
//! meldet er `Unknown`, und genau das soll er.

use takt_rt_core::{Clock, Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};
use takt_rt_linux::{Guarantee, RealtimeClock, Scheduling, prepare};

/// 12.2: Die drei Teile der Zeitgarantie gelten zusammen oder gar nicht.
///
/// Ein Thread mit Echtzeitprioritaet auf einem Kern, dessen Seiten
/// ausgelagert werden koennen, hat sie nicht: Ein Seitenfehler kostet
/// Millisekunden.
#[test]
fn the_guarantee_holds_only_when_all_three_parts_do() {
    let full = Guarantee { scheduling: Scheduling::Realtime { priority: 80 }, locked: true, cpus: 1 };
    assert!(full.is_complete());
    assert!(full.missing().is_empty());

    for teil in [
        Guarantee { scheduling: Scheduling::Normal, locked: true, cpus: 1 },
        Guarantee { scheduling: Scheduling::Realtime { priority: 80 }, locked: false, cpus: 1 },
        Guarantee { scheduling: Scheduling::Realtime { priority: 80 }, locked: true, cpus: 4 },
    ] {
        assert!(!teil.is_complete(), "{teil:?} sollte unvollstaendig sein");
        assert_eq!(teil.missing().len(), 1, "genau ein Teil fehlt: {teil:?}");
    }
}

/// Ein Befund ohne Abhilfe ist eine Klage: Jede Meldung nennt den Weg.
#[test]
fn every_missing_part_names_its_remedy() {
    let nothing = Guarantee { scheduling: Scheduling::Normal, locked: false, cpus: 8 };
    let missing = nothing.missing();
    assert_eq!(missing.len(), 3);
    assert!(missing.iter().any(|m| m.contains("chrt")), "die Prioritaet nennt `chrt`: {missing:?}");
    assert!(missing.iter().any(|m| m.contains("RLIMIT_MEMLOCK")), "`mlockall` nennt das Limit: {missing:?}");
    assert!(missing.iter().any(|m| m.contains("taskset")), "die Bindung nennt `taskset`: {missing:?}");
}

/// 12.2: Das Programm setzt die Prioritaet *nicht* selbst, und die
/// Meldung sagt warum — sonst versucht es der naechste.
#[test]
fn the_message_explains_why_the_program_does_not_set_the_priority() {
    let ohne = Guarantee { scheduling: Scheduling::Normal, locked: true, cpus: 1 };
    let text = ohne.missing().join(" ");
    assert!(text.contains("CAP_SYS_NICE"), "der Grund fehlt: {text}");
}

/// 11.3: Der Befund geht in den Lauf-Header — der Unterschied zwischen
/// „lief unter `linux_rt`" und „lief unter `linux_rt` *mit* der Zusage"
/// steht in der Aufzeichnung, nicht in der Erinnerung des Bedieners.
#[test]
fn the_guarantee_goes_into_the_run_header() {
    let g = Guarantee { scheduling: Scheduling::Realtime { priority: 80 }, locked: true, cpus: 1 };
    let lines = g.header_lines();
    assert!(lines.iter().any(|z| z == "scheduling fifo"));
    assert!(lines.iter().any(|z| z == "echtzeit ja"));

    let ohne = Guarantee { scheduling: Scheduling::Normal, locked: true, cpus: 1 };
    assert!(ohne.header_lines().iter().any(|z| z == "echtzeit nein"), "die fehlende Zusage steht im Kopf");
}

/// `prepare` laeuft auf jedem Rechner und meldet, was es vorgefunden hat.
#[test]
fn prepare_reports_what_it_found() {
    let g = prepare(None);
    // Auf einem Nicht-Linux ist die Klasse unbekannt, und das ist die
    // richtige Antwort: Die Zusagen aus 12.2 gibt es dort nicht.
    if cfg!(not(target_os = "linux")) {
        assert_eq!(g.scheduling, Scheduling::Unknown);
        assert!(!g.is_complete(), "ohne Linux keine Zeitgarantie");
    }
}

// --- Die Uhr (12.2, 7.1) ------------------------------------------------

/// Die Uhr laeuft vorwaerts und beginnt bei null.
#[test]
fn the_clock_starts_at_zero_and_moves_forward() {
    let c = RealtimeClock::new();
    let a = c.now();
    assert!(a >= 0, "die Uhr beginnt beim Start des Laufs");
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert!(c.now() > a, "die Uhr laeuft");
}

/// 12.2: absolute Deadlines — ein zu spaeter Tick verschiebt die
/// folgenden nicht.
///
/// Geprueft wird die Wirkung, nicht der Syscall: Nach drei Fristen im
/// Abstand von je 5 ms ist die Uhr bei mindestens 15 ms, nicht bei
/// 15 ms plus dreimal Verzug.
#[test]
fn absolute_deadlines_do_not_accumulate_drift() {
    let mut c = RealtimeClock::new();
    const STEP: i64 = 5_000_000;
    for k in 1..=3 {
        c.wait_until(STEP * k);
    }
    let verstrichen = c.now();
    assert!(verstrichen >= STEP * 3, "zu frueh: {verstrichen}");
    // Grosszuegig, weil ein gewoehnlicher Scheduler dazwischenkommt —
    // ohne `SCHED_FIFO` ist das der Normalfall, und der Test soll die
    // Drift pruefen, nicht die Maschine.
    assert!(verstrichen < STEP * 3 + 50_000_000, "zu viel Drift: {verstrichen}");
}

/// 7.3: Eine Frist, die schon vergangen ist, wird nicht nachgeholt — der
/// Tick ist ueberfaellig, und der Zaehler sagt es.
#[test]
fn an_overdue_deadline_is_counted_not_awaited() {
    let mut c = RealtimeClock::new();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let before = c.now();
    c.wait_until(1_000);
    assert!(c.now() - before < 1_000_000, "eine vergangene Frist wird nicht abgewartet");
    assert_eq!(c.late, 1, "sie wird gezaehlt");
}

// --- Die Schleife mit echter Uhr ----------------------------------------

#[derive(Default)]
struct Counter(u32);

impl Program for Counter {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.0 += 1;
    }
}

#[derive(Default)]
struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn kick(&mut self) {}
}

#[derive(Default)]
struct Silent;

impl Sink for Silent {
    fn record(&mut self, _t: &Tick) {}
}

/// Die Tickschleife aus 12.1 laeuft mit der echten Uhr.
///
/// Das ist die Naht, auf die es ankommt: `takt-rt-core` kennt die Zeit
/// nur ueber `Clock`, und `linux_rt` liefert sie. Zehn Ticks zu je 1 ms
/// dauern mindestens 10 ms.
#[test]
fn the_tick_loop_runs_on_the_real_clock() {
    const T0: i64 = 1_000_000;
    let clock = RealtimeClock::new();
    let mut rt = Runtime::new(Counter::default(), clock, NoWatchdog, Silent, Profile::LINUX_RT, T0, Policy::Fault);
    let start = std::time::Instant::now();
    rt.run(10);
    assert_eq!(rt.program.0, 10, "zehn Ticks");
    assert!(start.elapsed() >= std::time::Duration::from_millis(9), "die Schleife wartet");
}

/// 11.3 und 12.2: Der Befund steht als Zeilen im Lauf-Header und
/// ueberlebt Schreiben und Lesen.
///
/// Das ist die Naht, auf die es ankommt: `takt-interp` kennt das Profil
/// nicht (der Interpreter laeuft ueberall), also traegt der Kopf Zeilen,
/// die die Runtime liefert.
#[test]
fn the_guarantee_survives_the_recording() {
    let g = Guarantee { scheduling: Scheduling::Normal, locked: false, cpus: 8 };
    let mut header = takt_interp::record::Header {
        version: 1,
        edition: 1,
        logic: "0".repeat(64),
        tick: 1_000_000,
        ticks: 10,
        profile: None,
        params: Vec::new(),
        target: Some("linux_rt".into()),
        runtime: g.header_lines(),
        natives: Vec::new(),
    };
    let text = header.render();
    assert!(text.contains("#! runtime echtzeit nein"), "der Befund fehlt: {text}");

    let gelesen = takt_interp::record::Header::parse(&text).expect("lesbar");
    assert_eq!(gelesen.runtime, header.runtime, "die Zeilen ueberleben nicht");

    // Und mit Zusage ebenso.
    header.runtime =
        Guarantee { scheduling: Scheduling::Realtime { priority: 80 }, locked: true, cpus: 1 }.header_lines();
    let text = header.render();
    assert!(text.contains("#! runtime echtzeit ja"), "{text}");
}
