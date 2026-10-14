//! Der Weg ab dem Resetvektor eines Abbilds fuer den Cortex-M4 (12.3,
//! FB-752).
//!
//! Ein Vorgaenger, der ohne Reset in den Bootloader springt, kann die MPU mit
//! seinen Regionen eingeschaltet lassen. Darum schaltet jedes Abbild sie aus,
//! bevor es Speicher beruehrt: **Auf dem Weg ab dem Resetvektor kommt vor
//! jedem Speicherzugriff ausserhalb des PPB und vor jedem Push ein `str` von
//! 0 auf `MPU_CTRL` (0xE000ED94), gefolgt von `dsb` und `isb`.** Ein Literal
//! im Code (`ldr rX, [pc, …]`) zaehlt nicht: Den Code, der gerade laeuft,
//! sperrt keine Region, die den Start noch zulaesst.
//!
//! Gelesen wird die Folge der Befehle, wie `llvm-objdump` aus `llvm-tools` der
//! festgelegten Toolchain sie zeigt, nicht feste Bytes: Ein anderer
//! Uebersetzer, der dasselbe mit anderen Befehlen tut, besteht, einer, der
//! vorher etwas anderes tut, nicht. Was der Weg nicht kennt — eine bedingte
//! Verzweigung, ein Zugriff an einer Adresse, die er nicht ausrechnen kann —,
//! bricht die Bedingung. Der Bau prueft so jedes Abbild fuer den F401, bevor
//! es in den Zwischenspeicher kommt.

use std::collections::HashMap;
use std::path::Path;

use takt_llvm::inspect::Binutils;

/// `MPU_CTRL` des Cortex-M4.
const MPU_CTRL: u32 = 0xE000_ED94;

/// Der private Peripheriebus (PPB): Keine Region der MPU gilt dort.
const PPB: std::ops::Range<u32> = 0xE000_0000..0xE010_0000;

/// So viele Befehle liest der Weg hoechstens.
const STEPS: usize = 64;

/// So viele Byte liest der Weg auf einmal ab einer Adresse.
const WINDOW: u32 = 0x80;

/// Prueft das Abbild `elf`, dessen Vektortabelle bei `vectors` beginnt.
pub fn mpu_off_first(elf: &Path, vectors: u32) -> Result<(), String> {
    let tools = Binutils::llvm().ok_or_else(|| {
        "llvm-objdump aus llvm-tools der Toolchain fehlt (`rustup component add llvm-tools`): ohne es prueft der Bau \
         den Weg ab dem Resetvektor nicht (FB-752)"
            .to_string()
    })?;
    let image = Objdump { tools, elf };
    walk(&image, image.word(vectors.wrapping_add(4))? & !1).map_err(|e| format!("{}: {e} (FB-752)", elf.display()))
}

/// Was der Weg vom Abbild liest.
trait Image {
    /// Die Befehle ab `at`.
    fn listing(&self, at: u32) -> Result<Vec<Insn>, String>;
    /// Das Wort an `at`.
    fn word(&self, at: u32) -> Result<u32, String>;
}

/// Das Abbild hinter `llvm-objdump`.
struct Objdump<'a> {
    tools: Binutils,
    elf: &'a Path,
}

impl Image for Objdump<'_> {
    fn listing(&self, at: u32) -> Result<Vec<Insn>, String> {
        let text = self
            .tools
            .disassemble(self.elf, u64::from(at), u64::from(at.saturating_add(WINDOW)))
            .ok_or_else(|| format!("llvm-objdump liest {} nicht", self.elf.display()))?;
        Ok(listing(&text))
    }

    fn word(&self, at: u32) -> Result<u32, String> {
        let text = self
            .tools
            .contents(self.elf, u64::from(at), u64::from(at.saturating_add(4)))
            .ok_or_else(|| format!("llvm-objdump liest {} nicht", self.elf.display()))?;
        word(&text, at).ok_or_else(|| format!("kein Wort bei {at:#010x}"))
    }
}

/// Ein Befehl: Adresse, Mnemonik, Operanden und die Adresse im Kommentar.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Insn {
    at: u32,
    mnemonic: String,
    operands: Vec<String>,
    note: Option<u32>,
}

