//! Ein Takt-Programm auf der MCU (M5, 12.1, 12.3).
//!
//! **Das einzige Programm dieser Reihe, das wirklich Takt ausfuehrt.**
//! `blink` schaltet einen Pin, `minimal` prueft die Tickquelle, `tick`
//! misst sie — alle drei sind Rust. Dieses hier fuehrt den Code aus, den `takt-llvm` aus einer
//! `.takt`-Datei erzeugt hat, unter der Tickschleife aus
//! `takt-rt-core`.
//!
//! ## Wie es entsteht
//!
//! Drei Teile werden gebunden:
//!
//! 1. **Der erzeugte Code** — `takt-llvm` uebersetzt das Programm nach
//!    LLVM-IR, clang macht daraus ein Objekt fuer `thumbv7em`.
//! 2. **Der Rahmen** — `takt-conformance::mcu` erzeugt das C-Stueck, das
//!    Prozessabbild und Latch haelt und `<maschine>_step` ruft.
//! 3. **Dieses Programm** — es setzt das Board auf, liefert die
//!    Telemetriefunktionen und laesst die Schleife laufen.
//!
//! `build.rs` macht die ersten beiden Schritte; geschrieben und gelesen wird
//! das Abbild von den Board-Tests (`takt_conformance::board`).
//!
//! ## Was man sieht
//!
//! Auf USART1 (PA9, 921600 8N1) erscheint, was der Latch enthaelt —
//! dieselben `out`-Zeilen, die der Interpreter schreibt
//! (`grammar/trace.md`). Damit laesst sich der Hardwarelauf gegen
//! `takt sim` halten, und das ist der Kern des M5-Exits.
//!
//! **Die LED folgt dem Ausgang `led` des Programms**, nicht einem eigenen
//! Zaehler. Das ist der Unterschied zwischen einer Demonstration und einem
//! Ziel: Blinkt sie, dann weil eine Takt-Maschine den Zustand gewechselt
//! hat. Eine frueher Fassung liess sie unabhaengig vom Latch blinken, und
//! damit sagte sie ueber das Programm genau nichts.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und C-ABI; 9.5 fuehrt Treiber in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "Stufe 2 des Regelwerks, mit `unsafe` (plan/cert.md Abschnitt 6)"
    )
)]

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

#[cfg(not(form = "rtos"))]
use cortex_m_rt::entry;
use panic_halt as _;
#[cfg(form = "own")]
use stm32f4::stm32f401::Interrupt;
#[cfg(not(form = "rtos"))]
use stm32f4::stm32f401::NVIC;
use stm32f4::stm32f401::{FLASH, Peripherals, interrupt};
#[cfg(form = "own")]
use takt_board_stm32f401::JobContext;
#[cfg(timer = "free")]
use takt_board_stm32f401::alarm::Tim2Alarm;
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, FlashNvm, Iwdg, Led, Mpu, Telemetry, Wire, cycles, mpu, platform};
#[cfg(timer = "periodic")]
use takt_board_stm32f401::{Tim2Tick, tick};
#[cfg(form = "own")]
use takt_embed::Context as _;
#[cfg(timer = "periodic")]
use takt_rt_baremetal::TimerClock;
use takt_rt_baremetal::{Cadence, Console, DRAIN_ROUNDS, Guarded, JournalStats, Protection, Stats, Trace};
#[cfg(form = "own")]
use takt_rt_baremetal::{LogicalClock, Sleep};
use takt_rt_core::{Clock, Journal, Loaded, NextRun, Policy, Profile, Runtime, Store as _, Unloaded};

/// Das Programm als Lieferform (12.11): Konstanten, Arena, Treiber-Traits,
/// Kleber und Huelle, erzeugt von `takt build --emit embed` (`build.rs`).
pub mod app {
    #![allow(dead_code)]
    include!(env!("TAKT_APP_RS"));
}
use app::{HW_ADDRESSES, LOGIC_HASH, NVM_BLOCKING_NS, OVERRUN_ALERT, PERSIST_BOUND, PERSIST_MIN_INTERVAL_NS, TICK_NS};

/// Die Frist des Watchdogs im Betrieb (12.3): zwei Perioden und ein
/// blockierender NVM-Vorgang (8.10). Ein Tick, der darueber hinaus
/// ueberzieht, ist kein Ueberlauf mehr (7.3), sondern ein Stillstand.
const WATCHDOG_NS: i64 = 2 * TICK_NS + NVM_BLOCKING_NS;

/// Die Frist fuer das geordnete Ende eines Laufs (12.7): Abschlusszeile
/// und Leitung warten je hoechstens `DRAIN_ROUNDS` Anlaeufe auf den Host,
/// zusammen weit unter dieser Frist.
const END_OF_RUN_NS: i64 = 8_000_000_000;

/// Alle wie viele Ticks die Ausgaenge im Betrieb ausgegeben werden: jeden,
/// wenn eine Zeile ein Zehntel der Periode fuellt, sonst jeden hundertsten.
#[expect(
    clippy::as_conversions,
    reason = "MISRA C:2025 R.10.5: im Konstantenausdruck wandelt nur `as`; BAUD verbreitert verlustfrei, TICK_NS ist positiv (7.1)"
)]
const TRACE_EVERY: u64 = {
    const CHARS: u64 = 32;
    const NS_PER_LINE: u64 = CHARS * 10 * 1_000_000_000 / BAUD as u64;
    if TICK_NS as u64 >= NS_PER_LINE * 10 { 1 } else { 100 }
};

/// Konformitaetslauf: `TAKT_TICKS` beim Bau gesetzt heisst jeden Tick
/// ausgeben und nach so vielen Ticks `takt end`.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

/// Ein Konformitaetslauf zaehlt in logischer Zeit und verliert keine
/// Zeile, auch wenn die Leitung den Trace langsamer nimmt, als der Tick
/// dauert (FB-292). `TAKT_TIMED` beim Bau laesst die Uhr laufen, fuer den
/// Tick-Jitter von `takt bench`.
const LOGICAL: bool = TICKS.is_some() && option_env!("TAKT_TIMED").is_none();

/// `TAKT_FRESH_JOURNAL` beim Bau gesetzt: das Journal vor dem Lauf
/// loeschen, damit der Lauf wie der Interpreter ohne Speicher beginnt.
const FRESH_JOURNAL: bool = option_env!("TAKT_FRESH_JOURNAL").is_some();

/// `TAKT_HOSTILE_FPU` beim Bau: FPSCR und FPDSCR vor dem Lauf auf
/// Flush-to-Zero, Default-NaN und Rundung gegen null, wie ein Wirt sie fuer
/// seinen eigenen Code setzen darf (4.2, 12.11). Jeder Einstieg stellt die
/// IEEE-Umgebung selbst her; die Abschlusszeile nennt FPSCR nach dem Lauf.
const HOSTILE_FPU: bool = option_env!("TAKT_HOSTILE_FPU").is_some();

/// DN (25), FZ (24) und RMode = gegen null (23:22) in FPSCR und FPDSCR.
const HOSTILE_FPSCR: u32 = 0x03C0_0000;

/// `TAKT_PORT_FLAW` beim Bau: ein absichtlich falscher Port, den die
/// Portpruefung verwerfen muss (12.11, M11 Schritt 15), nur in der
/// Interruptform. `fpu` sichert die FPU-Register der Hauptschleife nicht,
/// wenn der Schritt sie unterbricht; `jobs` gibt den Jobs die Prioritaet des
/// Schritts, sodass er sie nicht unterbrechen kann.
const PORT_FLAW: Option<&str> = option_env!("TAKT_PORT_FLAW");

/// Ist [`PORT_FLAW`] der Fehler `name`?
const fn port_flaw(name: &str) -> bool {
    let Some(flaw) = PORT_FLAW else { return false };
    let (mut a, mut b) = (flaw.as_bytes(), name.as_bytes());
    loop {
        match (a.split_first(), b.split_first()) {
            (Some((x, rest_a)), Some((y, rest_b))) if *x == *y => (a, b) = (rest_a, rest_b),
            (None, None) => return true,
            _ => return false,
        }
    }
}

const _: () = assert!(
    PORT_FLAW.is_none() || (cfg!(form = "interrupt") && (port_flaw("fpu") || port_flaw("jobs"))),
    "`TAKT_PORT_FLAW` ist `fpu` oder `jobs` und gilt nur in der Interruptform"
);

/// Die Telemetrie, statisch: Die Senke des Programms ([`UartSink`]) hat
/// keinen Empfaenger und kommt an nichts heran, was in `main` liegt.
static mut UART: Option<Telemetry> = None;

/// Die LED, die das Geraet `devices::UiLed` schaltet; aus demselben Grund.
static mut LED: Option<Led> = None;

/// Die Messschleife an PA0 und PA1, wenn das Programm sie bindet (13.8).
static mut WIRE: Option<Wire> = None;

// Die Statics der Peripherie legt `setup` vor dem ersten Tick ab. Danach
// greift je Zeitpunkt nur ein Kontext auf sie zu: der des Schritts, solange
// der Lauf laeuft, davor und danach die Hauptschleife; die ISR der Leitung
// hat ihren eigenen FIFO (`takt_board_stm32f401::uart`).

fn wire() -> Option<&'static mut Wire> {
    let slot = &raw mut WIRE;
    // SAFETY: siehe oben.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

fn uart() -> Option<&'static mut Telemetry> {
    let slot = &raw mut UART;
    // SAFETY: siehe oben.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

fn led() -> Option<&'static mut Led> {
    let slot = &raw mut LED;
    // SAFETY: siehe oben; bindet das Programm die LED nicht, schaltet sie nur
    // die Hauptschleife der Interrupt- und Pollform.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

/// Die Leitung als Senke des Programms (12.11): Was der Rahmen ueber die
/// Haken schreibt, geht in den Ring vor der Leitung. Ohne Zustand; sie
/// kommt ueber [`uart`] an die Telemetrie.
struct UartSink;

impl takt_embed::Line for UartSink {
    fn write(&self, bytes: &[u8]) {
        let Some(uart) = uart() else { return };
        for &b in bytes {
            uart.write_byte(b);
        }
    }
}

takt_embed::hooks!(UartSink);

