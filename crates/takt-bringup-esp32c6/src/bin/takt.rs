//! Ein Takt-Programm auf dem ESP32-C6 (plan/esp32c6.md Schritte 4 und 6).
//!
//! Die Schleife aus `takt-rt-baremetal` (12.1) ueber dem SYSTIMER-Alarm,
//! das erzeugte Programm hinter der Huelle seiner Lieferform (`app`, 12.11),
//! das `persist`-Journal in
//! zwei Flash-Sektoren, Schlaf in `idle`-Zustaenden als virtuelle Ticks
//! (9.9), der Trace ueber USB-Serial-JTAG. Welches Programm laeuft, sagt
//! `takt.toml`. Hier steht nur, was das Board ist: Peripherie, Geraete
//! und die Telemetriefunktionen des Rahmens.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{Button, CORE_HZ, FlashNvm, JobContext, Mwdt, Telemetry, Wire, Ws2812, platform, route_uart0};
use takt_rt_baremetal::{Cadence, Console, DRAIN_ROUNDS, JournalStats, LogicalClock, Sleep, TimerClock, Trace};
use takt_rt_core::{Clock, Journal, Loaded, NextRun, Persist, Policy, Profile, Runtime};

esp_bootloader_esp_idf::esp_app_desc!();

/// Das Programm als Lieferform (12.11): Konstanten, Arena, Treiber-Traits,
/// Kleber und Huelle, erzeugt von `takt build --emit embed` (`build.rs`).
mod app {
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

/// Alle wie viele Ticks die Ausgaenge im Betrieb ausgegeben werden.
const TRACE_EVERY: u64 = 100;

/// Konformitaetslauf (plan/esp32c6.md 5): `TAKT_TICKS` beim Bau gesetzt
/// heisst jeden Tick ausgeben, nach so vielen Ticks das Journal schreiben,
/// `takt end` und Halt.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

/// Ein Konformitaetslauf zaehlt in logischer Zeit und verliert keine
/// Zeile, auch wenn die Leitung den Trace langsamer nimmt, als der Tick
/// dauert (FB-292, FB-271). `TAKT_TIMED` beim Bau laesst die Uhr laufen,
/// fuer den Tick-Jitter von `takt bench`.
const LOGICAL: bool = TICKS.is_some() && option_env!("TAKT_TIMED").is_none();

/// `TAKT_FRESH_JOURNAL` beim Bau gesetzt: das Journal vor dem Lauf
/// loeschen, damit der Lauf wie der Interpreter ohne Speicher beginnt.
const FRESH_JOURNAL: bool = option_env!("TAKT_FRESH_JOURNAL").is_some();

/// Das Journal: die `nvs`-Partition des ESP-IDF-Schemas, das `probe-rs`
/// flasht (0x9000, 24 KiB, sonst leer); zwei Sektoren davon.
const JOURNAL_AT: u32 = 0x9000;

static mut UART: Option<Telemetry> = None;
static mut LED: Option<Ws2812> = None;
static mut BTN: Option<Button> = None;
static mut WIRE: Option<Wire> = None;

fn uart() -> Option<&'static mut Telemetry> {
    unsafe { (&raw mut UART).as_mut().and_then(Option::as_mut) }
}

fn wire() -> Option<&'static mut Wire> {
    unsafe { (&raw mut WIRE).as_mut().and_then(Option::as_mut) }
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
        // SAFETY: der Rahmen uebergibt einen nullterminierten String; die
        // Schleife endet spaetestens nach 256 Byte.
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

/// Vom Rahmen gerufen: eine Fliesskommazahl im Trace, als kuerzeste
/// Ziffernfolge, die den Wert eindeutig zurueckgibt (Bitgleichheit, 4.2).
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