/// Die Befehle einer Ausgabe von `objdump -d --no-show-raw-insn`.
fn listing(text: &str) -> Vec<Insn> {
    text.lines()
        // Ein Befehl steht eingerueckt; Kopf- und Dateizeilen nicht.
        .filter(|line| line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let (at, rest) = line.trim().split_once(':')?;
            let at = u32::from_str_radix(at, 16).ok()?;
            let (code, note) = rest.split_once('@').map_or((rest, None), |(c, n)| (c, Some(n)));
            let mut words = code.trim().splitn(2, char::is_whitespace);
            let mnemonic = words.next().filter(|m| !m.is_empty())?.to_string();
            let operands = split_operands(words.next().unwrap_or(""));
            let note = note.and_then(|n| n.split_whitespace().next()).and_then(number);
            Some(Insn { at, mnemonic, operands, note })
        })
        .collect()
}

/// Die Operanden, durch Kommas getrennt, ausser innerhalb von `[…]` und `{…}`.
fn split_operands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0usize, String::new());
    for c in text.chars() {
        match c {
            '[' | '{' => depth = depth.saturating_add(1),
            ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur).trim().to_string());
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Eine Zahl wie `objdump` sie schreibt: `#0x1f`, `0x8010198`, `#12`.
fn number(text: &str) -> Option<u32> {
    let t = text.trim().trim_start_matches('#');
    match t.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => t.parse().ok(),
    }
}

/// Das Wort an `at` aus einer Ausgabe von `objdump -s`, Little Endian.
fn word(text: &str, at: u32) -> Option<u32> {
    text.lines().find_map(|line| {
        let mut parts = line.strip_prefix(' ')?.split(' ');
        let base = u32::from_str_radix(parts.next()?, 16).ok()?;
        let mut bytes = Vec::new();
        for group in parts.take(4).take_while(|g| !g.is_empty() && g.chars().all(|c| c.is_ascii_hexdigit())) {
            for pair in group.as_bytes().chunks(2) {
                bytes.push(u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?);
            }
        }
        let offset = usize::try_from(at.checked_sub(base)?).ok()?;
        let four: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
        Some(u32::from_le_bytes(four))
    })
}

/// Wie weit der Weg mit dem Ausschalten ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Die MPU kann noch an sein.
    On,
    /// `MPU_CTRL` ist null geschrieben.
    Written,
    /// Danach ein `dsb`.
    Synced,
}

/// Die bedingten Spruenge des Thumb-Befehlssatzes.
const CONDITIONS: &[&str] =
    &["eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le"];

/// Befehle, die Speicher lesen oder schreiben.
const MEMORY: &[&str] =
    &["push", "pop", "stm", "ldm", "str", "ldr", "vpush", "vpop", "vstm", "vldm", "vstr", "vldr", "tbb", "tbh"];