/// Ein Pruefzugriff der TCB auf geschuetzten Speicher (12.3): zwischen zwei
/// Ticks auf den Programmzustand (1), den Waechter unter dem Hauptstack (2)
/// oder den unter dem Job-Stack (3), oder aus einer ISR, die den Commit
/// unterbricht, auf den Programmzustand (4).
///
/// **Ein Pruefgeraet, kein Treiber.** Die Outputs `test/...` tun, was ein
/// fehlerhafter Treiber taete (`devices::TestTcbWrite` und die folgenden);
/// nur ein Programm, das sie bindet, loest sie aus, und der Board-Test tut
/// es, um zu zeigen, dass die MPU den Zugriff abweist und meldet.
static PROBE: AtomicU8 = AtomicU8::new(0);

/// Wie der vorige Lauf endete (12.7), beim Start aus dem Plattformblock gelesen.
static PREVIOUS_RUN: AtomicU32 = AtomicU32::new(0);

/// Die Arena, am Anfang des RAM: Ihren Programmbereich schuetzt die MPU
/// ausserhalb des Ticks (12.3, `takt_state.x`); die Lieferform richtet sie
/// an der Region aus. Unter ihrem Namen findet `takt check-image` sie.
#[unsafe(link_section = ".takt_state")]
#[unsafe(export_name = "app_arena")]
static mut ARENA: app::Arena = app::Arena::new();

/// Der Pruefstand des Programms (12.6): je Adresse das Geraet, das die
/// Verdrahtung nennt, sonst ein Stummel (`build.rs`).
pub mod drivers {
    include!(concat!(env!("OUT_DIR"), "/takt_rig.rs"));
}

/// Der Pruefstand, statisch: Das Programm haelt ihn so lange wie die Arena.
static mut RIG: Option<drivers::Rig> = None;

/// Der Stack des Job-Fadens (4.5): Ihn stellt der Wirt, so gross, wie die
/// Lieferform sagt; unter seinem Ende liegt der Waechter (12.3).
#[unsafe(export_name = "app_job_stack")]
static mut JOB_STACK: app::JobStack = app::JobStack::new();

/// Der Griff, mit dem der Job-Faden oder die Job-Aufgabe rechnet (4.5):
/// statisch, weil sie ihn ueber den Aufbau hinaus halten.
static mut JOBS: Option<app::Jobs<'static>> = None;

/// Die Puffer des Journals (5.9): der Stand des Programms und der zuletzt
/// geschriebene. Statisch wie die Arena: Auf dem Hauptstack laegen sie unter
/// jedem Tick (12.3).
static mut PERSIST_CURRENT: [u8; PERSIST_BOUND] = [0; PERSIST_BOUND];
static mut PERSIST_STORED: [u8; PERSIST_BOUND] = [0; PERSIST_BOUND];

/// Was das Programm ueber einen Lauf hinaus braucht (5.9, FB-385).
type Needs = <app::Program<'static> as takt_rt_core::Program>::Memory;

/// Ob das Programm Jobs hat (4.5, FB-385).
#[cfg(form = "own")]
type Work = <app::Program<'static> as takt_embed::Program>::Work;

/// Das Journal im Flash, das die Runtime fuer das Programm haelt: fuer ein
/// Programm ohne `persist` keines.
type Journaled = <Needs as takt_rt_core::Memory>::Store<'static, FlashNvm>;

/// Das Programm auf seiner Arena (12.11) und sein Journal (5.9). `s0` kommt
/// aus dem Journal, darum geladen vor dem ersten Eintritt; ohne `persist`
/// im Programm gibt es kein Journal und keinen Flash-Zugriff.
///
/// # Safety
///
/// Einmal je Lauf: Arena, Pruefstand und die Puffer des Journals gehoeren
/// danach dem Programm.
unsafe fn program(flash: FLASH) -> (app::Program<'static>, Journaled) {
    let (arena, rig) = (&raw mut ARENA, &raw mut RIG);
    // SAFETY: siehe oben.
    let arena = unsafe { &mut *arena };
    // SAFETY: siehe oben.
    let rig = unsafe { &mut *rig }.insert(drivers::Rig::default());
    let mut program = app::Program::new(arena, rig, &UartSink);
    let mut loaded = None;
    let journal = <Needs as takt_rt_core::Memory>::store(|| {
        let mut nvm = FlashNvm::new(flash).with_blocking_ns(NVM_BLOCKING_NS);
        if FRESH_JOURNAL && !nvm.wipe() {
            say(format_args!("journal: loeschen scheiterte"));
        }
        let (current, stored) = (&raw mut PERSIST_CURRENT, &raw mut PERSIST_STORED);
        // SAFETY: siehe oben.
        let current = unsafe { &mut *current };
        // SAFETY: siehe oben.
        let stored = unsafe { &mut *stored };
        let unloaded = Unloaded::new(Journal::new(nvm, LOGIC_HASH, PERSIST_MIN_INTERVAL_NS), current, stored);
        let (journal, found, applied) = unloaded.load(&mut program);
        loaded = Some((found, applied));
        journal
    });
    match loaded {
        Some((Loaded::Found { length, sequence }, applied)) => {
            say(format_args!("journal: Eintrag {sequence}, {length} Byte, {applied} Werte geladen"));
        }
        Some((Loaded::Empty, _)) => say(format_args!("journal: leer")),
        None => say(format_args!("journal: keins")),
    }
    (program, journal)
}

/// Eine Zeile auf die Leitung, vor dem Lauf.
fn say(text: core::fmt::Arguments) {
    if let Some(u) = uart() {
        let _ = u.write_fmt(text);
        u.newline();
        u.flush();
    }
}

/// Legt den Griff der Jobs ab, mit dem der Job-Faden oder die Job-Aufgabe
/// rechnet; `None`, wenn das Programm keine Jobs hat.
fn hand_over_jobs(program: &mut app::Program<'static>) -> Option<&'static mut dyn takt_embed::Jobs> {
    let jobs = program.jobs()?;
    let slot = &raw mut JOBS;
    // SAFETY: ein Aufruf je Lauf, bevor jemand rechnet; danach benutzt den
    // Griff nur der Job-Kontext.
    let jobs: &'static mut dyn takt_embed::Jobs = unsafe { &mut *slot }.insert(jobs);
    Some(jobs)
}

/// Die Geraete des Boards (12.6): je Adresse der Typ, den `takt-drivers.toml`
/// nennt. `setup` richtet die Peripherie ein; die Geraete erreichen sie
/// ueber die Statics oben.
#[allow(dead_code, reason = "ein Vorrat fuer jedes Programm; welche Geraete eines braucht, nennt sein Pruefstand")]
pub mod devices {
    use core::sync::atomic::Ordering;

    use stm32f4::stm32f401::{Interrupt, NVIC};
    use takt_embed::{Device, Input, Output, Sample};

    use super::{PREVIOUS_RUN, PROBE, led, wire};

    /// Ein Geraet, das fest am Board sitzt: Es lebt, solange das Board
    /// laeuft (12.4).
    #[derive(Debug, Default)]
    pub struct Fixed;

    impl Device for Fixed {
        fn alive(&mut self, _now: i64) -> bool {
            true
        }
    }

    /// `ui/led`: die LED der Black Pill an PC13, gegen 3V3 — was `true`
    /// elektrisch heisst, weiss nur dieses Geraet.
    #[derive(Debug, Default)]
    pub struct UiLed;

    impl Output<bool> for UiLed {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            let Some(led) = led() else { return false };
            if value {
                led.on();
            } else {
                led.off();
            }
            true
        }
    }

    /// `gpio/loop_out`: PA0, ueber die Bruecke an PA1 (13.8).
    #[derive(Debug, Default)]
    pub struct GpioLoopOut;

    impl Output<bool> for GpioLoopOut {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            let Some(w) = wire() else { return false };
            w.write(value);
            true
        }
    }

    /// `gpio/loop_in`: PA1, das Ende der Bruecke (13.8).
    #[derive(Debug, Default)]
    pub struct GpioLoopIn;

    impl Input<bool> for GpioLoopIn {
        fn sample(&mut self, now: i64) -> Option<Sample<bool>> {
            Some(Sample::good(wire()?.read(), now))
        }
    }

    /// `sys/previous_run` (12.7): wie der vorige Lauf endete.
    #[derive(Debug, Default)]
    pub struct SysPreviousRun;

    impl Input<u32> for SysPreviousRun {
        fn sample(&mut self, now: i64) -> Option<Sample<u32>> {
            Some(Sample::good(PREVIOUS_RUN.load(Ordering::SeqCst), now))
        }
    }

    /// Ein Ausgang, der einen Pruefzugriff anfordert: `true` setzt `probe`.
    fn request(value: bool, probe: u8) -> bool {
        if value {
            PROBE.store(probe, Ordering::SeqCst);
        }
        true
    }

    /// `test/tcb_write` (12.3): Der naechste Leerlauf schreibt in den Programmzustand.
    #[derive(Debug, Default)]
    pub struct TestTcbWrite;

    impl Output<bool> for TestTcbWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            request(value, 1)
        }
    }

    /// `test/guard_write` (12.3): Der naechste Leerlauf schreibt in den Waechter.
    #[derive(Debug, Default)]
    pub struct TestGuardWrite;

    impl Output<bool> for TestGuardWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            request(value, 2)
        }
    }

    /// `test/job_guard_write` (12.3): Der naechste Leerlauf schreibt in den
    /// Waechter unter dem Job-Stack.
    #[derive(Debug, Default)]
    pub struct TestJobGuardWrite;

    impl Output<bool> for TestJobGuardWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            request(value, 3)
        }
    }

    /// `test/isr_write` (12.3): Im naechsten Programmschritt schreibt die
    /// Pruef-ISR in den Programmzustand (`test/in_step`).
    #[derive(Debug, Default)]
    pub struct TestIsrWrite;

    impl Output<bool> for TestIsrWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            request(value, 4)
        }
    }

    /// `test/in_step` (12.3): Gelesen wird im Programmschritt, bei offenem
    /// Programmzustand. Ist die Pruef-ISR angefordert, loest das Geraet sie
    /// hier aus, und sie unterbricht den Schritt.
    #[derive(Debug, Default)]
    pub struct TestInStep;

    impl Input<bool> for TestInStep {
        fn sample(&mut self, now: i64) -> Option<Sample<bool>> {
            if PROBE.load(Ordering::SeqCst) == 4 {
                NVIC::pend(Interrupt::EXTI0);
                cortex_m::asm::dsb();
                cortex_m::asm::isb();
            }
            Some(Sample::good(false, now))
        }
    }

    /// `test/tick_stretch`, das Pruefgeraet fuer 12.6 Zeile 7: streckt die
    /// Periode von TIM2 um den geschriebenen Prozentsatz. In Interrupt- und
    /// Pollform laeuft TIM2 frei und hat keine Periode: Das Geraet schreibt
    /// nicht.
    #[derive(Debug, Default)]
    pub struct TestTickStretch;

    impl Output<u8> for TestTickStretch {
        #[cfg(timer = "periodic")]
        fn write(&mut self, percent: u8, _now: i64) -> bool {
            takt_board_stm32f401::tick::stretch(u32::from(percent));
            true
        }

        #[cfg(timer = "free")]
        fn write(&mut self, _percent: u8, _now: i64) -> bool {
            false
        }
    }
}

