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
//! `objdump` stehen in jeder Werkzeugkette, die ohnehin gebraucht wird.
//! Das Modul kennt darum nur ihre Ausgabe, nicht das Format.
//!
//! **Die LLVM-Werkzeuge sind der kuerzere Weg.** `rustup component add
//! llvm-tools` liefert `llvm-objdump`, `llvm-nm` und `llvm-size`, und ein
//! einziges Binaerprogramm liest ARM, ARM64 und RISC-V — die
//! GNU-Werkzeuge brauchen je Ziel eine eigene Kette (`arm-none-eabi-*`,
//! `riscv32-unknown-elf-*`). Fuer M5 heisst das: keine Cross-binutils zu
//! beschaffen, und dieselbe LLVM-Version, die auch den Code erzeugt hat,
//! liest ihn wieder. [`Binutils::llvm`] findet sie in der aktiven
//! Toolchain; [`Binutils::with_prefix`] bleibt fuer GNU-Ketten.
//!
//! **Was fehlt, ist kein Fehler.** Wo kein Werkzeug steht, liefert jede
//! Funktion `None`. Ein Test, der misst, ueberspringt sich dann — wie
//! jeder andere Werkzeugkettentest auch.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Code im Instruktions-RAM (`xip_flash`, 12.3); nicht in `text`.
    pub iram_text: u64,
    /// Konstanten im RAM, die der Tick liest (12.3); nicht in `rodata`.
    pub iram_rodata: u64,
}

impl Sections {
    /// Was im Flash liegt: Code und Konstanten (11.5, 12.3).
    ///
    /// `.data` zaehlt mit, weil seine Anfangswerte ebenfalls im Flash
    /// stehen muessen — die Runtime kopiert sie beim Start ins RAM.
    /// Fuer die RAM-residenten Abschnitte gilt dasselbe.
    pub fn flash(&self) -> u64 {
        self.text + self.rodata + self.data + self.iram_text + self.iram_rodata
    }

    /// Was im RAM liegt (11.5).
    pub fn ram(&self) -> u64 {
        self.data + self.bss
    }

    /// Was im Instruktions-RAM liegt (12.3); null ohne XIP.
    pub fn iram(&self) -> u64 {
        self.iram_text + self.iram_rodata
    }
}

/// Ein Symbol mit seiner Groesse (11.5, 13.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    /// Name, wie der Linker ihn fuehrt.
    pub name: String,
    /// Adresse im Abbild.
    pub address: u64,
    /// Groesse in Byte; null, wenn das Format sie nicht nennt.
    pub size: u64,
    /// Die Klasse, wie `nm` sie schreibt: `T` fuer Code, `t` fuer
    /// lokalen Code, `R`/`r` fuer Konstanten, `B`/`b` fuer `.bss`.
    pub kind: char,
}

/// Ein Abschnitt mit seiner Lage im Abbild (`objdump -h`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionRange {
    /// Name.
    pub name: String,
    /// Erste Adresse.
    pub start: u64,
    /// Groesse in Byte.
    pub size: u64,
}

impl SectionRange {
    fn contains(&self, address: u64) -> bool {
        address >= self.start && address < self.start.saturating_add(self.size)
    }
}

/// Welche Symbole eines Programms im RAM liegen und welche im Flash
/// (12.3, `xip_flash`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Residency {
    /// Symbole in RAM-residenten Abschnitten.
    pub ram: Vec<String>,
    /// Symbole im Flash — bei asynchronem NVM ein Stillstand im Tick.
    pub flash: Vec<String>,
}

/// Ordnet die Code- und Konstantensymbole, die `is_program` waehlt, nach
/// dem Abschnitt ein, in dem ihre Adresse liegt.
pub fn residency(symbols: &[Symbol], sections: &[SectionRange], is_program: impl Fn(&str) -> bool) -> Residency {
    let mut out = Residency::default();
    for s in symbols.iter().filter(|s| matches!(s.kind, 'T' | 't' | 'R' | 'r') && is_program(&s.name)) {
        let resident = sections
            .iter()
            .find(|sec| sec.contains(s.address))
            .is_some_and(|sec| is_iram_text(&sec.name) || is_iram_rodata(&sec.name));
        if resident { &mut out.ram } else { &mut out.flash }.push(s.name.clone());
    }
    out
}

/// Liegt `address` in einem Abschnitt, der im RAM laeuft (12.3)? Eine
/// Adresse ausserhalb jedes Abschnitts ist absolut — eine Routine im ROM
/// des Chips — und laeuft ebenso ohne Flash.
pub fn runs_without_flash(address: u64, sections: &[SectionRange]) -> bool {
    sections.iter().find(|sec| sec.contains(address)).is_none_or(|sec| is_iram_text(&sec.name))
}

