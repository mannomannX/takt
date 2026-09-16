//! Die Hardware-Konfiguration (8.10), soweit die Analyse sie braucht.
//!
//! **Was hier steht.** 8.10 beschreibt acht Feldgruppen. Umgesetzt sind
//! die, die ein Werkzeug liest: die Kalibrierung je Ziel (Pruefungen 12
//! und 32), Speicher und Stack-Reserven je Ziel (Pruefung 39), Geraete
//! und Kanaele mit Anschluss und Messwerten (Pruefungen 28 und 60; der
//! Anschluss geht an das Board weiter, 9.5). Topologie und Herkunft
//! kommen, wenn jemand danach fragt.
//!
//! **Warum die Kalibrierung zuerst.** Ohne `c_target` ist die zentrale
//! Zeitzusage der Sprache unbelegt: `takt cost` rechnet Operationen, aber
//! was sie dauern, kann niemand sagen. Prüfung 12 meldet darum heute nur
//! einen Hinweis, Prüfung 32 gibt es nicht, und `wcet` im Budget wird
//! abgelehnt statt geprueft. Alle drei haengen an dieser einen Tabelle.
//!
//! ## Das Format
//!
//! Zeilenweise Text, kein TOML und kein JSON: Die Datei hat heute zwei
//! Abschnittsarten und acht Werte, und eine Abhaengigkeit dafuer waere
//! teurer als der Leser. Sie ist von Hand lesbar, weil ein Mensch sie
//! nachrechnen koennen muss — eine Kalibrierung, der man nicht ansieht,
//! wie sie zustande kam, ist eine Zahl ohne Herkunft.
//!
//! ```text
//! # takt-hw 3
//! [target.thumbv7em]
//! core_hz = 84000000
//! i32 = 11900        # Pikosekunden je Operation
//! f64 = 1190000
//! t_io = 120000
//! ram = 65536
//! flash = 262144
//!
//! [device.gpio]
//! driver = "stm32-gpio"
//!
//! [channel ui/led]
//! direction = output
//! raw = bool
//! safe = false
//! device = gpio
//! port = "PC13 active_low"   # undurchsichtig, geht ans Board (8.10)
//! jitter_ns = 250000         # gemessen (13.8)
//! ```
//!
//! **Pikosekunden, nicht Nanosekunden.** Eine `i32`-Operation dauert bei
//! 84 MHz rund 12 ns, auf einem 1-GHz-Kern rund 1 ns. Ganzzahlige
//! Nanosekunden runden dort bei *jeder* Klasse, und die Fehler summieren
//! sich ueber Tausende Operationen je Tick. Keine Fliesskommazahl, weil
//! 4.2 bitgleiche Ergebnisse ueber alle Targets verlangt und das
//! Skalarprodukt sonst von der Rundung des Wirts abhinge.
//!
//! **Der Schluessel ist der Kern, nicht die Zielklasse.** Die Referenz
//! sagt an drei Stellen Verschiedenes („je Target", „je Zielklasse",
//! `target = <profil>`); sachlich entscheidet der Kern mit seiner
//! Frequenz: Zwei Boards derselben Zielklasse mit 84 und 168 MHz haben
//! verschiedene Tabellen. Die Zielklasse ist die Ebene, auf der `takt
//! bench` *berichtet*, nicht die, auf der gemessen wird.

use std::collections::BTreeMap;

use crate::fns::{CostClass, CostVec};

/// Formatversion dieses Schreibers (11.3).
///
/// Leser akzeptieren jede Version bis zu ihrer eigenen; Schreiber
/// schreiben die neueste. Dieselbe Regel wie beim MIR-Format.
///
/// 2: NVM-Geometrie fuer das `persist`-Journal (5.9). 3: Geraete, Kanaele,
/// Speicher und Stack-Reserven (8.10).
pub const FORMAT_VERSION: u32 = 3;

/// Die Kennung in der ersten Zeile.
const MAGIC: &str = "takt-hw";

/// Pikosekunden je Operation, je Klasse (9.4.3, 13.8).
///
/// **Die Tabelle, die Operationen zu Zeit macht.** `takt cost` zaehlt,
/// was eine Aktivierung an Operationen braucht; erst das Skalarprodukt
/// mit dieser Tabelle ergibt eine Dauer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CTarget {
    /// Pikosekunden je Operation, in der Reihenfolge von [`CostClass::ALL`].
    ps: [u64; 7],
}

