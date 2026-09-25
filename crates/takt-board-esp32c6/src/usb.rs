//! Neuanmeldung des USB-Serial-JTAG auf Wunsch des Hosts (FB-266, FB-264).
//!
//! Steht der Empfangsendpunkt der Konsole oder der JTAG-Teil, hilft nur ein
//! Neustecken — oder das, was der EN-Pin tut: Ein Reset auf RTC-Ebene setzt
//! auch den USB-Serial-JTAG-Block und das Debug-Modul zurueck, waehrend ein
//! Reset ueber RTS oder Software nur das HP-System trifft (Espressif,
//! openocd-esp32 #316). Der Host sieht ein Abstecken und zaehlt danach neu.
//!
//! Den Wunsch schreibt der Host ueber JTAG in LP_AON STORE0, das einen
//! Reset ueberlebt und sonst niemand nutzt — oder als `TAKT` in die
//! Konsole: Die Leitung liest ihren Empfangspuffer im Lauf (`uart.rs`), der
//! Start liest ihn vor allem anderen. Den Reset macht Stufe 0 des
//! RTC-Watchdogs; er ist darum fuer nichts anderes reserviert (FB-272).

use esp_hal::peripherals::{LP_AON, RTC_TIMER, USB_DEVICE};
use esp_hal::rtc_cntl::{Rtc, RwdtStage, RwdtStageAction};
use esp_hal::time::Duration;
use takt_board_support::console::MAGIC;
pub use takt_board_support::console::Magic;

/// `TAKT` in STORE0 oder in der Konsole: Der Host verlangt einen Chip-Reset.
pub const REENUMERATE_MAGIC: u32 = MAGIC;

/// Setzt den Chip auf RTC-Ebene zurueck, wenn der Host es verlangt hat.
pub fn reenumerate_if_requested() {
    let store = LP_AON::regs().store0();
    if store.read().bits() != REENUMERATE_MAGIC && !console_requests() {
        return;
    }
    store.reset();
    chip_reset();
}

/// Der Reset auf RTC-Ebene, wie der EN-Pin.
pub fn chip_reset() -> ! {
    // SAFETY: Den RTC-Timer nutzt sonst nichts im Bring-up, und nach diesem
    // Aufruf laeuft nichts mehr — der Watchdog setzt den Chip zurueck.
    let mut rtc = Rtc::new(unsafe { RTC_TIMER::steal() });
    rtc.rwdt.set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
    rtc.rwdt.set_timeout(RwdtStage::Stage0, Duration::from_millis(1));
    rtc.rwdt.enable();
    loop {
        core::hint::spin_loop();
    }
}

/// `TAKT` im Empfangspuffer der Konsole: Der Puffer ueberlebt den Reset,
/// und wer ihn beim Start leert, findet, was der Host davor schrieb.
fn console_requests() -> bool {
    let regs = USB_DEVICE::regs();
    let mut magic = Magic::default();
    let mut found = false;
    for _ in 0..64 {
        if !regs.ep1_conf().read().serial_out_ep_data_avail().bit_is_set() {
            break;
        }
        found |= magic.feed(regs.ep1().read().rdwr_byte().bits());
    }
    found
}
