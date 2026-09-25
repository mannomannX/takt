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
//! # takt-hw 7
//! [target.thumbv7em]
//! cost_model = 1     # die Version des Kostenmodells der Gewichte
//! core_hz = 84000000
//! i32 = 11900        # Pikosekunden je Operation
//! f64 = 1190000
//! i32_div = 142800   # je Division (7.2)
//! f32_fma = 35700    # je `fma`, ebenso `f32_sqrt` je Wurzel
//! t_io = 120000
//! tick_jitter_ns = 1500
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
//! guard_ns = 4000            # gemessen (13.8)
//! jitter_ns = 250000
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

use crate::fns::{CostClass, CostVec, Heavy};

/// Formatversion dieses Schreibers (11.3).
///
/// Leser akzeptieren jede Version bis zu ihrer eigenen; Schreiber
/// schreiben die neueste. Dieselbe Regel wie beim MIR-Format.
///
/// 2: NVM-Geometrie fuer das `persist`-Journal (5.9). 3: Geraete, Kanaele,
/// Speicher und Stack-Reserven (8.10). 4: NVM-Zeiten und `nvm_blocking`
/// (12.3, Pruefung 32). 5: eigene Gewichte der Division (7.2), der
/// Tick-Jitter je Ziel und `guard` je Output (7.5, 8.10) — die Messwerte
/// von `takt bench` und `takt driver-test` (13.8). 6: eigene Gewichte von
/// `fma` und `sqrt` (7.2). 7: `cost_model`, die Version des Kostenmodells,
/// zu der die Gewichte gemessen wurden.
pub const FORMAT_VERSION: u32 = 7;

/// Die Kennung in der ersten Zeile.
const MAGIC: &str = "takt-hw";