/// Eine Funktion im Aufrufgraphen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Function {
    /// Der demangelte Name (`objdump -C`). Lokale Namen gibt es je Objekt
    /// einmal (`OUTLINED_FUNCTION_2`); der Graph fuehrt die Funktion darum
    /// unter ihrer Adresse.
    pub name: String,
    /// Die Groesse in Byte aus der Symboltabelle; 0, wenn sie sie nicht nennt.
    pub size: u64,
    /// Die Startadressen der Funktionen, die sie ruft oder anspringt.
    pub calls: BTreeSet<u64>,
    /// Ziele, deren Adresse die Disassemblierung nicht nennt und deren Name
    /// mehrere Funktionen tragen.
    pub unresolved: BTreeSet<String>,
    /// Ein Aufruf ueber ein Register: Sein Ziel kennt der Graph nicht.
    pub indirect: bool,
}

/// Der Aufrufgraph eines gebundenen Abbilds (12.3, `takt check-image`):
/// je Funktion die Funktionen, die sie ruft oder anspringt, aus der
/// Disassemblierung. Ein Sprung innerhalb der Funktion ist keine Kante.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CallGraph {
    /// Die Funktionen nach ihrer Startadresse.
    pub functions: BTreeMap<u64, Function>,
}

impl CallGraph {
    /// Die Startadressen der Funktionen, die `name` tragen.
    pub fn named(&self, name: &str) -> Vec<u64> {
        self.functions.iter().filter(|(_, f)| f.name == name).map(|(at, _)| *at).collect()
    }

    /// Die Funktion, in der `address` liegt: ab ihrem Anfang bis zu ihrer
    /// Groesse, ohne Groesse bis zur naechsten.
    pub fn containing(&self, address: u64) -> Option<u64> {
        let (at, f) = self.functions.range(..=address).next_back()?;
        (f.size == 0 || address < at.saturating_add(f.size)).then_some(*at)
    }
}

/// Ein Ziel, wie die Zeile es nennt.
enum Target {
    Address(u64),
    Name(String),
}

/// Liest `objdump -d -C`: Ein Kopf `adresse <name>:` beginnt eine Funktion,
/// ein Sprung oder Aufruf mit Ziel `<name>` oder `<name+0x..>` ist eine
/// Kante, ein Aufruf ohne Ziel (`jalr a0`, `blx r3`, `call *%rax`) ein
/// indirekter. Die Adresse des Ziels steht in der Zeile (`jal 0x… <f>`);
/// bei `auipc` und `jalr off(ra)` (RISC-V) rechnet der Leser sie aus dem
/// Paar. Nur wo keine steht, entscheidet der Name, und nur, wenn er eindeutig
/// ist.
///
/// **Funktionen nach der Symboltabelle.** `code` nennt Anfang und Groesse
/// jeder Funktion (`nm -S`, Klasse `T`/`t`/`W`/`w`). Ein Kopf, der keine
/// beginnt — eine Konstante, eine Marke —, ist keine, und was hinter dem
/// Ende einer Funktion steht, liest der Leser nicht: In einem
/// RAM-Abschnitt mit Code liegen auch Konstanten, und `objdump` liest sie
/// als Befehle.
pub fn parse_call_graph(text: &str, code: &BTreeMap<u64, u64>) -> CallGraph {
    let mut graph = CallGraph::default();
    let mut raw: Vec<(u64, Target)> = Vec::new();
    let mut current: Option<u64> = None;
    let mut bases: BTreeMap<String, u64> = BTreeMap::new();
    for line in text.lines() {
        if let Some((at, name)) = function_header(line) {
            if let Some(&size) = code.get(&at) {
                graph.functions.entry(at).or_insert_with(|| Function {
                    name: name.to_string(),
                    size,
                    ..Function::default()
                });
                current = Some(at);
                bases.clear();
            }
            continue;
        }
        let Some(f) = current else { continue };
        // `  408018fc:      \tjalr\t0x54e(ra) <takt_board_trace>`
        let Some((pc, insn)) = line.split_once('\t') else { continue };
        let Some(pc) = pc.trim().strip_suffix(':').and_then(|pc| u64::from_str_radix(pc, 16).ok()) else { continue };
        let size = graph.functions.get(&f).map_or(0, |func| func.size);
        if size != 0 && pc >= f.saturating_add(size) {
            current = None;
            continue;
        }
        let insn = insn.trim();
        let (mnemonic, operands) = insn.split_once('\t').unwrap_or((insn, ""));
        if mnemonic == "auipc"
            && let Some((reg, imm)) = operands.split_once(", ")
            && let Some(imm) = parse_signed(imm)
        {
            // Das Feld hat 20 Bit, `objdump` schreibt es ohne Vorzeichen.
            let imm = if imm >= 0x8_0000 { imm - 0x10_0000 } else { imm };
            bases.insert(reg.to_string(), wrap(pc, imm << 12));
            continue;
        }
        if !is_branch(mnemonic) {
            continue;
        }
        match branch_target(operands) {
            Some((before, name)) => {
                let target = match last_operand(before) {
                    // `0x54e(ra)`: Versatz zu einem Register, das `auipc` setzte.
                    Some(op) if op.ends_with(')') => op
                        .strip_suffix(')')
                        .and_then(|o| o.split_once('('))
                        .and_then(|(off, reg)| Some(wrap(*bases.get(reg)?, parse_signed(off)?)))
                        .map(Target::Address),
                    Some(op) => parse_signed(op).and_then(|a| u64::try_from(a).ok()).map(Target::Address),
                    None => None,
                };
                let name = name.rsplit_once("+0x").map_or(name, |(n, _)| n);
                raw.push((f, target.unwrap_or_else(|| Target::Name(name.to_string()))));
            }
            None if is_indirect(mnemonic, insn) => {
                if let Some(func) = graph.functions.get_mut(&f) {
                    func.indirect = true;
                }
            }
            None => {}
        }
    }
    for (f, target) in raw {
        let to = match target {
            Target::Address(at) => graph.containing(at),
            Target::Name(name) => match graph.named(&name).as_slice() {
                [one] => Some(*one),
                _ => {
                    if let Some(func) = graph.functions.get_mut(&f) {
                        func.unresolved.insert(name);
                    }
                    None
                }
            },
        };
        if let Some(to) = to.filter(|to| *to != f)
            && let Some(func) = graph.functions.get_mut(&f)
        {
            func.calls.insert(to);
        }
    }
    graph
}