/// Fuehrt einen angeforderten Pruefzugriff aus; ausserhalb des Ticks, wo
/// kein Code der TCB den Programmzustand beschreiben darf.
fn probe() {
    let target = match PROBE.load(Ordering::SeqCst) {
        1 => Some(mpu::state_address()),
        2 => Some(mpu::guard_address()),
        3 => mpu::job_guard_address(),
        _ => return,
    };
    PROBE.store(0, Ordering::SeqCst);
    if let Some(target) = target {
        forbidden_write(target);
    }
}

/// Ein Byte an `target`, das die MPU abweisen muss.
fn forbidden_write(target: u32) {
    // SAFETY: Genau dieser Zugriff ist verboten und soll es sein: Die MPU
    // weist ihn ab, der Handler uebergeht ihn, und die Runtime meldet
    // `Runtime(Hardware)`. Ohne Schutz schriebe er ein Byte, das der
    // Board-Test als Abweichung saehe.
    let at = core::ptr::with_exposed_provenance_mut::<u8>(takt_board_stm32f401::address(target));
    // SAFETY: siehe oben.
    unsafe { at.write_volatile(0xA5) };
}

/// Die Pruef-ISR (`test/isr_write`): eine Leitung, die kein Treiber
/// benutzt, nur von `test/in_step` ausgeloest.
#[interrupt]
fn EXTI0() {
    mpu::isr(|| {
        if PROBE.compare_exchange(4, 0, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            forbidden_write(mpu::state_address());
        }
    });
}

/// Die Tickgrenze (12.3): Der Wecker misst die Periode und stellt die
/// naechste Grenze; ein Ueberlauf zaehlt nur die Epoche der Zeitachse.
#[cfg(timer = "periodic")]
fn on_tim2() {
    mpu::isr(tick::on_interrupt);
}

#[cfg(form = "own")]
#[interrupt]
fn TIM2() {
    on_tim2();
}

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[cfg(not(form = "rtos"))]
#[interrupt]
fn USART1() {
    mpu::isr(takt_board_stm32f401::uart::on_interrupt);
}

/// Die Leitung als Senke des Kerns (12.5).
type Line = Trace<fn() -> Option<&'static mut Telemetry>, Telemetry>;

/// Die Schleife ueber dem Programm: das Programm hinter dem
/// Speicherschutz (12.3), der Watchdog im Betrieb.
type Takt<C> = Runtime<Guarded<app::Program<'static>, Mpu>, C, Option<Iwdg>, Line, Journaled>;

/// Nach so vielen Ticks endet ein Konformitaetslauf; 0 im Betrieb.
fn limit() -> u64 {
    TICKS.and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// Baut die Schleife ueber `program` und sein Journal unter `clock` im
/// Profil `profile` (12.8).
fn runtime<C: Clock>(
    clock: C,
    protection: Mpu,
    profile: Profile,
    mut program: app::Program<'static>,
    journal: Journaled,
) -> Takt<C> {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit() == 0).then(|| Iwdg::arm(WATCHDOG_NS));
    // 12.3: Nach `init` ist der Programmzustand nur noch im Tick beschreibbar.
    program.ensure_init();
    let program = Guarded::new(program, protection);
    let line: Line = Trace::new(Cadence::of(limit(), TRACE_EVERY), TICK_NS, uart);
    Runtime::journaled(program, journal, clock, watchdog, line, profile, TICK_NS, policy)
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
#[cfg(form = "own")]
fn conduct(clock: impl Clock, protection: Mpu, program: app::Program<'static>, journal: Journaled) {
    let mut rt = runtime(clock, protection, Profile::BAREMETAL, program, journal);
    // 8.4: Tunes vom Host kommen ueber die Gegenrichtung der Konsole.
    let mut tunes = Console::new(takt_board_stm32f401::console_byte);
    let stats = takt_rt_baremetal::run(&mut rt, Some(&mut tunes));
    conclude(&rt, &stats);
}

/// Nach dem Lauf: die Abschlusszeile, im Betrieb das Kommando an die
/// Plattform.
fn conclude<C: Clock>(rt: &Takt<C>, stats: &Stats) {
    if rt.watchdog.is_some() {
        Iwdg::arm(END_OF_RUN_NS);
    }
    let journal = rt.journal().persist().map_or(JournalStats::default(), |p| {
        let (erase_ns, program_ns) = p.journal().device().measured_ns();
        JournalStats { writes: p.journal().writes(), failures: p.journal().failures(), erase_ns, program_ns }
    });
    if let Some(u) = uart() {
        // Die Messschleife vor der Bilanz: Nach `takt end` liest der Host nicht mehr.
        if let Some(w) = wire() {
            u.drain(DRAIN_ROUNDS);
            let _ = w.stats.report(CORE_HZ, u);
            u.newline();
        }
        // 12.3: was der Speicherschutz abgewiesen hat.
        if let Some(v) = rt.program.last {
            u.drain(DRAIN_ROUNDS);
            let _ = write!(
                u,
                "takt schutz verletzungen {} region {} adresse {:#010x}",
                rt.program.violations, v.region, v.address
            );
            u.newline();
        }
        if HOSTILE_FPU {
            u.drain(DRAIN_ROUNDS);
            let _ = write!(u, "takt fpscr {:#010x}", cortex_m::register::fpscr::read().bits() & 0x07C0_0000);
            u.newline();
        }
        let stacks = takt_rt_baremetal::Stacks {
            tick: Some(takt_board_stm32f401::stack::high_water()),
            // Unter RTIC und in der Interruptform rechnen die Jobs mit auf
            // dem Hauptstack (FB-459).
            tick_bound: u32::try_from(
                app::TICK_STACK_BYTES + if cfg!(form = "own") { 0 } else { app::JOB_STACK_BYTES },
            )
            .ok(),
            tick_program: env!("TAKT_TICK_STACK_PROGRAM").parse().ok(),
            job_shared: !cfg!(form = "own"),
            job: takt_board_stm32f401::jobs::high_water(),
            job_bound: u32::try_from(app::JOB_STACK_BYTES).ok(),
        };
        takt_rt_baremetal::report(u, rt.overrun(), stats, &journal, &stacks);
    }
    if limit() == 0 {
        platform(stats.next_run);
    }
}

/// Fuehrt aus, was zwischen zwei Laeufen geschieht (12.7, `next_run`).
///
/// Ein Konformitaetslauf endet wie der Wirtsrahmen mit dem Trace, und das
/// Board bleibt fuer das naechste Programm erreichbar; nur im Betrieb
/// beginnt es den naechsten Lauf. Vorher geht die Leitung ganz hinaus: Reset
/// und Tiefschlaf naehmen mit, was noch in ihrem Puffer steht (FB-314).
fn platform(next: Option<NextRun>) {
    let Some(next) = next else { return };
    if let Some(u) = uart() {
        u.finish(DRAIN_ROUNDS);
    }
    match next {
        NextRun::Now => platform::restart(),
        NextRun::After(delay) => platform::deep_sleep(Some(delay)),
        NextRun::OnWake | NextRun::OnStart => platform::deep_sleep(None),
    }
}

/// Verstellt FPSCR und FPDSCR ([`HOSTILE_FPU`]).
fn hostile_fpu() {
    use cortex_m::register::fpscr::{self, Fpscr};
    // SAFETY: nur die Modusbits der Fliesskomma-Umgebung dieses Kontexts.
    unsafe { fpscr::write(Fpscr::from_bits(fpscr::read().bits() | HOSTILE_FPSCR)) };
    // SAFETY: die Register der FPU.
    let fpu = unsafe { &*cortex_m::peripheral::FPU::PTR };
    // SAFETY: nur die Modusbits; FPDSCR gibt sie jedem Handler mit.
    unsafe { fpu.fpdscr.modify(|v| v | HOSTILE_FPSCR) };
}

/// Der Zeitgeber der Form: periodisch fuer den eigenen Kern und RTIC, frei
/// als Zeitachse in Interrupt- und Pollform.
#[cfg(timer = "periodic")]
type Timer = Tim2Tick;
#[cfg(timer = "free")]
type Timer = Tim2Alarm;

/// Was jede Bindung vor dem ersten Tick einrichtet.
struct Setup {
    timer: Timer,
    protection: Mpu,
    /// Der Stack des Job-Fadens im eigenen Kern (4.5); unter RTIC rechnen
    /// Jobs in einer eigenen Aufgabe, in der Interruptform im Job-Interrupt.
    #[cfg(form = "own")]
    job_stack: &'static mut [u8],
    /// Unter RTIC richtet die App die Interrupts nach ihren Prioritaeten ein.
    #[cfg(not(form = "rtos"))]
    nvic: NVIC,
    /// Das Flash fuer das Journal (5.9).
    flash: FLASH,
}

/// Richtet das Board ein: Takt, Leitung, Messschleife, Zyklenzaehler,
/// Speicherschutz, Anzeige. Haelt an, wenn Takt, Leitung oder
/// Speicherschutz nicht einzurichten sind.
fn setup(dp: Peripherals, cp: cortex_m::Peripherals) -> Setup {
    // Ein Tiefschlaf mit Rest schlaeft weiter, bevor irgendetwas laeuft (12.7).
    platform::continue_deep_sleep();
    PREVIOUS_RUN.store(platform::previous_run(), Ordering::SeqCst);
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    #[cfg(timer = "periodic")]
    let timer = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS);
    #[cfg(timer = "free")]
    let timer = takt_board_stm32f401::init_alarm(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, host::JOB_LINE);
    let Ok(timer) = timer else {
        // Ohne Takt keine Telemetrie: Die LED bleibt an.
        led.on();
        halt()
    };
    let Ok(telemetry) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        led.on();
        halt()
    };
    let telemetry = if LOGICAL { telemetry.lossless() } else { telemetry };
    // Erst jetzt sichtbar machen: Ein Trace vor der Einrichtung schriebe
    // in ein nicht konfiguriertes Register.
    // SAFETY: vor dem ersten Tick und vor jeder Freigabe eines Interrupts.
    unsafe { UART = Some(telemetry) };
    if HW_ADDRESSES.iter().any(|a| a.starts_with("gpio/loop_")) {
        // SAFETY: wie die Leitung.
        unsafe { WIRE = Some(Wire::new(dp.GPIOA, &dp.RCC)) };
    }

    // 4.5: Jobs rechnen im eigenen Kern in der Wartezeit bis zum Tick, im
    // eigenen Faden auf ihrem Stack; unter dessen Ende liegt ein Waechter
    // (12.3). Unter RTIC und in der Interruptform rechnen sie auf dem
    // Hauptstack.
    // Ohne Jobs ist der Stack leer, und es gibt keinen Waechter.
    #[cfg(form = "own")]
    let job_stack = &raw mut JOB_STACK;
    // SAFETY: einmal beim Aufbau; danach haelt nur der Job-Faden den Stack.
    #[cfg(form = "own")]
    let job_stack = unsafe { &mut *job_stack }.bytes();
    #[cfg(form = "own")]
    let guarded = (!job_stack.is_empty()).then_some(takt_board_stm32f401::word(job_stack.as_ptr().expose_provenance()));
    #[cfg(not(form = "own"))]
    let guarded = None;

    #[cfg(not(form = "rtos"))]
    let nvic = cp.NVIC;
    let (mut dcb, mut dwt, mut core_mpu, mut scb) = (cp.DCB, cp.DWT, cp.MPU, cp.SCB);
    cycles::enable(&mut dcb, &mut dwt);
    let Some(protection) = Mpu::arm(&mut core_mpu, &mut scb, guarded) else {
        if let Some(u) = uart() {
            u.write("takt: Speicherschutz nicht einrichtbar");
            u.newline();
            u.drain(DRAIN_ROUNDS);
        }
        halt()
    };
    if HOSTILE_FPU {
        hostile_fpu();
    }
    #[cfg(timer = "periodic")]
    banner(timer.nominal_ns());
    // Ohne periodischen Zeitgeber kommt jeder Schritt zu seiner Frist: Die
    // Periode ist die nominale.
    #[cfg(timer = "free")]
    banner(TICK_NS);
    // SAFETY: wie die Leitung.
    unsafe { LED = Some(led) };
    Setup {
        timer,
        protection,
        #[cfg(form = "own")]
        job_stack,
        #[cfg(not(form = "rtos"))]
        nvic,
        flash: dp.FLASH,
    }
}