impl CTarget {
    /// Das Gewicht einer Klasse in Pikosekunden.
    pub fn of(&self, c: CostClass) -> u64 {
        self.ps[c as usize]
    }

    /// Setzt das Gewicht einer Klasse.
    pub fn set(&mut self, c: CostClass, ps: u64) {
        self.ps[c as usize] = ps;
    }

    /// Ist die Tabelle vollstaendig?
    ///
    /// **Eine Null ist kein Messwert.** Fehlt auch nur eine Klasse, ist
    /// das Skalarprodukt zu klein, und eine Schedulability, die auf einer
    /// zu kleinen Zahl beruht, ist schlimmer als keine: Sie sagt „passt",
    /// wo sie nichts weiss. Einzige Ausnahme ist `native` — ein Programm
    /// ohne native Funktionen braucht das Gewicht nicht, und es zu
    /// verlangen hiesse, eine Messung fuer etwas zu fordern, das nicht
    /// vorkommt.
    pub fn is_complete(&self) -> bool {
        CostClass::ALL.iter().all(|c| *c == CostClass::Native || self.of(*c) > 0)
    }

    /// Die Klassen ohne Messwert.
    pub fn missing(&self) -> Vec<CostClass> {
        CostClass::ALL.iter().copied().filter(|c| *c != CostClass::Native && self.of(*c) == 0).collect()
    }

    /// Die Dauer eines Operationsvektors in Pikosekunden (9.4.3).
    ///
    /// `Σ_c N_c · c_target[c]`. Saettigt statt zu ueberlaufen: Ein
    /// Programm mit absurd vielen Operationen soll eine absurd grosse
    /// Dauer melden und daran scheitern, nicht eine kleine und
    /// durchgehen.
    pub fn duration_ps(&self, n: CostVec) -> u64 {
        CostClass::ALL.iter().fold(0u64, |acc, c| acc.saturating_add(n.of(*c).saturating_mul(self.of(*c))))
    }

    /// Dieselbe Dauer in Nanosekunden, kaufmaennisch gerundet.
    ///
    /// Fuer Meldungen: Nanosekunden sind die Einheit, in der die Sprache
    /// sonst ueber Zeit spricht (3.3). Gerechnet wird in Pikosekunden.
    pub fn duration_ns(&self, n: CostVec) -> u64 {
        self.duration_ps(n).saturating_add(500) / 1000
    }
}

/// Ein Ziel mit seiner Kalibrierung (8.10, „Kalibrierung je Ziel").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    /// Der Name, wie ihn `takt build --target` kennt.
    pub name: String,
    /// Kerntakt in Hertz; `None`, wenn die Messung ihn nicht festhielt.
    pub core_hz: Option<u32>,
    /// Die Kostentabelle.
    pub c_target: CTarget,
    /// Was der Tick ausserhalb des Programms kostet, in Pikosekunden.
    ///
    /// 7.2 zieht sie von `T₀` ab, bevor das Budget verglichen wird. Was
    /// genau hineinfaellt — Abtastung, Commit, Recorder —, sagt die
    /// Referenz nicht; gemessen wird die Differenz zwischen Tickperiode
    /// und dem, was das Programm davon nutzt (13.8).
    pub t_io_ps: u64,
    /// Die NVM-Geometrie hinter `persist var`, falls das Ziel eine hat.
    pub nvm: Option<NvmGeometry>,
    /// Speicher und Stack-Reserven (11.5, 12.3).
    pub memory: Memory,
}

/// Speicher eines Ziels (8.10: „Speicher und Stack"), in Byte.
///
/// `ram` und `flash` stehen im Datenblatt; die Reserven misst 13.8 und
/// die Marge waehlt das Projekt (12.3) — beides je Ziel, nicht je Sprache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// Daten-RAM.
    pub ram: Option<u64>,
    /// Flash fuer Code, Konstanten und Journal.
    pub flash: Option<u64>,
    /// Instruktions-RAM auf XIP-Zielen (12.3).
    pub iram: Option<u64>,
    /// Stack-Reserven fuer Runtime, Treiber, ISRs und RTOS (12.3).
    pub stack_reserve: Option<u64>,
    /// Marge auf den gerechneten Stack (12.3).
    pub stack_margin: Option<u64>,
}

