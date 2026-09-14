//! `takt-flash-weact`: Ein Abbild in den WeAct HID-Bootloader schreiben.
//!
//! ```text
//! takt-flash-weact <abbild.bin> [--dry-run] [--no-reboot]
//! ```
//!
//! **Der Name nennt das Protokoll, nicht die Aufgabe.** Flashen ist
//! nichts Allgemeines: Ein ST-Link spricht SWD, ein nRF52 mit
//! Adafruit-Bootloader UF2, ein ESP32 sein eigenes Protokoll ueber UART,
//! und dieses Werkzeug spricht das HID-Protokoll der WeAct-Boards. Ein
//! `takt-flash` ohne Zusatz verspraeche eine Abstraktion, die es nicht
//! gibt — und der erste Nutzer mit einem anderen Board hielte sie fuer
//! kaputt statt fuer unzustaendig.
//!
//! Wo es eine Abstraktion gibt, heisst sie `probe-rs`: Sie deckt SWD auf
//! STM32, nRF52 und vieles mehr ab, sobald eine Probe angeschlossen ist.
//! Dieses Werkzeug ist der Weg *ohne* Probe — mit dem Bootloader, den das
//! Board ab Werk mitbringt.
//!
//! **Das Board muss im Bootloader stehen.** KEY halten, NRST kurz
//! druecken, beide loslassen; die LED an PC13 blinkt dann. Nach dem
//! Schreiben springt das Werkzeug selbst in die Anwendung
//! (`--no-reboot` laesst es bleiben).

mod protocol;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use protocol::{Command, PRODUCT_ID, REPORT, REPORTS_PER_SECTOR, VENDOR_ID};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let reboot = !args.iter().any(|a| a == "--no-reboot");
    let Some(path) = args.iter().find(|a| !a.starts_with("--")) else {
        eprintln!("takt-flash-weact <abbild.bin> [--dry-run] [--no-reboot]");
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

    let sectors = protocol::sectors_for(image.len());
    let reports = protocol::reports(&image);
    println!("{path}: {} Byte, {sectors} Sektoren, {} Berichte", image.len(), reports.len());
    println!("  Ziel      {:#010x}", protocol::APP_ORIGIN);
    println!("  Stack     {:#010x}", u32::from_le_bytes([image[0], image[1], image[2], image[3]]));
    println!("  Reset     {:#010x}", u32::from_le_bytes([image[4], image[5], image[6], image[7]]));

    if dry_run {
        println!("\n--dry-run: nichts geschrieben.");
        return ExitCode::SUCCESS;
    }

    match flash(&reports, sectors, reboot) {
        Ok(()) => {
            println!("\nGeschrieben.{}", if reboot { " Das Board startet die Anwendung." } else { "" });
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("\n{e}");
            ExitCode::FAILURE
        }
    }
}

/// Schreibt die Berichte ueber HID.
fn flash(reports: &[[u8; REPORT]], sectors: usize, reboot: bool) -> Result<(), String> {
    let api = hidapi::HidApi::new().map_err(|e| format!("HID nicht verfuegbar: {e}"))?;
    let device = api.open(VENDOR_ID, PRODUCT_ID).map_err(|e| {
        format!(
            "Bootloader nicht gefunden ({VENDOR_ID:#06x}:{PRODUCT_ID:#06x}): {e}\n\
             Das Board muss im Bootloader stehen: KEY halten, NRST kurz druecken, beide loslassen."
        )
    })?;

    // Die Firmware-Fassung steht im USB-Deskriptor (`bcdDevice`). Der
    // Original-Flasher bricht unter 0x0200 ab; die Pruefung steht hier,
    // damit ein altes Board eine Aussage bekommt statt eines stillen
    // Fehlschlags — genau die Sorte Diagnose, die diesem Werkzeug in
    // seiner ersten Fassung fehlte.
    if let Some(info) = api.device_list().find(|d| d.vendor_id() == VENDOR_ID && d.product_id() == PRODUCT_ID) {
        let version = info.release_number();
        if version < protocol::MIN_FIRMWARE {
            return Err(format!(
                "Bootloader-Fassung {version:#06x} ist zu alt (noetig: {:#06x}). \
                 Der Flasher von WeAct kann sie aktualisieren.",
                protocol::MIN_FIRMWARE
            ));
        }
    }

    // Den Seitenzaehler zuruecksetzen. **Das Protokoll kennt keine
    // Adresse** — der Bootloader zaehlt selbst und legt den Versatz auf
    // 0x0800_4000; dieser Befehl stellt ihn auf Anfang.
    device.write(&protocol::command(Command::ResetPage)).map_err(|e| format!("ResetPage: {e}"))?;

    for (n, sector) in reports.chunks(REPORTS_PER_SECTOR).enumerate() {
        for (i, r) in sector.iter().enumerate() {
            device.write(r).map_err(|e| format!("Sektor {n}, Bericht {i}: {e}"))?;
            // Der Original-Flasher pausiert zwischen den Berichten; ohne
            // das ueberfaehrt man den Bootloader.
            std::thread::sleep(Duration::from_micros(500));
        }
        wait_for_ack(&device, n)?;
        print!("\r  Sektor {} von {sectors}", n + 1);
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    println!();

    if reboot {
        // Ohne Antwort: Der Bootloader springt und ist danach weg.
        device.write(&protocol::command(Command::Reboot)).map_err(|e| format!("Reboot: {e}"))?;
    }
    Ok(())
}

/// Wartet auf die Quittung eines Sektors.
///
/// **Mit Zeitgrenze, anders als das Original.** Der Flasher von WeAct
/// pollt endlos; bleibt die Quittung aus, haengt er ohne Aussage. Eine
/// Grenze macht aus dem Haenger eine Meldung — und die Meldung nennt den
/// Sektor, bei dem es stehenblieb.
fn wait_for_ack(device: &hidapi::HidDevice, sector: usize) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut reply = [0u8; 8];
    while Instant::now() < deadline {
        match device.read_timeout(&mut reply, 100) {
            Ok(0) => continue,
            Ok(_) if protocol::is_ack(&reply) => return Ok(()),
            Ok(_) => continue,
            Err(e) => return Err(format!("Sektor {sector}: Lesen fehlgeschlagen: {e}")),
        }
    }
    Err(format!(
        "Sektor {sector}: keine Quittung nach drei Sekunden.\n\
         Der Bootloader hat die Daten nicht angenommen — steht das Board noch im Bootloader?"
    ))
}