#[cfg(form = "own")]
#[entry]
fn main() -> ! {
    let (Some(dp), Some(cp)) = (Peripherals::take(), cortex_m::Peripherals::take()) else { halt() };
    let Setup { timer, protection, job_stack, mut nvic, flash } = setup(dp, cp);
    // Prioritaeten und Freigabe vor dem ersten Tick; die Handler oben sind
    // bereit.
    for line in [Interrupt::TIM2, Interrupt::USART1, Interrupt::EXTI0] {
        // SAFETY: siehe oben.
        unsafe { nvic.set_priority(line, mpu::ISR_PRIORITY) };
        // SAFETY: siehe oben.
        unsafe { NVIC::unmask(line) };
    }

    // SAFETY: einmal je Lauf; `main` kehrt nicht zurueck.
    let (mut program, journal) = unsafe { program(flash) };
    let (dispatch, worker) = (program.dispatch(), hand_over_jobs(&mut program));
    // 4.5: nur ein Programm mit Jobs bekommt seinen Faden (FB-385); die
    // Griffe gibt es einmal je Programm, hier zum ersten Mal.
    let mut jobs = <Work as takt_embed::Work>::context(|| match (dispatch, worker) {
        (Some(dispatch), Some(worker)) => JobContext::start(job_stack, dispatch, worker),
        // Nur fuer ein Programm mit Jobs ruft `Work` den Start, und ein
        // solches gibt beide Griffe beim ersten Aufruf.
        _ => halt(),
    });
    // Die Abschlusszeile meldet, wie tief der Stack in der Tickschleife
    // reichte: gemalt erst hier, Aufbau und Journal zaehlen nicht (12.3).
    takt_board_stm32f401::stack::paint();
    if LOGICAL {
        // Zwischen den Ticks leert die Schleife die Leitung ganz und rechnet
        // jeden Job zu Ende; dann steht die Uhr auf der Frist. In logischer
        // Zeit haelt so jeder Job seine Dauer (4.5).
        conduct(
            LogicalClock::new(|| {
                probe();
                if let Some(u) = uart() {
                    u.drain(DRAIN_ROUNDS);
                }
                jobs.finish();
            }),
            protection,
            program,
            journal,
        );
    } else {
        // Zwischen den Ticks fuellt die Schleife die Leitung nach, sooft ein
        // Interrupt den Kern weckt: Sie nimmt nur ab, was in ihren FIFO passt.
        conduct(
            TimerClock::new(timer, TICK_NS).with_idle(|| {
                probe();
                if let Some(u) = uart() {
                    u.flush();
                }
                jobs.run();
            }),
            protection,
            program,
            journal,
        );
    }
    // Nach dem Lauf bleibt die Leitung offen: Der Host holt das Board mit
    // `TAKT` zurueck, um das naechste Programm zu schreiben (FB-275).
    // Der Watchdog laeuft nach einem Lauf im Betrieb weiter; das Board
    // haelt an, statt neu zu starten.
    let mut sleep = takt_board_stm32f401::WfiSleep;
    loop {
        sleep.sleep_until_event();
        if let Some(u) = uart() {
            u.flush();
        }
        Iwdg::feed();
    }
}

#[cfg(any(form = "interrupt", form = "poll", form = "embassy"))]
#[entry]
fn main() -> ! {
    let (Some(dp), Some(cp)) = (Peripherals::take(), cortex_m::Peripherals::take()) else { halt() };
    let Setup { timer, protection, nvic, flash } = setup(dp, cp);
    #[cfg(form = "interrupt")]
    interrupt_form::run(timer, protection, nvic, flash);
    #[cfg(form = "poll")]
    poll_form::run(timer, protection, nvic, flash);
    #[cfg(form = "embassy")]
    embassy_form::run(timer, protection, nvic, flash);
}

/// Was Interrupt- und Pollform teilen (12.11): den Job-Interrupt, die
/// fremde Arbeit der Hauptschleife und das Ende des Laufs, das in diesen
/// Formen dem Wirt gehoert.
#[cfg(timer = "free")]
mod host {
    use stm32f4::stm32f401::Interrupt;
    use takt_board_stm32f401::{Iwdg, WfiSleep};
    use takt_rt_baremetal::Sleep;
    use takt_rt_core::port_test::{self, ForeignLoad};

    use super::{DRAIN_ROUNDS, HW_ADDRESSES, JOBS, led, probe, uart};

    /// Die Leitung des Job-Interrupts: eine, die kein Treiber benutzt (4.5).
    pub(crate) const JOB_LINE: Interrupt = Interrupt::EXTI1;

    /// Die Jobs unter allen Interrupts; nur die Hauptschleife liegt darunter.
    /// Unter RTIC setzt die App die Prioritaet der Job-Aufgabe.
    #[cfg(any(form = "interrupt", form = "poll", form = "embassy"))]
    pub(crate) const JOB_PRIORITY: u8 = 0xF0;

    /// Die halbe Periode der LED, wenn die Hauptschleife sie fuehrt.
    const BLINK_NS: i64 = 500_000_000;

    /// Rechnet den Auftrag, den der Schrittkontext dem Job-Interrupt gab
    /// (4.5).
    pub(crate) fn work_jobs() {
        let slot = &raw mut JOBS;
        // SAFETY: Den Griff legt die Form vor dem ersten Schritt ab; danach
        // rechnet nur der Job-Interrupt mit ihm.
        if let Some(jobs) = unsafe { slot.as_mut() }.and_then(Option::as_mut) {
            jobs.work();
        }
    }

    /// Die fremde Arbeit der Hauptschleife: Sie zaehlt ihre Runden, rechnet
    /// mit der FPU wie eine fremde Aufgabe (die Last der Portpruefung, 12.11)
    /// und laesst die LED blinken, wenn das Programm sie nicht selbst fuehrt.
    pub(crate) struct Work {
        blink: bool,
        lit: bool,
        rounds: u64,
        load: ForeignLoad,
    }

    impl Work {
        pub(crate) fn new() -> Work {
            Work { blink: !HW_ADDRESSES.contains(&"ui/led"), lit: false, rounds: 0, load: ForeignLoad::new() }
        }

        /// Eine Runde zur Zeit `now_ns` der Zeitachse.
        pub(crate) fn round(&mut self, now_ns: i64) {
            self.rounds = self.rounds.wrapping_add(1);
            self.load.round();
            let on = now_ns / BLINK_NS % 2 == 0;
            if self.blink && on != self.lit {
                self.lit = on;
                // Das Programm bindet die LED nicht; nur diese Schleife
                // schaltet sie.
                if let Some(led) = led() {
                    if on { led.on() } else { led.off() }
                }
            }
            probe();
        }

        /// Die Zeilen vor der Bilanz: Die Runden zeigen, dass der Wirt neben
        /// dem Schritt lief; `longest` ist die laengste in Nanosekunden. Die
        /// fremde Last nennt, wie oft sie falsch rechnete.
        pub(crate) fn report(&self, longest: Option<u64>) {
            let Some(u) = uart() else { return };
            u.drain(DRAIN_ROUNDS);
            u.write("takt wirt runden ");
            u.write_u64(self.rounds);
            if let Some(ns) = longest {
                u.write(" laengste ");
                u.write_u64(ns);
                u.write(" ns");
            }
            u.newline();
            u.write(port_test::LINE);
            u.write_u64(self.load.rounds());
            u.write(port_test::ERRORS);
            u.write_u64(self.load.errors());
            u.newline();
        }
    }

