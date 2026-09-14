//! Was das HID-Geraet ueber sich selbst sagt.
//!
//! **Warum das ein eigenes Werkzeug ist.** Das Flash-Protokoll in
//! `protocol.rs` beruht auf Annahmen: Berichtsgroesse 64 Byte, eine
//! fuehrende Report-ID, Befehlscode `0x01`. Keine davon ist belegt — und
//! solange das Board im Bootloader stehen bleibt, obwohl das Werkzeug
//! „Geschrieben" meldet, ist der Verdacht, dass eine davon falsch ist.
//!
//! Dieses Programm schreibt nichts. Es liest die Deskriptoren und sagt,
//! was das Geraet tatsaechlich erwartet.

use hidapi::HidApi;

fn main() {
    let api = match HidApi::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("HID nicht verfuegbar: {e}");
            return;
        }
    };

    let mut found = 0;
    for d in api.device_list() {
        // Alles von ST, damit auch ein DFU-Bootloader auftaucht.
        if d.vendor_id() != 0x0483 {
            continue;
        }
        found += 1;
        println!("VID {:#06x} PID {:#06x}", d.vendor_id(), d.product_id());
        println!("  Hersteller   {:?}", d.manufacturer_string());
        println!("  Produkt      {:?}", d.product_string());
        println!("  Seriennummer {:?}", d.serial_number());
        println!("  UsagePage    {:#06x}, Usage {:#06x}", d.usage_page(), d.usage());
        println!("  Schnittstelle {}", d.interface_number());
        println!("  Pfad         {:?}", d.path());

        match api.open_path(d.path()) {
            Ok(_) => println!("  Oeffnen      geht"),
            Err(e) => println!("  Oeffnen      FEHLER: {e}"),
        }
        println!();
    }

    if found == 0 {
        println!("Kein ST-Geraet gefunden. Steht das Board im Bootloader?");
        println!("KEY halten, NRST kurz druecken, beide loslassen.");
    }
}
