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
//! `tools/takt-on-board.sh` macht alle drei Schritte.
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

#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "Interrupt-Handler und C-ABI; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

#[cfg(not(feature = "rtos"))]
use cortex_m_rt::entry;
use panic_halt as _;
#[cfg(not(feature = "rtos"))]
use stm32f4::stm32f401::{Interrupt, NVIC};
use stm32f4::stm32f401::{Peripherals, interrupt};
#[cfg(not(feature = "rtos"))]
use takt_board_stm32f401::JobContext;
use takt_board_stm32f401::{
    BAUD, Board, CORE_HZ, Iwdg, Led, Mpu, Telemetry, Tim2Tick, Wire, cycles, mpu, platform, tick,
};
use takt_rt_baremetal::{Cadence, Console, DRAIN_ROUNDS, Guarded, JournalStats, Stats, TimerClock, Trace};
#[cfg(not(feature = "rtos"))]
use takt_rt_baremetal::{LogicalClock, Sleep};
use takt_rt_core::{Clock, FakeNvm, NextRun, Persist, Policy, Profile, Runtime};

/// Das Programm als Lieferform (12.11): Konstanten, Arena, Treiber-Traits,
/// Kleber und Huelle, erzeugt von `takt build --emit embed` (`build.rs`).
mod app {
    #![allow(dead_code)]
    include!(env!("TAKT_APP_RS"));
}
use app::{HW_ADDRESSES, NVM_BLOCKING_NS, OVERRUN_ALERT, TICK_NS};

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

/// `TAKT_HOSTILE_FPU` beim Bau: FPSCR und FPDSCR vor dem Lauf auf
/// Flush-to-Zero, Default-NaN und Rundung gegen null, wie ein Wirt sie fuer
/// seinen eigenen Code setzen darf (4.2, 12.11). Jeder Einstieg stellt die
/// IEEE-Umgebung selbst her; die Abschlusszeile nennt FPSCR nach dem Lauf.
const HOSTILE_FPU: bool = option_env!("TAKT_HOSTILE_FPU").is_some();

/// DN (25), FZ (24) und RMode = gegen null (23:22) in FPSCR und FPDSCR.
const HOSTILE_FPSCR: u32 = 0x03C0_0000;

static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Die Telemetrie, statisch: Der erzeugte Rahmen ruft `takt_board_trace`
/// als C-Symbol, und eine Funktion ohne Empfaenger kommt an nichts heran,
/// was in `main` liegt.
static mut UART: Option<Telemetry> = None;

/// Die LED, die das Geraet `devices::UiLed` schaltet; aus demselben Grund.
static mut LED: Option<Led> = None;

/// Die Messschleife an PA0 und PA1, wenn das Programm sie bindet (13.8).
static mut WIRE: Option<Wire> = None;

fn wire() -> Option<&'static mut Wire> {
    unsafe { (&raw mut WIRE).as_mut().and_then(Option::as_mut) }
}

fn uart() -> Option<&'static mut Telemetry> {
    unsafe { (&raw mut UART).as_mut().and_then(Option::as_mut) }
}

/// Vom Rahmen gerufen: eine Zeile Trace, nullterminiert.
///
/// # Safety
///
/// Der Rahmen uebergibt einen nullterminierten Zeiger auf statischen
/// Text; die Schranke haelt einen Zeiger ohne Null auf (4.1, von Hand).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const u8) {
    let Some(uart) = uart() else { return };
    let mut p = text;
    for _ in 0..256 {
        let b = unsafe { *p };
        if b == 0 {
            return;
        }
        uart.write_byte(b);
        p = unsafe { p.add(1) };
    }
}

