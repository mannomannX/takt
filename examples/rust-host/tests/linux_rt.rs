//! `linux_rt` mit erzeugtem Code (12.2, 12.11, M11 Schritt 17): Programme
//! der Form `linux` im Tick-Faden des Ports aus `takt-rt-linux`, in
//! Echtzeit, jedes gleich dem Interpreter, die Zusage im Kopf des Laufs.
//!
//! **Allein und mit allen Kernen.** Der Test misst Echtzeit und ist darum in
//! `cargo test` ausgelassen; `crates/takt-cli/tests/linux_rt.rs` faehrt ihn
//! mit `--ignored`, und nextest gibt ihm den ganzen Rechner
//! (`threads-required`).

use std::time::{Duration, Instant};

use rust_host::{Seed, Tank, idle_multirate_rt, instance_rt, lifecycle_rt, valve_rt};
use takt_embed::Jobs as _;
use takt_embed::rt::{
    FakeNvm, Fits, Journal, NextRun, NoJournal, Outputs, Policy, Profile, Runtime, Sink, Store, Tick, Unloaded,
    Watchdog,
};
use takt_rt_linux::port::{self, Config, Served};
use takt_rt_linux::{Guarantee, RealtimeClock};

/// So viele Ticks faehrt jeder Lauf: vier Sekunden zu 10 ms.
const TICKS: u64 = 400;

/// Kein Watchdog: Ein Test hat keinen.
struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

/// Der Anfangszustand ganz, danach je Tick die Aenderungen, wie ein
/// Konformitaetslauf (9.3).
struct Conformance;

impl Sink for Conformance {
    fn outputs(&mut self, tick: Option<&Tick>) -> Outputs {
        if tick.is_some() { Outputs::Changed } else { Outputs::All }
    }

    fn record(&mut self, _tick: &Tick) {}
}

/// Ein Lauf im Tick-Faden: Kopf und Trace, die Zusage, die Bilanz des Ports
/// und wie lange er dauerte.
struct Run {
    text: String,
    guarantee: Guarantee,
    served: Served,
    took: Duration,
}

/// Faehrt das Programm, das `start` im Tick-Faden baut, mit seinem Journal
/// unter `linux_rt`: die Uhr des Profils, der Job-Kontext auf dem Job-Faden
/// des Ports. Der Kopf des Laufs nennt das Profil und die Zusage (11.3,
/// 12.2), die letzte Zeile, wie weit der Lauf reichte.
fn in_tick_thread<P, J>(config: &Config, tick_ns: i64, start: impl FnOnce() -> (P, J) + Send) -> Run
where
    P: takt_embed::Program,
    J: Store + Fits<P::Memory>,
{
    let began = Instant::now();
    let (text, guarantee, served) = port::spawn(config, |guarantee| {
        takt_embed::testing::take();
        let (mut program, journal) = start();
        let mut jobs = program.jobs().expect("der Job-Kontext gehoert dem Port");
        let clock = RealtimeClock::under(guarantee);
        let profile = Profile::LINUX_RT;
        let mut rt = Runtime::journaled(program, journal, clock, Quiet, Conformance, profile, tick_ns, Policy::Fault);
        let served = port::serve(&mut rt, Some(move || jobs.work()), config).expect("Job-Faden");
        let mut text = String::from("#! target linux_rt\n");
        for line in guarantee.header_lines() {
            text.push_str(&format!("#! runtime {line}\n"));
        }
        text.push_str(&takt_embed::testing::take());
        text.push_str(&format!("takt end {}\n", served.ticks));
        (text, guarantee.clone(), served)
    })
    .expect("Tick-Faden");
    Run { text, guarantee, served, took: began.elapsed() }
}

/// Die Kerne dieses Rechners.
fn cores() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from)
}

/// Der Lauf gleicht dem Interpreter und traegt die Zusage im Kopf; was ohne
/// Rechte fehlt, nennt die Zusage selbst.
fn check(name: &str, stimulus: &str, run: &Run) {
    let source = format!("{}/takt/{name}.takt", env!("CARGO_MANIFEST_DIR"));
    if let Err(e) = takt_embed::testing::same_as_interpreter_with(&source, TICKS, stimulus, &run.text) {
        panic!("{name}: {e}\nLauf:\n{}", run.text);
    }
    assert_eq!(run.served.overruns, 0, "{name}: kein Tick ueber seiner Periode");
    let head: Vec<&str> = run.text.lines().take_while(|l| l.starts_with("#! ")).collect();
    for line in run.guarantee.header_lines() {
        assert!(head.contains(&format!("#! runtime {line}").as_str()), "{name}: `{line}` fehlt im Kopf {head:?}");
    }
    let echtzeit = if run.guarantee.is_complete() { "ja" } else { "nein" };
    assert!(head.contains(&format!("#! runtime echtzeit {echtzeit}").as_str()), "{name}: {head:?}");
    assert_eq!(run.guarantee.is_complete(), run.guarantee.missing().is_empty(), "{name}: {:?}", run.guarantee);
    // Ohne Rechte gilt, was jeder Prozess darf: die Bindung an einen Kern
    // und die Trennung vom Job-Faden.
    assert_eq!(run.guarantee.cpus, 1, "{name}: an seinen Kern gebunden: {:?}", run.guarantee);
    assert_eq!(run.guarantee.apart, cores() > 1, "{name}: der Job-Faden rechnet daneben: {:?}", run.guarantee);
}

