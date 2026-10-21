//! Zwei Programme auf einem Kern (12.11, M11 Schritt 16): `fast` mit dem
//! kuerzeren Tick in der ISR von TIM2, `slow` darunter in EXTI2, beide in der
//! Interruptform auf einer Zeitachse.
//!
//! **Prioritaeten, von oben.** `MemoryManagement` (12.3), die Leitung
//! (USART1), der Schritt von `fast` (TIM2), der von `slow` (EXTI2), ganz
//! unten die Hauptschleife des Wirts. TIM2 traegt beide Vergleiche (CC1 fuer
//! `fast`, CC2 fuer `slow`); ist die Frist von `slow` erreicht, laesst die ISR
//! EXTI2 anstehen, das rechnet, sobald `fast` fertig ist. An einer
//! gemeinsamen Grenze rechnet `fast` darum zuerst (12.11).
//!
//! **Gekoppelt ueber den Briefkasten des Boards** (`takt_board_stm32f401::
//! mailbox`): `fast` schreibt `pair/count`, `slow` liest ihn als `pair/seen`;
//! `slow` schreibt `pair/last`, `fast` liest ihn als `pair/back`. Die
//! Kopplung nennt `pair.hw` (`from`), und der Rahmen jedes Programms prueft den
//! Zeitstempel (12.11).
//!
//! **Je Programm eine Senke.** Was ein Programm schreibt — Trace, Zeitzeilen,
//! Bilanz —, geht in seine Schlange; die Hauptschleife nimmt nur ganze Zeilen
//! daraus und stellt `fast ` oder `slow ` voran (12.11). `takt end` steht
//! einmal, nach beiden Bilanzen.
//!
//! **Je Arena eine Region der MPU** (12.3): `fast` liegt in `.takt_state`,
//! `slow` in `.takt_state2`, und ein Schritt oeffnet nur die eigene. Das
//! Geraet `test/foreign_arena` schreibt aus dem Commit von `fast` in die
//! Arena von `slow`; die MPU weist es ab, und `fast` bekommt
//! `Runtime(Hardware)`.

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

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4::stm32f401::{Interrupt, NVIC, Peripherals, interrupt};
use takt_board_stm32f401::alarm::{self, Tim2Alarm};
use takt_board_stm32f401::mailbox::Mailbox;
use takt_board_stm32f401::{BAUD, Board, CORE_HZ, Led, Mpu, Telemetry, cycles, mpu};
use takt_rt_baremetal::interrupt::Form;
use takt_rt_baremetal::{Cadence, Guarded, JournalStats, NoWatchdog, Port, Protection, Stacks, Stats, Trace};
use takt_rt_core::{Clock, Policy, Profile, Runtime};

/// Das schnelle Programm (Praefix `fast`), die Lieferform aus `build.rs`.
pub mod fast {
    #![allow(dead_code)]
    include!(env!("TAKT_FAST_RS"));
}

/// Das langsame Programm (Praefix `slow`).
pub mod slow {
    #![allow(dead_code)]
    include!(env!("TAKT_SLOW_RS"));
}

/// Der Pruefstand von `fast` (12.6).
pub mod fast_drivers {
    include!(concat!(env!("OUT_DIR"), "/fast_rig.rs"));
}

/// Der Pruefstand von `slow` (12.6).
pub mod slow_drivers {
    include!(concat!(env!("OUT_DIR"), "/slow_rig.rs"));
}

/// Nach so vielen Ticks von `fast` endet der Lauf (`TAKT_TICKS`); 0 heisst nie.
const TICKS: Option<&str> = option_env!("TAKT_TICKS");

/// So weit vor dem gemeinsamen Ursprung beginnt der Lauf: Davor rechnet jedes
/// Programm in der Rangfolge Tick 0 bis zum Commit (12.11), und der erste
/// Tick beider kommt aus ihrer ISR, nicht aus `main`. Beide `init` mit Kopf
/// brauchen am F401 unter 1 ms.
const START_NS: i64 = 20_000_000;

