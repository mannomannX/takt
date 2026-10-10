//! Board 2, ESP32-C6, vom Host aus (plan/esp32c6.md).
//!
//! Geschrieben und gestartet wird mit `probe-rs` ueber den JTAG-Teil des
//! USB-Serial-JTAG; der Trace kommt ueber dessen Konsole. Beide Haelften
//! koennen stehen bleiben — der JTAG-Teil nach einem Ueberlauf (FB-264),
//! der Empfangsendpunkt der Konsole (FB-266) —, und dann hilft, was der
//! EN-Pin taete: ein Chip-Reset auf RTC-Ebene, den das Board auf Wunsch
//! selbst ausloest, verlangt ueber JTAG oder ueber die Konsole.
//!
//! `TAKT_ESP32C6_PORT` nennt den Port der Konsole (`COM4`, `/dev/ttyACM0`).

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use takt_llvm::inspect::Binutils;
use takt_llvm::target::Target;

use super::{Board, Bringup, Builder, ConsoleLine, Failure, Form, Options, capture, port_listed, run_bounded};

const BRINGUP: Bringup = Bringup {
    dir: "crates/takt-bringup-esp32c6",
    triple: "riscv32imac-unknown-none-elf",
    inputs: &["build.rs", "Cargo.toml", "Cargo.lock", "board_ram.x", "millicode.S"],
    // 12.3: RAM-Residenz des Takt-Programms.
    env: &[("ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK", "true"), ("ESP_HAL_CONFIG_PLACE_SWITCH_TABLES_IN_RAM", "false")],
};

/// LP_AON STORE0, das einen Reset ueberlebt: Hier liest der Start den
/// Wunsch nach einem Chip-Reset (`takt-board-esp32c6/src/usb.rs`).
pub const REENUMERATE_REG: &str = "0x600b1000";

/// Der Wunsch `TAKT` als Wort, wie `takt-board-support::console::MAGIC`.
pub const REENUMERATE_MAGIC: &str = "0x54414b54";

/// Die Konsole spricht mit jeder Rate; der Wert zaehlt nicht.
const BAUD: u32 = 115_200;

/// Frist fuer einen `probe-rs`-Aufruf.
const PROBE_RS: Duration = Duration::from_secs(90);

/// Frist fuer den Trace nach dem Start.
const TRACE: Duration = Duration::from_secs(30);

/// Board 2 am Host.
#[derive(Clone, Debug)]
pub struct Esp32c6 {
    port: String,
}

impl Esp32c6 {
    /// Das Board an `TAKT_ESP32C6_PORT`; `None` ohne die Variable.
    pub fn from_env() -> Option<Esp32c6> {
        std::env::var("TAKT_ESP32C6_PORT").ok().map(|port| Esp32c6 { port })
    }

    /// Der Port der Konsole.
    pub fn port(&self) -> &str {
        &self.port
    }

    /// Schreibt ein Abbild, ohne es zu starten.
    pub fn download(&self, elf: &Path) -> Result<(), String> {
        self.probe_rs(&["download", "--chip", "esp32c6", &elf.to_string_lossy()]).map(|_| ())
    }

