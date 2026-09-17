//! Ein Panic ist auf dem Board ein Halt mit Meldung (12.3, 12.6).
//!
//! `esp-hal` meldet Ausnahmen des Kerns (Trap, Stack-Ueberlauf) als Panic
//! mit `mepc` und Ursache; ein stiller Halt (`panic-halt`) wuerfe genau
//! diese Information weg. Die Meldung geht ueber die Telemetrie, dann
//! steht der Kern — ein Neustart ist Sache des Watchdogs oder des Hosts.

use core::fmt::Write as _;

use crate::uart::Telemetry;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut uart = Telemetry::new();
    let _ = write!(uart, "\r\ntakt panic: {info}\r\n");
    loop {
        crate::wfi();
    }
}
