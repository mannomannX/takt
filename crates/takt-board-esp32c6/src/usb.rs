//! Neuanmeldung des USB-Serial-JTAG auf Wunsch des Hosts (FB-266, FB-264).
//!
//! Steht der Empfangsendpunkt der Konsole oder der JTAG-Teil, hilft nur ein
//! Neustecken — oder das, was der EN-Pin tut: Ein Reset auf RTC-Ebene setzt
//! auch den USB-Serial-JTAG-Block und das Debug-Modul zurueck, waehrend ein
//! Reset ueber RTS oder Software nur das HP-System trifft (Espressif,
//! openocd-esp32 #316). Der Host sieht ein Abstecken und zaehlt danach neu.
//!
//! Den Wunsch schreibt der Host ueber JTAG in LP_AON STORE0, das einen
//! Reset ueberlebt und sonst niemand nutzt — oder, wenn der JTAG-Teil selbst
//! steht, als `TAKT` in den Empfangspuffer der Konsole vor einem Reset ueber
//! RTS. Der Start liest beides und loescht es.

use esp_hal::peripherals::{LP_AON, RTC_TIMER, USB_DEVICE};
use esp_hal::rtc_cntl::{Rtc, RwdtStage, RwdtStageAction};
use esp_hal::time::Duration;

/// `TAKT` in STORE0 oder im Empfangspuffer: Der Host verlangt eine Neuanmeldung.
pub const REENUMERATE_MAGIC: u32 = 0x5441_4B54;

/// Setzt den Chip auf RTC-Ebene zurueck, wenn der Host es verlangt hat.
pub fn reenumerate_if_requested(rtc_timer: RTC_TIMER<'static>) {
    let store = LP_AON::regs().store0();
    if store.read().bits() != REENUMERATE_MAGIC && !console_requests() {
        return;
    }
    store.reset();
    let mut rtc = Rtc::new(rtc_timer);
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
    let magic = REENUMERATE_MAGIC.to_be_bytes();
    let (mut matched, mut found) = (0, false);
    for _ in 0..64 {
        if !regs.ep1_conf().read().serial_out_ep_data_avail().bit_is_set() {
            break;
        }
        let b = regs.ep1().read().rdwr_byte().bits();
        matched = if b == magic[matched] { matched + 1 } else { usize::from(b == magic[0]) };
        if matched == magic.len() {
            found = true;
            matched = 0;
        }
    }
    found
}