/// Ein Geraet (8.10: Treibertyp, Adresse, Heartbeat, Zykluszeit).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// Name des Abschnitts.
    pub name: String,
    /// Treibertyp; das Board stellt ihn (9.5).
    pub driver: Option<String>,
    /// Busadresse.
    pub address: Option<String>,
    /// Heartbeat in Nanosekunden (12.4).
    pub heartbeat_ns: Option<i64>,
    /// Zykluszeit in Nanosekunden.
    pub cycle_ns: Option<i64>,
    /// `fifo_depth` gepollter Geraete (Pruefung 59).
    pub fifo_depth: Option<u32>,
    /// `byte_rate` gepollter Geraete.
    pub byte_rate: Option<u64>,
}

/// Ein Kanal (8.10), adressiert wie im Programm (`@ hw("…")`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HwChannel {
    /// Die Adresse, Schluessel der Bindung (8.1).
    pub address: String,
    /// Richtung.
    pub direction: Option<crate::program::Direction>,
    /// Rohtyp des Geraets.
    pub raw: Option<String>,
    /// Einheit, nominal wie in 3.2.
    pub unit: Option<String>,
    /// Range in der Einheit, beide Grenzen einschliesslich.
    pub range: Option<(f64, f64)>,
    /// `safe`-Wert eines Outputs, als Text des Literals.
    pub safe: Option<String>,
    /// Das Geraet, das ihn bedient.
    pub device: Option<String>,
    /// Anschluss: treiberspezifisch, undurchsichtig (8.10).
    pub port: Option<String>,
    /// Rate in Hertz.
    pub rate_hz: Option<u64>,
    /// Gemessener Jitter eines Outputs in Nanosekunden (13.8).
    pub jitter_ns: Option<i64>,
    /// Gemessene Abtastlatenz eines Inputs in Nanosekunden (13.8).
    pub latency_ns: Option<i64>,
}

/// Was das Journal vom nichtfluechtigen Speicher wissen muss (5.9, 11.5).
///
/// Sie gehoert zum Ziel, nicht zur Sprache: Ein Flash mit 10^5 Zyklen
/// vertraegt haeufigeres Schreiben als einer mit 10^4, und die Sektorgroesse
/// entscheidet, wie viel Flash `takt size` fuer die zwei Slots ausweist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NvmGeometry {
    /// Groesse eines Sektors in Byte; ein Slot belegt ganze Sektoren.
    pub sector_bytes: u32,
    /// Wie viele Sektoren fuer das Journal bereitstehen, beide Slots
    /// zusammen.
    pub sectors: u32,
    /// Schreibabstand, wenn kein `min_interval` deklariert ist.
    pub default_min_interval_ns: i64,
}

impl NvmGeometry {
    /// Groesse eines Slots in Byte: die Haelfte der Sektoren, abgerundet.
    pub fn slot_bytes(&self) -> u32 {
        self.sector_bytes.saturating_mul(self.sectors / 2)
    }
}

/// Die Hardware-Konfiguration, soweit gelesen (8.10).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    /// Die Formatversion der gelesenen Datei.
    pub format_version: u32,
    /// Die Ziele, nach Namen.
    pub targets: BTreeMap<String, Target>,
    /// Die Geraete, nach Namen.
    pub devices: BTreeMap<String, Device>,
    /// Die Kanaele, nach Adresse.
    pub channels: BTreeMap<String, HwChannel>,
}

impl Hardware {
    /// Die Kalibrierung eines Ziels.
    pub fn target(&self, name: &str) -> Option<&Target> {
        self.targets.get(name)
    }

    /// Der Kanal zu einer Adresse.
    pub fn channel(&self, address: &str) -> Option<&HwChannel> {
        self.channels.get(address)
    }
}

/// Welcher Abschnitt gerade gelesen wird.
enum Section {
    Target(String),
    Device(String),
    Channel(String),
}

/// Was beim Lesen schiefgehen kann.
///
/// **Jeder Fall nennt die Zeile.** Eine Konfiguration wird von Hand
/// bearbeitet und von Werkzeugen geschrieben; ein Fehler ohne Fundstelle
/// laesst den Nutzer suchen, und die Datei ist kein Programm, dem ein
/// Compiler Spalten anstreichen wuerde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// Zeilennummer, ab 1.
    pub line: u32,
    /// Was nicht stimmt.
    pub message: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Zeile {}: {}", self.line, self.message)
    }
}

