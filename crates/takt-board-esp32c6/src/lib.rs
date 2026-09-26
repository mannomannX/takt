//! ESP32-C6 als Takt-Board (12.3, 12.8; plan/esp32c6.md).
//!
//! **Was hier steht.** Alles, was den Chip anfasst — Alarm, Zyklenzaehler,
//! USB-Serial-JTAG —, und sonst nichts. Die Regeln daraus stehen eine
//! Ebene hoeher in `takt-rt-baremetal`, das dieses Crate nicht kennt: Es
//! kennt nur seine Traits, und die erfuellt dieses hier (plan/m5.md 2.2).
//!
//! **Board 2, nicht Board 1.** Der Chip ist die Zielklasse `Mcu32NoFpu`:
//! `f32` und `f64` kommen aus `libtaktm`, und der Differentialtest
//! Interpreter ≡ x86-64 ≡ riscv32imac laeuft hier zum ersten Mal auf
//! einem Mikrocontroller. Code laeuft aus dem SPI-Flash ueber einen
//! Cache; Zeitmessungen beschreiben darum das `xip_flash`-Profil (12.3),
//! nicht `baremetal` — die gehoeren zum STM32F401.
//!
//! **Warum `esp-hal` und nicht die PAC.** Start, Speicherkarte,
//! Abbildformat fuer den ROM-Bootloader und das Cache-Mapping kommen aus
//! `esp-hal`; das selbst zu schreiben waere Bring-up-Arbeit ohne
//! Erkenntnis. Das Zeitmodell der HAL bleibt aussen vor: Der Tick ist ein
//! periodischer Alarm des SYSTIMER, die gemessene Periode kommt aus dem
//! Zyklenzaehler des Kerns, wie beim F401 aus TIM2 und DWT.
//!
//! **`unsafe` in den Abhaengigkeiten.** `esp-hal` braucht es fuer
//! Registerzugriffe; das workspace-weite `forbid` (13.4) gilt fuer eigenen
//! Code. Wo dieses Crate selbst `unsafe` schreibt — CSR-Zugriffe, `wfi`
//! und die C-ABI des erzeugten Programms —, steht es um einen Ausdruck,
//! nicht um einen Block.
//!
//! ## Der Chip
//!
//! | | |
//! |---|---|
//! | Kern | RV32IMAC, 160 MHz, ohne FPU |
//! | Flash | 4 MB im Modul, XIP ueber Cache — Profilfamilie `xip_flash` |
//! | RAM | 512 KB HP-SRAM |
//! | Tick | SYSTIMER, 16 MHz, Alarm 0 periodisch |
//! | Zyklen | Performance-Zaehler des Kerns (CSR 0x7e2) |
//! | Journal | zwei Flash-Sektoren in der `nvs`-Partition (`nvm.rs`) |
//! | Telemetrie | USB-Serial-JTAG als Leitung hinter dem Ring aus `takt-rt-baremetal` (`uart.rs`) |
//! | LED | WS2812 an IO8 (DevKitM-1), ueber RMT-Kanal 0 |

#![no_std]
#![allow(unsafe_code, reason = "Registerzugriff und C-ABI; 9.5 fuehrt Treiber in der TCB")]

pub mod button;
pub mod cycles;
pub mod guard;
pub mod led;
pub mod nvm;
pub mod panic;
pub mod pins;
pub mod platform;
pub mod stack;
pub mod tick;
pub mod uart;
pub mod usb;

use core::cell::{Cell, RefCell};

use critical_section::Mutex;
use esp_hal::interrupt::Priority;
use esp_hal::peripherals::SYSTIMER;
use esp_hal::time::Duration;
use esp_hal::timer::Timer;
use esp_hal::timer::systimer::{Alarm, SystemTimer, Unit};

pub use button::Button;
pub use guard::WfiSleep;
pub use led::Ws2812;
pub use nvm::FlashNvm;
pub use pins::route_uart0;
pub use takt_mcu_program::Generated;
pub use tick::{SystimerTick, on_timer_interrupt};
pub use uart::{Telemetry, UsbJtag, telemetry};
pub use usb::reenumerate_if_requested;

