//! `takt-flash-weact`: Ein Abbild in den WeAct HID-Bootloader schreiben.
//!
//! ```text
//! takt-flash-weact <abbild.bin> [--dry-run]
//! ```
//!
//! **Der Name nennt das Protokoll, nicht die Aufgabe.** Flashen ist
//! nichts Allgemeines: Ein ST-Link spricht SWD, ein nRF52 mit
//! Adafruit-Bootloader UF2, ein ESP32 sein eigenes Protokoll ueber UART,
//! und dieses Werkzeug spricht das HID-Protokoll der WeAct-Boards. Ein
//! `takt-flash` ohne Zusatz verspraeche eine Abstraktion, die es nicht
//! gibt — und der erste Nutzer mit einem anderen Board haelte sie fuer
//! kaputt statt fuer unzustaendig.
//!
//! Wo es eine Abstraktion gibt, heisst sie `probe-rs`: Sie deckt SWD auf
//! STM32, nRF52 und vieles mehr ab, sobald eine Probe angeschlossen ist.
//! Dieses Werkzeug ist der Weg *ohne* Probe — mit dem Bootloader, den das
//! Board ab Werk mitbringt.
//!
//! **Was es prueft, bevor es schreibt.** Ein Abbild an der falschen
//! Adresse ueberschreibt den Bootloader, und danach hilft nur noch SWD.
//! Die ersten acht Byte sagen genug, um das auszuschliessen — siehe
//! [`protocol::check`]. `--dry-run` fuehrt alle Pruefungen aus und
//! schreibt nichts; damit laesst sich ein Abbild beurteilen, ohne ein
//! Board anzuschliessen.
//!
//! **Das Board muss im Bootloader stehen.** Der WeAct-Bootloader startet,
//! wenn beim Reset die KEY-Taste gehalten wird: BOOT/KEY druecken, NRST
//! kurz druecken, beide loslassen. Windows meldet dann „WeAct Studio HID
//! Bootloader"; solange die Anwendung laeuft, ist nichts zu sehen.

mod protocol;

use std::process::ExitCode;

use protocol::{PACKET, PRODUCT_ID, VENDOR_ID};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("takt-flash <abbild.bin> [--dry-run]");
        return ExitCode::FAILURE;
    };

    let image = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Erst pruefen, dann suchen: Ein falsches Abbild soll auffallen, auch
    // wenn gar kein Board angeschlossen ist.
    if let Err(e) = protocol::check(&image) {
        eprintln!("{path}: {e}");
        return ExitCode::FAILURE;
    }

    let pages = protocol::pages_for(image.len());
    let packets = protocol::packets(&image);
    println!("{path}: {} Byte, {pages} Seiten, {} Pakete", image.len(), packets.len());
    println!("  Ziel      {:#010x}", protocol::APP_ORIGIN);
    println!("  Stack     {:#010x}", u32::from_le_bytes([image[0], image[1], image[2], image[3]]));
    println!("  Reset     {:#010x}", u32::from_le_bytes([image[4], image[5], image[6], image[7]]));

    if dry_run {
        println!("\n--dry-run: nichts geschrieben.");
        return ExitCode::SUCCESS;
    }

    match flash(&packets, pages) {
        Ok(()) => {
            println!("\nGeschrieben. Das Board startet die Anwendung nach einem Reset (NRST).");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("\n{e}");
            ExitCode::FAILURE
        }
    }
}

/// Schreibt die Pakete ueber HID.
fn flash(packets: &[[u8; PACKET]], pages: u32) -> Result<(), String> {
    let api = hidapi::HidApi::new().map_err(|e| format!("HID nicht verfuegbar: {e}"))?;
    let device = api.open(VENDOR_ID, PRODUCT_ID).map_err(|e| {
        format!(
            "Bootloader nicht gefunden ({VENDOR_ID:#06x}:{PRODUCT_ID:#06x}): {e}\n\
             Das Board muss im Bootloader stehen: KEY halten, NRST kurz druecken, beide loslassen."
        )
    })?;

    // Das erste Byte jedes HID-Berichts ist die Report-ID. Der Bootloader
    // nutzt keine, also steht dort null und die Nutzdaten folgen.
    let mut report = [0u8; PACKET + 1];

    report[1..].copy_from_slice(&protocol::write_command(protocol::APP_ORIGIN, pages));
    device.write(&report).map_err(|e| format!("Kommando nicht angenommen: {e}"))?;

    for (i, p) in packets.iter().enumerate() {
        report[1..].copy_from_slice(p);
        device.write(&report).map_err(|e| format!("Paket {i} von {}: {e}", packets.len()))?;
        if i % 16 == 15 {
            print!("\r  Seite {} von {pages}", i / 16 + 1);
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    }
    println!();
    Ok(())
}