/// Vom Rahmen gerufen: eine Zahl im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    let Some(uart) = uart() else { return };
    uart.write_i64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    let Some(uart) = uart() else { return };
    uart.write_u64(value);
    uart.write_byte(b' ');
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige Ziffernfolge (4.2).
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    let Some(uart) = uart() else { return };
    let _ = write!(uart, "{value:?} ");
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    let Some(uart) = uart() else { return };
    uart.write_hex8(value);
}

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
/// an der Region aus.
#[unsafe(link_section = ".takt_state")]
static mut ARENA: app::Arena = app::Arena::new();

/// Der Pruefstand des Programms (12.6): je Adresse das Geraet, das die
/// Verdrahtung nennt, sonst ein Stummel (`build.rs`).
mod drivers {
    include!(concat!(env!("OUT_DIR"), "/takt_rig.rs"));
}

/// Der Pruefstand, statisch: Das Programm haelt ihn so lange wie die Arena.
static mut RIG: Option<drivers::Rig> = None;

/// Der Griff, mit dem der Job-Faden oder die Job-Aufgabe rechnet (4.5):
/// statisch, weil sie ihn ueber den Aufbau hinaus halten.
static mut JOBS: Option<app::Jobs<'static>> = None;

/// Das Programm auf seiner Arena (12.11).
///
/// # Safety
///
/// Einmal je Lauf: Arena und Pruefstand gehoeren danach dem Programm.
unsafe fn program() -> app::Program<'static> {
    // SAFETY: siehe oben.
    let (arena, rig) = unsafe { (&mut *(&raw mut ARENA), (*(&raw mut RIG)).insert(drivers::Rig::default())) };
    app::Program::new(arena, rig)
}

/// Legt den Griff der Jobs ab, mit dem der Job-Faden oder die Job-Aufgabe
/// rechnet; `None`, wenn das Programm keine Jobs hat.
fn hand_over_jobs(program: &mut app::Program<'static>) -> Option<&'static mut dyn takt_embed::Jobs> {
    // SAFETY: ein Aufruf je Lauf, bevor jemand rechnet; danach benutzt den
    // Griff nur der Job-Kontext.
    program.jobs().map(|j| unsafe { (*(&raw mut JOBS)).insert(j) as &mut dyn takt_embed::Jobs })
}

/// Die Geraete des Boards (12.6): je Adresse der Typ, den `takt-drivers.toml`
/// nennt. `setup` richtet die Peripherie ein; die Geraete erreichen sie
/// ueber die Statics oben.
#[allow(dead_code, reason = "ein Vorrat fuer jedes Programm; welche Geraete eines braucht, nennt sein Pruefstand")]
mod devices {
    use core::sync::atomic::Ordering;

    use stm32f4::stm32f401::{Interrupt, NVIC};
    use takt_embed::{Device, Input, Output, Sample};

    use super::{LED, PREVIOUS_RUN, PROBE, wire};

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
            // SAFETY: ein Faden; `setup` setzt die LED vor dem ersten Tick.
            let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return false };
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
            Some(Sample::good(PREVIOUS_RUN.load(Ordering::Relaxed), now))
        }
    }

    /// Ein Ausgang, der einen Pruefzugriff anfordert: `true` setzt `probe`.
    fn request(value: bool, probe: u8) -> bool {
        if value {
            PROBE.store(probe, Ordering::Relaxed);
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
            if PROBE.load(Ordering::Relaxed) == 4 {
                NVIC::pend(Interrupt::EXTI0);
                cortex_m::asm::dsb();
                cortex_m::asm::isb();
            }
            Some(Sample::good(false, now))
        }
    }

    /// `test/tick_stretch`, das Pruefgeraet fuer 12.6 Zeile 7: streckt die
    /// Periode von TIM2 um den geschriebenen Prozentsatz.
    #[derive(Debug, Default)]
    pub struct TestTickStretch;

    impl Output<u8> for TestTickStretch {
        fn write(&mut self, percent: u8, _now: i64) -> bool {
            takt_board_stm32f401::tick::stretch(u32::from(percent));
            true
        }
    }
}