/// Der Schritt von `fast` unter der Leitung.
const FAST_PRIORITY: u8 = 0x20;
/// Der Schritt von `slow` unter dem von `fast`.
const SLOW_PRIORITY: u8 = 0x30;
/// Die Leitung, auf der `slow` rechnet: eine, die kein Treiber benutzt.
const SLOW_LINE: Interrupt = Interrupt::EXTI2;
/// Die Leitung der Jobs; die Programme dieses Abbilds haben keine.
const JOB_LINE: Interrupt = Interrupt::EXTI1;

/// Die Briefkaesten der Kopplung (12.11).
static COUNT: Mailbox = Mailbox::new();
static LAST: Mailbox = Mailbox::new();

/// Die Geraete des Pruefstands (12.6).
pub mod devices {
    use takt_embed::{Device, Input, Output, Sample};

    use super::{COUNT, LAST, Mailbox};

    /// Ein Geraet, das fest am Board sitzt (12.4).
    #[derive(Debug, Default)]
    pub struct Fixed;

    impl Device for Fixed {
        fn alive(&mut self, _now: i64) -> bool {
            true
        }
    }

    fn put(mailbox: &Mailbox, value: i64, now: i64) -> bool {
        mailbox.write(value, now);
        true
    }

    fn take(mailbox: &Mailbox) -> Option<Sample<i64>> {
        mailbox.read().map(|(v, t)| Sample::good(v, t))
    }

    /// `pair/count`: der Zaehler von `fast`.
    #[derive(Debug, Default)]
    pub struct PairCount;

    impl Output<i64> for PairCount {
        fn write(&mut self, value: i64, now: i64) -> bool {
            put(&COUNT, value, now)
        }
    }

    /// `pair/seen`: was `slow` von `pair/count` liest.
    #[derive(Debug, Default)]
    pub struct PairSeen;

    impl Input<i64> for PairSeen {
        fn sample(&mut self, _now: i64) -> Option<Sample<i64>> {
            take(&COUNT)
        }
    }

    /// `pair/last`: was `slow` zuletzt freigab.
    #[derive(Debug, Default)]
    pub struct PairLast;

    impl Output<i64> for PairLast {
        fn write(&mut self, value: i64, now: i64) -> bool {
            put(&LAST, value, now)
        }
    }

    /// `pair/back`: was `fast` von `pair/last` liest.
    #[derive(Debug, Default)]
    pub struct PairBack;

    impl Input<i64> for PairBack {
        fn sample(&mut self, _now: i64) -> Option<Sample<i64>> {
            take(&LAST)
        }
    }

    /// `test/foreign_arena`: ein Pruefgeraet, kein Treiber. Mit `true`
    /// schreibt es aus dem Commit von `fast` in die Arena von `slow`; die
    /// MPU weist das ab (12.3, 12.11).
    #[derive(Debug, Default)]
    pub struct ForeignArena;

    impl Output<bool> for ForeignArena {
        fn write(&mut self, value: bool, _now: i64) -> bool {
            if value {
                let target = (&raw mut super::SLOW_ARENA).cast::<u8>();
                // SAFETY: Ein Pruefzugriff, den die MPU abweist; erreicht er
                // die Arena doch, zeigt der Trace von `slow` es.
                unsafe { target.write_volatile(0xA5) };
            }
            true
        }
    }
}

/// Die Arena von `fast` in `.takt_state`, die von `slow` in `.takt_state2`;
/// jede ist eine Region der MPU (12.3), und unter ihrem Namen findet `takt
/// check-image` sie.
#[unsafe(link_section = ".takt_state")]
#[unsafe(export_name = "fast_arena")]
static mut FAST_ARENA: fast::Arena = fast::Arena::new();
#[unsafe(link_section = ".takt_state2")]
#[unsafe(export_name = "slow_arena")]
static mut SLOW_ARENA: slow::Arena = slow::Arena::new();

