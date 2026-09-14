//! Was wirklich im Objekt steht (11.5, 12.3, 13.4).
//!
//! **Wofuer.** `takt size` rechnet den Speicherbedarf aus der MIR (11.5)
//! und nennt jeden Posten `exakt`. Das ist eine Behauptung ueber ein
//! Binary, das der Compiler noch nicht gesehen hat — und sie stimmt nur,
//! solange Rechnung und Codegen dasselbe meinen. Dieses Modul misst am
//! erzeugten Objekt nach.
//!
//! Drei Anwendungen, drei Meilensteine:
//!
//! - **`takt size` gegen die Wirklichkeit** (11.5): Der Posten „Flash
//!   (Code, Konstanten)" steht heute auf `offen`, obwohl `.text` und
//!   `.rodata` ihn liefern. Und die gerechneten Posten lassen sich gegen
//!   das Gemessene halten — so fiel auf, dass die DFA-Rechnung je Muster
//!   zaehlte statt je Automat (FB-121).
//! - **Der Stack-Vertrag** (12.3): „Stack-Tiefe des Programms = laengster
//!   Pfad im azyklischen Aufrufgraphen." Der Prolog jeder Funktion sagt,
//!   wie viel sie wirklich nimmt. Das wird mit M5 scharf, wo ein
//!   Ueberlauf kein Absturz, sondern ein Sicherheitsfall ist.
//! - **Zertifizierung** (13.4): Objektcode gegen Quelle nachvollziehen.
//!   Das ist v2-Gebiet; hier entsteht nur das Werkzeug dafuer.
//!
//! **Warum ueber die Binutils und nicht mit einem eigenen ELF-Leser.**
//! Ein Leser fuer ELF *und* PE *und* Mach-O waere ein Crate fuer sich,
//! und er muesste jede Eigenheit jedes Ziels kennen. `size`, `nm` und
//! `objdump` stehen in jeder Werkzeugkette, die ohnehin gebraucht wird —
//! fuer aarch64 als `aarch64-linux-gnu-*`, fuer die MCU als
//! `arm-none-eabi-*`. Das Modul kennt darum nur ihre Ausgabe, nicht das
//! Format.
//!
//! **Was fehlt, ist kein Fehler.** Wo kein Werkzeug steht, liefert jede
//! Funktion `None`. Ein Test, der misst, ueberspringt sich dann — wie
//! jeder andere Werkzeugkettentest auch.

use std::path::Path;
use std::process::Command;

/// Die Abschnitte eines Objekts, in Byte (11.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sections {
    /// `.text`: der Code.
    pub text: u64,
    /// `.rodata`: Konstanten, die der Tick liest (DFA-Tabellen, `safe`).
    pub rodata: u64,
    /// `.data`: initialisierte Daten.
    pub data: u64,
    /// `.bss`: genullte Daten (Zustaende, Ringe, 11.2).
    pub bss: u64,
}

impl Sections {
    /// Was im Flash liegt: Code und Konstanten (11.5, 12.3).
    ///
    /// `.data` zaehlt mit, weil seine Anfangswerte ebenfalls im Flash
    /// stehen muessen — die Runtime kopiert sie beim Start ins RAM.
    pub fn flash(&self) -> u64 {
        self.text + self.rodata + self.data
    }

    /// Was im RAM liegt (11.5).
    pub fn ram(&self) -> u64 {
        self.data + self.bss
    }
}

/// Ein Symbol mit seiner Groesse (11.5, 13.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Name, wie der Linker ihn fuehrt.
    pub name: String,
    /// Groesse in Byte; null, wenn das Format sie nicht nennt.
    pub size: u64,
    /// Die Klasse, wie `nm` sie schreibt: `T` fuer Code, `t` fuer
    /// lokalen Code, `R`/`r` fuer Konstanten, `B`/`b` fuer `.bss`.
    pub kind: char,
}