/// Fuehrt einen angeforderten Pruefzugriff aus; ausserhalb des Ticks, wo
/// kein Code der TCB den Programmzustand beschreiben darf.
#[cfg(not(feature = "rtos"))]
fn probe() {
    let target = match PROBE.load(Ordering::Relaxed) {
        1 => Some(mpu::state_address()),
        2 => Some(mpu::guard_address()),
        3 => mpu::job_guard_address(),
        _ => return,
    };
    PROBE.store(0, Ordering::Relaxed);
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
    unsafe { (target as *mut u8).write_volatile(0xA5) };
}

/// Die Pruef-ISR (`test/isr_write`): eine Leitung, die kein Treiber
/// benutzt, nur von `test/in_step` ausgeloest.
#[interrupt]
fn EXTI0() {
    mpu::isr(|| {
        if PROBE.compare_exchange(4, 0, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            forbidden_write(mpu::state_address());
        }
    });
}

/// Die Tickgrenze (12.3): Zeitstempel fuer die Periode, Tickzaehler.
fn on_tim2() {
    mpu::isr(|| {
        let tim2 = unsafe { &*stm32f4::stm32f401::TIM2::ptr() };
        let now = cycles::now();
        let elapsed = now.wrapping_sub(LAST_STAMP.swap(now, Ordering::Relaxed));
        // `rc_w0`: Nullen loeschen, Einsen lassen stehen.
        tim2.sr().write(|w| unsafe { w.bits(!1) });
        tick::on_timer_interrupt(elapsed);
    });
}

#[cfg(not(feature = "rtos"))]
#[interrupt]
fn TIM2() {
    on_tim2();
}

/// Die Leitung: senden ohne zu warten, und der Host kann das Board
/// zurueckverlangen.
#[cfg(not(feature = "rtos"))]
#[interrupt]
fn USART1() {
    mpu::isr(takt_board_stm32f401::uart::on_interrupt);
}

/// Die Leitung als Senke des Kerns (12.5).
type Line = Trace<fn() -> Option<&'static mut Telemetry>, Telemetry>;

/// Die Schleife ueber dem Programm: das Programm hinter dem
/// Speicherschutz (12.3), der Watchdog im Betrieb.
type Takt<C> = Runtime<Guarded<app::Program<'static>, Mpu>, C, Option<Iwdg>, Line>;