static mut FAST_RIG: Option<fast_drivers::Rig> = None;
static mut SLOW_RIG: Option<slow_drivers::Rig> = None;

unsafe extern "C" {
    static __takt_state2_start: u8;
    static __takt_state2_end: u8;
}

/// So viele Byte fasst die Schlange eines Programms.
const QUEUE: usize = 4096;

/// Die Schlange eines Programms: ein Schreiber (sein Schritt), ein Leser
/// (die Hauptschleife), ohne Sperre.
struct Queue {
    bytes: [AtomicU8; QUEUE],
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl Queue {
    const fn new() -> Queue {
        Queue { bytes: [const { AtomicU8::new(0) }; QUEUE], head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    fn after(at: usize) -> usize {
        let next = at.wrapping_add(1);
        if next >= QUEUE { 0 } else { next }
    }

    fn push(&self, b: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = Queue::after(head);
        let Some(slot) = self.bytes.get(head) else { return false };
        if next == self.tail.load(Ordering::Acquire) {
            return false;
        }
        slot.store(b, Ordering::Relaxed);
        self.head.store(next, Ordering::Release);
        true
    }

    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let b = self.bytes.get(tail)?.load(Ordering::Relaxed);
        self.tail.store(Queue::after(tail), Ordering::Release);
        Some(b)
    }

    /// Steht eine ganze Zeile darin?
    fn has_line(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        let mut at = self.tail.load(Ordering::Relaxed);
        while at != head {
            if self.bytes.get(at).is_some_and(|b| b.load(Ordering::Relaxed) == b'\n') {
                return true;
            }
            at = Queue::after(at);
        }
        false
    }

    fn is_empty(&self) -> bool {
        self.tail.load(Ordering::Acquire) == self.head.load(Ordering::Acquire)
    }
}

static FAST_QUEUE: Queue = Queue::new();
static SLOW_QUEUE: Queue = Queue::new();

/// Die Leitung eines Programms: seine Schlange.
struct Lane(&'static Queue);

impl Port for Lane {
    fn try_write(&mut self, b: u8) -> bool {
        self.0.push(b)
    }

    fn idle(&mut self) -> bool {
        self.0.is_empty()
    }
}

type Lead = fn() -> Option<&'static mut takt_rt_baremetal::Telemetry<Lane>>;

/// Die Telemetrie jedes Programms vor seiner Schlange; sein Schritt schreibt
/// hinein, die Hauptschleife nur vor dem Lauf und nach seinem Ende.
static mut FAST_TELEMETRY: Option<takt_rt_baremetal::Telemetry<Lane>> = None;
static mut SLOW_TELEMETRY: Option<takt_rt_baremetal::Telemetry<Lane>> = None;
static mut FAST_RING: [u8; 1024] = [0; 1024];
static mut SLOW_RING: [u8; 1024] = [0; 1024];

fn fast_telemetry() -> Option<&'static mut takt_rt_baremetal::Telemetry<Lane>> {
    let slot = &raw mut FAST_TELEMETRY;
    // SAFETY: siehe `FAST_TELEMETRY`.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

fn slow_telemetry() -> Option<&'static mut takt_rt_baremetal::Telemetry<Lane>> {
    let slot = &raw mut SLOW_TELEMETRY;
    // SAFETY: siehe `FAST_TELEMETRY`.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

/// Die Telemetrie der Leitung (USART1); nur die Hauptschleife schreibt.
static mut UART: Option<Telemetry> = None;

fn uart() -> Option<&'static mut Telemetry> {
    let slot = &raw mut UART;
    // SAFETY: siehe `UART`.
    unsafe { slot.as_mut() }.and_then(Option::as_mut)
}

/// Die Senke eines Programms (12.11): Was sein Rahmen ueber die Haken
/// schreibt, geht in seine Telemetrie.
pub struct PairSink {
    lead: Lead,
}

impl takt_embed::Line for PairSink {
    fn write(&self, bytes: &[u8]) {
        let Some(t) = (self.lead)() else { return };
        for &b in bytes {
            t.write_byte(b);
        }
    }
}

takt_embed::hooks!(PairSink);

static FAST_SINK: PairSink = PairSink { lead: fast_telemetry };
static SLOW_SINK: PairSink = PairSink { lead: slow_telemetry };

/// Die Zeitachse beider Programme mit gemeinsamem Ursprung (12.11).
struct Axis {
    alarm: Tim2Alarm,
    origin: i64,
}

impl Clock for Axis {
    fn now(&self) -> i64 {
        self.alarm.now_ns()
    }

    fn wait_until(&mut self, deadline: i64) {
        while self.alarm.now_ns() < deadline {
            cortex_m::asm::nop();
        }
    }

    fn origin(&self) -> i64 {
        self.origin
    }
}

type Line = Trace<Lead, takt_rt_baremetal::Telemetry<Lane>>;
type FastRun = Runtime<Guarded<fast::Program<'static>, Mpu>, Axis, NoWatchdog, Line>;
type SlowRun = Runtime<Guarded<slow::Program<'static>, Mpu>, Axis, NoWatchdog, Line>;

/// Was die ISR eines Programms haelt.
struct Stepping<R> {
    rt: R,
    form: Form<Tim2Alarm>,
    ended: Option<Stats>,
}

/// Bis zu seinem Ende rechnet nur die ISR eines Programms mit ihm; danach
/// gehoert es der Hauptschleife.
static mut FAST: Option<Stepping<FastRun>> = None;
static mut SLOW: Option<Stepping<SlowRun>> = None;
static FAST_ENDED: AtomicBool = AtomicBool::new(false);
static SLOW_ENDED: AtomicBool = AtomicBool::new(false);

/// Der laengste Lauf der ISR der Leitung in Takten, eines Stoerers beider
/// Programme (12.11): Die Hardware-Konfiguration sagt seine Kosten zu
/// (`isr.uart`), und der Board-Test vergleicht.
static UART_LONGEST: AtomicU32 = AtomicU32::new(0);

#[interrupt]
fn USART1() {
    let start = cycles::now();
    mpu::isr(takt_board_stm32f401::uart::on_interrupt);
    UART_LONGEST.fetch_max(cycles::now().wrapping_sub(start), Ordering::Relaxed);
}

/// Die Zeile `takt isr uart laengste N ns` (12.11).
fn isr_line() {
    let Some(u) = uart() else { return };
    let cycles = UART_LONGEST.load(Ordering::Relaxed);
    u.write("takt isr uart laengste ");
    u.write_i64(takt_board_support::cycles::Measurement::between(0, cycles, CORE_HZ).ns());
    u.write(" ns");
    u.newline();
}

/// Der Schrittkontext von `fast`; ist die Frist von `slow` erreicht, laesst
/// er dessen Leitung anstehen, die erst nach ihm rechnet.
#[interrupt]
fn TIM2() {
    mpu::isr(|| {
        let [fast_due, slow_due] = alarm::on_interrupt_all();
        if slow_due {
            NVIC::pend(SLOW_LINE);
        }
        if !fast_due || FAST_ENDED.load(Ordering::SeqCst) {
            return;
        }
        let slot = &raw mut FAST;
        // SAFETY: Bis zu seinem Ende gehoert `fast` dieser ISR.
        let Some(s) = unsafe { slot.as_mut() }.and_then(Option::as_mut) else { return };
        if let Some(stats) = s.form.on_alarm(&mut s.rt, None) {
            s.ended = Some(stats);
            FAST_ENDED.store(true, Ordering::SeqCst);
        }
    });
}

/// Der Schrittkontext von `slow`.
#[interrupt]
fn EXTI2() {
    mpu::isr(|| {
        if SLOW_ENDED.load(Ordering::SeqCst) {
            return;
        }
        let slot = &raw mut SLOW;
        // SAFETY: Bis zu seinem Ende gehoert `slow` dieser ISR.
        let Some(s) = unsafe { slot.as_mut() }.and_then(Option::as_mut) else { return };
        if let Some(stats) = s.form.on_alarm(&mut s.rt, None) {
            s.ended = Some(stats);
            SLOW_ENDED.store(true, Ordering::SeqCst);
        }
    });
}

/// Haelt den Kern an, wenn das Board nicht weiter kann.
fn halt() -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}

/// Nach so vielen Ticks von `fast` endet der Lauf.
fn limit() -> u64 {
    TICKS.and_then(|t| t.parse().ok()).unwrap_or(0)
}

/// Dieselbe Dauer in Ticks von `slow`.
fn slow_limit() -> u64 {
    let (fast_ns, slow_ns) = (u64::try_from(fast::TICK_NS).unwrap_or(1), u64::try_from(slow::TICK_NS).unwrap_or(1));
    limit().saturating_mul(fast_ns).checked_div(slow_ns).unwrap_or(0)
}

fn policy(alert: bool) -> Policy {
    if alert { Policy::Alert } else { Policy::Fault }
}

/// Nimmt aus `queue` ganze Zeilen und schreibt sie mit `prefix` auf die
/// Leitung.
fn merge(queue: &Queue, prefix: &str) {
    let Some(u) = uart() else { return };
    while queue.has_line() {
        u.write(prefix);
        while let Some(b) = queue.pop() {
            u.write_byte(b);
            if b == b'\n' {
                break;
            }
        }
    }
    u.flush();
}

/// Der Kopf des Laufs; die Regionen der MPU zaehlt er ab (12.11).
fn banner() {
    let Some(u) = uart() else { return };
    u.newline();
    u.write("takt auf stm32f401");
    u.newline();
    u.write("  Kerntakt ");
    u.write_u64(u64::from(CORE_HZ));
    u.write(" Hz, Tick ");
    u.write_i64(fast::TICK_NS);
    u.write(" ns und ");
    u.write_i64(slow::TICK_NS);
    u.write(" ns");
    u.newline();
    u.write("  Form interrupt, zwei Programme: fast und slow");
    u.newline();
    u.write(<Mpu as Protection>::CASE.line());
    u.newline();
    u.write("  MPU");
    let names = ["Arena fast", "Waechter", "Waechter Jobs", "Arena slow", "", "", "", ""];
    let mut used = 0u64;
    for (n, (on, name)) in mpu::enabled_regions().iter().zip(names).enumerate() {
        if *on {
            used = used.saturating_add(1);
            u.write(" ");
            u.write_u64(u64::try_from(n).unwrap_or(u64::MAX));
            u.write(if name.is_empty() { " fremd" } else { " " });
            u.write(name);
        }
    }
    u.write(", ");
    u.write_u64(used);
    u.write(" von 8 Regionen");
    u.newline();
    u.write("  DWT ");
    u.write(if cycles::running() { "laeuft" } else { "STEHT" });
    u.newline();
    u.newline();
    u.mark();
    u.flush();
}

#[entry]
fn main() -> ! {
    let (Some(dp), Some(cp)) = (Peripherals::take(), cortex_m::Peripherals::take()) else { halt() };
    let board = Board::WEACT_BLACKPILL;
    let led = Led::new(dp.GPIOC, &dp.RCC, board);
    let Ok(alarm) = takt_board_stm32f401::init_alarm(board, &dp.RCC, &dp.FLASH, &dp.PWR, &dp.TIM2, JOB_LINE) else {
        led.on();
        halt()
    };
    let Ok(telemetry) = takt_board_stm32f401::telemetry(dp.USART1, &dp.GPIOA, &dp.RCC, CORE_HZ, BAUD) else {
        led.on();
        halt()
    };
    // Die Leitung wartet, statt zu verwerfen: Nur die Hauptschleife schreibt
    // sie, und unter ihr laeuft nichts.
    // SAFETY: vor jeder Freigabe eines Interrupts.
    unsafe { UART = Some(telemetry.lossless()) };
    let (mut nvic, mut core_mpu, mut scb, mut dcb, mut dwt) = (cp.NVIC, cp.MPU, cp.SCB, cp.DCB, cp.DWT);
    cycles::enable(&mut dcb, &mut dwt);
    let second = (
        takt_board_stm32f401::word((&raw const __takt_state2_start).expose_provenance()),
        takt_board_stm32f401::word((&raw const __takt_state2_end).expose_provenance()),
    );
    let Some((fast_mpu, slow_mpu)) = Mpu::arm_pair(&mut core_mpu, &mut scb, second) else {
        if let Some(u) = uart() {
            u.write("takt: Speicherschutz nicht einrichtbar");
            u.newline();
            u.flush();
        }
        halt()
    };
    banner();
    for (slot, queue, ring) in [
        (&raw mut FAST_TELEMETRY, &FAST_QUEUE, &raw mut FAST_RING),
        (&raw mut SLOW_TELEMETRY, &SLOW_QUEUE, &raw mut SLOW_RING),
    ] {
        // SAFETY: einmal vor dem Lauf; danach schreibt nur der Schritt des Programms.
        let ring = unsafe { &mut *ring };
        let mut t = takt_rt_baremetal::Telemetry::new(Lane(queue), ring);
        t.mark();
        // SAFETY: wie oben.
        unsafe { *slot = Some(t) };
    }
    takt_board_stm32f401::stack::paint();

    // Gemeinsamer Ursprung (12.11): Beide Raster beginnen nach Tick 0 beider.
    let origin = alarm.now_ns().saturating_add(START_NS);
    let (fast_lead, slow_lead): (Lead, Lead) = (fast_telemetry, slow_telemetry);
    let mut fast_program = {
        let (arena, rig) = (&raw mut FAST_ARENA, &raw mut FAST_RIG);
        // SAFETY: einmal je Lauf; danach gehoert die Arena `fast`.
        let arena = unsafe { &mut *arena };
        // SAFETY: ebenso der Pruefstand.
        let rig = unsafe { &mut *rig }.insert(fast_drivers::Rig::default());
        fast::Program::new(arena, rig, &FAST_SINK)
    };
    // 12.3: Nach `init` ist der Programmzustand nur noch im Schritt beschreibbar.
    fast_program.ensure_init();
    let mut fast_rt: FastRun = Runtime::new(
        Guarded::new(fast_program, fast_mpu),
        Axis { alarm, origin },
        NoWatchdog,
        Trace::new(Cadence::of(limit(), 1), fast::TICK_NS, fast_lead),
        Profile::SHARED,
        fast::TICK_NS,
        policy(fast::OVERRUN_ALERT),
    );
    // Vor dem Ursprung committet `fast` Tick 0; erst danach tastet `slow` in
    // seinem `init` ab (12.11). Vor dem Ursprung rechnet `service` keinen Tick.
    let _ = fast_rt.service_with(None);
    let fast_form = Form::start(&mut fast_rt, alarm, None);
    let mut slow_program = {
        let (arena, rig) = (&raw mut SLOW_ARENA, &raw mut SLOW_RIG);
        // SAFETY: wie oben, fuer `slow`.
        let arena = unsafe { &mut *arena };
        // SAFETY: wie oben.
        let rig = unsafe { &mut *rig }.insert(slow_drivers::Rig::default());
        slow::Program::new(arena, rig, &SLOW_SINK)
    };
    slow_program.ensure_init();
    let slow_alarm = alarm.on_channel(1);
    let mut slow_rt: SlowRun = Runtime::new(
        Guarded::new(slow_program, slow_mpu),
        Axis { alarm: slow_alarm, origin },
        NoWatchdog,
        Trace::new(Cadence::of(slow_limit(), 1), slow::TICK_NS, slow_lead),
        Profile::SHARED,
        slow::TICK_NS,
        policy(slow::OVERRUN_ALERT),
    );
    let _ = slow_rt.service_with(None);
    let slow_form = Form::start(&mut slow_rt, slow_alarm, None);
    // SAFETY: TIM2 und EXTI2 sind noch maskiert; ab ihrer Freigabe gehoert
    // jedes Programm seiner ISR.
    unsafe { FAST = Some(Stepping { rt: fast_rt, form: fast_form, ended: None }) };
    // SAFETY: wie oben.
    unsafe { SLOW = Some(Stepping { rt: slow_rt, form: slow_form, ended: None }) };
    for (line, priority) in
        [(Interrupt::USART1, mpu::ISR_PRIORITY), (Interrupt::TIM2, FAST_PRIORITY), (SLOW_LINE, SLOW_PRIORITY)]
    {
        // SAFETY: Die Handler oben sind bereit.
        unsafe { nvic.set_priority(line, priority) };
        // SAFETY: wie oben.
        unsafe { NVIC::unmask(line) };
    }
    host()
}

/// Die Hauptschleife des Wirts: Sie nimmt ganze Zeilen aus den Schlangen,
/// bis beide Programme geendet haben, und schreibt dann je Programm die
/// Bilanz und einmal `takt end`.
fn host() -> ! {
    while !(FAST_ENDED.load(Ordering::SeqCst) && SLOW_ENDED.load(Ordering::SeqCst)) {
        merge(&FAST_QUEUE, "fast ");
        merge(&SLOW_QUEUE, "slow ");
    }
    let stacks = Stacks {
        tick: Some(takt_board_stm32f401::stack::high_water()),
        tick_bound: u32::try_from(fast::TICK_STACK_BYTES.saturating_add(slow::TICK_STACK_BYTES)).ok(),
        tick_program: None,
        job_shared: true,
        job: None,
        job_bound: None,
    };
    let (fast_slot, slow_slot) = (&raw mut FAST, &raw mut SLOW);
    // SAFETY: Nach dem Ende steht kein Alarm mehr; die Laeufe gehoeren
    // jetzt dieser Schleife.
    if let (Some(f), Some(t)) = (unsafe { fast_slot.as_mut() }.and_then(Option::as_mut), fast_telemetry()) {
        t.flush();
        takt_rt_baremetal::balance(t, f.rt.overrun(), &f.ended.unwrap_or_default(), &JournalStats::default(), &stacks);
        t.flush();
    }
    // SAFETY: wie oben.
    if let (Some(s), Some(t)) = (unsafe { slow_slot.as_mut() }.and_then(Option::as_mut), slow_telemetry()) {
        t.flush();
        takt_rt_baremetal::balance(t, s.rt.overrun(), &s.ended.unwrap_or_default(), &JournalStats::default(), &stacks);
        t.flush();
    }
    // Was noch in den Ringen der Telemetrie steht, geht durch die Schlangen.
    for _ in 0..takt_rt_baremetal::DRAIN_ROUNDS {
        let mut pending = 0usize;
        for t in [fast_telemetry(), slow_telemetry()].into_iter().flatten() {
            t.flush();
            pending = pending.saturating_add(t.pending());
        }
        merge(&FAST_QUEUE, "fast ");
        merge(&SLOW_QUEUE, "slow ");
        if pending == 0 && !FAST_QUEUE.has_line() && !SLOW_QUEUE.has_line() {
            break;
        }
    }
    isr_line();
    if let Some(u) = uart() {
        u.write("takt end");
        u.newline();
        u.flush();
    }
    loop {
        if let Some(u) = uart() {
            u.flush();
        }
        cortex_m::asm::wfi();
    }
}