/// Pikosekunden je Operation, je Klasse (9.4.3, 13.8).
///
/// **Die Tabelle, die Operationen zu Zeit macht.** `takt cost` zaehlt,
/// was eine Aktivierung an Operationen braucht; erst das Skalarprodukt
/// mit dieser Tabelle ergibt eine Dauer.
///
/// **Division, `fma` und `sqrt` haben eigene Gewichte** (7.2), je
/// Zahlklasse eines. Fehlt eines — die Division in Tabellen bis Version 4,
/// `fma` und `sqrt` bis Version 5 —, wiegt die Operation wie ihre Klasse,
/// das Modell jener Tabellen. Und keine wiegt weniger als ihre Klasse: Nur
/// dann bleibt das komponentenweise Maximum ueber Zweige (9.4.3) eine obere
/// Schranke der Zeit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CTarget {
    /// Pikosekunden je Operation, in der Reihenfolge von [`CostClass::ALL`].
    ps: [u64; 7],
    /// Pikosekunden je Operation eigenen Gewichts, nach [`Heavy::ALL`] und
    /// [`CostClass::ALL`]; null heisst nicht gemessen.
    heavy_ps: [[u64; 7]; 3],
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

    /// Das gemessene Gewicht einer Operation der Art `h` in der Klasse `c`,
    /// falls es eines gibt.
    pub fn measured(&self, h: Heavy, c: CostClass) -> Option<u64> {
        Some(self.heavy_ps[h as usize][c as usize]).filter(|ps| *ps > 0)
    }

    /// Das Gewicht einer Operation der Art `h` in Pikosekunden: das
    /// gemessene, aber nie weniger als das der Klasse.
    pub fn heavy(&self, h: Heavy, c: CostClass) -> u64 {
        self.measured(h, c).unwrap_or(0).max(self.of(c))
    }

    /// Setzt das Gewicht einer Operation der Art `h`; wo es die Art in der
    /// Klasse nicht gibt, wirkungslos.
    pub fn set_heavy(&mut self, h: Heavy, c: CostClass, ps: u64) {
        if h.exists_in(c) {
            self.heavy_ps[h as usize][c as usize] = ps;
        }
    }

    /// Ist die Tabelle vollstaendig?
    ///
    /// **Eine Null ist kein Messwert.** Fehlt auch nur eine Klasse, ist
    /// das Skalarprodukt zu klein, und eine Schedulability, die auf einer
    /// zu kleinen Zahl beruht, ist schlimmer als keine: Sie sagt „passt",
    /// wo sie nichts weiss. Einzige Ausnahme ist `native` — ein Programm
    /// ohne native Funktionen braucht das Gewicht nicht, und es zu
    /// verlangen hiesse, eine Messung fuer etwas zu fordern, das nicht
    /// vorkommt. Ob ein Programm es braucht, sagt [`CTarget::missing_for`].
    pub fn is_complete(&self) -> bool {
        CostClass::ALL.iter().all(|c| *c == CostClass::Native || self.of(*c) > 0)
    }

    /// Die Klassen ohne Messwert.
    pub fn missing(&self) -> Vec<CostClass> {
        CostClass::ALL.iter().copied().filter(|c| *c != CostClass::Native && self.of(*c) == 0).collect()
    }

    /// Die Klassen ohne Messwert, die ein Operationsvektor braucht: jede
    /// ausser `native` immer, `native`, sobald `n` Operationen darin hat —
    /// eine Deklaration `cost = {native: …}` (4.5) waere sonst zeitlos.
    pub fn missing_for(&self, n: CostVec) -> Vec<CostClass> {
        CostClass::ALL
            .iter()
            .copied()
            .filter(|c| self.of(*c) == 0 && (*c != CostClass::Native || n.of(*c) > 0))
            .collect()
    }

    /// Die Dauer eines Operationsvektors in Pikosekunden (9.4.3, 7.2).
    ///
    /// `Σ_c N_c · c_target[c] + Σ_h H_h,c · (c_h[c] − c_target[c])` mit
    /// `H_h,c` den Operationen der Art `h` in der Klasse: Jede zaehlt in
    /// ihrer Klasse und traegt dazu bei, was sie mehr kostet. Das bleibt
    /// eine Schranke, auch wenn das Maximum ueber Zweige mehr Operationen
    /// eigenen Gewichts traegt als Operationen der Klasse. Saettigt statt zu
    /// ueberlaufen: Ein Programm mit absurd vielen Operationen soll eine
    /// absurd grosse Dauer melden und daran scheitern, nicht eine kleine und
    /// durchgehen.
    pub fn duration_ps(&self, n: CostVec) -> u64 {
        CostClass::ALL.iter().fold(0u64, |acc, c| {
            let extra = Heavy::ALL.iter().fold(0u64, |sum, h| {
                sum.saturating_add(n.heavy(*h, *c).saturating_mul(self.heavy(*h, *c) - self.of(*c)))
            });
            acc.saturating_add(n.of(*c).saturating_mul(self.of(*c))).saturating_add(extra)
        })
    }

    /// Dieselbe Dauer in Nanosekunden, kaufmaennisch gerundet.
    ///
    /// Fuer Meldungen: Nanosekunden sind die Einheit, in der die Sprache
    /// sonst ueber Zeit spricht (3.3). Gerechnet wird in Pikosekunden.
    pub fn duration_ns(&self, n: CostVec) -> u64 {
        self.duration_ps(n).saturating_add(500) / 1000
    }
}

/// Der Schluessel des eigenen Gewichts einer Art in einer Klasse
/// (`i32_div`, `f32_fma`, `f64_sqrt`), wo es die Art dort gibt.
pub fn heavy_key(h: Heavy, c: CostClass) -> Option<String> {
    h.exists_in(c).then(|| format!("{}_{}", c.name(), h.suffix()))
}

