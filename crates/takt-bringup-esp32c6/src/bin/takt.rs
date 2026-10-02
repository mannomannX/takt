//! Ein Takt-Programm auf dem ESP32-C6 (plan/esp32c6.md Schritte 4 und 6).
//!
//! Die Schleife aus `takt-rt-baremetal` (12.1) ueber dem SYSTIMER-Alarm,
//! das erzeugte Programm hinter `Generated`, das `persist`-Journal in
//! zwei Flash-Sektoren, Schlaf in `idle`-Zustaenden als virtuelle Ticks
//! (9.9), der Trace ueber USB-Serial-JTAG. Welches Programm laeuft, sagt
//! `takt.toml`. Hier steht nur, was das Board ist: Peripherie, Treiber
//! und die Telemetriefunktionen des Rahmens.
#![no_std]
#![no_main]
#![allow(unsafe_code, reason = "C-ABI des Rahmens; 9.5 fuehrt Treiber in der TCB")]

use core::fmt::Write as _;
use core::sync::atomic::{AtomicI32, AtomicU8, AtomicU32, Ordering};

use esp_hal::clock::CpuClock;
use esp_hal::main;
use takt_board_esp32c6::{
    Button, CORE_HZ, FlashNvm, Generated, JobContext, Mwdt, Telemetry, Wire, Ws2812, platform, route_uart0,
};
use takt_board_support::edge_probe;
use takt_rt_baremetal::{Cadence, DRAIN_ROUNDS, JournalStats, LogicalClock, Sleep, TimerClock};
use takt_rt_core::{Clock, Journal, Loaded, Persist, NextRun, Policy, Profile, Runtime};

esp_bootloader_esp_idf::esp_app_desc!();

mod takt {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/takt_consts.rs"));
}
use takt::{HW_ADDRESSES, LOGIC_HASH, NVM_BLOCKING_NS, OVERRUN_ALERT, PERSIST_BOUND, PERSIST_MIN_INTERVAL_NS, TICK_NS};

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

/// `TAKT_INSTRUMENT=statements`: den Programmzaehler je Tick mitgeben (11.2).
const TRACE_PC: bool = matches!(option_env!("TAKT_INSTRUMENT"), Some(m) if matches!(m.as_bytes(), b"statements"));

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

/// Der Output `ui_led` des Programms auf der RGB-LED.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_ui_led(value: u8) -> bool {
    let Some(led) = (unsafe { (&raw mut LED).as_mut().and_then(Option::as_mut) }) else { return false };
    if value != 0 {
        led.on();
    } else {
        led.off();
    }
    true
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
/// bindet, loest es aus; der Board-Test tut es.
static PROBE: AtomicU8 = AtomicU8::new(0);

/// Das untere Ende des Job-Stacks, 0 ohne Job-Kontext.
static JOB_STACK: AtomicU32 = AtomicU32::new(0);

/// `test/guard_write` (12.3): der naechste Leerlauf schreibt in den Waechter
/// unter dem Hauptstack.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_guard_write(value: u8) -> bool {
    if value != 0 {
        PROBE.store(1, Ordering::Relaxed);
    }
    true
}

/// `test/job_guard_write` (12.3): der naechste Leerlauf schreibt in den
/// Waechter unter dem Job-Stack.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_job_guard_write(value: u8) -> bool {
    if value != 0 {
        PROBE.store(2, Ordering::Relaxed);
    }
    true
}

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

/// Der Output `gpio/loop_out` auf GPIO7, ueber die Bruecke an GPIO17 (13.8).
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_gpio_loop_out(value: u8) -> bool {
    let Some(w) = wire() else { return false };
    w.write(value != 0);
    true
}

/// Der Input `gpio/loop_in` an GPIO17, das Ende der Bruecke (13.8).
///
/// # Safety
///
/// Der Rahmen uebergibt gueltige Zeiger in sein Prozessabbild; den
/// Zeitstempel belegt er mit der Tickgrenze vor, und dabei bleibt es.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_gpio_loop_in(value: *mut u8, quality: *mut u8, _t: *mut i64) -> bool {
    let Some(w) = wire() else { return false };
    let level = w.read();
    unsafe {
        *value = u8::from(level);
        *quality = 0;
    }
    true
}

/// Wie der vorige Lauf endete (12.7), beim Start aus dem Plattformblock gelesen.
static PREVIOUS_RUN: AtomicI32 = AtomicI32::new(0);

/// Der Treiber fuer `input … @ hw("sys/previous_run")` (12.7).
///
/// # Safety
///
/// Der Rahmen uebergibt gueltige Zeiger in sein Prozessabbild; den
/// Zeitstempel belegt er mit der Tickgrenze vor, und dabei bleibt es.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_sys_previous_run(value: *mut i32, quality: *mut u8, _t: *mut i64) -> bool {
    unsafe {
        *value = PREVIOUS_RUN.load(Ordering::Relaxed);
        *quality = 0;
    }
    true
}