    /// Nach dem Lauf bleibt die Leitung offen wie im eigenen Kern (FB-275).
    pub(crate) fn park() -> ! {
        let mut sleep = WfiSleep;
        loop {
            sleep.sleep_until_event();
            if let Some(u) = uart() {
                u.flush();
            }
            Iwdg::feed();
        }
    }
}

/// Die Interruptform (12.11, `plan/m11.md` 2.4): Den Schritt rechnet die
/// ISR von TIM2 zu der Frist, die `service` nennt, Jobs rechnen im
/// Job-Interrupt (EXTI1). Darunter laeuft die Hauptschleife des Wirts: Sie
/// zaehlt ihre Runden und laesst die LED blinken, wenn das Programm sie
/// nicht selbst fuehrt. Das Ende des Laufs gehoert in dieser Form dem Wirt
/// (12.11): Die Hauptschleife schreibt die Bilanz und fuehrt `next_run` aus.
///
/// **Prioritaeten, von oben.** `MemoryManagement` steht ueber allen (12.3).
/// Die Leitung (USART1) und die Pruef-ISR (EXTI0) unterbrechen den Schritt:
/// Im Konformitaetslauf wartet ein voller Ring im Schritt auf die Leitung,
/// und die Pruef-ISR soll den offenen Programmzustand treffen. Darunter der
/// Schritt (TIM2), ganz unten die Jobs (EXTI1). Die Hauptschleife laeuft
/// nur, wenn keine ISR aktiv ist.
#[cfg(form = "interrupt")]
mod interrupt_form {
    use core::sync::atomic::{AtomicBool, Ordering};

    use stm32f4::stm32f401::{FLASH, Interrupt, NVIC, interrupt};
    use takt_board_stm32f401::alarm::{self, Tim2Alarm};
    use takt_rt_baremetal::interrupt::{Form, Time, release};

    use super::host::{self, JOB_LINE, JOB_PRIORITY, Work};
    use super::{
        Console, LOGICAL, Mpu, Profile, Stats, Takt, conclude, hand_over_jobs, mpu, port_flaw, program, runtime, uart,
    };

    /// Der Schritt unter Leitung und Pruef-ISR.
    const STEP_PRIORITY: u8 = 0x20;

    /// Die Jobs: unter allen Interrupts, im falschen Port `jobs` neben dem
    /// Schritt ([`super::PORT_FLAW`]).
    const JOB_FORM_PRIORITY: u8 = if port_flaw("jobs") { STEP_PRIORITY } else { JOB_PRIORITY };

    /// Der falsche Port `fpu` ([`super::PORT_FLAW`]): Ohne `ASPEN` und `LSPEN`
    /// in FPCCR und ohne `FPCA` legt eine ISR die FPU-Register der
    /// Hauptschleife nicht ab, und der Schritt rechnet in ihnen weiter.
    fn unsaved_fpu() {
        use cortex_m::register::control::{self, Fpca};
        let mut c = control::read();
        c.set_fpca(Fpca::NotActive);
        // Bevor ein Interrupt freigegeben ist; nur die Sicherung der FPU
        // aendert sich, und genau das soll die Portpruefung sehen.
        // SAFETY: die Register der FPU, siehe oben.
        let fpu = unsafe { &*cortex_m::peripheral::FPU::PTR };
        // SAFETY: `ASPEN` und `LSPEN` aus, siehe oben.
        unsafe { fpu.fpccr.modify(|v| v & !(0b11 << 30)) };
        // SAFETY: `FPCA` aus, siehe oben.
        unsafe { control::write(c) };
    }

    /// Was die ISR des Alarms haelt.
    struct Stepping {
        rt: Takt<Time<Tim2Alarm>>,
        form: Form<Tim2Alarm>,
        /// 8.4: Tunes vom Host ueber die Gegenrichtung der Konsole.
        tunes: Console<fn() -> Option<u8>>,
        /// Die Bilanz, sobald der Lauf endet.
        ended: Option<Stats>,
    }

    /// Der Lauf. Bis zu seinem Ende rechnet nur die ISR des Alarms mit ihm;
    /// danach ([`ENDED`]) steht kein Alarm mehr, und er gehoert der
    /// Hauptschleife.
    static mut STEPPING: Option<Stepping> = None;

    /// Der Lauf ist zu Ende, seine Bilanz steht in [`STEPPING`].
    static ENDED: AtomicBool = AtomicBool::new(false);

    /// In logischer Zeit das Tor, hinter dem der Alarm wartet.
    static GATE: AtomicBool = AtomicBool::new(false);

    /// Der Job-Interrupt hat seinen Auftrag gerechnet; der Schrittkontext
    /// verteilt den naechsten.
    static JOB_DONE: AtomicBool = AtomicBool::new(false);

    /// Der Schrittkontext: An der Frist rechnet er den Schritt, nach einem
    /// Auftrag des Job-Interrupts verteilt er den naechsten; danach nimmt
    /// die Leitung, was in ihren FIFO passt. Ein Ueberlauf der Zeitachse
    /// zaehlt nur ihre Epoche.
    #[interrupt]
    fn TIM2() {
        mpu::isr(|| {
            let due = alarm::on_interrupt();
            let job_done = JOB_DONE.swap(false, Ordering::SeqCst);
            if ENDED.load(Ordering::SeqCst) {
                return;
            }
            let slot = &raw mut STEPPING;
            // SAFETY: Bis zum Ende gehoert der Lauf dieser ISR (`STEPPING`).
            let Some(s) = unsafe { slot.as_mut() }.and_then(Option::as_mut) else { return };
            if due {
                if let Some(stats) = s.form.on_alarm(&mut s.rt, Some(&mut s.tunes)) {
                    s.ended = Some(stats);
                    ENDED.store(true, Ordering::SeqCst);
                }
            } else if job_done {
                s.form.on_job_done(&mut s.rt, Some(&mut s.tunes));
            } else {
                return;
            }
            if let Some(u) = uart() {
                u.flush();
            }
        });
    }

    /// Der Job-Interrupt (4.5): rechnet den Auftrag, den der Schritt gab,
    /// und laesst den Schrittkontext den naechsten verteilen.
    #[interrupt]
    fn EXTI1() {
        mpu::isr(|| {
            host::work_jobs();
            JOB_DONE.store(true, Ordering::SeqCst);
            NVIC::pend(Interrupt::TIM2);
        });
    }

    /// Beginnt den Lauf und gibt den Kern an die Hauptschleife des Wirts.
    pub(crate) fn run(alarm: Tim2Alarm, protection: Mpu, mut nvic: NVIC, flash: FLASH) -> ! {
        // SAFETY: einmal je Lauf; `run` kehrt nicht zurueck.
        let (mut program, journal) = unsafe { program(flash) };
        hand_over_jobs(&mut program);
        // Die Abschlusszeile meldet, wie tief der Stack unter Wirt, Jobs und
        // Schritt reichte: gemalt nach `init`, der Aufbau zaehlt nicht (12.3),
        // und vor der Uhr des Laufs — das Malen dauert Millisekunden, und die
        // ersten Fristen kaemen sonst um so viel zu spaet.
        takt_board_stm32f401::stack::paint();
        let clock = if LOGICAL { Time::Logical(0) } else { Time::Board(alarm) };
        let mut rt = runtime(clock, protection, Profile::SHARED, program, journal);
        let form = Form::start(&mut rt, alarm, LOGICAL.then_some(&GATE));
        let byte: fn() -> Option<u8> = takt_board_stm32f401::console_byte;
        let tunes = Console::new(byte);
        // SAFETY: TIM2 ist noch maskiert; ab seiner Freigabe gehoert der Lauf
        // seiner ISR.
        unsafe { STEPPING = Some(Stepping { rt, form, tunes, ended: None }) };
        if port_flaw("fpu") {
            unsaved_fpu();
        }
        // Prioritaeten und Freigabe nach dem Aufbau; die Handler oben sind
        // bereit.
        for (line, priority) in [
            (Interrupt::USART1, mpu::ISR_PRIORITY),
            (Interrupt::EXTI0, mpu::ISR_PRIORITY),
            (Interrupt::TIM2, STEP_PRIORITY),
            (JOB_LINE, JOB_FORM_PRIORITY),
        ] {
            // SAFETY: siehe oben.
            unsafe { nvic.set_priority(line, priority) };
            // SAFETY: siehe oben.
            unsafe { NVIC::unmask(line) };
        }
        host(alarm)
    }

    /// Die Hauptschleife des Wirts: fremde Arbeit, die jede ISR
    /// unterbricht. In logischer Zeit gibt sie den Alarm frei, sobald sie
    /// laeuft, denn dann ruht das System (`release`). Nach dem Ende schreibt
    /// sie die Bilanz und fuehrt im Betrieb `next_run` aus.
    fn host(mut alarm: Tim2Alarm) -> ! {
        let mut work = Work::new();
        while !ENDED.load(Ordering::SeqCst) {
            if LOGICAL {
                release(&GATE, &mut alarm);
            }
            work.round(alarm.now_ns());
        }
        let slot = &raw mut STEPPING;
        // SAFETY: Nach dem Ende steht kein Alarm mehr; der Lauf gehoert jetzt
        // dieser Schleife.
        if let Some(s) = unsafe { slot.as_mut() }.and_then(Option::take) {
            work.report(None);
            conclude(&s.rt, &s.ended.unwrap_or_default());
        }
        host::park()
    }
}