/// Alle Arten mit ihren Klassen, in der Reihenfolge der Konfiguration.
fn heavy_pairs() -> impl Iterator<Item = (Heavy, CostClass)> {
    Heavy::ALL
        .into_iter()
        .flat_map(|h| CostClass::ALL.into_iter().filter(move |c| h.exists_in(*c)).map(move |c| (h, c)))
}

/// Ein Ziel mit seiner Kalibrierung (8.10, „Kalibrierung je Ziel").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    /// Der Name, wie ihn `takt build --target` kennt.
    pub name: String,
    /// Kerntakt in Hertz; `None`, wenn die Messung ihn nicht festhielt.
    pub core_hz: Option<u32>,
    /// Die Version des Kostenmodells, zu der die Tabelle gemessen wurde;
    /// `None` bei Tabellen von Hand oder von vor Version 7.
    pub cost_model: Option<u32>,
    /// Die Kostentabelle.
    pub c_target: CTarget,
    /// Was der Tick ausserhalb des Programms kostet, in Pikosekunden.
    ///
    /// 7.2 zieht sie von `T₀` ab, bevor das Budget verglichen wird. Was
    /// genau hineinfaellt — Abtastung, Commit, Recorder —, sagt die
    /// Referenz nicht; gemessen wird die Differenz zwischen Tickperiode
    /// und dem, was das Programm davon nutzt (13.8).
    pub t_io_ps: u64,
    /// Der gemessene Tick-Jitter in Nanosekunden: um so viel kann der
    /// Abstand zweier Aktivierungen die Periode ueberschreiten (7.3, 13.8,
    /// Pruefung 59).
    pub tick_jitter_ns: Option<i64>,
    /// Die NVM-Geometrie hinter `persist var`, falls das Ziel eine hat.
    pub nvm: Option<NvmGeometry>,
    /// Speicher und Stack-Reserven (11.5, 12.3).
    pub memory: Memory,
}

impl Target {
    /// Gehoert die Kostentabelle zum Kostenmodell dieses Compilers?
    ///
    /// **Sonst ist sie keine Kalibrierung.** Aendert sich, wofuer eine
    /// gezaehlte Operation steht, passen die Gewichte nicht mehr zu den
    /// Zaehlungen: Seit FB-299 zaehlt eine Endlichkeitspruefung in `i32`, und
    /// eine Tabelle, deren Gleitkommagewicht sie mitgemessen hatte, ergaebe
    /// fuer Gleitkommacode eine Schranke von kaum mehr als der Haelfte. Eine
    /// Tabelle ohne Version oder zu einer anderen gilt darum als nicht
    /// kalibriert — `takt bench` misst sie neu.
    pub fn fits_cost_model(&self) -> bool {
        self.cost_model == Some(crate::analysis::cost::MODEL_VERSION)
    }
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
    /// Gemessene Treiberlatenz eines Outputs in Nanosekunden: So frueh muss
    /// eine geplante Ausgabe feststehen (`guard`, 7.5, 13.8).
    pub guard_ns: Option<i64>,
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
    /// Dauer einer Sektorloeschung in Nanosekunden (12.3).
    pub erase_ns: Option<i64>,
    /// Dauer eines Programmiervorgangs ueber einen Slot in Nanosekunden.
    pub program_ns: Option<i64>,
    /// Haelt ein Vorgang den Kern an (`true`), oder laeuft er in der
    /// Hardware weiter (`false`)? `None`: nicht angegeben.
    pub blocking: Option<bool>,
}

impl NvmGeometry {
    /// Groesse eines Slots in Byte: die Haelfte der Sektoren, abgerundet.
    pub fn slot_bytes(&self) -> u32 {
        self.sector_bytes.saturating_mul(self.sectors / 2)
    }

    /// Wie lange ein blockierender Vorgang den Kern hoechstens haelt:
    /// die laengere der beiden Phasen. `None`, wenn das Ziel nicht
    /// blockiert oder die Zeiten fehlen.
    pub fn blocking_phase_ns(&self) -> Option<i64> {
        (self.blocking == Some(true)).then_some(self.erase_ns?.max(self.program_ns?))
    }