/// **Erzeugter Code rechnet im Tick-Faden unter `linux_rt` wie der
/// Interpreter** (12.2, 12.11, 13.1): vier Programme der Form `linux`
/// zugleich, jedes in seinem Tick-Faden auf seinem Kern, mit absoluten
/// Fristen in Echtzeit — `valve` mit Treiber, `instance` mit Eingang und
/// einem Job je Runde, `lifecycle` mit Journal, Job, Schlaf und `next_run`,
/// `idle_multirate` mit Schlaf ueber Maschinen eigener Periode. Jeder Trace
/// gleicht dem Interpreter, kein Tick lief ueber, und `valve` brauchte die
/// vier Sekunden seiner 400 Ticks, nicht mehr.
///
/// **Was ohne Rechte gilt.** `SCHED_FIFO` (`chrt`) und `mlockall`
/// (`RLIMIT_MEMLOCK`) braucht die Zusage aus 12.2; ein Runner hat sie nicht.
/// Ohne sie gelten Trace, absolute Fristen, Kernbindung und die Trennung des
/// Job-Fadens; der Kopf jedes Laufs sagt `echtzeit nein`, und die Zusage
/// nennt, was fehlt, mit seiner Abhilfe (die Ausgabe des Tests).
#[test]
#[ignore = "Echtzeit: laeuft allein aus crates/takt-cli/tests/linux_rt.rs"]
fn programs_of_the_linux_form_run_in_their_tick_threads_like_the_interpreter() {
    let core = |i: usize| i % cores();
    let ((valve, openings), (instance, seed), lifecycle, idle) = std::thread::scope(|s| {
        let valve = s.spawn(|| {
            let (mut arena, mut tank) = (valve_rt::Arena::new(), Tank::default());
            let config = Config::new(valve_rt::TICK_STACK_BYTES, valve_rt::JOB_STACK_BYTES).cpu(core(0)).ticks(TICKS);
            let run = in_tick_thread(&config, valve_rt::TICK_NS, || {
                (valve_rt::Program::init(&mut arena, &mut tank), NoJournal::default())
            });
            (run, tank.openings)
        });
        let instance = s.spawn(|| {
            let (mut arena, mut seed) = (instance_rt::Arena::new(), Seed::new(99));
            let config =
                Config::new(instance_rt::TICK_STACK_BYTES, instance_rt::JOB_STACK_BYTES).cpu(core(1)).ticks(TICKS);
            let run = in_tick_thread(&config, instance_rt::TICK_NS, || {
                (instance_rt::Program::init(&mut arena, &mut seed), NoJournal::default())
            });
            (run, seed)
        });
        let lifecycle = s.spawn(|| {
            let mut arena = lifecycle_rt::Arena::new();
            let (mut current, mut stored) = ([0u8; lifecycle_rt::PERSIST_BOUND], [0u8; lifecycle_rt::PERSIST_BOUND]);
            let config =
                Config::new(lifecycle_rt::TICK_STACK_BYTES, lifecycle_rt::JOB_STACK_BYTES).cpu(core(2)).ticks(TICKS);
            in_tick_thread(&config, lifecycle_rt::TICK_NS, || {
                let mut program = lifecycle_rt::Program::new(&mut arena);
                let device = FakeNvm::<256>::new();
                let journal = Journal::new(device, lifecycle_rt::LOGIC_HASH, lifecycle_rt::PERSIST_MIN_INTERVAL_NS);
                let (journal, ..) = Unloaded::new(journal, &mut current, &mut stored).load(&mut program);
                (program, journal)
            })
        });
        let idle = s.spawn(|| {
            let mut arena = idle_multirate_rt::Arena::new();
            let config = Config::new(idle_multirate_rt::TICK_STACK_BYTES, idle_multirate_rt::JOB_STACK_BYTES)
                .cpu(core(3))
                .ticks(TICKS);
            in_tick_thread(&config, idle_multirate_rt::TICK_NS, || {
                (idle_multirate_rt::Program::init(&mut arena), NoJournal::default())
            })
        });
        let join = "ein Lauf brach ab";
        (
            valve.join().expect(join),
            instance.join().expect(join),
            lifecycle.join().expect(join),
            idle.join().expect(join),
        )
    });

    for (name, run) in
        [("valve", &valve), ("instance", &instance), ("lifecycle", &lifecycle), ("idle_multirate", &idle)]
    {
        eprintln!("{name}: {:?}, {} ms", run.served, run.took.as_millis());
        for missing in run.guarantee.missing() {
            eprintln!("{name}: es fehlt {missing}");
        }
    }
    check("valve", "", &valve);
    let seed_stimulus: String = (0..TICKS).map(|k| format!("t={k} in seed 99\n")).collect();
    check("instance", &seed_stimulus, &instance);
    check("lifecycle", "", &lifecycle);
    check("idle_multirate", "", &idle);

    assert_eq!(openings, 1, "das Ventil oeffnete nach 3 s einmal");
    let t0 = Duration::from_nanos(u64::try_from(valve_rt::TICK_NS).expect("Periode"));
    let span = t0 * u32::try_from(TICKS).expect("Ticks");
    assert!(valve.took >= span - t0, "die Fristen wurden abgewartet: {:?}", valve.took);
    assert!(valve.took < span + Duration::from_secs(1), "absolute Fristen holen jeden Verzug auf: {:?}", valve.took);
    assert_eq!(seed.thread.as_deref(), Some("takt-tick"), "der Schritt rechnet im Tick-Faden");
    assert_eq!(instance.served.jobs, 29, "ein Job je Runde auf dem Job-Faden");
    assert_eq!(lifecycle.served.ended, Some(NextRun::After(5_000_000_000)), "`next_run` beendet den Lauf");
    assert!(lifecycle.served.flushed, "das Journal schrieb am Ende des Laufs");
    assert!(lifecycle.served.jobs > 0, "der Job von `lifecycle` rechnete auf dem Job-Faden");
}