/// Nach so vielen Ticks endet ein Konformitaetslauf; 0 im Betrieb.
fn limit() -> u64 {
    TICKS.and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// Baut die Schleife ueber `program` unter `clock` im Profil `profile` (12.8).
fn runtime<C: Clock>(clock: C, protection: Mpu, profile: Profile, mut program: app::Program<'static>) -> Takt<C> {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit() == 0).then(|| Iwdg::arm(WATCHDOG_NS));
    // 12.3: Nach `init` ist der Programmzustand nur noch im Tick beschreibbar.
    program.ensure_init();
    let program = Guarded::new(program, protection);
    let line: Line = Trace::new(Cadence::of(limit(), TRACE_EVERY), TICK_NS, uart);
    Runtime::new(program, clock, watchdog, line, profile, TICK_NS, policy)
}

/// Kein Journal: Das Board hat noch keinen `Nvm`-Treiber (5.9).
fn no_journal<'a>() -> Option<&'a mut Persist<'a, FakeNvm<0>>> {
    None
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
#[cfg(not(feature = "rtos"))]
fn conduct(clock: impl Clock, protection: Mpu, program: app::Program<'static>) {
    let mut rt = runtime(clock, protection, Profile::BAREMETAL, program);
    // 8.4: Tunes vom Host kommen ueber die Gegenrichtung der Konsole.
    let mut tunes = Console::new(takt_board_stm32f401::console_byte);
    let stats = takt_rt_baremetal::run(&mut rt, no_journal(), Some(&mut tunes));
    conclude(&rt, &stats);
}

/// Nach dem Lauf: die Abschlusszeile, im Betrieb das Kommando an die
/// Plattform.
fn conclude<C: Clock>(rt: &Takt<C>, stats: &Stats) {
    if rt.watchdog.is_some() {
        Iwdg::arm(END_OF_RUN_NS);
    }
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
        let stack = Some(takt_board_stm32f401::stack::high_water());
        takt_rt_baremetal::report(u, rt.overrun(), stats, &JournalStats::default(), stack);
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
    // SAFETY: nur die Modusbits; FPDSCR gibt sie jedem Handler mit.
    unsafe {
        fpscr::write(Fpscr::from_bits(fpscr::read().bits() | HOSTILE_FPSCR));
        (*cortex_m::peripheral::FPU::PTR).fpdscr.modify(|v| v | HOSTILE_FPSCR);
    }
}

/// Was jede Bindung vor dem ersten Tick einrichtet.
struct Setup {
    timer: Tim2Tick,
    protection: Mpu,
    /// Der Stack des Job-Fadens auf dem blanken Board (4.5); unter RTIC
    /// rechnen Jobs in einer eigenen Aufgabe.
    #[cfg(not(feature = "rtos"))]
    job_stack: Option<&'static mut [u8]>,
    /// Unter RTIC richtet die App die Interrupts nach ihren Prioritaeten ein.
    #[cfg(not(feature = "rtos"))]
    nvic: NVIC,
}

/// Richtet das Board ein: Takt, Leitung, Messschleife, Zyklenzaehler,
/// Speicherschutz, Anzeige. Haelt an, wenn Takt, Leitung oder
/// Speicherschutz nicht einzurichten sind.
fn setup(dp: Peripherals, cp: cortex_m::Peripherals) -> Setup {
    // Ein Tiefschlaf mit Rest schlaeft weiter, bevor irgendetwas laeuft (12.7).
    platform::continue_deep_sleep();
    PREVIOUS_RUN.store(platform::previous_run(), Ordering::Relaxed);
    // Dann: Die Abschlusszeile meldet, wie tief der Stack unter Last reichte.
    takt_board_stm32f401::stack::paint();
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);

    let Ok(timer) = takt_board_stm32f401::init(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, TICK_NS) else {
        // Ohne Takt keine Telemetrie: Die LED bleibt an.
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };
    let Ok(telemetry) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        led.on();
        loop {
            cortex_m::asm::wfi();
        }
    };
    let telemetry = if LOGICAL { telemetry.lossless() } else { telemetry };
    // Erst jetzt sichtbar machen: Ein Trace vor der Einrichtung schriebe
    // in ein nicht konfiguriertes Register.
    unsafe { UART = Some(telemetry) };
    if HW_ADDRESSES.iter().any(|a| a.starts_with("gpio/loop_")) {
        unsafe { WIRE = Some(Wire::new(dp.GPIOA, &dp.RCC)) };
    }

    // 4.5: Jobs rechnen in der Wartezeit bis zum Tick, im eigenen Faden auf
    // dem Stack der Lieferform; unter dessen Ende liegt ein Waechter (12.3).
    let job_stack = if cfg!(feature = "rtos") { None } else { app::job_stack() };

    #[cfg(not(feature = "rtos"))]
    let nvic = cp.NVIC;
    let (mut dcb, mut dwt, mut core_mpu, mut scb) = (cp.DCB, cp.DWT, cp.MPU, cp.SCB);
    cycles::enable(&mut dcb, &mut dwt);
    let Some(protection) = Mpu::arm(&mut core_mpu, &mut scb, job_stack.as_ref().map(|s| s.as_ptr() as u32)) else {
        if let Some(u) = uart() {
            u.write("takt: Speicherschutz nicht einrichtbar");
            u.newline();
            u.drain(DRAIN_ROUNDS);
        }
        loop {
            cortex_m::asm::wfi();
        }
    };
    if HOSTILE_FPU {
        hostile_fpu();
    }
    banner(timer.nominal_ns());
    unsafe { LED = Some(led) };
    Setup {
        timer,
        protection,
        #[cfg(not(feature = "rtos"))]
        job_stack,
        #[cfg(not(feature = "rtos"))]
        nvic,
    }
}