    /// Ein ganzer Schreibvorgang: eine Loeschung und zwei
    /// Programmiervorgaenge (Nutzlast, Kopf).
    pub fn blocking_write_ns(&self) -> Option<i64> {
        (self.blocking == Some(true)).then_some(self.erase_ns?.saturating_add(self.program_ns?.saturating_mul(2)))
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

fn boolean(value: &str, line: u32) -> Result<bool, ParseError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(ParseError { line, message: format!("`{value}` ist weder `true` noch `false`") }),
    }
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
        "cost_model" => {
            let n = number(value, line)?;
            target.cost_model = Some(
                u32::try_from(n)
                    .map_err(|_| ParseError { line, message: format!("Kostenmodell {n} passt nicht in 32 Bit") })?,
            );
        }
        "t_io" => target.t_io_ps = number(value, line)?,
        "tick_jitter_ns" => target.tick_jitter_ns = Some(number(value, line)? as i64),
        "nvm_sector_bytes" => nvm_of(target).sector_bytes = number(value, line)? as u32,
        "nvm_sectors" => nvm_of(target).sectors = number(value, line)? as u32,
        "nvm_min_interval" => nvm_of(target).default_min_interval_ns = number(value, line)? as i64,
        "nvm_erase_ns" => nvm_of(target).erase_ns = Some(number(value, line)? as i64),
        "nvm_program_ns" => nvm_of(target).program_ns = Some(number(value, line)? as i64),
        "nvm_blocking" => nvm_of(target).blocking = Some(boolean(value, line)?),
        "ram" => target.memory.ram = Some(number(value, line)?),
        "flash" => target.memory.flash = Some(number(value, line)?),
        "iram" => target.memory.iram = Some(number(value, line)?),
        "stack_reserve" => target.memory.stack_reserve = Some(number(value, line)?),
        "stack_margin" => target.memory.stack_margin = Some(number(value, line)?),
        _ => {
            if let Some((h, c)) = heavy_pairs().find(|(h, c)| heavy_key(*h, *c).as_deref() == Some(key)) {
                target.c_target.set_heavy(h, c, number(value, line)?);
                return Ok(());
            }
            let class = CostClass::ALL.iter().find(|c| c.name() == key).ok_or_else(|| ParseError {
                line,
                message: format!(
                    "unbekannter Schluessel `{key}`; bekannt: cost_model, core_hz, t_io, tick_jitter_ns, ram, flash, iram, \
                     stack_reserve, stack_margin, nvm_sector_bytes, nvm_sectors, nvm_min_interval, nvm_erase_ns, \
                     nvm_program_ns, nvm_blocking, die Klassen {} und die eigenen Gewichte {}",
                    CostClass::ALL.iter().map(|c| c.name()).collect::<Vec<_>>().join(", "),
                    heavy_pairs().filter_map(|(h, c)| heavy_key(h, c)).collect::<Vec<_>>().join(", ")
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
        "guard_ns" => channel.guard_ns = Some(number(value, line)? as i64),
        "jitter_ns" => channel.jitter_ns = Some(number(value, line)? as i64),
        "latency_ns" => channel.latency_ns = Some(number(value, line)? as i64),
        _ => {
            return Err(ParseError {
                line,
                message: format!(
                    "unbekannter Schluessel `{key}`; bekannt: direction, raw, unit, range, safe, device, port, \
                     rate_hz, guard_ns, jitter_ns, latency_ns"
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

/// Traegt Messwerte in eine bestehende Konfiguration ein, ohne sie neu zu
/// schreiben.
///
/// **Die Datei gehoert einem Menschen.** Sie traegt Kommentare, die sagen,
/// woher eine Zahl kommt und was an ihr unsicher ist; [`render`] wuerde sie
/// verwerfen. Darum ersetzt diese Funktion nur die Werte der genannten
/// Schluessel im Abschnitt `[target.<ziel>]` — ein Kommentar hinter einem
/// Wert bleibt stehen —, haengt fehlende Schluessel an das Ende des
/// Abschnitts an, legt einen fehlenden Abschnitt an und hebt die
/// Formatversion im Kopf auf die dieses Schreibers. Das Ergebnis wird
/// gelesen, bevor es zurueckkommt: Was diese Funktion schreibt, ist lesbar.
pub fn with_values<K: AsRef<str>>(text: &str, target: &str, values: &[(K, String)]) -> Result<String, ParseError> {
    let header = format!("[target.{target}]");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match lines.iter().position(|l| magic_version(l).is_some()) {
        Some(i) => lines[i] = format!("# {MAGIC} {FORMAT_VERSION}"),
        None => lines.insert(0, format!("# {MAGIC} {FORMAT_VERSION}")),
    }
    let start = match lines.iter().position(|l| l.split('#').next().unwrap_or("").trim() == header) {
        Some(i) => i,
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header.clone());
            lines.len() - 1
        }
    };
    let end =
        lines[start + 1..].iter().position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| start + 1 + i);
    let mut last_value = start;
    for (i, line) in lines.iter().enumerate().take(end).skip(start + 1) {
        if line.split('#').next().unwrap_or("").contains('=') {
            last_value = i;
        }
    }
    let mut appended = Vec::new();
    for (key, value) in values {
        let key = key.as_ref();
        let found = (start + 1..end)
            .find(|i| lines[*i].split('#').next().unwrap_or("").split_once('=').is_some_and(|(k, _)| k.trim() == key));
        match found {
            Some(i) => {
                let comment = lines[i].find('#').map(|at| lines[i][at..].to_string());
                lines[i] = match comment {
                    Some(c) => format!("{key} = {value}   {c}"),
                    None => format!("{key} = {value}"),
                };
            }
            None => appended.push(format!("{key} = {value}")),
        }
    }
    let at = last_value + 1;
    for (offset, line) in appended.into_iter().enumerate() {
        lines.insert(at + offset, line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    parse(&out)?;
    Ok(out)
}

/// Die Werte eines Ziels, wie `takt bench` sie misst, fuer [`with_values`]:
/// Kostenmodell, Kerntakt, alle Klassen, die gemessenen eigenen Gewichte,
/// `T_IO`, Tick-Jitter und Stack-Reserve, soweit vorhanden.
pub fn measured_values(t: &Target) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Some(model) = t.cost_model {
        v.push(("cost_model".to_string(), model.to_string()));
    }
    if let Some(hz) = t.core_hz {
        v.push(("core_hz".to_string(), hz.to_string()));
    }
    for c in CostClass::ALL {
        v.push((c.name().to_string(), t.c_target.of(c).to_string()));
    }
    for (h, c) in heavy_pairs() {
        if let (Some(key), Some(ps)) = (heavy_key(h, c), t.c_target.measured(h, c)) {
            v.push((key, ps.to_string()));
        }
    }
    v.push(("t_io".to_string(), t.t_io_ps.to_string()));
    if let Some(j) = t.tick_jitter_ns {
        v.push(("tick_jitter_ns".to_string(), j.to_string()));
    }
    if let Some(r) = t.memory.stack_reserve {
        v.push(("stack_reserve".to_string(), r.to_string()));
    }
    v
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
        if let Some(model) = target.cost_model {
            s.push_str(&format!("cost_model = {model}\n"));
        }
        if let Some(hz) = target.core_hz {
            s.push_str(&format!("core_hz = {hz}\n"));
        }
        for c in CostClass::ALL {
            s.push_str(&format!("{} = {}\n", c.name(), target.c_target.of(c)));
        }
        for (h, c) in heavy_pairs() {
            if let (Some(key), Some(ps)) = (heavy_key(h, c), target.c_target.measured(h, c)) {
                s.push_str(&format!("{key} = {ps}\n"));
            }
        }
        s.push_str(&format!("t_io = {}\n", target.t_io_ps));
        if let Some(j) = target.tick_jitter_ns {
            s.push_str(&format!("tick_jitter_ns = {j}\n"));
        }
        if let Some(nvm) = target.nvm {
            s.push_str(&format!("nvm_sector_bytes = {}\n", nvm.sector_bytes));
            s.push_str(&format!("nvm_sectors = {}\n", nvm.sectors));
            s.push_str(&format!("nvm_min_interval = {}\n", nvm.default_min_interval_ns));
            for (key, value) in [("nvm_erase_ns", nvm.erase_ns), ("nvm_program_ns", nvm.program_ns)] {
                if let Some(v) = value {
                    s.push_str(&format!("{key} = {v}\n"));
                }
            }
            if let Some(b) = nvm.blocking {
                s.push_str(&format!("nvm_blocking = {b}\n"));
            }
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
        for (key, value) in [("guard_ns", c.guard_ns), ("jitter_ns", c.jitter_ns), ("latency_ns", c.latency_ns)] {
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
        let mut c = hw.target("thumbv7em").expect("Ziel").c_target;
        assert!(!c.is_complete());
        assert_eq!(c.missing(), vec![CostClass::Mem]);
        // `native` fehlt erst, wenn die Last sie braucht.
        c.set(CostClass::Mem, 1);
        assert!(c.missing_for(CostVec { i32: 5, ..CostVec::default() }).is_empty());
        assert_eq!(c.missing_for(CostVec { native: 1, ..CostVec::default() }), vec![CostClass::Native]);
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

    /// 12.3: Die NVM-Zeiten lesen sich zurueck, und die Rechnung folgt
    /// dem Modell „eine Loeschung, zwei Programmiervorgaenge".
    #[test]
    fn nvm_times_read_back_and_add_up() {
        let text = format!(
            "{BEISPIEL}nvm_sector_bytes = 4096\nnvm_erase_ns = 200000000\nnvm_program_ns = 5000000\nnvm_blocking = true\n"
        );
        let hw = parse(&text.replace("takt-hw 1", "takt-hw 4")).expect("lesbar");
        let nvm = hw.target("thumbv7em").expect("Ziel").nvm.expect("NVM");
        assert_eq!(nvm.blocking_phase_ns(), Some(200_000_000));
        assert_eq!(nvm.blocking_write_ns(), Some(210_000_000));
        let again = parse(&render(&hw)).expect("Rundreise");
        assert_eq!(again.target("thumbv7em").expect("Ziel").nvm, Some(nvm));
    }

    /// Ohne `nvm_blocking = true` gibt es keine Kosten, auch mit Zeiten.
    #[test]
    fn a_non_blocking_device_costs_nothing() {
        let text = format!("{BEISPIEL}nvm_erase_ns = 200000000\nnvm_program_ns = 5000000\nnvm_blocking = false\n");
        let hw = parse(&text).expect("lesbar");
        assert_eq!(hw.target("thumbv7em").expect("Ziel").nvm.expect("NVM").blocking_write_ns(), None);
        let e = parse(&format!("{BEISPIEL}nvm_blocking = maybe\n")).expect_err("abgelehnt");
        assert!(e.message.contains("weder `true` noch `false`"), "{e}");
    }

    /// **Division, `fma` und `sqrt` haben eigene Gewichte** (7.2): Jede
    /// zaehlt in ihrer Klasse und wiegt mit ihrem eigenen Gewicht.
    #[test]
    fn a_heavy_operation_weighs_with_its_own_weight() {
        let hw = parse(&format!("{BEISPIEL}i32_div = 142800\nf32_fma = 35700\nf32_sqrt = 166600\n")).expect("lesbar");
        let c = hw.target("thumbv7em").expect("Ziel").c_target;
        let n = CostVec { i32: 10, i32_div: 2, ..CostVec::default() };
        // 8 · 11900 + 2 · 142800
        assert_eq!(c.duration_ps(n), 8 * 11_900 + 2 * 142_800);
        let n = CostVec { f32: 5, f32_fma: 3, f32_sqrt: 1, ..CostVec::default() };
        assert_eq!(c.duration_ps(n), 11_900 + 3 * 35_700 + 166_600);
        assert_eq!(c.measured(Heavy::Div, CostClass::I32), Some(142_800));
        assert_eq!(c.measured(Heavy::Fma, CostClass::F32), Some(35_700));
    }

    /// Ohne Messung wiegt eine solche Operation wie ihre Klasse: das Modell
    /// der Tabellen bis Version 4 fuer die Division, bis 5 fuer `fma`.
    #[test]
    fn an_unmeasured_heavy_operation_weighs_like_its_class() {
        let c = parse(BEISPIEL).expect("lesbar").target("thumbv7em").expect("Ziel").c_target;
        let with = CostVec { f64: 3, f64_div: 2, f64_fma: 1, ..CostVec::default() };
        let without = CostVec { f64: 3, ..CostVec::default() };
        assert_eq!(c.duration_ps(with), c.duration_ps(without));
        assert_eq!(c.measured(Heavy::Div, CostClass::F64), None);
        assert_eq!(c.measured(Heavy::Fma, CostClass::F64), None);
    }

    /// Ganzzahlen kennen kein `fma`: Der Schluessel fehlt, und ein Gewicht
    /// dafuer bleibt wirkungslos.
    #[test]
    fn integers_have_no_fma() {
        assert_eq!(heavy_key(Heavy::Fma, CostClass::I32), None);
        assert_eq!(heavy_key(Heavy::Sqrt, CostClass::F64).as_deref(), Some("f64_sqrt"));
        let e = parse(&format!("{BEISPIEL}i32_fma = 1000\n")).expect_err("unbekannt");
        assert!(e.message.contains("f32_fma"), "{e}");
        let mut c = CTarget::default();
        c.set_heavy(Heavy::Fma, CostClass::I64, 1_000);
        assert_eq!(c.measured(Heavy::Fma, CostClass::I64), None);
    }

    /// **Keine Operation eigenen Gewichts wiegt weniger als ihre Klasse.**
    /// Sonst waere das komponentenweise Maximum ueber Zweige (9.4.3) keine
    /// Schranke mehr: Ein Zweig mit Divisionen koennte teurer sein als die
    /// Summe, die der Vektor des Maximums ergibt. Das gilt auch, wenn das
    /// Maximum mehr solche Operationen traegt als Operationen der Klasse —
    /// hier eine Division aus dem einen Zweig und ein `fma` aus dem anderen.
    #[test]
    fn the_maximum_over_branches_stays_a_bound() {
        let hw = parse(&format!("{BEISPIEL}i32_div = 1000\nf32_div = 50000\nf32_fma = 30000\n")).expect("lesbar");
        let c = hw.target("thumbv7em").expect("Ziel").c_target;
        assert_eq!(c.heavy(Heavy::Div, CostClass::I32), 11_900, "geklemmt auf das Klassengewicht");
        let divides = CostVec { i32: 4, i32_div: 4, ..CostVec::default() };
        let adds = CostVec { i32: 6, ..CostVec::default() };
        let bound = c.duration_ps(divides.max(adds));
        assert!(bound >= c.duration_ps(divides) && bound >= c.duration_ps(adds));
        let divide = CostVec { f32: 1, f32_div: 1, ..CostVec::default() };
        let fuse = CostVec { f32: 1, f32_fma: 1, ..CostVec::default() };
        let bound = c.duration_ps(divide.max(fuse));
        assert!(bound >= c.duration_ps(divide) && bound >= c.duration_ps(fuse), "{bound}");
    }

    /// **Messwerte ersetzen Werte, nicht Kommentare.** Die Datei gehoert
    /// einem Menschen: Was er ueber eine Zahl geschrieben hat, bleibt; die
    /// Zahl selbst wird ersetzt, fehlende Schluessel kommen dazu, und der
    /// Kopf nennt die neue Version.
    #[test]
    fn measured_values_keep_the_comments() {
        let text = "# takt-hw 3\n# Von Hand.\n[target.thumbv7em]\ni32 = 11905   # geschaetzt\nram = 65536\n\n[device.gpio]\ndriver = \"x\"\n";
        let out =
            with_values(text, "thumbv7em", &[("i32", "12000".into()), ("i32_div", "140000".into())]).expect("lesbar");
        assert!(out.starts_with(&format!("# takt-hw {FORMAT_VERSION}\n# Von Hand.\n")), "{out}");
        assert!(out.contains("i32 = 12000   # geschaetzt"), "{out}");
        assert!(out.contains("ram = 65536\ni32_div = 140000\n"), "angehaengt am Ende des Abschnitts: {out}");
        let hw = parse(&out).expect("lesbar");
        assert_eq!(hw.target("thumbv7em").expect("Ziel").c_target.measured(Heavy::Div, CostClass::I32), Some(140_000));
        assert!(hw.devices.contains_key("gpio"));
    }

    /// Ein fehlendes Ziel bekommt seinen Abschnitt.
    #[test]
    fn a_missing_target_gets_its_section() {
        let out = with_values("# takt-hw 4\n", "riscv32imac", &[("core_hz", "160000000".into())]).expect("lesbar");
        assert_eq!(parse(&out).expect("lesbar").target("riscv32imac").and_then(|t| t.core_hz), Some(160_000_000));
    }

    /// Was `takt bench` und `driver-test` messen, wird geschrieben und
    /// wieder gelesen.
    #[test]
    fn measured_values_round_trip() {
        let text = format!(
            "{BEISPIEL}i32_div = 142800\nf64_div = 9000000\nf32_fma = 35700\nf64_sqrt = 2000000\n\
             tick_jitter_ns = 1500\ncost_model = 1\n\n[channel ui/led]\ndirection = output\nguard_ns = 4000\n\
             jitter_ns = 250\n"
        );
        let hw = parse(&text.replace("takt-hw 1", &format!("takt-hw {FORMAT_VERSION}"))).expect("lesbar");
        assert_eq!(hw.target("thumbv7em").expect("Ziel").tick_jitter_ns, Some(1_500));
        assert_eq!(hw.channel("ui/led").expect("Kanal").guard_ns, Some(4_000));
        let rendered = render(&hw);
        assert!(rendered.starts_with(&format!("# takt-hw {FORMAT_VERSION}\n")), "{rendered}");
        assert!(rendered.contains("f32_fma = 35700\nf64_sqrt = 2000000\n"), "{rendered}");
        assert!(rendered.contains("[target.thumbv7em]\ncost_model = 1\n"), "{rendered}");
        assert_eq!(parse(&rendered).expect("Rundreise"), hw);
    }

    /// Nur eine Tabelle zum Kostenmodell dieses Compilers ist eine
    /// Kalibrierung; ohne Version oder zu einer anderen gilt sie als keine.
    #[test]
    fn a_table_counts_only_for_its_cost_model() {
        let current = crate::analysis::cost::MODEL_VERSION;
        let fits = |text: &str| parse(text).expect("lesbar").target("thumbv7em").expect("Ziel").fits_cost_model();
        assert!(!fits(BEISPIEL), "ohne Version");
        assert!(fits(&format!("{BEISPIEL}cost_model = {current}\n")));
        assert!(!fits(&format!("{BEISPIEL}cost_model = {}\n", current + 1)));
    }
}