/// Ein Pruefzugriff der TCB auf einen Waechter, zwischen zwei Ticks
/// (12.3): 1 unter dem Hauptstack, 2 unter dem Job-Stack.
///
/// **Die Waechter des C6 sind Daten-Watchpoints:** der von `esp-hal` auf
/// einem Wort (`__stack_chk_guard`) knapp ueber dem unteren Ende des
/// Hauptstacks, der des Boards auf den 32 Byte unter dem Job-Stack
/// (`takt_board_esp32c6::JobContext`). Ein Schreibzugriff haelt den Kern
/// mit einer Meldung an (`takt panic`), bevor ein Ueberlauf weiterschreibt.
/// Eine Region, die den Programmzustand je Tick schuetzt, gibt es hier
/// nicht: Im Maschinenmodus greifen PMP-Eintraege nur gesperrt, und
/// gesperrte lassen sich nicht je Tick umschalten.
///
/// **Ein Pruefgeraet, kein Treiber.** Nur ein Programm, das `test/...`
/// bindet, loest es aus (`devices::TestGuardWrite`); der Board-Test tut es.
static PROBE: AtomicU8 = AtomicU8::new(0);

/// Das untere Ende des Job-Stacks, 0 ohne Job-Kontext.
static JOB_STACK: AtomicU32 = AtomicU32::new(0);

/// Fuehrt einen angeforderten Pruefzugriff aus.
fn probe() {
    unsafe extern "C" {
        static mut __stack_chk_guard: u32;
    }
    let target = match PROBE.swap(0, Ordering::Relaxed) {
        1 => &raw mut __stack_chk_guard,
        2 => match JOB_STACK.load(Ordering::Relaxed) {
            0 => return,
            bottom => bottom as *mut u32,
        },
        _ => return,
    };
    // SAFETY: Genau dieser Zugriff ist verboten und soll es sein: Der
    // Watchpoint faengt ihn, und das Board haelt mit Meldung an.
    unsafe { target.write_volatile(0) };
}

/// Wie der vorige Lauf endete (12.7), beim Start aus dem Plattformblock gelesen.
static PREVIOUS_RUN: AtomicU32 = AtomicU32::new(0);

/// Die Arena, statisch. Unter ihrem Namen liest der Host ueber JTAG den Tick,
/// wenn die Konsole schweigt (`__takt_tick_at`).
#[unsafe(export_name = "app_arena")]
static mut ARENA: app::Arena = app::Arena::new();

/// Der Pruefstand des Programms (12.6): je Adresse das Geraet, das die
/// Verdrahtung nennt, sonst ein Stummel (`build.rs`).
mod drivers {
    include!(concat!(env!("OUT_DIR"), "/takt_rig.rs"));
}

/// Der Pruefstand, statisch: Das Programm haelt ihn so lange wie die Arena.
static mut RIG: Option<drivers::Rig> = None;

/// Der Griff, mit dem der Job-Faden rechnet (4.5): statisch, weil der Faden
/// ihn ueber `main` hinaus haelt.
static mut JOBS: Option<app::Jobs<'static>> = None;

/// Die Geraete des Boards (12.6): je Adresse der Typ, den `takt-drivers.toml`
/// nennt. `main` richtet die Peripherie ein; die Geraete erreichen sie ueber
/// die Statics oben.
#[allow(dead_code, reason = "ein Vorrat fuer jedes Programm; welche Geraete eines braucht, nennt sein Pruefstand")]
mod devices {
    use core::sync::atomic::Ordering;

    use takt_embed::{Device, Input, Output, Quality, Sample};

    use super::{BTN, LED, PREVIOUS_RUN, PROBE, wire};

    /// Ein Geraet, das fest am Board sitzt: Es lebt, solange das Board
    /// laeuft (12.4).
    #[derive(Debug, Default)]
    pub struct Fixed;

    impl Device for Fixed {
        fn alive(&mut self, _now: i64) -> bool {
            true
        }
    }

    /// `ui/led`: die RGB-LED an IO8.
    #[derive(Debug, Default)]
    pub struct UiLed;

