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
use core::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, Ordering};

#[cfg(not(feature = "rtos"))]
use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Interrupt, NVIC, Peripherals, interrupt};
use takt_board_stm32f401::{
    BAUD, Board, CORE_HZ, Generated, Iwdg, JobContext, Led, Mpu, Telemetry, Tim2Tick, Wire, cycles, mpu, platform, tick,
};
use takt_board_support::platform::image_state;
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, Guarded, JournalStats, Stats, TimerClock};
#[cfg(not(feature = "rtos"))]
use takt_rt_baremetal::{LogicalClock, Sleep};
use takt_rt_core::{Clock, FakeNvm, Persist, PlatformCommand, Policy, Profile, Runtime};

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{HW_ADDRESSES, NVM_BLOCKING_NS, OVERRUN_ALERT, TICK_NS};

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

/// `TAKT_INSTRUMENT=statements`: den Programmzaehler je Tick mitgeben (11.2).
const TRACE_PC: bool = matches!(option_env!("TAKT_INSTRUMENT"), Some(m) if matches!(m.as_bytes(), b"statements"));

static LAST_STAMP: AtomicU32 = AtomicU32::new(0);

/// Die Telemetrie, statisch: Der erzeugte Rahmen ruft `takt_board_trace`
/// als C-Symbol, und eine Funktion ohne Empfaenger kommt an nichts heran,
/// was in `main` liegt.
static mut UART: Option<Telemetry> = None;

/// Die LED, die der Treiber `takt_out_ui_led` schaltet; aus demselben Grund.
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

/// Womit dieser Lauf begann (12.7), beim Start aus der Reset-Ursache gelesen.
static BOOT_REASON: AtomicI32 = AtomicI32::new(0);

/// Der Treiber fuer `input … @ hw("sys/boot_reason")` (12.7).
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_boot_reason(value: *mut i32, quality: *mut u8) -> bool {
    unsafe {
        *value = BOOT_REASON.load(Ordering::Relaxed);
        *quality = 0;
    }
    true
}

/// Die Starts in Folge ohne geordnetes Ende (12.7), beim Start gezaehlt.
static RESET_COUNT: AtomicU32 = AtomicU32::new(0);

/// Der Treiber fuer `input … @ hw("sys/reset_count")` (12.7).
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild; `int`
/// liegt dort als `long long`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_reset_count(value: *mut i64, quality: *mut u8) -> bool {
    unsafe {
        *value = i64::from(RESET_COUNT.load(Ordering::Relaxed));
        *quality = 0;
    }
    true
}

/// Der Treiber fuer `input … @ hw("sys/image_state")` (12.7): Ohne
/// Startstufe gibt es ein Image, und es ist bestaetigt.
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_image_state(value: *mut i32, quality: *mut u8) -> bool {
    unsafe {
        *value = image_state::CONFIRMED;
        *quality = 0;
    }
    true
}

/// Der Treiber fuer `output led : bool @ hw("ui/led")`.
///
/// Der Name ist die Adresse: Der Rahmen bildet `hw("ui/led")` auf
/// `takt_out_ui_led` ab und ruft es in Schritt 10 (12.1); wer es nicht
/// stellt, bekommt einen Linkfehler mit diesem Namen (8.10). Die LED der
/// Black Pill liegt an PC13 gegen 3V3 — was `true` elektrisch heisst,
/// weiss nur diese Zeile.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_ui_led(value: u8) {
    let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return };
    if value != 0 {
        led.on();
    } else {
        led.off();
    }
}

/// Ein Pruefzugriff der TCB auf geschuetzten Speicher (12.3): zwischen zwei
/// Ticks auf den Programmzustand (1), den Waechter unter dem Hauptstack (2)
/// oder den unter dem Job-Stack (3), oder aus einer ISR, die den Commit
/// unterbricht, auf den Programmzustand (4).
///
/// **Ein Pruefgeraet, kein Treiber.** Die Outputs `test/...` tun, was ein
/// fehlerhafter Treiber taete; nur ein Programm, das sie bindet, loest sie
/// aus, und der Board-Test tut es, um zu zeigen, dass die MPU den Zugriff
/// abweist und meldet.
static PROBE: AtomicU8 = AtomicU8::new(0);

/// `test/tcb_write` (12.3): der naechste Leerlauf schreibt in den Programmzustand.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_tcb_write(value: u8) {
    if value != 0 {
        PROBE.store(1, Ordering::Relaxed);
    }
}

/// `test/guard_write` (12.3): der naechste Leerlauf schreibt in den Waechter.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_guard_write(value: u8) {
    if value != 0 {
        PROBE.store(2, Ordering::Relaxed);
    }
}