/// Der Kerntakt in Hertz, wie `esp_hal::init` ihn mit `CpuClock::max()` setzt.
pub const CORE_HZ: u32 = 160_000_000;

/// Der Takt des SYSTIMER in Hertz (XTAL 40 MHz durch 2,5).
pub const TIMER_HZ: u32 = 16_000_000;

/// Warum der Tick nicht eingerichtet werden konnte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    /// Die Periode ist mit dem SYSTIMER nicht darstellbar.
    Period(takt_board_support::PeriodError),
    /// Die Periode ist kein Vielfaches einer Mikrosekunde: der Alarm nimmt
    /// nur ganze Mikrosekunden an.
    SubMicrosecond,
}

/// Der Alarm, dessen Interrupt die ISR quittiert.
static ALARM: Mutex<RefCell<Option<Alarm<'static>>>> = Mutex::new(RefCell::new(None));

/// SYSTIMER-Stand beim letzten Alarm, fuer die gemessene Periode. Der
/// Zyklenzaehler taugt dafuer nicht: Er steht, solange der Kern in `wfi`
/// wartet, und zaehlte so nur die Arbeit, nicht die Zeit.
static LAST_STAMP: Mutex<Cell<u64>> = Mutex::new(Cell::new(0));

/// Richtet den Tick ein: Alarm 0 des SYSTIMER periodisch mit `tick_ns`,
/// Interrupt an [`on_alarm`], Zyklenzaehler eingeschaltet.
///
/// # Errors
/// Wenn die Periode nicht darstellbar ist (3.4 der Board-Schicht: eine
/// falsche Periode bricht nichts, sie verschiebt nur die Zeit — darum
/// ein Fehler und kein Runden).
pub fn init(systimer: SYSTIMER<'static>, tick_ns: i64) -> Result<SystimerTick, InitError> {
    let counts = takt_board_support::counts_for(TIMER_HZ, tick_ns).map_err(InitError::Period)?;
    if tick_ns % 1_000 != 0 {
        return Err(InitError::SubMicrosecond);
    }
    cycles::enable();
    tick::set_counts_per_tick(counts);
    let alarm = SystemTimer::new(systimer).alarm0;
    alarm.set_interrupt_handler(on_alarm);
    alarm.enable_auto_reload(true);
    alarm
        .load_value(Duration::from_micros((tick_ns / 1_000).unsigned_abs()))
        .map_err(|_| InitError::Period(takt_board_support::PeriodError::TooLong))?;
    alarm.enable_interrupt(true);
    alarm.start();
    critical_section::with(|cs| {
        LAST_STAMP.borrow(cs).set(SystemTimer::unit_value(Unit::Unit0));
        ALARM.borrow_ref_mut(cs).replace(alarm);
    });
    Ok(SystimerTick::new(TIMER_HZ, counts))
}

/// Die Alarm-ISR: misst die Periode in SYSTIMER-Schritten, quittiert den
/// Interrupt und zaehlt den Tick (12.3: „die ISR setzt ein Flag").
///
/// Im RAM: Waehrend eines Flash-Schreibvorgangs ist der Cache aus, und
/// eine ISR im Flash liefe erst danach (12.3, `xip_flash`).
#[esp_hal::ram]
#[esp_hal::handler(priority = Priority::Priority1)]
fn on_alarm() {
    let now = SystemTimer::unit_value(Unit::Unit0);
    let elapsed = critical_section::with(|cs| {
        let last = LAST_STAMP.borrow(cs).replace(now);
        if let Some(alarm) = ALARM.borrow_ref(cs).as_ref() {
            alarm.clear_interrupt();
        }
        now.wrapping_sub(last)
    });
    tick::on_timer_interrupt(u32::try_from(elapsed).unwrap_or(u32::MAX));
}

/// Wartet auf den naechsten Interrupt.
#[esp_hal::ram]
pub(crate) fn wfi() {
    // SAFETY: `wfi` haelt den Kern an, bis ein Interrupt kommt. Ohne
    // `nomem`: Die ISR schreibt den Tickzaehler, und der Aufrufer liest ihn
    // in einer Schleife — der Compiler muss ihn danach neu laden.
    unsafe { core::arch::asm!("wfi", options(nostack)) };
}
