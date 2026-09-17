//! Zyklenzaehler des Kerns (12.3, 13.8).
//!
//! Der ESP-RISC-V-Kern zaehlt nicht in `mcycle`, sondern in seinen
//! Performance-CSRs: 0x7e0 waehlt das Ereignis (Bit 0: Zyklen), 0x7e1
//! schaltet den Zaehler ein, 0x7e2 ist der Stand — ESP-IDF liest
//! `esp_cpu_get_cycle_count` genauso. Die Rechnung von Zyklen in Zeit
//! steht in `takt-board-support`, weil sie einen Ueberlauf hat, den man
//! testen will.

use takt_board_support::Measurement;

/// Schaltet den Zaehler ein: Ereignis Zyklen, Zaehlen erlaubt.
pub fn enable() {
    // SAFETY: Schreibzugriffe auf die Performance-CSRs des eigenen Kerns;
    // sie beeinflussen nur den Zaehler.
    unsafe {
        core::arch::asm!("csrw 0x7e0, {0}", in(reg) 1usize, options(nomem, nostack));
        core::arch::asm!("csrw 0x7e1, {0}", in(reg) 1usize, options(nomem, nostack));
    }
}

/// Der aktuelle Zaehlerstand; laeuft bei 160 MHz alle 27 Sekunden ueber,
/// was `Measurement::between` mit `wrapping_sub` traegt.
pub fn now() -> u32 {
    let r: usize;
    // SAFETY: ein Lesezugriff auf den Zaehler-CSR ohne Nebenwirkung.
    unsafe { core::arch::asm!("csrr {0}, 0x7e2", out(reg) r, options(nomem, nostack)) };
    r as u32
}

/// Misst, wie lange `f` dauert.
pub fn measure<F: FnOnce()>(core_hz: u32, f: F) -> Measurement {
    let start = now();
    f();
    Measurement::between(start, now(), core_hz)
}