/// Liest eine Hardware-Konfiguration.
///
/// Der Leser ist bewusst streng: Ein unbekannter Schluessel ist ein
/// Fehler, kein ueberlesenes Feld. Das ist die Gegenrichtung zum
/// MIR-Format, wo unbekannte Feldnummern ueberlesen werden — dort
/// schreibt ein Werkzeug fuer ein Werkzeug, hier ein Mensch fuer einen
/// Compiler, und ein Tippfehler in `c_targt` soll auffallen statt still
/// zu wirken.
pub fn parse(text: &str) -> Result<Hardware, ParseError> {
    let mut out = Hardware::default();
    let mut current: Option<Section> = None;
    let mut seen_magic = false;

    for (i, raw) in text.lines().enumerate() {
        let line_no = i as u32 + 1;
        let line = raw.split('#').next().unwrap_or("").trim();

        // Die Kennung steht im Kommentar der ersten Zeile, damit die Datei
        // mit einem lesbaren Kopf beginnt und trotzdem maschinell
        // erkennbar ist.
        if !seen_magic {
            if let Some(v) = magic_version(raw) {
                out.format_version = v;
                seen_magic = true;
                if v > FORMAT_VERSION {
                    return Err(ParseError {
                        line: line_no,
                        message: format!("Formatversion {v} ist neuer als {FORMAT_VERSION} (11.3)"),
                    });
                }
                continue;
            }
            if !line.is_empty() {
                return Err(ParseError {
                    line: line_no,
                    message: format!("die Datei muss mit `# {MAGIC} <version>` beginnen"),
                });
            }
        }

        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').ok_or_else(|| ParseError {
                line: line_no,
                message: "unabgeschlossener Abschnitt: `]` fehlt".into(),
            })?;
            current = Some(section(name, &mut out, line_no)?);
            continue;
        }

        let (key, value) = line.split_once('=').ok_or_else(|| ParseError {
            line: line_no,
            message: format!("weder Abschnitt noch Zuweisung: `{line}`"),
        })?;
        let (key, value) = (key.trim(), value.trim());
        match &current {
            Some(Section::Target(name)) => {
                let target = out.targets.get_mut(name).expect("Abschnitt angelegt");
                target_key(target, key, value, line_no)?;
            }
            Some(Section::Device(name)) => {
                let device = out.devices.get_mut(name).expect("Abschnitt angelegt");
                device_key(device, key, value, line_no)?;
            }
            Some(Section::Channel(address)) => {
                let channel = out.channels.get_mut(address).expect("Abschnitt angelegt");
                channel_key(channel, key, value, line_no)?;
            }
            None => {
                return Err(ParseError {
                    line: line_no,
                    message: format!("`{key}` steht vor jedem Abschnitt; erwartet `[target.<name>]`"),
                });
            }
        }
    }

    if !seen_magic {
        return Err(ParseError { line: 1, message: format!("die Datei muss mit `# {MAGIC} <version>` beginnen") });
    }
    Ok(out)
}

/// Die NVM-Geometrie eines Ziels, bei Bedarf angelegt.
fn nvm_of(target: &mut Target) -> &mut NvmGeometry {
    target.nvm.get_or_insert_with(NvmGeometry::default)
}

/// Legt den Abschnitt an: `target.<name>`, `device.<name>`, `channel <adresse>`.
fn section(name: &str, out: &mut Hardware, line: u32) -> Result<Section, ParseError> {
    if let Some(target) = name.strip_prefix("target.") {
        if target.is_empty() {
            return Err(ParseError { line, message: "`target.` ohne Namen".into() });
        }
        out.targets
            .entry(target.to_string())
            .or_insert_with(|| Target { name: target.to_string(), ..Target::default() });
        return Ok(Section::Target(target.to_string()));
    }
    if let Some(device) = name.strip_prefix("device.") {
        if device.is_empty() {
            return Err(ParseError { line, message: "`device.` ohne Namen".into() });
        }
        out.devices
            .entry(device.to_string())
            .or_insert_with(|| Device { name: device.to_string(), ..Device::default() });
        return Ok(Section::Device(device.to_string()));
    }
    if let Some(address) = name.strip_prefix("channel ") {
        let address = address.trim();
        if address.is_empty() {
            return Err(ParseError { line, message: "`channel` ohne Adresse".into() });
        }
        out.channels
            .entry(address.to_string())
            .or_insert_with(|| HwChannel { address: address.to_string(), ..HwChannel::default() });
        return Ok(Section::Channel(address.to_string()));
    }
    Err(ParseError {
        line,
        message: format!(
            "unbekannter Abschnitt `{name}`; bekannt: `target.<name>`, `device.<name>`, `channel <adresse>`"
        ),
    })
}