#[cfg(not(feature = "rtos"))]
#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let Setup { timer, protection, job_stack, mut nvic } = setup(dp, cp);
    // SAFETY: Prioritaeten und Freigabe vor dem ersten Tick; die Handler
    // oben sind bereit.
    unsafe {
        for line in [Interrupt::TIM2, Interrupt::USART1, Interrupt::EXTI0] {
            nvic.set_priority(line, mpu::ISR_PRIORITY);
            NVIC::unmask(line);
        }
    }

    // SAFETY: einmal je Lauf; `main` kehrt nicht zurueck.
    let mut program = unsafe { program() };
    let dispatch = program.dispatch();
    let mut jobs = JobContext::start(job_stack, dispatch, hand_over_jobs(&mut program));
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
                if let Some(context) = jobs.as_mut() {
                    context.finish();
                }
            }),
            protection,
            program,
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
                if let Some(context) = jobs.as_mut() {
                    context.run();
                }
            }),
            protection,
            program,
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

/// Die Tickgrenze unter RTIC (12.8): Vor dem Warten gibt die Takt-Aufgabe
/// den naechsten Job-Auftrag aus (4.5) und fuellt die Leitung nach.
#[cfg(feature = "rtos")]
struct TaskBoundary {
    reached: rtic_sync::signal::SignalReader<'static, ()>,
    work: rtic_sync::signal::SignalWriter<'static, ()>,
}

/// Die Tickgrenze mit dem Griff, der verteilt: Er entsteht mit dem Programm
/// in der Takt-Aufgabe und bleibt dort, darum nicht in [`TaskBoundary`], das
/// RTIC von `init` an die Aufgabe gibt.
#[cfg(feature = "rtos")]
struct Dispatching {
    boundary: TaskBoundary,
    dispatch: Option<app::Dispatch<'static>>,
}

#[cfg(feature = "rtos")]
impl takt_rt_rtos::Boundary for Dispatching {
    async fn reached(&mut self) {
        if self.dispatch.as_mut().is_some_and(app::Dispatch::next) {
            self.boundary.work.write(());
        }
        if let Some(u) = uart() {
            u.flush();
        }
        self.boundary.reached.wait().await;
    }
}

/// Die Takt-Aufgabe (12.8): der Lauf, die Abschlusszeile, danach die
/// offene Leitung wie auf dem blanken Board.
#[cfg(feature = "rtos")]
async fn conduct_rtos(timer: Tim2Tick, protection: Mpu, boundary: TaskBoundary) {
    // SAFETY: einmal je Lauf; die Aufgabe kehrt nicht zurueck.
    let mut program = unsafe { program() };
    let mut boundary = Dispatching { boundary, dispatch: program.dispatch() };
    hand_over_jobs(&mut program);
    let mut tunes = Console::new(takt_board_stm32f401::console_byte);
    if LOGICAL {
        // In logischer Zeit ist jede Grenze eine Periode (13.8); die
        // Aufgaben darunter rechnen wie im Betrieb.
        let now = core::cell::Cell::new(0);
        let mut rt = runtime(takt_rt_rtos::LogicalTime(&now), protection, Profile::SHARED, program);
        let mut logical = takt_rt_rtos::Logical::new(&mut boundary, &now, TICK_NS);
        let stats = takt_rt_rtos::run(&mut rt, no_journal(), Some(&mut tunes), &mut logical).await;
        conclude(&rt, &stats);
    } else {
        let mut rt = runtime(TimerClock::new(timer, TICK_NS), protection, Profile::SHARED, program);
        let stats = takt_rt_rtos::run(&mut rt, no_journal(), Some(&mut tunes), &mut boundary).await;
        conclude(&rt, &stats);
    }
    loop {
        takt_rt_rtos::Boundary::reached(&mut boundary).await;
        Iwdg::feed();
    }
}

