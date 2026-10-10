//! Ein Panic ist auf dem Board ein Halt mit Meldung (12.3, 12.6).
//!
//! `esp-hal` meldet Ausnahmen des Kerns (Trap, Stack-Ueberlauf) als Panic
//! mit `mepc` und Ursache; ein stiller Halt (`panic-halt`) wuerfe genau
//! diese Information weg. Die Meldung geht ueber die Telemetrie, dann
//! steht der Kern — ein Neustart ist Sache des Watchdogs oder des Hosts.

use core::fmt::Write as _;

use esp_hal::peripherals::USB_DEVICE;

use takt_rt_baremetal::DRAIN_ROUNDS;

use crate::uart::{Telemetry, UsbJtag};

/// Der Ring der Meldung: Den des Laufs haelt noch dessen Telemetrie.
static mut RING: [u8; 128] = [0; 128];

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // SAFETY: Nach dem Panic laeuft nichts mehr, das die Schnittstelle
    // haelt; der Treiber richtet sie nur ein, ohne sie zurueckzusetzen.
    // SAFETY: Nach dem Panic rechnet niemand mehr mit dem Ring; er wird
    // einmal genommen, denn der Handler kehrt nicht zurueck.
    let ring = unsafe { (&raw mut RING).as_mut() }.map_or(&mut [][..], |r| &mut r[..]);
    // Verlustfrei: Die Meldung wartet auf die Leitung, statt zu verwerfen.
    let mut uart = Telemetry::new(UsbJtag::new(unsafe { USB_DEVICE::steal() }), ring).lossless();
    let _ = write!(uart, "\r\ntakt panic: {info}\r\n");
    uart.drain(DRAIN_ROUNDS);
    loop {
        crate::wfi();
    }
}