    /// Setzt das Board zurueck und liest den Trace bis `takt end`.
    ///
    /// Fehlt das Ende, entscheidet der Tickzaehler ueber JTAG: Steht das
    /// Programm vor dem Ende, ist das der Befund, und der Text gehoert in
    /// die Meldung. Lief es durch, hat die Konsole geschwiegen — gleich nach
    /// dem Flashen, weil der USB-Serial-JTAG neu anlegt und ein Handle von
    /// davor nichts liefert, genuegt ein zweiter Reset; sonst steht ihr
    /// Empfangsendpunkt (FB-266), und das Board meldet sein USB-Geraet auf
    /// Wunsch neu an — ein Neustecken ohne Hand.
    pub fn capture(&self, elf: &Path, options: &Options) -> Result<String, String> {
        let ticks = options.ticks;
        let mut last = String::new();
        for attempt in 0..3 {
            let next = if attempt == 0 { "neuer Versuch" } else { "Neuanmeldung des USB-Geraets" };
            let text = match self.capture_within(&options.console, options.within()) {
                Ok(text) => text,
                Err(e) if attempt < 2 => {
                    eprintln!("{e}; {next}");
                    last = e;
                    if attempt == 1 {
                        self.reenumerate()?;
                    }
                    continue;
                }
                Err(e) => return Err(e),
            };
            if text.contains(super::END) {
                return Ok(text);
            }
            let tick = self.tick_over_jtag(elf)?;
            if u64::from(tick) < ticks {
                return Err(format!(
                    "kein `takt end` binnen {} s, das Programm steht bei Tick {tick}; gelesen:\n{text}",
                    options.within().as_secs()
                ));
            }
            let when = if text.is_empty() { "" } else { " mittendrin" };
            last = format!("das Programm lief bis Tick {tick}, die Konsole schwieg{when}");
            eprintln!("{last}; {next}");
            if attempt > 0 {
                self.reenumerate()?;
            }
        }
        Err(format!("{last}; auch nach Neuanmeldung des USB-Geraets — Kabel neu stecken"))
    }

    /// Ein Reset und ein Lesen mit harter Frist; `console` geht waehrenddessen
    /// an die Konsole des Boards.
    pub fn capture_once(&self, console: &[ConsoleLine]) -> Result<String, String> {
        self.capture_within(console, TRACE)
    }

    /// [`Esp32c6::capture_once`] mit der Frist `within`.
    fn capture_within(&self, console: &[ConsoleLine], within: Duration) -> Result<String, String> {
        // Nach dem Flashen legt der USB-Serial-JTAG neu an; ein Handle von
        // davor liefert nichts. Darum kurz warten und je Versuch neu oeffnen.
        std::thread::sleep(Duration::from_millis(500));
        // USB staut selbst zurueck; XON/XOFF braucht es hier nicht.
        let flow = serialport::FlowControl::None;
        let reset = || self.probe_rs(&["reset", "--chip", "esp32c6"]).map(|_| ());
        capture(&self.port, BAUD, flow, within, true, console, reset)
    }

    /// Schreibt das Abbild, startet es und liest bis `takt end`, hoechstens
    /// `within` lang — auch einen Lauf, der nicht endet oder ueber Resets
    /// hinweg geht (12.3, 12.7). Ohne die Genesung aus [`Esp32c6::capture`]:
    /// Wer ueber Resets liest, will sie sehen.
    pub fn run_for(&mut self, elf: &Path, within: Duration) -> Result<String, String> {
        self.download(elf)?;
        std::thread::sleep(Duration::from_millis(500));
        let reset = || self.probe_rs(&["reset", "--chip", "esp32c6"]).map(|_| ());
        capture(&self.port, BAUD, serialport::FlowControl::None, within, true, &[], reset)
    }

    /// Liest, was das Board von sich aus schreibt, ohne es zurueckzusetzen —
    /// nach einem Neustart, den das Programm befiehlt (12.7).
    pub fn listen(&self, within: Duration) -> Result<String, String> {
        capture(&self.port, BAUD, serialport::FlowControl::None, within, false, &[], || Ok(()))
    }

    /// Wartet, bis der Port verschwindet: Im Tiefschlaf ist der
    /// USB-Serial-JTAG aus (12.7).
    pub fn port_gone(&self, within: Duration) -> bool {
        port_listed(&self.port, false, within)
    }

    /// Wartet, bis der Port wieder da ist: Der Chip ist aus dem Tiefschlaf
    /// erwacht (12.7).
    pub fn listen_port(&self, within: Duration) -> bool {
        port_listed(&self.port, true, within)
    }