/// Die Form `rtos` unter Embassy (12.8, 12.11; M11 Schritt 14d): Takt als
/// Aufgabe eines rohen Executors, den der Interrupt SPI1 treibt; sie wartet
/// mit `embassy-time` auf die Frist, deren Zeittreiber auf TIM2 liegt
/// (`takt_board_stm32f401::time_driver`). Jobs rechnen im Job-Interrupt
/// (EXTI1) wie in der Interruptform und wecken die Aufgabe danach. Die
/// Hauptschleife des Wirts laeuft darunter, und das Ende des Laufs gehoert
/// ihr (12.11).
///
/// **Warum ein roher Executor.** Der `InterruptExecutor` von Embassy nimmt
/// nur Aufgaben, die `Send` sind; die Takt-Aufgabe haelt die Arena und das
/// Journal, und beide sind es nicht. Der rohe Executor ist derselbe
/// Mechanismus — `__pender` laesst den Interrupt anstehen, die ISR fragt den
/// Executor ab —, und seine Aufgaben spawnt die Hauptschleife, bevor der
/// Interrupt freigegeben ist.
///
/// **Prioritaeten, von oben.** Leitung und Pruef-ISR wie in der
/// Interruptform, dann TIM2 (der Zeittreiber), darunter SPI1 (der Executor
/// mit dem Schritt), ganz unten die Jobs.
#[cfg(form = "embassy")]
mod embassy_form {
    use core::sync::atomic::{AtomicBool, Ordering};

    use embassy_executor::raw::Executor;
    use stm32f4::stm32f401::{FLASH, Interrupt, NVIC, interrupt};
    use takt_board_stm32f401::alarm::Tim2Alarm;
    use takt_board_stm32f401::time_driver;
    use takt_rt_baremetal::embassy::{TaskAlarm, Wake};
    use takt_rt_baremetal::interrupt::{Form, Time, release};

    use super::host::{self, JOB_LINE, JOB_PRIORITY, Work};
    use super::{Console, LOGICAL, Mpu, Profile, Stats, Takt, conclude, hand_over_jobs, mpu, program, runtime, uart};

    /// Der Zeittreiber unter Leitung und Pruef-ISR.
    const TIMER_PRIORITY: u8 = 0x20;
    /// Der Executor mit dem Schritt, unter dem Zeittreiber.
    const STEP_PRIORITY: u8 = 0x30;
    /// Der Interrupt des Executors: eine Leitung, die kein Treiber benutzt.
    const STEP_LINE: Interrupt = Interrupt::SPI1;

    /// Was die Takt-Aufgabe weckt.
    static WAKE: Wake = Wake::new();

    /// In logischer Zeit das Tor der Frist; die Hauptschleife oeffnet es.
    static GATE: AtomicBool = AtomicBool::new(false);

    /// Der Lauf ist zu Ende und liegt in [`ENDED_RUN`].
    static ENDED: AtomicBool = AtomicBool::new(false);

    /// Der beendete Lauf mit seiner Bilanz; ab [`ENDED`] gehoert er der
    /// Hauptschleife.
    static mut ENDED_RUN: Option<(Takt<Time<Tim2Alarm>>, Stats)> = None;

    /// Der Executor des Schritts; die Hauptschleife legt ihn an, bevor SPI1
    /// freigegeben ist.
    static mut EXECUTOR: Option<Executor> = None;

    /// Der Executor, sobald er angelegt ist.
    fn executor() -> Option<&'static Executor> {
        let slot = &raw const EXECUTOR;
        // SAFETY: Angelegt einmal vor der Freigabe von SPI1, danach nur
        // gelesen.
        unsafe { &*slot }.as_ref()
    }

    /// Embassy ruft es, wenn eine Aufgabe des Executors bereit ist.
    #[unsafe(export_name = "__pender")]
    fn pender(_context: *mut ()) {
        NVIC::pend(STEP_LINE);
    }

    /// Der Zeittreiber von `embassy-time`.
    #[interrupt]
    fn TIM2() {
        mpu::isr(time_driver::on_interrupt);
    }

    /// Der Executor des Schritts.
    #[interrupt]
    fn SPI1() {
        mpu::isr(|| {
            if let Some(executor) = executor() {
                // SAFETY: Nur diese ISR fragt den Executor ab.
                unsafe { executor.poll() };
            }
        });
    }

    /// Der Job-Interrupt (4.5): rechnet den Auftrag und weckt die
    /// Takt-Aufgabe, die den naechsten verteilt.
    #[interrupt]
    fn EXTI1() {
        mpu::isr(|| {
            host::work_jobs();
            WAKE.job_done();
        });
    }

    /// Laesst den Job-Interrupt anstehen.
    fn pend_jobs() {
        NVIC::pend(JOB_LINE);
    }

    /// Die Takt-Aufgabe: der Kern unter dem Planer von Embassy, danach geht
    /// der Lauf an die Hauptschleife.
    #[embassy_executor::task]
    async fn takt(mut rt: Takt<Time<Tim2Alarm>>) {
        let pend: fn() = pend_jobs;
        let alarm = TaskAlarm::new(&WAKE, pend);
        let mut form = Form::start(&mut rt, alarm, LOGICAL.then_some(&GATE));
        let byte: fn() -> Option<u8> = takt_board_stm32f401::console_byte;
        let mut tunes = Console::new(byte);
        let flush = || {
            if let Some(u) = uart() {
                u.flush();
            }
        };
        let stats = takt_rt_baremetal::embassy::run(&mut rt, &mut form, &WAKE, Some(&mut tunes), flush).await;
        // SAFETY: Bis `ENDED` liest niemand den Platz; danach rechnet diese
        // Aufgabe nicht mehr.
        unsafe { ENDED_RUN = Some((rt, stats)) };
        ENDED.store(true, Ordering::SeqCst);
    }

    /// Beginnt den Lauf und gibt den Kern an die Hauptschleife des Wirts.
    pub(crate) fn run(alarm: Tim2Alarm, protection: Mpu, mut nvic: NVIC, flash: FLASH) -> ! {
        // SAFETY: einmal je Lauf; `run` kehrt nicht zurueck.
        let (mut program, journal) = unsafe { program(flash) };
        hand_over_jobs(&mut program);
        // Gemalt nach `init` und vor der Uhr des Laufs, wie in der
        // Interruptform (12.3).
        takt_board_stm32f401::stack::paint();
        let clock = if LOGICAL { Time::Logical(0) } else { Time::Board(alarm) };
        let rt = runtime(clock, protection, Profile::SHARED, program, journal);
        let slot = &raw mut EXECUTOR;
        // SAFETY: einmal, bevor SPI1 freigegeben ist; danach liest nur die
        // ISR.
        let executor = unsafe { &mut *slot }.insert(Executor::new(core::ptr::null_mut()));
        // Die Takt-Aufgabe gibt es einmal, und sie startet hier.
        let Ok(task) = takt(rt) else { super::halt() };
        executor.spawner().spawn(task);
        // Prioritaeten und Freigabe nach dem Aufbau; die Handler oben sind
        // bereit.
        for (line, priority) in [
            (Interrupt::USART1, mpu::ISR_PRIORITY),
            (Interrupt::EXTI0, mpu::ISR_PRIORITY),
            (Interrupt::TIM2, TIMER_PRIORITY),
            (STEP_LINE, STEP_PRIORITY),
            (JOB_LINE, JOB_PRIORITY),
        ] {
            // SAFETY: siehe oben.
            unsafe { nvic.set_priority(line, priority) };
            // SAFETY: siehe oben.
            unsafe { NVIC::unmask(line) };
        }
        // Die Aufgabe steht bereit; ihr erster Lauf stellt die erste Frist.
        NVIC::pend(STEP_LINE);
        host(alarm)
    }

    /// Die Hauptschleife des Wirts: fremde Arbeit unter jeder ISR. In
    /// logischer Zeit gibt sie die Frist frei, sobald sie laeuft, denn dann
    /// ruht das System (`release`). Nach dem Ende schreibt sie die Bilanz.
    fn host(alarm: Tim2Alarm) -> ! {
        let mut work = Work::new();
        let pend: fn() = pend_jobs;
        let mut gate = TaskAlarm::new(&WAKE, pend);
        while !ENDED.load(Ordering::SeqCst) {
            if LOGICAL {
                release(&GATE, &mut gate);
            }
            work.round(alarm.now_ns());
        }
        let slot = &raw mut ENDED_RUN;
        // SAFETY: Ab `ENDED` gehoert der Lauf dieser Schleife.
        if let Some((rt, stats)) = unsafe { slot.as_mut() }.and_then(Option::take) {
            work.report(None);
            conclude(&rt, &stats);
        }
        host::park()
    }
}

/// Die Pollform (12.11, `plan/m11.md` 2.4): Die Hauptschleife des Wirts ist
/// der Schrittkontext. Sie fragt je Runde die Frist ab und ruft `service`,
/// sobald sie erreicht ist (`Form::poll`); dazwischen tut sie ihre eigene
/// Arbeit, [`HOST_WORK_US`](poll_form::HOST_WORK_US) je Runde. Die laengste
/// Runde steht in der Hardware-Konfiguration (`main_loop_ns`), und die Bilanz
/// nennt die gemessene. Jobs rechnen im Job-Interrupt (EXTI1); hat er einen
/// Auftrag gerechnet, verteilt die naechste Runde den naechsten. TIM2 laeuft
/// frei als Zeitachse, ohne Alarm.
///
/// **Prioritaeten.** Leitung und Pruef-ISR wie in der Interruptform; TIM2
/// zaehlt nur die Epochen der Zeitachse. Der Job-Interrupt liegt unter allen
/// und doch ueber der Hauptschleife im Thread-Modus: Er rechnet, sobald er
/// ansteht.
#[cfg(form = "poll")]
mod poll_form {
    use core::sync::atomic::{AtomicBool, Ordering};

    use stm32f4::stm32f401::{FLASH, Interrupt, NVIC, interrupt};
    use takt_board_stm32f401::alarm::{self, Tim2Alarm};
    use takt_board_stm32f401::cycles;
    use takt_rt_baremetal::interrupt::{Form, Polled, Time};

    use super::host::{self, JOB_LINE, JOB_PRIORITY, Work};
    use super::{CORE_HZ, Console, LOGICAL, Mpu, Profile, conclude, hand_over_jobs, mpu, program, runtime, uart};

    /// Die fremde Arbeit je Runde der Hauptschleife, in Mikrosekunden.
    pub(crate) const HOST_WORK_US: u32 = 10;

    /// In logischer Zeit das Tor der Frist; die Hauptschleife oeffnet es,
    /// sobald sie laeuft.
    static GATE: AtomicBool = AtomicBool::new(false);

    /// Der Job-Interrupt hat seinen Auftrag gerechnet; die naechste Runde
    /// verteilt den naechsten.
    static JOB_DONE: AtomicBool = AtomicBool::new(false);