/// Der Input `ui_button` aus dem BOOT-Taster an IO9 (12.1 Schritt 2).
///
/// Ein wackelnder Kontakt meldet `Suspect` statt `Good`: Der Wert ist da,
/// aber noch nicht stabil (12.6). Ohne Treiber bliebe der Eintrag `Bad`,
/// und das waere hier falsch — der Taster ist verdrahtet.
///
/// # Safety
///
/// Der Rahmen uebergibt gueltige Zeiger in sein Prozessabbild; den
/// Zeitstempel belegt er mit der Tickgrenze vor, und dabei bleibt es.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_ui_button(value: *mut u8, quality: *mut u8, _t: *mut i64) -> bool {
    let Some(btn) = (unsafe { (&raw mut BTN).as_mut().and_then(Option::as_mut) }) else { return false };
    let (level, stable) = btn.poll();
    unsafe {
        *value = u8::from(level);
        *quality = if stable { 0 } else { 1 };
    }
    true
}

// --- Das Pruefgeraet des Treiberrands (12.6, M10 Schritt 29c) ---------------
//
// Die Folge steht in `takt_board_support::edge_probe`; hier nur die Grenze
// zum Rahmen. Das Programm dazu: `takt-conformance/tests/programs/
// driver_edge.takt`. Ein Programm, das diese Adressen nicht bindet, ruft
// keinen dieser Treiber.

unsafe extern "C" {
    fn takt_mcu_current_tick() -> i64;
}

/// Der Tick, den der Rahmen gerade rechnet.
fn edge_tick() -> u64 {
    // SAFETY: liest nur eine Zahl des Rahmens.
    u64::try_from(unsafe { takt_mcu_current_tick() }).unwrap_or(0)
}

/// Eine Abtastung des Pruefgeraets.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
unsafe fn edge_reading(ch: edge_probe::Scalar, value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    let Some((v, at)) = edge_probe::reading(ch, edge_tick()) else { return false };
    unsafe {
        *value = v;
        *quality = 0;
        if let Some(at) = at {
            *t = at;
        }
    }
    true
}

/// Der Treiber fuer `edge_a/p`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_a_p(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::P, value, quality, t) }
}

/// Der Treiber fuer `edge_a/q`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_a_q(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::Q, value, quality, t) }
}

/// Der Treiber fuer `edge_b/k`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_b_k(value: *mut i64, quality: *mut u8, t: *mut i64) -> bool {
    unsafe { edge_reading(edge_probe::Scalar::K, value, quality, t) }
}

/// Je Strom der Tick und die Zahl der Elemente, die er darin schon geliefert hat.
static EDGE_POLLS: [(AtomicU32, AtomicU32); 2] =
    [(AtomicU32::new(u32::MAX), AtomicU32::new(0)), (AtomicU32::new(u32::MAX), AtomicU32::new(0))];

/// Wie oft die Stroeme des Pruefgeraets, die `recorded.takt` nicht liest,
/// im laufenden Tick gefragt wurden.
static UNREAD_POLLS: [(AtomicU32, AtomicU32); 3] = [
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
    (AtomicU32::new(u32::MAX), AtomicU32::new(0)),
];

/// Ein Element des Pruefgeraets: Bytes, Zeitstempel und Folgenummer, wenn eigene.
type ProbeElement = (&'static [u8], Option<i64>, Option<i64>);

/// Das naechste Element einer Folge des Pruefgeraets: `element(tick, i)`
/// liefert Bytes, Zeitstempel und Folgenummer, ohne Angabe die des Rahmens.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
unsafe fn probe_poll(
    polls: &(AtomicU32, AtomicU32),
    element: impl Fn(u64, usize) -> Option<ProbeElement>,
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let (tick_at, polled) = polls;
    let tick = edge_tick() as u32;
    if tick_at.swap(tick, Ordering::Relaxed) != tick {
        polled.store(0, Ordering::Relaxed);
    }
    let i = polled.fetch_add(1, Ordering::Relaxed) as usize;
    let Some((bytes, at, s)) = element(u64::from(tick), i) else { return false };
    let n = bytes.len().min(usize::try_from(cap).unwrap_or(0));
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, n);
        *len = n as i32;
        if let Some(at) = at {
            *t = at;
        }
        if let Some(s) = s {
            *seq = s;
        }
    }
    true
}

/// Ein Strom des Treiberrands: Bytes und Folgenummer, der Zeitstempel ist
/// die Tickgrenze.
fn edge_element(stream: edge_probe::Stream) -> impl Fn(u64, usize) -> Option<ProbeElement> {
    move |tick, i| edge_probe::element(stream, tick, i).map(|(bytes, s)| (bytes, None, Some(s)))
}