    /// Der Tickzaehler des Rahmens, ueber JTAG gelesen: in der Arena
    /// Die Arena `app_arena` des Bring-ups an der Stelle, die es als `__takt_tick_at` ablegt.
    pub fn tick_over_jtag(&self, elf: &Path) -> Result<u32, String> {
        let symbols = symbols(elf)?;
        let at = |name: &str| {
            symbols.iter().find(|s| s.name == name).map(|s| s.address).ok_or(format!("`{name}` fehlt im Abbild"))
        };
        self.word_at(at("app_arena")? + at("__takt_tick_at")?)
    }

    /// Ein Wort des laufenden Programms ueber JTAG, am ersten Symbol, auf
    /// dessen Namen `wanted` passt — auch wenn die Konsole schweigt.
    pub fn word_over_jtag(&self, elf: &Path, wanted: impl Fn(&str) -> bool) -> Result<u32, String> {
        let symbols = symbols(elf)?;
        let symbol = symbols.iter().find(|s| wanted(&s.name)).ok_or("das Symbol fehlt im Abbild")?;
        self.word_at(symbol.address)
    }

    /// Das Wort an `address` ueber JTAG.
    fn word_at(&self, address: u64) -> Result<u32, String> {
        let address = format!("{address:#x}");
        let out = self
            .probe_rs(&["read", "--chip", "esp32c6", "b32", &address, "1"])
            .map_err(|e| format!("JTAG antwortet nicht (Kabel neu stecken):\n{e}"))?;
        // `40802478: 0000003c`
        out.lines()
            .find_map(|l| l.split_once(": ").and_then(|(_, v)| u32::from_str_radix(v.trim(), 16).ok()))
            .ok_or_else(|| "probe-rs read: kein Wert".to_string())
    }

    /// `probe-rs` mit seiner Ausgabe. Steht der JTAG-Teil (DMI-Timeout,
    /// FB-264) oder kehrt `probe-rs` nicht zurueck, verlangt der Aufruf den
    /// Chip-Reset ueber die Konsole und versucht es einmal neu.
    pub fn probe_rs(&self, args: &[&str]) -> Result<String, String> {
        match run_bounded("probe-rs", args, PROBE_RS) {
            Err(Failure::Timeout) => self.recover(args, "probe-rs kehrt nicht zurueck".to_string()),
            Err(Failure::Status(e)) if e.contains("DMI") => self.recover(args, e),
            r => r.map_err(|e| e.to_string()),
        }
    }

    /// `probe-rs` ohne Genesung, fuer den Test der Genesung selbst.
    pub fn probe_rs_plain(&self, args: &[&str]) -> Result<String, String> {
        run_bounded("probe-rs", args, PROBE_RS).map_err(|e| format!("probe-rs: {e}"))
    }

    fn recover(&self, args: &[&str], why: String) -> Result<String, String> {
        eprintln!("JTAG antwortet nicht ({}); Chip-Reset ueber die Konsole", why.lines().next().unwrap_or(""));
        self.reenumerate_via_console()?;
        run_bounded("probe-rs", args, PROBE_RS).map_err(|e| format!("probe-rs nach dem Chip-Reset: {e}"))
    }

    /// Laesst das Board sein USB-Geraet neu anmelden und wartet den Port ab.
    pub fn reenumerate(&self) -> Result<(), String> {
        self.probe_rs(&["write", "--chip", "esp32c6", "b32", REENUMERATE_REG, REENUMERATE_MAGIC])?;
        self.probe_rs(&["reset", "--chip", "esp32c6"])?;
        self.settle_port()
    }