    /// Die Zeitachse: Ein Ueberlauf von TIM2 zaehlt ihre Epoche.
    #[interrupt]
    fn TIM2() {
        mpu::isr(|| {
            alarm::on_interrupt();
        });
    }

    /// Der Job-Interrupt (4.5): rechnet den Auftrag und meldet ihn der
    /// naechsten Runde.
    #[interrupt]
    fn EXTI1() {
        mpu::isr(|| {
            host::work_jobs();
            JOB_DONE.store(true, Ordering::SeqCst);
        });
    }

    /// Laesst den Job-Interrupt anstehen. Er rechnet, bevor die Hauptschleife
    /// weiterlaeuft: Die naechste Runde sieht seinen Auftrag schon fertig.
    fn pend_jobs() {
        NVIC::pend(JOB_LINE);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }

    /// Rechnet `us` Mikrosekunden lang nichts: die Arbeit des Wirts.
    fn spin(us: u32) {
        let n = (CORE_HZ / 1_000_000).saturating_mul(us);
        let start = cycles::now();
        while cycles::now().wrapping_sub(start) < n {}
    }

    /// Der Lauf in der Hauptschleife des Wirts, danach die Bilanz.
    pub(crate) fn run(alarm: Tim2Alarm, protection: Mpu, mut nvic: NVIC, flash: FLASH) -> ! {
        // SAFETY: einmal je Lauf; `run` kehrt nicht zurueck.
        let (mut program, journal) = unsafe { program(flash) };
        hand_over_jobs(&mut program);
        // Die Abschlusszeile meldet, wie tief der Stack unter Wirt, Jobs und
        // Schritt reichte: gemalt nach `init`, der Aufbau zaehlt nicht (12.3),
        // und vor der Uhr des Laufs — das Malen dauert Millisekunden, und die
        // ersten Fristen kaemen sonst um so viel zu spaet.
        takt_board_stm32f401::stack::paint();
        let clock = if LOGICAL { Time::Logical(0) } else { Time::Board(alarm) };
        let mut rt = runtime(clock, protection, Profile::SHARED, program, journal);
        let mut form = Form::start(&mut rt, Polled(pend_jobs), LOGICAL.then_some(&GATE));
        let mut tunes = Console::new(takt_board_stm32f401::console_byte);
        // Prioritaeten und Freigabe nach dem Aufbau; die Handler oben sind
        // bereit.
        for (line, priority) in [
            (Interrupt::USART1, mpu::ISR_PRIORITY),
            (Interrupt::EXTI0, mpu::ISR_PRIORITY),
            (Interrupt::TIM2, mpu::ISR_PRIORITY),
            (JOB_LINE, JOB_PRIORITY),
        ] {
            // SAFETY: siehe oben.
            unsafe { nvic.set_priority(line, priority) };
            // SAFETY: siehe oben.
            unsafe { NVIC::unmask(line) };
        }
        let mut work = Work::new();
        let mut longest: u32 = 0;
        let stats = loop {
            let job_done = JOB_DONE.swap(false, Ordering::SeqCst);
            if let Some(stats) = form.poll(&mut rt, Some(&mut tunes), job_done) {
                break stats;
            }
            // Eine Runde des Wirts, vom Ende eines Aufrufs bis zum naechsten.
            let started = cycles::now();
            if let Some(u) = uart() {
                u.flush();
            }
            spin(HOST_WORK_US);
            work.round(alarm.now_ns());
            longest = longest.max(cycles::now().wrapping_sub(started));
        };
        work.report(u64::from(longest).saturating_mul(1_000_000_000).checked_div(u64::from(CORE_HZ)));
        conclude(&rt, &stats);
        host::park()
    }
}

/// Die Form `rtos` unter RTIC 2 (12.8, 12.11): Den Schritt rechnet eine
/// Aufgabe hoher Prioritaet, die die ISR des Alarms zur Frist weckt — dieselbe
/// [`Form`](takt_rt_baremetal::interrupt::Form) wie in der Interruptform, mit
/// dem Planer von RTIC statt der Prioritaet einer ISR. Nach jedem Auftrag
/// weckt die Job-Aufgabe sie ebenso, und sie verteilt den naechsten (FB-512).
/// Die Idle-Aufgabe ist die Hauptschleife des Wirts: Sie zaehlt ihre Runden,
/// oeffnet in logischer Zeit das Tor der Frist und fuehrt am Ende den Lauf
/// aus (12.11).
#[cfg(form = "rtos")]
mod rtic_form {
    use core::sync::atomic::{AtomicBool, Ordering};

    use rtic_sync::signal::{SignalReader, SignalWriter};
    use stm32f4::stm32f401::FLASH;
    use takt_board_stm32f401::alarm::Tim2Alarm;
    use takt_rt_baremetal::interrupt::{Alarm, Form, Time, release};

    use super::host::{self, Work};
    use super::{Console, LOGICAL, Mpu, Profile, Stats, Takt, conclude, hand_over_jobs, program, runtime, uart};

    /// Die Frist ist erreicht; die Takt-Aufgabe rechnet den Schritt.
    pub(crate) static DUE: AtomicBool = AtomicBool::new(false);

    /// Die Job-Aufgabe hat ihren Auftrag gerechnet; die Takt-Aufgabe
    /// verteilt den naechsten.
    pub(crate) static JOB_DONE: AtomicBool = AtomicBool::new(false);

    /// In logischer Zeit das Tor der Frist; die Idle-Aufgabe oeffnet es.
    static GATE: AtomicBool = AtomicBool::new(false);

    /// Der Lauf ist zu Ende und liegt in [`ENDED_RUN`].
    static ENDED: AtomicBool = AtomicBool::new(false);

    /// Ein beendeter Lauf mit seiner Bilanz.
    struct Ended {
        rt: Takt<Time<Tim2Alarm>>,
        stats: Stats,
    }

    /// Der beendete Lauf. Die Takt-Aufgabe legt ihn ab und rechnet danach
    /// nicht mehr mit ihm; ab [`ENDED`] gehoert er der Idle-Aufgabe.
    static mut ENDED_RUN: Option<Ended> = None;

    /// Der Alarm der Takt-Aufgabe: TIM2 auf die Frist, die Jobs an die
    /// Job-Aufgabe.
    pub(crate) struct TaskAlarm {
        pub(crate) tim2: Tim2Alarm,
        pub(crate) work: SignalWriter<'static, ()>,
    }

    impl Alarm for TaskAlarm {
        fn arm(&mut self, at: i64) {
            self.tim2.arm(at);
        }

        fn pend_jobs(&mut self) {
            self.work.write(());
        }
    }

    /// Die Takt-Aufgabe: an der Frist der Schritt, nach einem Auftrag der
    /// Job-Aufgabe das Verteilen, danach die Leitung; am Ende geht der Lauf
    /// an die Idle-Aufgabe.
    pub(crate) async fn run(alarm: TaskAlarm, protection: Mpu, mut wake: SignalReader<'static, ()>, flash: FLASH) {
        // SAFETY: einmal je Lauf; die Aufgabe startet einmal.
        let (mut program, journal) = unsafe { program(flash) };
        hand_over_jobs(&mut program);
        let clock = if LOGICAL { Time::Logical(0) } else { Time::Board(alarm.tim2) };
        let mut rt = runtime(clock, protection, Profile::SHARED, program, journal);
        let mut form = Form::start(&mut rt, alarm, LOGICAL.then_some(&GATE));
        let mut tunes = Console::new(takt_board_stm32f401::console_byte);
        let stats = loop {
            wake.wait().await;
            if DUE.swap(false, Ordering::SeqCst)
                && let Some(stats) = form.on_alarm(&mut rt, Some(&mut tunes))
            {
                break stats;
            }
            if JOB_DONE.swap(false, Ordering::SeqCst) {
                form.on_job_done(&mut rt, Some(&mut tunes));
            }
            if let Some(u) = uart() {
                u.flush();
            }
        };
        // SAFETY: Bis `ENDED` liest niemand den Platz; danach rechnet diese
        // Aufgabe nicht mehr mit dem Lauf.
        unsafe { ENDED_RUN = Some(Ended { rt, stats }) };
        ENDED.store(true, Ordering::SeqCst);
    }

    /// Die Idle-Aufgabe: die Hauptschleife des Wirts. In logischer Zeit gibt
    /// sie den Alarm frei, sobald sie laeuft, denn dann ruht jede Aufgabe
    /// (`release`); nach dem Ende schreibt sie die Bilanz.
    pub(crate) fn host(mut alarm: Tim2Alarm) -> ! {
        let mut work = Work::new();
        while !ENDED.load(Ordering::SeqCst) {
            if LOGICAL {
                release(&GATE, &mut alarm);
            }
            work.round(alarm.now_ns());
        }
        let slot = &raw mut ENDED_RUN;
        // SAFETY: Ab `ENDED` gehoert der Lauf dieser Aufgabe.
        if let Some(ended) = unsafe { slot.as_mut() }.and_then(Option::take) {
            work.report(None);
            conclude(&ended.rt, &ended.stats);
        }
        host::park()
    }
}

/// Die Last unter RTIC (Profil `shared`, 12.8, M10 Schritt 16): eine Funk-ISR, die alles
/// unterbricht, und eine Treiber-Aufgabe mit kritischen Abschnitten. Ihre
/// Zahlen liegen in der Groessenordnung, die 12.8 nennt — die Takt-Aufgabe
/// beginnt zweistellige Mikrosekunden nach der Grenze.
#[cfg(form = "rtos")]
mod load {
    use takt_board_stm32f401::{CORE_HZ, TIMER_HZ, cycles};

    /// Die Funk-ISR kommt alle 577 us und rechnet je Aufruf 20 us. Die
    /// Periode hat keinen gemeinsamen Teiler mit einem Tick in ganzen
    /// Millisekunden: Ihre Phase wandert ueber die Tickgrenzen, statt
    /// immer neben ihnen zu liegen — beide Timer zaehlen denselben Takt.
    const RADIO_PERIOD_US: u32 = 577;
    const RADIO_US: u32 = 20;
    /// Jeder dritte Aufruf weckt die Treiber-Aufgabe.
    pub(crate) const DRIVER_EVERY: u32 = 3;
    /// Die Treiber-Aufgabe haelt einen kritischen Abschnitt von 30 us und
    /// rechnet dann 200 us.
    pub(crate) const CRITICAL_US: u32 = 30;
    const WORK_US: u32 = 200;