/// Der Treiber fuer `edge_u/rx`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_u_rx(buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64) -> bool {
    let stream = edge_probe::Stream::Lines;
    unsafe { probe_poll(&EDGE_POLLS[stream as usize], edge_element(stream), buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_c/rx`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_c_rx(buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64) -> bool {
    let stream = edge_probe::Stream::Pairs;
    unsafe { probe_poll(&EDGE_POLLS[stream as usize], edge_element(stream), buf, cap, len, t, seq) }
}

/// Ein Strom, den `recorded.takt` nicht liest (8.2).
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
unsafe fn unread_poll(
    stream: edge_probe::Unread,
    buf: *mut u8,
    cap: i32,
    len: *mut i32,
    t: *mut i64,
    seq: *mut i64,
) -> bool {
    let element = move |tick, i| edge_probe::unread(stream, tick, i);
    unsafe { probe_poll(&UNREAD_POLLS[stream as usize], element, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/frames`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_frames(buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Frames, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/text`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_text(buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Text, buf, cap, len, t, seq) }
}

/// Der Treiber fuer `edge_r/raw`.
///
/// # Safety
///
/// `buf` zeigt auf `cap` schreibbare Bytes, die uebrigen auf je einen Platz.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_poll_edge_r_raw(buf: *mut u8, cap: i32, len: *mut i32, t: *mut i64, seq: *mut i64) -> bool {
    unsafe { unread_poll(edge_probe::Unread::Raw, buf, cap, len, t, seq) }
}

/// Der Ausgang `edge_o/o`: bestaetigt, ausser wenn das Pruefgeraet es nicht tut.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_edge_o_o(_value: u8) -> bool {
    edge_probe::confirms(edge_tick())
}

/// Der Heartbeat des Geraets `edge_o`.
#[unsafe(no_mangle)]
pub extern "C" fn takt_alive_edge_o() -> bool {
    edge_probe::alive(edge_tick())
}

/// Der Treiber fuer `edge_r/level`, einen Kanal, den `recorded.takt` nicht
/// liest (8.2).
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_level(value: *mut i32, quality: *mut u8, _t: *mut i64) -> bool {
    let Some((v, q)) = edge_probe::level(edge_tick()) else { return false };
    unsafe {
        *value = v;
        *quality = q;
    }
    true
}

/// Der Treiber fuer `edge_r/temp`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_temp(value: *mut f64, _quality: *mut u8, t: *mut i64) -> bool {
    let (v, at) = edge_probe::temp(edge_tick());
    unsafe {
        *value = v;
        if let Some(at) = at {
            *t = at;
        }
    }
    true
}

/// Der Treiber fuer `edge_r/on`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_on(value: *mut u8, _quality: *mut u8, _t: *mut i64) -> bool {
    unsafe { *value = u8::from(edge_probe::on(edge_tick())) };
    true
}

/// Der Treiber fuer `edge_r/mode`.
///
/// # Safety
///
/// Drei gueltige Zeiger des Rahmens.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_in_edge_r_mode(value: *mut u32, _quality: *mut u8, _t: *mut i64) -> bool {
    unsafe { *value = edge_probe::mode(edge_tick()) };
    true
}

/// Das Pruefgeraet fuer 12.6 Zeile 7: streckt die Periode des Alarms um
/// den geschriebenen Prozentsatz.
#[unsafe(no_mangle)]
pub extern "C" fn takt_out_test_tick_stretch(percent: u8) -> bool {
    takt_board_esp32c6::tick::stretch(u32::from(percent));
    true
}

/// Fuehrt das Programm unter `clock` aus und schreibt die Abschlusszeile.
fn conduct(program: Generated, clock: impl Clock, persist: &mut Option<Persist<'_, FlashNvm>>) {
    let policy = if OVERRUN_ALERT { Policy::Alert } else { Policy::Fault };
    let limit = TICKS.and_then(|t| t.parse().ok()).unwrap_or(0);
    // Der Watchdog wacht im Betrieb (12.3); ein Konformitaetslauf wartet
    // auf die Leitung und ist kein Betrieb.
    let watchdog = (limit == 0).then(|| Mwdt::arm(WATCHDOG_NS));
    let mut rt = Runtime::new(program, clock, watchdog, (), Profile::BAREMETAL, TICK_NS, policy);
    let stats = takt_rt_baremetal::run(&mut rt, persist.as_mut(), Cadence::of(limit, TRACE_EVERY, TRACE_PC), uart);
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
    let mut program = Generated::new(false);
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
    let mut jobs = JobContext::start();
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