    /// Derselbe Wunsch ueber die Konsole, wenn JTAG nicht antwortet: erst ein
    /// Reset ueber RTS, damit der Start einen alten, ungelesenen Puffer leert
    /// (sonst blockiert der Host beim Schreiben), dann `TAKT` in den
    /// Empfangspuffer und ein zweiter Reset — der Puffer ueberlebt ihn, und
    /// das Bring-up liest ihn beim Start. EN liegt tief, solange RTS steht
    /// und DTR nicht (die Logik des USB-Serial-JTAG, wie bei esptool).
    /// Meldet sich das Geraet schon unterwegs ab — ein alter Puffer trug den
    /// Wunsch schon —, ist das der gewuenschte Ausgang, und der Rest entfaellt.
    pub fn reenumerate_via_console(&self) -> Result<(), String> {
        let port = &self.port;
        let mut serial =
            serialport::new(port, BAUD).timeout(Duration::from_secs(1)).open().map_err(|e| format!("{port}: {e}"))?;
        let steps: [&Step; 4] = [
            &|s| control_lines(s, true, 100),
            &|s| control_lines(s, false, 400),
            &|s| {
                s.write_all(b"TAKT")?;
                s.flush()?;
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            },
            &|s| control_lines(s, true, 100).and_then(|()| control_lines(s, false, 0)),
        ];
        let mut failure = None;
        for step in steps {
            if let Err(e) = step(&mut serial) {
                // Ein abgemeldetes Geraet meldet der Treiber als Fehler; ob
                // es das war, sagt die Portliste, nicht der Text.
                failure = Some(format!("{port}: {e}"));
                break;
            }
        }
        drop(serial);
        if !port_listed(port, false, Duration::from_secs(3)) {
            return Err(failure.unwrap_or_else(|| {
                "das USB-Geraet hat sich nicht abgemeldet: Bring-up ohne Neuanmeldung?".to_string()
            }));
        }
        if !port_listed(port, true, Duration::from_secs(10)) {
            return Err(format!("{port} kam nach der Neuanmeldung nicht zurueck"));
        }
        std::thread::sleep(Duration::from_secs(1));
        Ok(())
    }

    /// Wartet, bis der Port weg war und wieder da ist.
    fn settle_port(&self) -> Result<(), String> {
        if !port_listed(&self.port, false, Duration::from_secs(3)) {
            return Err("das USB-Geraet hat sich nicht abgemeldet: Bring-up ohne Neuanmeldung?".to_string());
        }
        if !port_listed(&self.port, true, Duration::from_secs(10)) {
            return Err(format!("{} kam nach der Neuanmeldung nicht zurueck", self.port));
        }
        std::thread::sleep(Duration::from_secs(1));
        Ok(())
    }
}

impl Board for Esp32c6 {
    fn name(&self) -> &'static str {
        "esp32c6"
    }

    fn target(&self) -> &'static str {
        "riscv32imac"
    }

    fn builder(&self, slot: usize) -> Builder {
        Box::new(move |program, options| BRINGUP.build(program, options, slot))
    }

    /// Die Interruptform auf dem SYSTIMER (12.11).
    fn forms(&self) -> &'static [Form] {
        &[Form::Own, Form::Interrupt]
    }

    fn run(&mut self, elf: &Path, options: &Options) -> Result<String, String> {
        self.download(elf)?;
        self.capture(elf, options).and_then(|text| super::complete(text, options))
    }
}

/// Die Symbole des Abbilds.
fn symbols(elf: &Path) -> Result<Vec<takt_llvm::inspect::Symbol>, String> {
    Binutils::best_for(Target::RISCV32IMAC).symbols(elf).ok_or_else(|| "`nm` fehlt: kein Blick ins Abbild".to_string())
}

/// Ein Schritt an der Leitung.
type Step = dyn Fn(&mut Box<dyn serialport::SerialPort>) -> serialport::Result<()>;

/// DTR tief, RTS wie angegeben, dann warten: RTS mit tiefem DTR ist EN tief.
fn control_lines(serial: &mut Box<dyn serialport::SerialPort>, rts: bool, wait_ms: u64) -> serialport::Result<()> {
    serial.write_data_terminal_ready(false)?;
    serial.write_request_to_send(rts)?;
    std::thread::sleep(Duration::from_millis(wait_ms));
    Ok(())
}
