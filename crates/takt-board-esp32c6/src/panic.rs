//! Ein Panic ist auf dem Board ein Halt mit Meldung (12.3, 12.6).
//!
//! `esp-hal` meldet Ausnahmen des Kerns (Trap, Stack-Ueberlauf) als Panic
//! mit `mepc` und Ursache; ein stiller Halt (`panic-halt`) wuerfe genau
//! diese Information weg. Die Meldung geht ueber die Telemetrie, dann
//! steht der Kern — ein Neustart ist Sache des Watchdogs oder des Hosts.

use core::fmt::Write as _;

use esp_hal::peripherals::USB_DEVICE;

use crate::uart::Telemetry;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // SAFETY: Nach dem Panic laeuft nichts mehr, das die Schnittstelle
    // haelt; der Treiber richtet sie nur ein, ohne sie zurueckzusetzen.
    let mut uart = Telemetry::new(unsafe { USB_DEVICE::steal() });
    let _ = write!(uart, "\r\ntakt panic: {info}\r\n");
    loop {
        crate::wfi();
    }
}