fn number(value: &str, line: u32) -> Result<u64, ParseError> {
    value.parse().map_err(|_| ParseError { line, message: format!("`{value}` ist keine ganze Zahl") })
}

/// Ein Text, mit oder ohne Anfuehrungszeichen.
fn text(value: &str) -> String {
    value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value).to_string()
}

fn target_key(target: &mut Target, key: &str, value: &str, line: u32) -> Result<(), ParseError> {
    match key {
        "core_hz" => {
            let n = number(value, line)?;
            target.core_hz = Some(
                u32::try_from(n).map_err(|_| ParseError { line, message: format!("{n} Hz passt nicht in 32 Bit") })?,
            );
        }
        "t_io" => target.t_io_ps = number(value, line)?,
        "nvm_sector_bytes" => nvm_of(target).sector_bytes = number(value, line)? as u32,
        "nvm_sectors" => nvm_of(target).sectors = number(value, line)? as u32,
        "nvm_min_interval" => nvm_of(target).default_min_interval_ns = number(value, line)? as i64,
        "ram" => target.memory.ram = Some(number(value, line)?),
        "flash" => target.memory.flash = Some(number(value, line)?),
        "iram" => target.memory.iram = Some(number(value, line)?),
        "stack_reserve" => target.memory.stack_reserve = Some(number(value, line)?),
        "stack_margin" => target.memory.stack_margin = Some(number(value, line)?),
        _ => {
            let class = CostClass::ALL.iter().find(|c| c.name() == key).ok_or_else(|| ParseError {
                line,
                message: format!(
                    "unbekannter Schluessel `{key}`; bekannt: core_hz, t_io, ram, flash, iram, stack_reserve, \
                     stack_margin, nvm_sector_bytes, nvm_sectors, nvm_min_interval und die Klassen {}",
                    CostClass::ALL.iter().map(|c| c.name()).collect::<Vec<_>>().join(", ")
                ),
            })?;
            target.c_target.set(*class, number(value, line)?);
        }
    }
    Ok(())
}

fn device_key(device: &mut Device, key: &str, value: &str, line: u32) -> Result<(), ParseError> {
    match key {
        "driver" => device.driver = Some(text(value)),
        "address" => device.address = Some(text(value)),
        "heartbeat_ns" => device.heartbeat_ns = Some(number(value, line)? as i64),
        "cycle_ns" => device.cycle_ns = Some(number(value, line)? as i64),
        "fifo_depth" => device.fifo_depth = Some(number(value, line)? as u32),
        "byte_rate" => device.byte_rate = Some(number(value, line)?),
        _ => {
            return Err(ParseError {
                line,
                message: format!(
                    "unbekannter Schluessel `{key}`; bekannt: driver, address, heartbeat_ns, cycle_ns, fifo_depth, \
                     byte_rate"
                ),
            });
        }
    }
    Ok(())
}

fn channel_key(channel: &mut HwChannel, key: &str, value: &str, line: u32) -> Result<(), ParseError> {
    match key {
        "direction" => {
            channel.direction = Some(match value {
                "input" => crate::program::Direction::Input,
                "output" => crate::program::Direction::Output,
                other => {
                    return Err(ParseError { line, message: format!("`{other}` ist keine Richtung (input, output)") });
                }
            });
        }
        "raw" => channel.raw = Some(text(value)),
        "unit" => channel.unit = Some(text(value)),
        "range" => {
            let (lo, hi) = value
                .split_once("..")
                .ok_or_else(|| ParseError { line, message: format!("`{value}` ist keine Range (`lo..hi`)") })?;
            let parse = |t: &str| {
                t.trim().parse::<f64>().map_err(|_| ParseError { line, message: format!("`{t}` ist keine Zahl") })
            };
            channel.range = Some((parse(lo)?, parse(hi)?));
        }
        "safe" => channel.safe = Some(text(value)),
        "device" => channel.device = Some(text(value)),
        "port" => channel.port = Some(text(value)),
        "rate_hz" => channel.rate_hz = Some(number(value, line)?),
        "jitter_ns" => channel.jitter_ns = Some(number(value, line)? as i64),
        "latency_ns" => channel.latency_ns = Some(number(value, line)? as i64),
        _ => {
            return Err(ParseError {
                line,
                message: format!(
                    "unbekannter Schluessel `{key}`; bekannt: direction, raw, unit, range, safe, device, port, \
                     rate_hz, jitter_ns, latency_ns"
                ),
            });
        }
    }
    Ok(())
}