/// Die Werkzeuge einer Zielkette (`size`, `nm`, `objdump`).
///
/// Der Praefix ist leer fuer den Wirt und sonst der der Cross-Kette,
/// etwa `aarch64-linux-gnu-`. Er steht im Typ, weil ein Objekt fuer
/// aarch64 mit dem `nm` des Wirts nicht zuverlaessig zu lesen ist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binutils {
    prefix: String,
}

impl Binutils {
    /// Die Werkzeuge des Wirts.
    pub fn host() -> Binutils {
        Binutils { prefix: String::new() }
    }

    /// Die Werkzeuge einer Cross-Kette, etwa `aarch64-linux-gnu-`.
    pub fn with_prefix(prefix: &str) -> Binutils {
        Binutils { prefix: prefix.to_string() }
    }

    /// Die Werkzeuge, die zu einem Ziel gehoeren (12.8).
    ///
    /// Der Praefix steht damit an einer Stelle — am Ziel, wo auch Triple
    /// und Zielklasse stehen. Ein frei getippter Praefix an der
    /// Aufrufstelle waere eine zweite Quelle fuer dieselbe Angabe.
    pub fn for_target(target: crate::target::Target) -> Binutils {
        Binutils { prefix: target.prefix.to_string() }
    }

    /// Stehen sie zur Verfuegung?
    ///
    /// Geprueft wird `nm`, weil die drei Werkzeuge aus demselben Paket
    /// kommen: Wer eines hat, hat alle.
    pub fn available(&self) -> bool {
        Command::new(self.tool("nm")).arg("--version").output().is_ok_and(|o| o.status.success())
    }

    fn tool(&self, name: &str) -> String {
        format!("{}{name}", self.prefix)
    }

    /// Die Abschnittsgroessen eines Objekts oder Binaries (11.5).
    ///
    /// `None` heisst: Das Werkzeug fehlt oder die Datei ist keines, das
    /// es lesen kann.
    pub fn sections(&self, file: &Path) -> Option<Sections> {
        let out = Command::new(self.tool("size")).arg("-A").arg(file).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut s = Sections::default();
        for line in text.lines() {
            let mut w = line.split_whitespace();
            let (Some(name), Some(size)) = (w.next(), w.next()) else { continue };
            let Ok(bytes) = size.parse::<u64>() else { continue };
            // Ein Abschnitt kann Zusaetze tragen (`.text.startup`); die
            // Zuordnung geht darum ueber das Praefix.
            //
            // **Die Namen unterscheiden sich je Format.** ELF sagt
            // `.rodata`, PE/COFF sagt `.rdata` — und dort liegen auch
            // `.xdata` und `.pdata`, die Ausnahmebehandlung des
            // Aufrufers. Beide zaehlen als Konstanten, weil sie im Flash
            // stehen und der Tick sie nicht schreibt. Ohne `.rdata`
            // meldete die Messung auf Windows null, und der Vergleich
            // mit der Rechnung schlug fehl, obwohl beide stimmten.
            match name {
                n if n.starts_with(".text") => s.text += bytes,
                n if n.starts_with(".rodata") || n.starts_with(".rdata") => s.rodata += bytes,
                n if n.starts_with(".xdata") || n.starts_with(".pdata") => s.rodata += bytes,
                n if n.starts_with(".data") => s.data += bytes,
                n if n.starts_with(".bss") => s.bss += bytes,
                _ => {}
            }
        }
        Some(s)
    }