/// Die Last unter RTIC (Profil `shared`, 12.8, M10 Schritt 16): eine Funk-ISR, die alles
/// unterbricht, und eine Treiber-Aufgabe mit kritischen Abschnitten. Ihre
/// Zahlen liegen in der Groessenordnung, die 12.8 nennt — die Takt-Aufgabe
/// beginnt zweistellige Mikrosekunden nach der Grenze.
#[cfg(feature = "rtos")]
mod load {
    use takt_board_stm32f401::{CORE_HZ, TIMER_HZ, cycles};

    /// Die Funk-ISR kommt alle 577 us und rechnet je Aufruf 20 us. Die
    /// Periode hat keinen gemeinsamen Teiler mit einem Tick in ganzen
    /// Millisekunden: Ihre Phase wandert ueber die Tickgrenzen, statt
    /// immer neben ihnen zu liegen — beide Timer zaehlen denselben Takt.
    const RADIO_PERIOD_US: u32 = 577;
    const RADIO_US: u32 = 20;
    /// Jeder dritte Aufruf weckt die Treiber-Aufgabe.
    pub const DRIVER_EVERY: u32 = 3;
    /// Die Treiber-Aufgabe haelt einen kritischen Abschnitt von 30 us und
    /// rechnet dann 200 us.
    pub const CRITICAL_US: u32 = 30;
    const WORK_US: u32 = 200;

    /// Rechnet `us` Mikrosekunden lang nichts.
    pub fn spin(us: u32) {
        let n = CORE_HZ / 1_000_000 * us;
        let start = cycles::now();
        while cycles::now().wrapping_sub(start) < n {}
    }

    /// TIM3 als Takt der Funk-ISR, alle `RADIO_PERIOD_US`.
    pub fn start() {
        // SAFETY: TIM3 und sein Takt gehoeren allein der Last; niemand
        // sonst benutzt sie.
        let (rcc, tim3) = unsafe { (&*stm32f4::stm32f401::RCC::ptr(), &*stm32f4::stm32f401::TIM3::ptr()) };
        rcc.apb1enr().modify(|_, w| w.tim3en().set_bit());
        let _ = rcc.apb1enr().read();
        let psc = takt_board_support::prescaler_for(CORE_HZ, TIMER_HZ).unwrap_or(0);
        tim3.psc().write(|w| unsafe { w.psc().bits(psc) });
        tim3.arr().write(|w| unsafe { w.bits(TIMER_HZ / 1_000_000 * RADIO_PERIOD_US - 1) });
        tim3.egr().write(|w| w.ug().set_bit());
        tim3.sr().write(|w| unsafe { w.bits(!1) });
        tim3.dier().modify(|_, w| w.uie().set_bit());
        tim3.cr1().modify(|_, w| w.cen().set_bit());
    }

    /// Ein Aufruf der Funk-ISR.
    pub fn radio() {
        // SAFETY: nur das Statusregister des eigenen Timers.
        let tim3 = unsafe { &*stm32f4::stm32f401::TIM3::ptr() };
        tim3.sr().write(|w| unsafe { w.bits(!1) });
        spin(RADIO_US);
    }

    /// Was die Treiber-Aufgabe nach ihrem kritischen Abschnitt rechnet.
    pub fn driver_work() {
        spin(WORK_US);
    }
}