/// Die Version aus `# takt-hw <n>`.
fn magic_version(line: &str) -> Option<u32> {
    let rest = line.trim().strip_prefix('#')?.trim().strip_prefix(MAGIC)?;
    rest.split_whitespace().next()?.parse().ok()
}

/// Schreibt eine Konfiguration im kanonischen Format.
///
/// Die Ausgabe ist wieder einlesbar und stabil sortiert: `takt bench`
/// schreibt sie, ein Mensch liest sie, und ein Vergleich zweier Laeufe
/// soll die Unterschiede zeigen statt einer anderen Reihenfolge.
pub fn render(hw: &Hardware) -> String {
    let mut s = format!("# {MAGIC} {FORMAT_VERSION}\n");
    s.push_str("# Kalibrierung je Ziel (8.10, 13.8). Zeiten in Pikosekunden.\n");
    for target in hw.targets.values() {
        s.push_str(&format!("\n[target.{}]\n", target.name));
        if let Some(hz) = target.core_hz {
            s.push_str(&format!("core_hz = {hz}\n"));
        }
        for c in CostClass::ALL {
            s.push_str(&format!("{} = {}\n", c.name(), target.c_target.of(c)));
        }
        s.push_str(&format!("t_io = {}\n", target.t_io_ps));
        if let Some(nvm) = target.nvm {
            s.push_str(&format!("nvm_sector_bytes = {}\n", nvm.sector_bytes));
            s.push_str(&format!("nvm_sectors = {}\n", nvm.sectors));
            s.push_str(&format!("nvm_min_interval = {}\n", nvm.default_min_interval_ns));
        }
        let m = target.memory;
        for (key, value) in [
            ("ram", m.ram),
            ("flash", m.flash),
            ("iram", m.iram),
            ("stack_reserve", m.stack_reserve),
            ("stack_margin", m.stack_margin),
        ] {
            if let Some(v) = value {
                s.push_str(&format!("{key} = {v}\n"));
            }
        }
    }
    for d in hw.devices.values() {
        s.push_str(&format!("\n[device.{}]\n", d.name));
        if let Some(v) = &d.driver {
            s.push_str(&format!("driver = \"{v}\"\n"));
        }
        if let Some(v) = &d.address {
            s.push_str(&format!("address = \"{v}\"\n"));
        }
        for (key, value) in [("heartbeat_ns", d.heartbeat_ns), ("cycle_ns", d.cycle_ns)] {
            if let Some(v) = value {
                s.push_str(&format!("{key} = {v}\n"));
            }
        }
        if let Some(v) = d.fifo_depth {
            s.push_str(&format!("fifo_depth = {v}\n"));
        }
        if let Some(v) = d.byte_rate {
            s.push_str(&format!("byte_rate = {v}\n"));
        }
    }
    for c in hw.channels.values() {
        s.push_str(&format!("\n[channel {}]\n", c.address));
        if let Some(d) = c.direction {
            let name = match d {
                crate::program::Direction::Input => "input",
                crate::program::Direction::Output => "output",
            };
            s.push_str(&format!("direction = {name}\n"));
        }
        for (key, value) in [("raw", &c.raw), ("unit", &c.unit), ("safe", &c.safe), ("device", &c.device)] {
            if let Some(v) = value {
                s.push_str(&format!("{key} = {v}\n"));
            }
        }
        if let Some((lo, hi)) = c.range {
            s.push_str(&format!("range = {lo:?}..{hi:?}\n"));
        }
        if let Some(v) = &c.port {
            s.push_str(&format!("port = \"{v}\"\n"));
        }
        if let Some(v) = c.rate_hz {
            s.push_str(&format!("rate_hz = {v}\n"));
        }
        for (key, value) in [("jitter_ns", c.jitter_ns), ("latency_ns", c.latency_ns)] {
            if let Some(v) = value {
                s.push_str(&format!("{key} = {v}\n"));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEISPIEL: &str = "\
# takt-hw 1
[target.thumbv7em]
core_hz = 84000000
i32 = 11900
i64 = 23800
f32 = 11900
f64 = 1190000
mem = 23800
call = 47600
native = 0
t_io = 120000
";

    #[test]
    fn a_calibration_reads_back() {
        let hw = parse(BEISPIEL).expect("lesbar");
        let t = hw.target("thumbv7em").expect("Ziel");
        assert_eq!(t.core_hz, Some(84_000_000));
        assert_eq!(t.c_target.of(CostClass::I32), 11_900);
        assert_eq!(t.c_target.of(CostClass::F64), 1_190_000);
        assert_eq!(t.t_io_ps, 120_000);
    }

    /// Was geschrieben wurde, liest sich wieder ein.
    #[test]
    fn rendering_round_trips() {
        let hw = parse(BEISPIEL).expect("lesbar");
        let wieder = parse(&render(&hw)).expect("wieder lesbar");
        assert_eq!(hw.targets, wieder.targets);
    }

    /// **Das Skalarprodukt aus 9.4.3.**
    #[test]
    fn the_duration_is_the_dot_product() {
        let hw = parse(BEISPIEL).expect("lesbar");
        let c = hw.target("thumbv7em").expect("Ziel").c_target;
        let n = CostVec { i32: 10, f64: 2, ..CostVec::default() };
        // 10 · 11900 + 2 · 1190000 = 119000 + 2380000
        assert_eq!(c.duration_ps(n), 2_499_000);
        assert_eq!(c.duration_ns(n), 2_499);
    }

    /// Eine fehlende Klasse macht die Tabelle unvollstaendig.
    ///
    /// Sonst rechnete die Schedulability mit einer zu kleinen Zahl und
    /// meldete „passt", wo sie nichts weiss.
    #[test]
    fn a_missing_class_makes_the_table_incomplete() {
        let ohne_mem = BEISPIEL.replace("mem = 23800\n", "");
        let hw = parse(&ohne_mem).expect("lesbar");
        let c = hw.target("thumbv7em").expect("Ziel").c_target;
        assert!(!c.is_complete());
        assert_eq!(c.missing(), vec![CostClass::Mem]);
    }

    /// `native` darf fehlen: Ein Programm ohne native Funktionen braucht
    /// das Gewicht nicht.
    #[test]
    fn the_native_class_may_stay_unmeasured() {
        let hw = parse(BEISPIEL).expect("lesbar");
        assert!(hw.target("thumbv7em").expect("Ziel").c_target.is_complete());
    }

    /// Ein Tippfehler im Schluessel ist ein Fehler, kein ueberlesenes Feld.
    #[test]
    fn an_unknown_key_is_an_error() {
        let text = BEISPIEL.replace("i32 =", "i32x =");
        let e = parse(&text).expect_err("abgelehnt");
        assert!(e.message.contains("i32x"), "{e}");
        assert!(e.message.contains("i64"), "die Meldung nennt die bekannten: {e}");
    }

    /// Eine neuere Formatversion wird abgelehnt, nicht geraten (11.3).
    #[test]
    fn a_newer_format_version_is_refused() {
        let text = BEISPIEL.replace("takt-hw 1", "takt-hw 99");
        let e = parse(&text).expect_err("abgelehnt");
        assert!(e.message.contains("99"), "{e}");
    }

    /// Ohne Kennung keine Datei.
    #[test]
    fn a_file_without_the_magic_line_is_refused() {
        let e = parse("[target.x]\ni32 = 1\n").expect_err("abgelehnt");
        assert_eq!(e.line, 1);
    }

    /// Ein Wert vor jedem Abschnitt hat kein Ziel.
    #[test]
    fn a_value_before_any_section_is_an_error() {
        let e = parse("# takt-hw 1\ni32 = 5\n").expect_err("abgelehnt");
        assert!(e.message.contains("vor jedem Abschnitt"), "{e}");
    }

    /// Kommentare und Leerzeilen stoeren nicht.
    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# takt-hw 1\n\n# gemessen am 15.09.\n[target.x]\n\ni32 = 7  # je Operation\n";
        let hw = parse(text).expect("lesbar");
        assert_eq!(hw.target("x").expect("Ziel").c_target.of(CostClass::I32), 7);
    }

    /// Die Dauer saettigt, statt zu ueberlaufen.
    #[test]
    fn an_absurd_program_saturates_instead_of_wrapping() {
        let mut c = CTarget::default();
        c.set(CostClass::I32, u64::MAX / 2);
        let n = CostVec { i32: 4, ..CostVec::default() };
        assert_eq!(c.duration_ps(n), u64::MAX);
    }
}
