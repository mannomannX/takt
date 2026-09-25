//! Neuanmeldung des USB-Serial-JTAG auf Wunsch des Hosts (FB-266).
//!
//! Steht der Empfangsendpunkt der USB-Konsole, hilft nur ein Neustecken —
//! oder das Geraet meldet sich selbst neu an: ohne D+-Pull-up sieht der
//! Host ein Abstecken, der Block wird derweil zurueckgesetzt, und danach
//! zaehlt der Host neu. Den Wunsch schreibt der Host ueber JTAG in LP_AON
//! STORE0, das einen Reset ueberlebt und sonst niemand nutzt; der Start
//! liest und loescht ihn. Steht der JTAG-Teil selbst (DMI-Timeout), hilft
//! das nicht: Der sitzt im Debug-Modul, und nur der Strom setzt ihn zurueck.

use esp_hal::delay::Delay;
use esp_hal::peripherals::{IO_MUX, LP_AON, PCR, USB_DEVICE};

/// `TAKT` in STORE0: Der Host verlangt eine Neuanmeldung.
pub const REENUMERATE_MAGIC: u32 = 0x5441_4B54;

/// Meldet das USB-Geraet neu an, wenn STORE0 den Wunsch traegt.
pub fn reenumerate_if_requested() {
    let store = LP_AON::regs().store0();
    if store.read().bits() != REENUMERATE_MAGIC {
        return;
    }
    store.reset();
    // Nur die Pull-ups gehen, das Pad bleibt: Ohne Pad verliert der Kern
    // seine Adresse, waehrend der Host nichts bemerkt — danach hilft nur
    // das Kabel. Neben dem PHY haelt auch der schwache Pull-up der IO-MUX
    // an GPIO13 (D+) die Leitung an der Schwelle; beide muessen weg.
    let conf0 = USB_DEVICE::regs().conf0();
    let pad = IO_MUX::regs().gpio(13);
    conf0.modify(|_, w| w.dp_pullup().clear_bit());
    pad.modify(|_, w| w.fun_wpu().clear_bit());
    Delay::new().delay_millis(250);
    // Der Block selbst ueberlebt jeden Chip-Reset, nur nicht den Strom: Ein
    // steckender Endpunkt bleibt sonst stecken. Sein Reset, waehrend der Host
    // das Geraet fuer abgesteckt haelt, ist das Neustecken des Blocks; die
    // Voreinstellung danach traegt den Pull-up wieder.
    let reset = PCR::regs().usb_device_conf();
    reset.modify(|_, w| w.usb_device_rst_en().set_bit());
    reset.modify(|_, w| w.usb_device_rst_en().clear_bit());
    pad.modify(|_, w| w.fun_wpu().set_bit());
}