/// Profil `shared` (12.8): Takt als hoechstpriore Aufgabe unter RTIC 2.
///
/// **Prioritaeten, von oben.** Die Leitung (USART1, 6): Ihr Empfangsregister
/// fasst ein Byte, bei 921 600 Baud kommt alle 10,9 us eines, und das Wort
/// `TAKT` des Hosts darf keines verlieren. Die Funk-ISR (TIM3, 5) steht fuer
/// einen Funkstack, der den Rest unterbricht; der Tick (TIM2, 4); dann die
/// Takt-Aufgabe (3), die Treiber-Aufgabe (2) und die Jobs (1).
/// `MemoryManagement` steht ueber allen (12.3).
///
/// **Der kritische Abschnitt ist eine Sperre, keine Interruptsperre.** Die
/// Treiber-Aufgabe teilt mit der Tick-ISR den Zaehler der Tickgrenzen; ihre
/// Sperre hebt die Prioritaet bis zur Decke 4, wie ein kritischer Abschnitt
/// eines RTOS bis zu seiner hoechsten Systemprioritaet maskiert. Sie haelt
/// so Tick und Takt-Aufgabe auf, die Funk-ISR und die Leitung nicht. Was
/// die Takt-Aufgabe verspaetet, misst `drift` jedes Ticks.
#[cfg(feature = "rtos")]
#[rtic::app(device = stm32f4::stm32f401, peripherals = true, dispatchers = [SPI1, SPI2, SPI3])]
mod rtic_app {
    use rtic_sync::signal::{Signal, SignalReader, SignalWriter};
    use takt_board_stm32f401::{Mpu, Tim2Tick, mpu};

    /// Die Tickgrenzen vom Timer an die Takt-Aufgabe.
    static BOUNDARY: Signal<()> = Signal::new();
    /// Ein Auftrag fuer die Job-Aufgabe (4.5).
    static WORK: Signal<()> = Signal::new();
    /// Ein Anlass fuer die Treiber-Aufgabe, aus der Funk-ISR.
    static DRIVER: Signal<()> = Signal::new();

    #[shared]
    struct Shared {
        /// Die Tickgrenzen seit dem Start; die Tick-ISR zaehlt, die
        /// Treiber-Aufgabe haelt ihn in ihrem kritischen Abschnitt.
        boundaries: u32,
    }

    #[local]
    struct Local {
        boundary: SignalWriter<'static, ()>,
        takt: Option<(Tim2Tick, Mpu, super::TaskBoundary)>,
        driver_in: SignalWriter<'static, ()>,
        driver_out: SignalReader<'static, ()>,
        work: SignalReader<'static, ()>,
    }

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let super::Setup { timer, protection } = super::setup(cx.device, cx.core);
        super::load::start();
        let (boundary, reached) = BOUNDARY.split();
        let (hand_over, work) = WORK.split();
        let (driver_in, driver_out) = DRIVER.split();
        takt::spawn().expect("Takt-Aufgabe");
        driver::spawn().expect("Treiber-Aufgabe");
        jobs::spawn().expect("Job-Aufgabe");
        let takt = Some((timer, protection, super::TaskBoundary { reached, work: hand_over }));
        (Shared { boundaries: 0 }, Local { boundary, takt, driver_in, driver_out, work })
    }

    #[task(binds = TIM2, priority = 4, local = [boundary], shared = [boundaries])]
    fn tim2(mut cx: tim2::Context) {
        super::on_tim2();
        cx.shared.boundaries.lock(|b| *b = b.wrapping_add(1));
        cx.local.boundary.write(());
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
        if let Some((timer, protection, boundary)) = cx.local.takt.take() {
            super::conduct_rtos(timer, protection, boundary).await;
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

    #[task(priority = 1, local = [work])]
    async fn jobs(cx: jobs::Context) {
        loop {
            cx.local.work.wait().await;
            // SAFETY: Die Takt-Aufgabe legt den Griff ab, bevor sie verteilt;
            // danach rechnet nur diese Aufgabe mit ihm.
            if let Some(jobs) = unsafe { (*(&raw mut super::JOBS)).as_mut() } {
                jobs.work();
            }
        }
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
    uart.write("  DWT ");
    uart.write(if cycles::running() { "laeuft" } else { "STEHT" });
    uart.newline();
    uart.newline();
    uart.mark();
    uart.flush();
}