    /// Rechnet `us` Mikrosekunden lang nichts.
    pub(crate) fn spin(us: u32) {
        let n = (CORE_HZ / 1_000_000).saturating_mul(us);
        let start = cycles::now();
        while cycles::now().wrapping_sub(start) < n {}
    }

    /// TIM3 als Takt der Funk-ISR, alle `RADIO_PERIOD_US`.
    pub(crate) fn start() {
        // TIM3 und sein Takt gehoeren allein der Last; niemand sonst benutzt
        // sie.
        // SAFETY: die Register des RCC; geschrieben wird nur `TIM3EN`.
        let rcc = unsafe { &*stm32f4::stm32f401::RCC::ptr() };
        // SAFETY: die Register von TIM3, siehe oben.
        let tim3 = unsafe { &*stm32f4::stm32f401::TIM3::ptr() };
        rcc.apb1enr().modify(|_, w| w.tim3en().set_bit());
        let _ = rcc.apb1enr().read();
        let psc = takt_board_support::prescaler_for(CORE_HZ, TIMER_HZ).unwrap_or(0);
        // SAFETY: `PSC` nimmt jeden 16-Bit-Wert.
        tim3.psc().write(|w| unsafe { w.psc().bits(psc) });
        // SAFETY: `ARR` nimmt jeden Wert seiner 16 Bit; die Periode passt hinein.
        tim3.arr().write(|w| unsafe { w.bits(TIMER_HZ / 1_000_000 * RADIO_PERIOD_US - 1) });
        tim3.egr().write(|w| w.ug().set_bit());
        // SAFETY: `SR` ist `rc_w0`; das Muster loescht nur `UIF`.
        tim3.sr().write(|w| unsafe { w.bits(!1) });
        tim3.dier().modify(|_, w| w.uie().set_bit());
        tim3.cr1().modify(|_, w| w.cen().set_bit());
    }

    /// Ein Aufruf der Funk-ISR.
    pub(crate) fn radio() {
        // SAFETY: nur das Statusregister des eigenen Timers.
        let tim3 = unsafe { &*stm32f4::stm32f401::TIM3::ptr() };
        // SAFETY: `SR` ist `rc_w0`; das Muster loescht nur `UIF`.
        tim3.sr().write(|w| unsafe { w.bits(!1) });
        spin(RADIO_US);
    }

    /// Was die Treiber-Aufgabe nach ihrem kritischen Abschnitt rechnet.
    pub(crate) fn driver_work() {
        spin(WORK_US);
    }
}

/// Profil `shared` (12.8): Takt als hoechstpriore Aufgabe unter RTIC 2.
///
/// **Prioritaeten, von oben.** Die Leitung (USART1, 6): Ihr Empfangsregister
/// fasst ein Byte, bei 921 600 Baud kommt alle 10,9 us eines, und das Wort
/// `TAKT` des Hosts darf keines verlieren. Die Funk-ISR (TIM3, 5) steht fuer
/// einen Funkstack, der den Rest unterbricht; der Alarm (TIM2, 4); dann die
/// Takt-Aufgabe (3), die Treiber-Aufgabe (2), die Jobs (1) und die
/// Idle-Aufgabe als Hauptschleife des Wirts. `MemoryManagement` steht ueber
/// allen (12.3).
///
/// **Der kritische Abschnitt ist eine Sperre, keine Interruptsperre.** Die
/// Treiber-Aufgabe teilt mit der ISR des Alarms den Zaehler der Fristen; ihre
/// Sperre hebt die Prioritaet bis zur Decke 4, wie ein kritischer Abschnitt
/// eines RTOS bis zu seiner hoechsten Systemprioritaet maskiert. Sie haelt
/// so Alarm und Takt-Aufgabe auf, die Funk-ISR und die Leitung nicht. Was
/// die Takt-Aufgabe verspaetet, misst `drift` jedes Ticks.
#[cfg(form = "rtos")]
#[rtic::app(device = stm32f4::stm32f401, peripherals = true, dispatchers = [SPI1, SPI2, SPI3])]
mod rtic_app {
    use core::sync::atomic::Ordering;

    use rtic_sync::signal::{Signal, SignalReader, SignalWriter};
    use takt_board_stm32f401::alarm::{self, Tim2Alarm};
    use takt_board_stm32f401::{Mpu, mpu};

    use super::rtic_form::{DUE, JOB_DONE, TaskAlarm};

    /// Was die Takt-Aufgabe weckt: die Frist aus der ISR des Alarms und der
    /// gerechnete Auftrag der Job-Aufgabe; den Grund tragen `DUE` und
    /// `JOB_DONE`.
    static WAKE: Signal<()> = Signal::new();
    /// Ein Auftrag fuer die Job-Aufgabe (4.5).
    static WORK: Signal<()> = Signal::new();
    /// Ein Anlass fuer die Treiber-Aufgabe, aus der Funk-ISR.
    static DRIVER: Signal<()> = Signal::new();

    #[shared]
    struct Shared {
        /// Die Fristen seit dem Start; die ISR des Alarms zaehlt, die
        /// Treiber-Aufgabe haelt den Zaehler in ihrem kritischen Abschnitt.
        boundaries: u32,
    }

    #[local]
    struct Local {
        due: SignalWriter<'static, ()>,
        done: SignalWriter<'static, ()>,
        takt: Option<(TaskAlarm, Mpu, SignalReader<'static, ()>, stm32f4::stm32f401::FLASH)>,
        host: Tim2Alarm,
        driver_in: SignalWriter<'static, ()>,
        driver_out: SignalReader<'static, ()>,
        work: SignalReader<'static, ()>,
    }

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let super::Setup { timer, protection, flash } = super::setup(cx.device, cx.core);
        super::load::start();
        let (due, wake) = WAKE.split();
        let (hand_over, work) = WORK.split();
        let (driver_in, driver_out) = DRIVER.split();
        // Jede Aufgabe startet hier zum ersten Mal; `spawn` scheitert nur fuer
        // eine, die schon laeuft.
        let (Ok(()), Ok(()), Ok(())) = (takt::spawn(), driver::spawn(), jobs::spawn()) else { super::halt() };
        // Die Abschlusszeile meldet, wie tief der Stack unter den Aufgaben
        // reichte: gemalt am Ende des Aufbaus (12.3).
        takt_board_stm32f401::stack::paint();
        let takt = Some((TaskAlarm { tim2: timer, work: hand_over }, protection, wake, flash));
        let done = due.clone();
        (Shared { boundaries: 0 }, Local { due, done, takt, host: timer, driver_in, driver_out, work })
    }

    /// Die Hauptschleife des Wirts (`rtic_form::host`).
    #[idle(local = [host])]
    fn idle(cx: idle::Context) -> ! {
        super::rtic_form::host(*cx.local.host)
    }

    /// Der Alarm: An der Frist weckt er die Takt-Aufgabe; ein Ueberlauf der
    /// Zeitachse zaehlt nur ihre Epoche.
    #[task(binds = TIM2, priority = 4, local = [due], shared = [boundaries])]
    fn tim2(mut cx: tim2::Context) {
        if mpu::isr(alarm::on_interrupt) {
            cx.shared.boundaries.lock(|b| *b = b.wrapping_add(1));
            DUE.store(true, Ordering::SeqCst);
            cx.local.due.write(());
        }
    }

    #[task(binds = USART1, priority = 6)]
    fn usart1(_: usart1::Context) {
        mpu::isr(takt_board_stm32f401::uart::on_interrupt);
    }

    #[task(binds = TIM3, priority = 5, local = [driver_in, calls: u32 = 0])]
    fn radio(cx: radio::Context) {
        mpu::isr(super::load::radio);
        *cx.local.calls = cx.local.calls.wrapping_add(1);
        if *cx.local.calls % super::load::DRIVER_EVERY == 0 {
            cx.local.driver_in.write(());
        }
    }

    #[task(priority = 3, local = [takt])]
    async fn takt(cx: takt::Context) {
        if let Some((alarm, protection, wake, flash)) = cx.local.takt.take() {
            super::rtic_form::run(alarm, protection, wake, flash).await;
        }
    }

    #[task(priority = 2, local = [driver_out], shared = [boundaries])]
    async fn driver(mut cx: driver::Context) {
        loop {
            cx.local.driver_out.wait().await;
            cx.shared.boundaries.lock(|_| super::load::spin(super::load::CRITICAL_US));
            super::load::driver_work();
        }
    }

    /// Die Job-Aufgabe (4.5): rechnet den Auftrag und weckt die
    /// Takt-Aufgabe, die den naechsten verteilt.
    #[task(priority = 1, local = [work, done])]
    async fn jobs(cx: jobs::Context) {
        loop {
            cx.local.work.wait().await;
            super::host::work_jobs();
            JOB_DONE.store(true, Ordering::SeqCst);
            cx.local.done.write(());
        }
    }
}

/// Haelt den Kern an, wenn das Board nicht weiter kann: Er schlaeft, und was
/// ihn weckt, laesst ihn wieder schlafen.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

/// Was beim Start feststeht.
fn banner(nominal_ns: i64) {
    let Some(uart) = uart() else { return };
    uart.newline();
    uart.write("takt auf stm32f401");
    uart.newline();
    uart.write("  Kerntakt ");
    uart.write_u64(u64::from(CORE_HZ));
    uart.write(" Hz, Tick ");
    uart.write_i64(nominal_ns);
    uart.write(" ns");
    uart.newline();
    // 12.11: Die Form steht im Kopf des Laufs; `takt bench` ordnet danach
    // die Stack-Reserve zu (FB-513).
    uart.write("  Form ");
    uart.write(env!("TAKT_FORM"));
    uart.newline();
    // 12.11: Der Schutz, den die MPU herstellt; sie ist hier schon
    // eingerichtet (`Mpu::arm`), sonst stuende das Board.
    uart.write(<Mpu as Protection>::CASE.line());
    uart.newline();
    uart.write("  DWT ");
    uart.write(if cycles::running() { "laeuft" } else { "STEHT" });
    uart.newline();
    uart.newline();
    uart.mark();
    uart.flush();
}