/// `test/job_guard_write` (12.3): der naechste Leerlauf schreibt in den
/// Waechter unter dem Job-Stack.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_job_guard_write(value: u8) {
    if value != 0 {
        PROBE.store(3, Ordering::Relaxed);
    }
}

/// `test/isr_write` (12.3): Im naechsten Programmschritt schreibt die
/// Pruef-ISR in den Programmzustand (`test/in_step`).
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_isr_write(value: u8) {
    if value != 0 {
        PROBE.store(4, Ordering::Relaxed);
    }
}

/// `test/in_step` (12.3): Gelesen wird im Programmschritt, bei offenem
/// Programmzustand. Ist die Pruef-ISR angefordert, loest der Treiber sie
/// hier aus, und sie unterbricht den Schritt.
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_test_in_step(value: *mut u8, quality: *mut u8) -> bool {
    if PROBE.load(Ordering::Relaxed) == 4 {
        NVIC::pend(Interrupt::EXTI0);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
    unsafe {
        *value = 0;
        *quality = 0;
    }
    true
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

/// Der Output `gpio/loop_out` auf PA0, ueber die Bruecke an PA1 (13.8).
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_gpio_loop_out(value: u8) {
    if let Some(w) = wire() {
        w.write(value != 0);
    }
}

/// Der Input `gpio/loop_in` an PA1, das Ende der Bruecke (13.8).
///
/// # Safety
///
/// Der Rahmen uebergibt zwei gueltige Zeiger in sein Prozessabbild.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_gpio_loop_in(value: *mut u8, quality: *mut u8) -> bool {
    let Some(w) = wire() else { return false };
    let level = w.read();
    unsafe {
        *value = u8::from(level);
        *quality = 0;
    }
    true
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

/// Die Schleife ueber dem Programm: das Programm hinter dem
/// Speicherschutz (12.3), der Watchdog im Betrieb.
type Takt<C> = Runtime<Guarded<Generated, Mpu>, C, Option<Iwdg>, ()>;

/// Nach so vielen Ticks endet ein Konformitaetslauf; 0 im Betrieb.
fn limit() -> u64 {
    TICKS.and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// Baut die Schleife unter `clock` im Profil `profile` (12.8).
fn runtime<C: Clock>(clock: C, protection: Mpu, profile: Profile) -> Takt<C> {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit() == 0).then(|| Iwdg::arm(WATCHDOG_NS));
    // 12.3: Nach `init` ist der Programmzustand nur noch im Tick beschreibbar.
    let program = Guarded::new(Generated::init(false), protection);
    Runtime::new(program, clock, watchdog, (), profile, TICK_NS, policy)
}

/// Wie oft der Lauf ausgibt und wann er endet.
fn cadence() -> Cadence {
    Cadence::of(limit(), TRACE_EVERY, TRACE_PC)
}

/// Kein Journal: Das Board hat noch keinen `Nvm`-Treiber (5.9).
fn no_journal<'a>() -> Option<&'a mut Persist<'a, FakeNvm<0>>> {
    None
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
#[cfg(not(feature = "rtos"))]
fn conduct(clock: impl Clock, protection: Mpu) {
    let mut rt = runtime(clock, protection, Profile::BAREMETAL);
    let stats = takt_rt_baremetal::run(&mut rt, no_journal(), cadence(), uart);
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
        let stack = Some(takt_board_stm32f401::stack::high_water());
        takt_rt_baremetal::report(u, rt.overrun(), stats, &JournalStats::default(), stack);
    }
    if limit() == 0 {
        platform(stats.command);
    }
}

/// Fuehrt ein Kommando an die Plattform aus (12.7).
///
/// Ein Konformitaetslauf endet wie der Wirtsrahmen mit dem Trace, und das
/// Board bleibt fuer das naechste Programm erreichbar; nur im Betrieb
/// fuehrt es das Kommando aus. Vorher geht die Leitung ganz hinaus: Reset
/// und Tiefschlaf naehmen mit, was noch in ihrem Puffer steht (FB-314).
fn platform(command: Option<PlatformCommand>) {
    let Some(command) = command else { return };
    if let Some(u) = uart() {
        u.finish(DRAIN_ROUNDS);
    }
    match command {
        PlatformCommand::Restart => platform::restart(),
        PlatformCommand::DeepSleep(duration) => platform::deep_sleep(duration),
        // TODO(M10 Schritt 17): Der Sprung braucht Slots, die erst das
        // Profil `boot` einrichtet; bis dahin haelt das Board mit
        // `safe`-Ausgaengen an und sagt es.
        PlatformCommand::Jump(_) => {
            if let Some(u) = uart() {
                u.write("takt: der Sprung in einen Slot braucht das Profil `boot`");
                u.newline();
                u.drain(DRAIN_ROUNDS);
            }
        }
    }
}

/// Was jede Bindung vor dem ersten Tick einrichtet.
struct Setup {
    timer: Tim2Tick,
    protection: Mpu,
    /// Der Job-Faden des blanken Boards (4.5); unter RTIC rechnen Jobs in
    /// einer eigenen Aufgabe.
    #[cfg(not(feature = "rtos"))]
    jobs: Option<JobContext>,
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
    let boot_reason = platform::boot_reason();
    BOOT_REASON.store(boot_reason, Ordering::Relaxed);
    RESET_COUNT.store(platform::reset_count(boot_reason), Ordering::Relaxed);
    // Dann: Die Abschlusszeile meldet, wie tief der Stack unter Last reichte.
    takt_board_stm32f401::stack::paint();
    // 12.3: Der Programmzustand liegt ausserhalb von `.bss`; genullt wird er,
    // bevor der Rahmen ihn zum ersten Mal beschreibt.
    mpu::clear_state();
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

    // 4.5: Jobs rechnen in der Wartezeit bis zum Tick, im eigenen Faden;
    // der Tick holt den Kern zurueck.
    let jobs = if cfg!(feature = "rtos") { None } else { JobContext::start() };

    #[cfg(not(feature = "rtos"))]
    let nvic = cp.NVIC;
    let (mut dcb, mut dwt, mut core_mpu, mut scb) = (cp.DCB, cp.DWT, cp.MPU, cp.SCB);
    cycles::enable(&mut dcb, &mut dwt);
    let Some(protection) = Mpu::arm(&mut core_mpu, &mut scb, jobs.as_ref().map(JobContext::bottom)) else {
        if let Some(u) = uart() {
            u.write("takt: Speicherschutz nicht einrichtbar");
            u.newline();
            u.drain(DRAIN_ROUNDS);
        }
        loop {
            cortex_m::asm::wfi();
        }
    };
    banner(timer.nominal_ns());
    unsafe { LED = Some(led) };
    Setup {
        timer,
        protection,
        #[cfg(not(feature = "rtos"))]
        jobs,
        #[cfg(not(feature = "rtos"))]
        nvic,
    }
}

#[cfg(not(feature = "rtos"))]
#[entry]
fn main() -> ! {
    let dp = Peripherals::take().expect("Peripherie");
    let cp = cortex_m::Peripherals::take().expect("Kern-Peripherie");
    let Setup { timer, protection, mut jobs, mut nvic } = setup(dp, cp);
    // SAFETY: Prioritaeten und Freigabe vor dem ersten Tick; die Handler
    // oben sind bereit.
    unsafe {
        for line in [Interrupt::TIM2, Interrupt::USART1, Interrupt::EXTI0] {
            nvic.set_priority(line, mpu::ISR_PRIORITY);
            NVIC::unmask(line);
        }
    }

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

#[cfg(feature = "rtos")]
impl takt_rt_rtos::Boundary for TaskBoundary {
    async fn reached(&mut self) {
        if takt_mcu_program::jobs::dispatch() {
            self.work.write(());
        }
        if let Some(u) = uart() {
            u.flush();
        }
        self.reached.wait().await;
    }
}

/// Die Takt-Aufgabe (12.8): der Lauf, die Abschlusszeile, danach die
/// offene Leitung wie auf dem blanken Board.
#[cfg(feature = "rtos")]
async fn conduct_rtos(timer: Tim2Tick, protection: Mpu, mut boundary: TaskBoundary) {
    if LOGICAL {
        // In logischer Zeit ist jede Grenze eine Periode (13.8); die
        // Aufgaben darunter rechnen wie im Betrieb.
        let now = core::cell::Cell::new(0);
        let mut rt = runtime(takt_rt_rtos::LogicalTime(&now), protection, Profile::RTOS);
        let mut logical = takt_rt_rtos::Logical::new(&mut boundary, &now, TICK_NS);
        let stats = takt_rt_rtos::run(&mut rt, no_journal(), cadence(), uart, &mut logical).await;
        conclude(&rt, &stats);
    } else {
        let mut rt = runtime(TimerClock::new(timer, TICK_NS), protection, Profile::RTOS);
        let stats = takt_rt_rtos::run(&mut rt, no_journal(), cadence(), uart, &mut boundary).await;
        conclude(&rt, &stats);
    }
    loop {
        takt_rt_rtos::Boundary::reached(&mut boundary).await;
        Iwdg::feed();
    }
}

/// Die Last unter `rtos` (12.8, M10 Schritt 16): eine Funk-ISR, die alles
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

/// Profil `rtos` (12.8): Takt als hoechstpriore Aufgabe unter RTIC 2.
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
mod app {
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
            takt_mcu_program::jobs::work();
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