/// Geht den Weg ab `reset`, bis die MPU aus ist; ein Fehler nennt den Befehl,
/// an dem die Bedingung bricht.
fn walk(image: &impl Image, reset: u32) -> Result<(), String> {
    let mut regs: HashMap<String, u32> = HashMap::new();
    let mut calls: Vec<u32> = Vec::new();
    let (mut phase, mut pc, mut steps) = (Phase::On, reset, 0usize);
    loop {
        let insns = image.listing(pc)?;
        if insns.is_empty() {
            return Err(format!("{pc:#010x}: kein Befehl"));
        }
        let mut next = None;
        for insn in &insns {
            steps = steps.saturating_add(1);
            if steps > STEPS {
                return Err(format!("nach {STEPS} Befehlen ab {reset:#010x} ist die MPU nicht aus"));
            }
            let text = format!("{:#010x}: {} {}", insn.at, insn.mnemonic, insn.operands.join(", "));
            let op = |i: usize| insn.operands.get(i).map(String::as_str).unwrap_or("");
            let m = insn.mnemonic.trim_end_matches(".w").trim_end_matches(".n");
            match m {
                "movw" => set(&mut regs, op(0), number(op(1))),
                "movt" => {
                    let low = regs.get(op(0)).map(|v| v & 0xFFFF);
                    set(&mut regs, op(0), low.zip(number(op(1))).map(|(l, h)| (h << 16) | l));
                }
                "mov" | "movs" if op(1).starts_with('#') => set(&mut regs, op(0), number(op(1))),
                "mov" | "movs" => {
                    let v = regs.get(op(1)).copied();
                    set(&mut regs, op(0), v);
                }
                "ldr" if op(1).starts_with("[pc") => {
                    let at = insn.note.ok_or_else(|| format!("{text}: Literal ohne Adresse"))?;
                    set(&mut regs, op(0), Some(image.word(at)?));
                }
                "str" | "ldr" => {
                    let at =
                        address(&regs, op(1)).ok_or_else(|| format!("{text}: Zugriff vor dem Ausschalten der MPU"))?;
                    if !PPB.contains(&at) {
                        return Err(format!("{text}: Zugriff auf {at:#010x} vor dem Ausschalten der MPU"));
                    }
                    if m == "str" && at == MPU_CTRL && regs.get(op(0)) == Some(&0) {
                        phase = Phase::Written;
                    }
                }
                "dsb" if phase == Phase::Written => phase = Phase::Synced,
                "isb" if phase == Phase::Synced => return Ok(()),
                "dsb" | "isb" | "nop" => {}
                "bl" => {
                    calls.push(insn.at.wrapping_add(4));
                    next = Some(target(&text, op(0))?);
                }
                "b" => next = Some(target(&text, op(0))?),
                "bx" if op(0) == "lr" => {
                    next = Some(calls.pop().ok_or_else(|| format!("{text}: Rueckkehr ohne Aufruf"))?);
                }
                _ if MEMORY.iter().any(|p| m.starts_with(p)) => {
                    return Err(format!("{text}: Speicherzugriff vor dem Ausschalten der MPU"));
                }
                _ if branches(m) => return Err(format!("{text}: Verzweigung vor dem Ausschalten der MPU")),
                _ => {
                    regs.remove(op(0));
                }
            }
            if next.is_some() {
                break;
            }
        }
        pc = match next {
            Some(n) => n,
            None => insns.last().map_or(pc, |i| i.at).wrapping_add(2),
        };
    }
}

/// Setzt oder vergisst den Wert eines Registers.
fn set(regs: &mut HashMap<String, u32>, reg: &str, value: Option<u32>) {
    match value {
        Some(v) => regs.insert(reg.to_string(), v),
        None => regs.remove(reg),
    };
}

/// Die Adresse von `[rN]` oder `[rN, #imm]`, wenn `rN` bekannt ist.
fn address(regs: &HashMap<String, u32>, operand: &str) -> Option<u32> {
    let inner = operand.strip_prefix('[')?.strip_suffix(']')?;
    let mut parts = inner.split(',').map(str::trim);
    let base = regs.get(parts.next()?)?;
    let offset = parts.next().map_or(Some(0), number)?;
    Some(base.wrapping_add(offset))
}

/// Das Ziel eines Sprungs: `0x8016b4c <__pre_init>`.
fn target(text: &str, operand: &str) -> Result<u32, String> {
    operand.split_whitespace().next().and_then(number).ok_or_else(|| format!("{text}: Ziel unbekannt"))
}