    /// Die definierten Symbole mit ihren Groessen (11.5, 13.4).
    ///
    /// Nur definierte: Ein undefiniertes Symbol ist ein Aufruf in die
    /// Runtime und traegt hier nichts bei.
    pub fn symbols(&self, file: &Path) -> Option<Vec<Symbol>> {
        let out = Command::new(self.tool("nm")).args(["-S", "--defined-only"]).arg(file).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut list = Vec::new();
        for line in text.lines() {
            let w: Vec<&str> = line.split_whitespace().collect();
            // Mit Groesse: `adresse groesse klasse name`. Ohne:
            // `adresse klasse name` — dann ist die Groesse unbekannt.
            let (size, kind, name) = match w.len() {
                4 => (u64::from_str_radix(w[1], 16).unwrap_or(0), w[2], w[3]),
                3 => (0, w[1], w[2]),
                _ => continue,
            };
            let Some(k) = kind.chars().next() else { continue };
            if kind.len() != 1 {
                continue;
            }
            list.push(Symbol { name: name.to_string(), size, kind: k });
        }
        // Nach Namen sortiert: Die Reihenfolge von `nm` haengt an der
        // Adresse, und ein Vergleich zweier Ziele soll sie nicht sehen.
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Some(list)
    }

    /// Der Stackbedarf einer Funktion aus ihrem Prolog, in Byte (12.3).
    ///
    /// **Was gemessen wird.** Die erste Anpassung des Stackzeigers im
    /// Prolog: `sub $N, %rsp` auf x86-64, `sub sp, sp, #N` auf aarch64.
    /// Das ist der Rahmen, den die Funktion *selbst* nimmt — nicht die
    /// Tiefe des Aufrufbaums. 12.3 rechnet daraus den laengsten Pfad;
    /// hier entsteht die Zahl je Knoten.
    ///
    /// `None` heisst: kein Werkzeug, oder die Funktion steht nicht in der
    /// Datei. `Some(0)` heisst: Sie hat keinen Rahmen — ein Blatt, das
    /// mit Registern auskommt.
    pub fn stack_frame(&self, file: &Path, symbol: &str) -> Option<u64> {
        let out = Command::new(self.tool("objdump")).args(["-d", "--no-show-raw-insn"]).arg(file).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut in_fn = false;
        for line in text.lines() {
            if line.contains(&format!("<{symbol}>:")) {
                in_fn = true;
                continue;
            }
            if !in_fn {
                continue;
            }
            // Der naechste Funktionskopf beendet die Suche: Ein Prolog
            // steht am Anfang, und was danach kommt, gehoert nicht mehr
            // dazu.
            if line.contains(">:") {
                return Some(0);
            }
            if let Some(n) = frame_adjust(line) {
                return Some(n);
            }
        }
        in_fn.then_some(0)
    }
}

/// Die Stackanpassung einer Zeile, falls sie eine ist.
///
/// Zwei Formen, weil zwei Architekturen: `sub $0x68,%rsp` (x86-64) und
/// `sub sp, sp, #0x68` (aarch64). Ein `stp x29, x30, [sp, #-N]!`
/// verschiebt den Zeiger ebenfalls und kommt auf aarch64 zuerst.
fn frame_adjust(line: &str) -> Option<u64> {
    // Jede Zeile beginnt mit Adresse und Tabulator (`  2d:\tsub …`); der
    // Befehl steht dahinter. Ohne das Abtrennen begaenne keine Zeile mit
    // dem Mnemonic, und jede Pruefung liefe ins Leere.
    let l = line.split('\t').next_back().unwrap_or(line).trim();
    // x86-64: `sub    $0x68,%rsp`
    if let Some(rest) = l.strip_prefix("sub").map(str::trim_start)
        && rest.ends_with("%rsp")
        && let Some(n) = rest.strip_prefix('$').and_then(|s| s.split(',').next())
    {
        return parse_num(n);
    }
    // aarch64: `sub sp, sp, #0x68`
    if l.starts_with("sub") && l.contains("sp, sp, #") {
        return l.split('#').nth(1).and_then(parse_num);
    }
    // aarch64: `stp x29, x30, [sp, #-16]!` — der Prolog legt den Rahmen
    // beim Sichern an.
    if (l.starts_with("stp") || l.starts_with("str")) && l.contains("[sp, #-") {
        return l.split("#-").nth(1).and_then(|s| s.split(']').next()).and_then(parse_num);
    }
    None
}

/// Eine Zahl, dezimal oder hexadezimal.
fn parse_num(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}