/// Adresse und Name im Kopf einer Funktion: `408018b8 <app_tick>:`.
fn function_header(line: &str) -> Option<(u64, &str)> {
    let (address, rest) = line.split_once(' ')?;
    if address.is_empty() || !address.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((u64::from_str_radix(address, 16).ok()?, rest.strip_prefix('<')?.strip_suffix(">:")?))
}

/// Eine Zahl mit Vorzeichen, hexadezimal (`0x54e`, `-0x48`) oder dezimal.
fn parse_signed(s: &str) -> Option<i64> {
    let s = s.trim();
    let (negative, digits) = s.strip_prefix('-').map_or((false, s), |d| (true, d));
    let value = match digits.strip_prefix("0x") {
        Some(hex) => i64::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    Some(if negative { -value } else { value })
}

/// `base + offset` in der Breite der Adresse: 32 Bit, solange sie passt.
fn wrap(base: u64, offset: i64) -> u64 {
    let sum = base.wrapping_add_signed(offset);
    if base <= u64::from(u32::MAX) { sum & u64::from(u32::MAX) } else { sum }
}

/// Der letzte Operand: `t0, 0x40802bbc` → `0x40802bbc`, `0x54e(ra)` bleibt.
fn last_operand(operands: &str) -> Option<&str> {
    operands.rsplit([',', ' ']).map(str::trim).find(|s| !s.is_empty())
}

/// Ein Sprung, Aufruf oder bedingter Sprung: RISC-V (`jal`, `j`, `beqz`,
/// `tail`), ARM (`bl`, `b.w`, `cbz`) und x86 (`call`, `jmp`, `jne`).
fn is_branch(mnemonic: &str) -> bool {
    mnemonic.starts_with('j')
        || mnemonic.starts_with('b')
        || mnemonic.starts_with("call")
        || mnemonic.starts_with("tail")
        || mnemonic.starts_with("cb")
}

/// Ein Aufruf, dessen Ziel in einem Register steht (`jalr a0`, `blx r3`,
/// `call *%rax`). Ein Sprung ueber ein Register ist keiner: Er ist eine
/// Sprungtabelle in der Funktion oder die Rueckkehr aus Millicode und
/// ausgelagerten Stuecken (`jr t0`).
fn is_indirect(mnemonic: &str, insn: &str) -> bool {
    match mnemonic {
        "jalr" | "blx" => true,
        m if m.starts_with("call") => insn.contains('*'),
        _ => false,
    }
}

/// Das Ziel am Ende der Operanden, `<…>` mit allen Klammern des
/// demangelten Namens darin, und was davor steht.
fn branch_target(operands: &str) -> Option<(&str, &str)> {
    let body = operands.strip_suffix('>')?;
    let mut depth = 0usize;
    for (at, c) in body.char_indices().rev() {
        match c {
            '>' => depth += 1,
            '<' if depth == 0 => return Some((body[..at].trim_end(), &body[at + 1..])),
            '<' => depth -= 1,
            _ => {}
        }
    }
    None
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

    /// Die LLVM-Werkzeuge der aktiven Rust-Toolchain.
    ///
    /// `rustup component add llvm-tools` legt sie unter
    /// `<sysroot>/lib/rustlib/<host>/bin` ab. Ein einziges `llvm-objdump`
    /// liest ARM, ARM64 und RISC-V, also brauchen alle vier Ziele aus
    /// [`crate::target::Target::ALL`] keine eigene Kette.
    ///
    /// `None`, wenn die Komponente fehlt oder `rustc` nicht erreichbar
    /// ist — dann bleibt [`Binutils::with_prefix`] fuer eine GNU-Kette.
    pub fn llvm() -> Option<Binutils> {
        let out = Command::new("rustc").args(["--print", "sysroot"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let sysroot = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let host = Command::new("rustc").arg("-vV").output().ok()?;
        let host =
            String::from_utf8_lossy(&host.stdout).lines().find_map(|l| l.strip_prefix("host: ").map(str::to_string))?;
        let dir = Path::new(&sysroot).join("lib").join("rustlib").join(host).join("bin");
        let probe = dir.join(if cfg!(windows) { "llvm-nm.exe" } else { "llvm-nm" });
        probe.exists().then(|| Binutils { prefix: format!("{}{}llvm-", dir.display(), std::path::MAIN_SEPARATOR) })
    }

    /// Die Werkzeuge fuer ein Ziel: LLVM, wo vorhanden, sonst die
    /// GNU-Kette des Ziels.
    ///
    /// Die Reihenfolge ist Absicht. LLVM liest jedes der vier Ziele und
    /// stammt aus derselben Version, die den Code erzeugt hat; eine
    /// GNU-Kette muss erst beschafft werden und gibt es je Ziel einzeln.
    pub fn best_for(target: crate::target::Target) -> Binutils {
        Binutils::llvm().unwrap_or_else(|| Binutils::for_target(target))
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
        out.status.success().then(|| parse_sections(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Das Rohabbild eines ELF (`objcopy -O binary`), wie ein Bootloader es
    /// schreibt: die ladbaren Abschnitte ab der niedrigsten Adresse.
    ///
    /// Falsch, wenn das Werkzeug fehlt oder scheitert.
    pub fn raw_image(&self, elf: &Path, out: &Path) -> bool {
        Command::new(self.tool("objcopy")).args(["-O", "binary"]).arg(elf).arg(out).status().is_ok_and(|s| s.success())
    }

    /// Die Abschnitte mit ihrer Lage im Abbild (`objdump -h`).
    pub fn section_ranges(&self, file: &Path) -> Option<Vec<SectionRange>> {
        let out = Command::new(self.tool("objdump")).arg("-h").arg(file).output().ok()?;
        out.status.success().then(|| parse_section_table(&String::from_utf8_lossy(&out.stdout)))
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
            let address = u64::from_str_radix(w[0], 16).unwrap_or(0);
            list.push(Symbol { name: name.to_string(), address, size, kind: k });
        }
        // Nach Namen sortiert: Die Reihenfolge von `nm` haengt an der
        // Adresse, und ein Vergleich zweier Ziele soll sie nicht sehen.
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Some(list)
    }

    /// Die definierten Symbole mit demangelten Namen (`nm -C`), so wie
    /// [`Binutils::call_graph`] sie nennt.
    pub fn symbols_demangled(&self, file: &Path) -> Option<Vec<Symbol>> {
        let out = Command::new(self.tool("nm")).args(["-S", "--defined-only", "-C"]).arg(file).output().ok()?;
        out.status.success().then(|| parse_demangled_symbols(&String::from_utf8_lossy(&out.stdout)))
    }

    /// Der Aufrufgraph eines gebundenen Abbilds (`objdump -d -C`), mit den
    /// Funktionen, die die Symboltabelle nennt.
    pub fn call_graph(&self, file: &Path) -> Option<CallGraph> {
        let code: BTreeMap<u64, u64> = self
            .symbols(file)?
            .into_iter()
            .filter(|s| matches!(s.kind, 'T' | 't' | 'W' | 'w'))
            .map(|s| (s.address, s.size))
            .collect();
        let out =
            Command::new(self.tool("objdump")).args(["-d", "-C", "--no-show-raw-insn"]).arg(file).output().ok()?;
        out.status.success().then(|| parse_call_graph(&String::from_utf8_lossy(&out.stdout), &code))
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
    /// Die Rahmengroessen aller genannten Funktionen in einem Durchlauf.
    ///
    /// **Einmal lesen statt je Funktion.** [`Binutils::stack_frame`] ruft
    /// `objdump` fuer jedes Symbol neu; bei einem Programm mit vielen
    /// Funktionen ist das dieselbe Disassemblierung N-mal. Fuer die
    /// Stackrechnung (12.3) werden ohnehin alle gebraucht, also entsteht
    /// die Tabelle in einem Zug.
    ///
    /// Die Rueckgabe hat dieselbe Laenge und Reihenfolge wie `symbols`.
    /// `None` an einer Stelle heisst „nicht gefunden" — die Rechnung
    /// meldet dann eine unbekannte Tiefe statt einer zu kleinen.
    pub fn stack_frames(&self, file: &Path, symbols: &[String]) -> Vec<Option<u64>> {
        let mut out = vec![None; symbols.len()];
        let Ok(res) = Command::new(self.tool("objdump")).args(["-d", "-r", "--no-show-raw-insn"]).arg(file).output()
        else {
            return out;
        };
        if !res.status.success() {
            return out;
        }
        let text = String::from_utf8_lossy(&res.stdout);

        // Ein Durchlauf: Beim Funktionskopf merken, welches Symbol gerade
        // laeuft, und die erste Rahmenanpassung danach nehmen.
        let mut current: Option<usize> = None;
        // Was `__riscv_save_N` vor der Rahmenanpassung sichert (`-msave-restore`).
        let mut saved = 0;
        for line in text.lines() {
            if line.contains(">:") && !line.contains("jalr") {
                // Ein Kopf ohne Rahmenanpassung ist ein Blatt mit Registern.
                if let Some(i) = current.take()
                    && out[i].is_none()
                {
                    out[i] = Some(saved);
                }
                current = symbols.iter().position(|sym| line.contains(&format!("<{sym}>:")));
                saved = 0;
                continue;
            }
            if current.is_some()
                && let Some(n) = millicode_save(line)
            {
                saved = n;
            }
            if let Some(i) = current
                && out[i].is_none()
                && let Some(n) = frame_adjust(line)
            {
                out[i] = Some(n + saved);
                current = None;
            }
        }
        if let Some(i) = current
            && out[i].is_none()
        {
            out[i] = Some(saved);
        }
        out
    }

    /// Der Stackrahmen einer Funktion, aus ihrem Prolog gelesen (12.3).
    ///
    /// 12.3 rechnet die Tiefe als laengsten Pfad im Aufrufgraphen; hier
    /// entsteht die Zahl je Knoten.
    ///
    /// `None` heisst: kein Werkzeug, oder die Funktion steht nicht in der
    /// Datei. `Some(0)` heisst: Sie hat keinen Rahmen — ein Blatt, das
    /// mit Registern auskommt.
    ///
    /// Fuer mehrere Funktionen ist [`Binutils::stack_frames`] der Weg: Es
    /// liest die Disassemblierung einmal statt je Aufruf.
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
    let l = line.split_once('\t').map_or(line, |(_, rest)| rest).replace('\t', " ");
    let l = l.trim();
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
    // RISC-V: `addi sp, sp, -0x430`
    if l.starts_with("addi") && l.contains("sp, sp, -") {
        return l.split("sp, sp, -").nth(1).and_then(parse_num);
    }
    None
}

/// Die Sicherung von `__riscv_save_N` (`millicode.S`): 16 Byte bis N = 3,
/// sonst 64 — im Objekt als Relokation, im Abbild als Sprungziel.
fn millicode_save(line: &str) -> Option<u64> {
    let at = line.find("__riscv_save_")?;
    let n: u64 =
        line[at + "__riscv_save_".len()..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    Some(if n <= 3 { 16 } else { 64 })
}

/// Eine Zahl, dezimal oder hexadezimal.
fn parse_num(s: &str) -> Option<u64> {
    let t = s.trim();
    match t.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// Die Ausgabe von `size -A`: je Zeile Abschnitt, Groesse, Adresse.
///
/// **Die Namen unterscheiden sich je Format.** ELF sagt `.rodata`,
/// PE/COFF sagt `.rdata` — und dort liegen auch `.xdata` und `.pdata`,
/// die Ausnahmebehandlung des Aufrufers. Beide zaehlen als Konstanten,
/// weil sie im Flash stehen und der Tick sie nicht schreibt.
///
/// RAM-residente Abschnitte stehen zuerst: `.rwtext` faenge sonst als
/// `.r…` in den Flash-Konstanten (12.3).
fn parse_sections(text: &str) -> Sections {
    let mut s = Sections::default();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        let (Some(name), Some(size)) = (w.next(), w.next()) else { continue };
        let Ok(bytes) = size.parse::<u64>() else { continue };
        // Ein Abschnitt kann Zusaetze tragen (`.text.startup`); die
        // Zuordnung geht darum ueber das Praefix.
        match name {
            n if is_iram_text(n) => s.iram_text += bytes,
            n if is_iram_rodata(n) => s.iram_rodata += bytes,
            n if n.starts_with(".text") => s.text += bytes,
            // `.srodata`, `.sdata`, `.sbss`: die kleinen Daten auf riscv32.
            n if n.starts_with(".rodata") || n.starts_with(".rdata") || n.starts_with(".srodata") => s.rodata += bytes,
            n if n.starts_with(".xdata") || n.starts_with(".pdata") => s.rodata += bytes,
            n if n.starts_with(".data") || n.starts_with(".sdata") => s.data += bytes,
            n if n.starts_with(".bss") || n.starts_with(".sbss") => s.bss += bytes,
            _ => {}
        }
    }
    s
}

/// `nm -S -C`: `adresse [groesse] klasse name`, der Name mit Leerzeichen
/// (`<impl Foo for Bar>::f`). Eine Zeile ohne Adresse (der Kopf eines
/// Archivmitglieds) zaehlt nicht.
fn parse_demangled_symbols(text: &str) -> Vec<Symbol> {
    let hex = |w: &str| u64::from_str_radix(w, 16).ok();
    let mut list = Vec::new();
    for line in text.lines() {
        let Some((address, rest)) = line.split_once(' ') else { continue };
        let Some(address) = hex(address) else { continue };
        let Some((second, rest)) = rest.split_once(' ') else { continue };
        let (size, kind, name) = match second.len() {
            1 => (0, second, rest),
            _ => {
                let Some((kind, name)) = rest.split_once(' ') else { continue };
                (hex(second).unwrap_or(0), kind, name)
            }
        };
        let mut chars = kind.chars();
        let (Some(k), None) = (chars.next(), chars.next()) else { continue };
        list.push(Symbol { name: name.to_string(), address, size, kind: k });
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// Die Tabelle von `objdump -h`: `Idx Name Size VMA Type`, hexadezimal.
fn parse_section_table(text: &str) -> Vec<SectionRange> {
    text.lines()
        .filter_map(|line| {
            let w: Vec<&str> = line.split_whitespace().collect();
            let (name, size, vma) = (w.get(1)?, w.get(2)?, w.get(3)?);
            if !name.starts_with('.') {
                return None;
            }
            Some(SectionRange {
                name: name.to_string(),
                start: u64::from_str_radix(vma, 16).ok()?,
                size: u64::from_str_radix(size, 16).ok()?,
            })
        })
        .collect()
}

/// Praefix eines Abschnittsnamens, mit `.suffix` als Treffer.
fn has_prefix(name: &str, prefixes: &[&str]) -> bool {
    let Some(n) = name.strip_prefix('.') else { return false };
    prefixes.iter().any(|p| n == *p || n.strip_prefix(p).is_some_and(|r| r.starts_with('.')))
}

/// RAM-residenter Code (12.3): `esp-hal` sagt `.rwtext` und legt den
/// Trap-Vektor in `.trap`, ESP-IDF sagt `.iram0.text`, RP2040
/// `.time_critical`.
fn is_iram_text(name: &str) -> bool {
    has_prefix(name, &["rwtext", "trap", "iram0.text", "iram.text", "iram", "time_critical", "ramfunc"])
}

/// Konstanten, die der Tick im RAM liest (12.3): DFA- und `const`-Tabellen,
/// `safe`-Werte.
fn is_iram_rodata(name: &str) -> bool {
    has_prefix(name, &["rwtext.rodata", "iram0.rodata", "iram.rodata"])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `llvm-objdump -d -C` auf dem Bring-up-Abbild des ESP32-C6: Aufrufe
    /// ueber `auipc`/`jalr`, Spruenge innerhalb der Funktion und ueber eine
    /// Tabelle, der Millicode, zwei ausgelagerte Stuecke desselben Namens,
    /// ein indirekter Aufruf und ein Name mit Klammern.
    const RISCV: &str = "
408000f0 <OUTLINED_FUNCTION_2>:
408000f0:      \tjr\tt0
408000f2:      \tjal\ta4, 0x42000000 <core::fmt::write>

40800400 <OUTLINED_FUNCTION_2>:
40800400:      \tjal\t0x42000000 <core::fmt::write>

4080047c <<esp_hal::peripherals::UART0 as esp_hal::uart::low_level::Instance>::parts::handler>:
4080047c:      \tjalr\ta0
40800480:      \tjal\t0x40801000 <<takt_rt_core::loopcore::Runtime<P, C>>::finish>
40800484:      \tret

40801000 <<takt_rt_core::loopcore::Runtime<P, C>>::finish>:
40801000:      \tret

408018b8 <app_tick>:
408018b8:      \tjal\tt0, 0x40802bbc <__riscv_save_3>
408018ca:      \tbeqz\ta2, 0x408018d4 <app_tick+0x1c>
408018d0:      \tj\t0x40802be0 <__riscv_restore_3>
408018d4:      \tjr\ta1
408018f8:      \tauipc\tra, 0x1812
408018fc:      \tjalr\t0x54e(ra) <takt_board_trace>
40801900:      \tauipc\tra, 0xfffff
40801904:      \tjalr\t-0x810(ra) <OUTLINED_FUNCTION_2>
40801908:      \tjalr\t0x10(a5) <OUTLINED_FUNCTION_2>
4080190a:      \tauipc\tra, 0xff7fe
4080190e:      \tjalr\t0x4ea(ra) <.Lline_table_start1+0x3ffcd76d>

40802bbc <__riscv_save_3>:
40802bbc:      \tjr\tt0

40802be0 <__riscv_restore_3>:
40802be0:      \tret

42000000 <core::fmt::write>:
42000000:      \tret

42013e46 <takt_board_trace>:
42013e46:      \tret
";

    /// Die Funktionen des Abbilds mit ihrer Groesse (`nm -S`); hinter dem
    /// ersten Stueck stehen Konstanten, die `objdump` als `jal` liest.
    fn code() -> BTreeMap<u64, u64> {
        [
            (0x4080_00f0, 2),
            (0x4080_0400, 4),
            (0x4080_047c, 12),
            (0x4080_1000, 2),
            (0x4080_18b8, 0x5a),
            (0x4080_2bbc, 2),
            (0x4080_2be0, 2),
            (0x4200_0000, 2),
            (0x4201_3e46, 2),
        ]
        .into()
    }

    fn names(g: &CallGraph, at: u64) -> Vec<&str> {
        g.functions[&at].calls.iter().map(|c| g.functions[c].name.as_str()).collect()
    }

    #[test]
    fn the_call_graph_follows_calls_and_tail_calls_but_not_local_branches() {
        let g = parse_call_graph(RISCV, &code());
        assert!(g.functions[&0x4080_00f0].calls.is_empty(), "Konstanten hinter dem Stueck sind keine Befehle");
        assert_eq!(
            names(&g, 0x4080_18b8),
            ["OUTLINED_FUNCTION_2", "__riscv_save_3", "__riscv_restore_3", "takt_board_trace"]
        );
        assert!(g.functions[&0x4080_18b8].calls.contains(&0x4080_00f0), "das Stueck, das `auipc` und `jalr` nennen");
        assert!(!g.functions[&0x4080_18b8].calls.contains(&0x4080_0400), "nicht das andere gleichen Namens");
        assert_eq!(
            g.functions[&0x4080_18b8].unresolved.iter().collect::<Vec<_>>(),
            ["OUTLINED_FUNCTION_2"],
            "ohne Basis bleibt es offen; der Aufruf in das ROM ist keiner in eine Funktion des Abbilds"
        );
        assert!(!g.functions[&0x4080_18b8].indirect, "eine Sprungtabelle ist kein Aufruf");
        let handler = &g.functions[&0x4080_047c];
        assert!(handler.indirect && handler.name.ends_with("::handler"));
        assert_eq!(names(&g, 0x4080_047c), ["<takt_rt_core::loopcore::Runtime<P, C>>::finish"]);
        assert_eq!(g.named("OUTLINED_FUNCTION_2"), [0x4080_00f0, 0x4080_0400]);
    }

    /// ARM (`bl`, `b.w`, `blx`, `bx lr`) und x86 (`call`, `call *`, `jmp *`).
    #[test]
    fn the_call_graph_reads_arm_and_x86() {
        let arm = "08000100 <app_tick>:\n 8000102:      \tbl\t0x8000200 <takt_board_trace>\n 8000106:      \tblx\tr3\n \
                   8000108:      \tb.w\t0x8000300 <takt_tail>\n 800010c:      \tbx\tlr\n\n08000200 <takt_board_trace>:\n \
                   8000200:      \tbx\tlr\n\n08000300 <takt_tail>:\n 8000300:      \tbx\tlr\n";
        let g = parse_call_graph(arm, &[(0x0800_0100, 0x10), (0x0800_0200, 2), (0x0800_0300, 2)].into());
        assert_eq!(names(&g, 0x0800_0100), ["takt_board_trace", "takt_tail"]);
        assert!(g.functions[&0x0800_0100].indirect);
        let x86 = "0000000000001000 <app_tick>:\n    1004:      \tcall\t0x2000 <takt_board_trace>\n    \
                   1009:      \tcall\t*%rax\n    100b:      \tjmp\t*%rcx\n\n0000000000002000 <takt_board_trace>:\n    \
                   2000:      \tret\n";
        let g = parse_call_graph(x86, &[(0x1000, 0x10), (0x2000, 1)].into());
        assert_eq!(names(&g, 0x1000), ["takt_board_trace"]);
        assert!(g.functions[&0x1000].indirect);
    }

    #[test]
    fn demangled_symbols_keep_their_spaces() {
        let text = "\nlibapp.a(app.o):\n40807418 00000168 B app_arena\n\
                    4081d96a 00000052 t <takt::app::Program as takt_rt_core::loopcore::Program>::trace\n\
                    40000000 T rom_routine\n";
        let s = parse_demangled_symbols(text);
        assert_eq!(s.len(), 3);
        let trace = s.iter().find(|s| s.name.ends_with("::trace")).expect("Hullenfunktion");
        assert_eq!((trace.size, trace.kind), (0x52, 't'));
        assert_eq!(trace.name, "<takt::app::Program as takt_rt_core::loopcore::Program>::trace");
    }

    #[test]
    fn rom_and_iram_run_without_flash_but_text_does_not() {
        let sections = [
            SectionRange { name: ".rwtext".into(), start: 0x4080_0000, size: 0x1000 },
            SectionRange { name: ".text".into(), start: 0x4200_0000, size: 0x1000 },
        ];
        assert!(runs_without_flash(0x4080_0010, &sections));
        assert!(!runs_without_flash(0x4200_0010, &sections));
        assert!(runs_without_flash(0x4000_0010, &sections), "eine Routine im ROM");
    }

    /// `llvm-size -A` auf dem Bring-up-Abbild des ESP32-C6 (12.3).
    const ESP32C6: &str = "\
section                    size         addr
.trap                      1600   1082130432
.rwtext                    7188   1082132032
.rwtext.wifi                  0   1082139220
.data                      3028   1082139220
.bss                        856   1082142248
.rodata                   11772   1107296544
.text                     38908   1107361824
.stack                   439440   1082143104
";

    #[test]
    fn ram_resident_code_counts_as_iram_not_as_text() {
        let s = parse_sections(ESP32C6);
        assert_eq!(s.iram_text, 1600 + 7188);
        assert_eq!(s.text, 38908);
        assert_eq!(s.iram(), 8788);
    }

    /// Der Inhalt des RAM-Abschnitts wird beim Start aus dem Flash
    /// geladen und belegt ihn damit ebenfalls.
    #[test]
    fn iram_belongs_to_the_flash_image() {
        let s = parse_sections(ESP32C6);
        assert_eq!(s.flash(), 38908 + 11772 + 3028 + 8788);
        assert_eq!(s.ram(), 3028 + 856);
    }

    /// Ohne XIP gibt es die Abschnitte nicht, und `iram` ist null.
    #[test]
    fn a_target_without_xip_has_no_iram() {
        let s = parse_sections(".text 100 0\n.rodata 50 0\n.bss 20 0\n");
        assert_eq!(s.iram(), 0);
        assert_eq!(s.flash(), 150);
    }

    /// `.rwtext` beginnt mit `.r` und darf nicht als Flash-Konstante
    /// zaehlen; `.rodata` umgekehrt nicht als IRAM.
    /// GEN-018: Kleine Daten (`-msmall-data-limit`, LLVM-Voreinstellung auf
    /// riscv32) sind Konstanten, Daten und `.bss` wie ihre grossen
    /// Geschwister. Ins RAM holt `.srodata` erst ein Linkerskript, und dann
    /// heisst der Abschnitt des Abbilds `.rwtext`.
    #[test]
    fn small_data_counts_by_its_kind() {
        let s = parse_sections(".text 100 0\n.srodata.cst8 8 0\n.sdata 4 0\n.sbss 4 0\n");
        assert_eq!(s.iram(), 0);
        assert_eq!(s.flash(), 112);
        assert_eq!(s.ram(), 8);
    }

    #[test]
    fn rwtext_and_rodata_are_told_apart() {
        let s = parse_sections(".rwtext 10 0\n.rwtext.literal 4 0\n.rodata 7 0\n");
        assert_eq!(s.iram_text, 14);
        assert_eq!(s.rodata, 7);
    }
}

#[cfg(test)]
mod residency_tests {
    use super::*;

    const TABLE: &str = "\
Sections:
Idx Name                 Size     VMA      Type
  0                      00000000 00000000 
  1 .trap                00000640 40800000 TEXT
  2 .rwtext              00001c34 40800640 TEXT
  4 .data                0000032c 40802274 DATA
  9 .rodata              00002dfc 42000120 DATA
 11 .text                000097fc 42010020 TEXT
";

    fn sym(name: &str, address: u64, kind: char) -> Symbol {
        Symbol { name: name.into(), address, size: 4, kind }
    }

    #[test]
    fn the_section_table_reads_names_sizes_and_addresses() {
        let t = parse_section_table(TABLE);
        assert_eq!(t.len(), 5);
        assert_eq!(t[1], SectionRange { name: ".rwtext".into(), start: 0x4080_0640, size: 0x1c34 });
    }

    /// Ein Symbol liegt im RAM, wenn sein Abschnitt RAM-resident ist;
    /// Daten und fremde Symbole zaehlen nicht.
    #[test]
    fn symbols_are_sorted_by_the_section_that_holds_them() {
        let sections = parse_section_table(TABLE);
        let symbols = [
            sym("device_step", 0x4080_2160, 'T'),
            sym("takt_mcu_tick", 0x4201_0100, 'T'),
            sym("takt_fn_scale", 0x4200_0200, 'R'),
            sym("STATE", 0x4080_2300, 'B'),
            sym("memcpy", 0x4201_0500, 'T'),
        ];
        let r = residency(&symbols, &sections, |n| n.starts_with("device_") || n.starts_with("takt_"));
        assert_eq!(r.ram, vec!["device_step"]);
        assert_eq!(r.flash, vec!["takt_mcu_tick", "takt_fn_scale"]);
    }
}