/// Verzweigt der Befehl bedingt oder ueber ein Register?
fn branches(m: &str) -> bool {
    let it = m.starts_with("it") && m.len() <= 5;
    it || ["cbz", "cbnz", "blx", "bx"].contains(&m) || m.strip_prefix('b').is_some_and(|c| CONDITIONS.contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Abbild aus Listings je Adresse und Woertern.
    struct Fake {
        listings: Vec<(u32, &'static str)>,
        words: Vec<(u32, u32)>,
    }

    impl Image for Fake {
        fn listing(&self, at: u32) -> Result<Vec<Insn>, String> {
            let text = self.listings.iter().find(|(a, _)| *a == at).map_or("", |(_, t)| t);
            Ok(listing(text))
        }

        fn word(&self, at: u32) -> Result<u32, String> {
            self.words.iter().find(|(a, _)| *a == at).map(|(_, w)| *w).ok_or_else(|| format!("{at:#x}"))
        }
    }

    /// Der Rust-Weg (`cortex-m-rt`): VTOR ueber Literale, dann `__pre_init`.
    const RUST_RESET: &str = "
 8010198:      \tldr\tr0, [pc, #0x40]          @ 0x80101dc <Reset+0x44>
 801019a:      \tldr\tr1, [pc, #0x44]          @ 0x80101e0 <Reset+0x48>
 801019c:      \tstr\tr1, [r0]
 801019e:      \tbl\t0x8016b4c <__pre_init>
 80101a2:      \tldr\tr0, [pc, #0x40]          @ 0x80101e4 <Reset+0x4c>
";

    fn rust(pre_init: &'static str) -> Fake {
        Fake {
            listings: vec![(0x0801_0198, RUST_RESET), (0x0801_6b4c, pre_init)],
            words: vec![(0x0801_01dc, 0xE000_ED08), (0x0801_01e0, 0x0801_0000)],
        }
    }

    /// **Der Weg ab dem Resetvektor schaltet die MPU aus, bevor er Speicher
    /// beruehrt** (12.3, FB-752): mit `__pre_init` im Board-Crate und mit dem
    /// nackten `Reset_Handler` der C-Wirte, nicht mit dem leeren
    /// `DefaultPreInit` von `cortex-m-rt`, nicht mit einem Prolog davor und
    /// nicht mit einer Verzweigung, die der Weg nicht kennt.
    #[test]
    fn the_reset_path_turns_the_mpu_off_before_it_touches_memory() {
        let ours = "
 8016b4c:      \tmovw\tr0, #0xed94
 8016b50:      \tmovt\tr0, #0xe000
 8016b54:      \tmovs\tr1, #0x0
 8016b56:      \tstr\tr1, [r0]
 8016b58:      \tdsb\tsy
 8016b5c:      \tisb\tsy
 8016b60:      \tbx\tlr
";
        assert_eq!(walk(&rust(ours), 0x0801_0198), Ok(()));
        let default = "
 8016b4c:      \tpush\t{r7, lr}
 8016b4e:      \tmov\tr7, sp
 8016b50:      \tpop\t{r7, pc}
";
        assert!(walk(&rust(default), 0x0801_0198).is_err_and(|e| e.contains("push")));
        let without_sync = "
 8016b4c:      \tmovw\tr0, #0xed94
 8016b50:      \tmovt\tr0, #0xe000
 8016b54:      \tmovs\tr1, #0x0
 8016b56:      \tstr\tr1, [r0]
 8016b58:      \tbx\tlr
";
        assert!(walk(&rust(without_sync), 0x0801_0198).is_err());
        let c = Fake {
            listings: vec![(
                0x0801_0198,
                "
 8010198:      \tmovw\tr0, #0xed94
 801019c:      \tmovt\tr0, #0xe000
 80101a0:      \tmovs\tr1, #0x0
 80101a2:      \tstr\tr1, [r0]
 80101a4:      \tdsb\tsy
 80101a8:      \tisb\tsy
 80101ac:      \tb.w\t0x80101b0 <board_reset>
",
            )],
            words: vec![],
        };
        assert_eq!(walk(&c, 0x0801_0198), Ok(()));
        let prologue = Fake {
            listings: vec![(0x0801_0198, "\n 8010198:      \tpush\t{r7, lr}\n 801019a:      \tmov\tr7, sp\n")],
            words: vec![],
        };
        assert!(walk(&prologue, 0x0801_0198).is_err_and(|e| e.contains("push")));
        let branch =
            Fake { listings: vec![(0x0801_0198, "\n 8010198:      \tcbz\tr0, 0x80101a0 <x>\n")], words: vec![] };
        assert!(walk(&branch, 0x0801_0198).is_err_and(|e| e.contains("Verzweigung")));
    }

    /// `objdump -s` zeigt ganze Zeilen; das Wort kommt aus seiner Stelle.
    #[test]
    fn a_word_comes_from_its_place_in_the_dump() {
        let dump = "file format elf32-littlearm\nContents of section .vector_table:\n \
                    8010000 00000120 99010108 476b0108 476b0108  ... ....Gk..Gk..\n";
        assert_eq!(word(dump, 0x0801_0004), Some(0x0801_0199));
        assert_eq!(word(dump, 0x0801_0000), Some(0x2001_0000));
        assert_eq!(word(dump, 0x0801_0010), None);
    }
}