    impl Output<bool> for UiLed {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            // SAFETY: ein Faden; `main` setzt die LED vor dem ersten Tick.
            let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return false };
            if value {
                led.on();
            } else {
                led.off();
            }
            true
        }
    }

    /// `ui/button`: der BOOT-Taster an IO9 (12.1 Schritt 2).
    ///
    /// Ein wackelnder Kontakt meldet `Suspect` statt `Good`: Der Wert ist
    /// da, aber noch nicht stabil (12.6).
    #[derive(Debug, Default)]
    pub struct UiButton;

    impl Input<bool> for UiButton {
        fn sample(&mut self, now: i64) -> Option<Sample<bool>> {
            // SAFETY: wie bei `UiLed`.
            let button = unsafe { (&raw mut BTN).as_mut().and_then(Option::as_mut) }?;
            let (value, stable) = button.poll();
            Some(Sample { value, quality: if stable { Quality::Good } else { Quality::Suspect }, t: now })
        }
    }

    /// `gpio/loop_out`: GPIO7, ueber die Bruecke an GPIO17 (13.8).
    #[derive(Debug, Default)]
    pub struct GpioLoopOut;

    impl Output<bool> for GpioLoopOut {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            let Some(w) = wire() else { return false };
            w.write(value);
            true
        }
    }

    /// `gpio/loop_in`: GPIO17, das Ende der Bruecke (13.8).
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

    /// `test/guard_write` (12.3): Der naechste Leerlauf schreibt in den
    /// Waechter unter dem Hauptstack.
    #[derive(Debug, Default)]
    pub struct TestGuardWrite;

    impl Output<bool> for TestGuardWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            if value {
                PROBE.store(1, Ordering::Relaxed);
            }
            true
        }
    }

    /// `test/job_guard_write` (12.3): Der naechste Leerlauf schreibt in den
    /// Waechter unter dem Job-Stack.
    #[derive(Debug, Default)]
    pub struct TestJobGuardWrite;

    impl Output<bool> for TestJobGuardWrite {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            if value {
                PROBE.store(2, Ordering::Relaxed);
            }
            true
        }
    }

    /// `test/tick_stretch`, das Pruefgeraet fuer 12.6 Zeile 7: streckt die
    /// Periode des Alarms um den geschriebenen Prozentsatz.
    #[derive(Debug, Default)]
    pub struct TestTickStretch;

    impl Output<u8> for TestTickStretch {
        fn write(&mut self, percent: u8, _now: i64) -> bool {
            takt_board_esp32c6::tick::stretch(u32::from(percent));
            true
        }
    }
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
fn conduct(program: app::Program<'static>, clock: impl Clock, persist: &mut Option<Persist<'_, FlashNvm>>) {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    let limit = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit == 0).then(|| Mwdt::arm(WATCHDOG_NS));
    let trace = Trace::new(Cadence::of(limit, TRACE_EVERY), TICK_NS, uart);
    let mut rt = Runtime::new(program, clock, watchdog, trace, Profile::BAREMETAL, TICK_NS, policy);
    // 8.4: Tunes vom Host kommen ueber die Gegenrichtung der Konsole.
    let mut tunes = Console::new(takt_board_esp32c6::console_byte);
    let stats = takt_rt_baremetal::run(&mut rt, persist.as_mut(), Some(&mut tunes));
    if rt.watchdog.is_some() {
        Mwdt::arm(END_OF_RUN_NS);
    }
    let journal = persist.as_ref().map_or(JournalStats::default(), |p| {
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
        let stack = Some(takt_board_esp32c6::stack::high_water());
        takt_rt_baremetal::report(u, rt.overrun(), &stats, &journal, stack);
    }
    if limit == 0 {
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

#[main]
fn main() -> ! {
    // Zuerst: Die Abschlusszeile meldet, wie tief der Stack unter Last reichte.
    takt_board_esp32c6::stack::paint();
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    takt_board_esp32c6::reenumerate_if_requested();
    PREVIOUS_RUN.store(platform::previous_run(), Ordering::Relaxed);
    let mut telemetry = takt_board_esp32c6::telemetry(peripherals.USB_DEVICE);
    let Ok(timer) = takt_board_esp32c6::init(peripherals.SYSTIMER, TICK_NS) else {
        telemetry.write("takt: Periode nicht einrichtbar");
        telemetry.newline();
        telemetry.drain(DRAIN_ROUNDS);
        loop {
            core::hint::spin_loop();
        }
    };
    telemetry.write("takt esp32c6: tick ");
    telemetry.write_i64(timer.nominal_ns());
    telemetry.write(" ns");
    telemetry.newline();
    telemetry.mark();
    let telemetry = if LOGICAL { telemetry.lossless() } else { telemetry };
    unsafe { UART = Some(telemetry) };
    if let Ok(led) = Ws2812::new(peripherals.RMT, peripherals.GPIO8) {
        unsafe { LED = Some(led) };
    }
    unsafe { BTN = Some(Button::new(peripherals.GPIO9)) };
    // 12.10: Ein `port @ mmio(...)` schreibt Register; die Verbindung zum
    // Pad macht die GPIO-Matrix, nicht der Treiber. Dieselbe Bruecke ist
    // die Messschleife, wenn das Programm sie bindet (13.8).
    if HW_ADDRESSES.iter().any(|a| a.starts_with("gpio/loop_")) {
        unsafe { WIRE = Some(Wire::new(peripherals.GPIO7, peripherals.GPIO17)) };
    } else {
        route_uart0(peripherals.GPIO7, peripherals.GPIO17);
    }

    // 5.9: s0 kommt aus dem Journal, darum laden vor dem ersten Eintritt.
    // Ohne `persist` im Programm gibt es kein Journal und keinen Flash-Zugriff.
    let (mut current, mut stored) = ([0u8; PERSIST_BOUND], [0u8; PERSIST_BOUND]);
    let mut persist = None;
    if PERSIST_BOUND != 0 {
        let mut nvm = FlashNvm::new(peripherals.FLASH, JOURNAL_AT).with_blocking_ns(NVM_BLOCKING_NS);
        if FRESH_JOURNAL && !nvm.wipe() {
            report("journal: loeschen scheiterte");
        }
        persist = Some(Persist::new(Journal::new(nvm, LOGIC_HASH, PERSIST_MIN_INTERVAL_NS), &mut current, &mut stored));
    }
    // SAFETY: Arena und Pruefstand gehoeren nur diesem Programm; `main`
    // kehrt nicht zurueck, und ausser ihm greift niemand auf sie zu.
    let (arena, rig) = unsafe { (&mut *(&raw mut ARENA), (*(&raw mut RIG)).insert(drivers::Rig::default())) };
    let mut program = app::Program::new(arena, rig);
    let loaded = persist.as_mut().map(|p| p.load(&mut program));
    program.ensure_init();
    if let Some(u) = uart() {
        match loaded {
            Some((Loaded::Found { length, sequence }, applied)) => {
                let _ = write!(u, "journal: Eintrag {sequence}, {length} Byte, {applied} Werte geladen\r\n");
            }
            Some((Loaded::Empty, _)) => u.write("journal: leer\r\n"),
            None => u.write("journal: keins\r\n"),
        }
        u.flush();
    }

    // 4.5: Jobs rechnen in der Wartezeit bis zum Tick, im eigenen Faden;
    // der Tick holt den Kern zurueck.
    let dispatch = program.dispatch();
    // SAFETY: ein Faden; der Job-Faden liest den Griff erst nach `start`.
    let worker = program.jobs().map(|j| unsafe { (*(&raw mut JOBS)).insert(j) as &mut dyn takt_embed::Jobs });
    let mut jobs = JobContext::start(app::job_stack(), dispatch, worker);
    JOB_STACK.store(jobs.as_ref().map_or(0, JobContext::bottom), Ordering::Relaxed);
    if LOGICAL {
        // Zwischen den Ticks leert die Schleife die Leitung ganz und rechnet
        // jeden Job zu Ende; dann steht die Uhr auf der Frist. In logischer
        // Zeit haelt so jeder Job seine Dauer (4.5).
        let clock = LogicalClock::new(|| {
            probe();
            if let Some(u) = uart() {
                u.drain(DRAIN_ROUNDS);
            }
            if let Some(context) = jobs.as_mut() {
                context.finish();
            }
        });
        conduct(program, clock, &mut persist);
    } else {
        let clock = TimerClock::new(timer, TICK_NS).with_idle(|| {
            probe();
            if let Some(u) = uart() {
                u.flush();
            }
            if let Some(context) = jobs.as_mut() {
                context.run();
            }
        });
        conduct(program, clock, &mut persist);
    }
    // Der Watchdog laeuft nach einem Lauf im Betrieb weiter; das Board
    // haelt an, statt neu zu starten.
    let mut sleep = takt_board_esp32c6::WfiSleep;
    loop {
        sleep.sleep_until_event();
        Mwdt::feed();
    }
}

/// Eine Zeile ausserhalb des Traces.
fn report(text: &str) {
    if let Some(u) = uart() {
        u.write(text);
        u.newline();
        u.flush();
    }
}
