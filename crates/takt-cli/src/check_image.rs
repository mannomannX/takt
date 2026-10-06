//! `takt check-image ABBILD --manifest P.manifest …` (12.11, 13.8; M11
//! Schritt 11): prueft das fertig gebundene Abbild, nicht die Annahmen davor.
//!
//! Je Programm, dessen Manifest genannt ist:
//!
//! 1. ABI-Version und Logik-Hash: Die Symbole `P_abi_<n>` und
//!    `P_logic_<hash>` stehen im Abbild — gebunden ist die Bibliothek, die das
//!    Manifest beschreibt;
//! 2. die Arena unter `arena_symbol`, mindestens `arena_bytes` gross und an
//!    `arena_align` ausgerichtet, und kein beschreibbares Symbol aus Takt
//!    ausserhalb der Arenen — aus der Bibliothek, mit dem Praefix, aus den
//!    geteilten Crates oder der Huelle;
//! 3. unter `xip_flash`: Jede Funktion, die von einem Einstieg des
//!    Tick-Pfads aus erreichbar ist, liegt im RAM oder im ROM, auch die
//!    Treiber des Wirts. Ausgenommen sind Wege, die nur in eine Panik
//!    fuehren.
//!
//! Das Manifest des Messprogramms von `takt bench` (`# takt-bench 1`, 13.8)
//! traegt keine Arena und keinen Logik-Hash: Fuer es gilt Punkt 3, mit
//! `takt_bench_measure` als Einstieg — ein Kern, der aus dem Flash liefe,
//! maesse den Cache mit.
//!
//! Die Stacks prueft Schritt 13, die Antwortzeiten mehrerer Programme eines
//! Kerns Schritt 16. Jede Zeile traegt ihren Ursprung (11.5): `exakt` aus
//! Symboltabelle und Disassemblierung, `offen`, wo das Abbild es nicht sagt.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use takt_llvm::inspect::Function;
use takt_llvm::inspect::{CallGraph, SectionRange, Symbol, runs_without_flash};

/// Die Crates, die jedes Programm teilt, mit ihrem Pfad im demangelten Namen.
const TAKT_CRATES: [&str; 8] = [
    "takt_rt_core::",
    "takt_rt_baremetal::",
    "takt_rt_rtos::",
    "takt_embed::",
    "takt_hal::",
    "libtaktm::",
    "takt_native::",
    "takt_native_abi::",
];

/// Was ein Manifest ueber sein Programm sagt (`P.manifest`, 12.11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Manifest {
    /// Das Messprogramm von `takt bench` statt eines Programms.
    pub(crate) bench: bool,
    pub(crate) prefix: String,
    pub(crate) abi: String,
    pub(crate) logic_hash: String,
    pub(crate) arena_symbol: String,
    pub(crate) arena_bytes: u64,
    pub(crate) arena_align: u64,
    pub(crate) xip_flash: bool,
    pub(crate) tick_path: Vec<String>,
}

impl Manifest {
    /// Liest `key = wert`-Zeilen; `#` beginnt einen Kommentar.
    pub(crate) fn parse(text: &str) -> Result<Manifest, String> {
        let values: BTreeMap<&str, &str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim(), v.trim()))
            .collect();
        let get = |key: &str| values.get(key).copied().ok_or_else(|| format!("kein `{key}`"));
        let number = |key: &str| get(key)?.parse::<u64>().map_err(|e| format!("`{key}`: {e}"));
        let prefix = get("prefix")?.to_string();
        let tick_path = || -> Result<Vec<String>, String> {
            Ok(get("tick_path")?.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
        };
        if text.trim_start().starts_with("# takt-bench ") {
            return Ok(Manifest {
                bench: true,
                xip_flash: get("xip_flash")? == "true",
                tick_path: tick_path()?,
                prefix,
                ..Manifest::default()
            });
        }
        Ok(Manifest {
            bench: false,
            arena_symbol: values.get("arena_symbol").map_or_else(|| format!("{prefix}_arena"), |s| s.to_string()),
            abi: get("abi")?.to_string(),
            logic_hash: get("logic_hash")?.to_string(),
            arena_bytes: number("arena_bytes")?,
            arena_align: number("arena_align")?,
            xip_flash: get("xip_flash")? == "true",
            tick_path: tick_path()?,
            prefix,
        })
    }
}

/// Was im Abbild steht: Symbole und Abschnitte, der Aufrufgraph und die
/// Symbole, die die Bibliothek des Programms definiert.
#[derive(Clone, Debug, Default)]
pub(crate) struct Image {
    /// Die Symbole des Abbilds, demangelt (`nm -C`).
    pub(crate) symbols: Vec<Symbol>,
    pub(crate) sections: Vec<SectionRange>,
    /// `None`, wenn kein `objdump` das Abbild liest.
    pub(crate) graph: Option<CallGraph>,
}

/// Wie sicher eine Zeile ist (11.5); ein Befund laesst die Pruefung scheitern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    Exact,
    Open,
    Finding,
}

/// Eine Zeile des Berichts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) origin: Origin,
    pub(crate) text: String,
}

fn exact(text: String) -> Line {
    Line { origin: Origin::Exact, text }
}

fn open(text: String) -> Line {
    Line { origin: Origin::Open, text }
}

fn finding(text: String) -> Line {
    Line { origin: Origin::Finding, text }
}

/// Prueft ein Programm im Abbild. `library` sind die Symbole, die seine
/// Bibliothek definiert (`nm` auf `libP.a`); `arenas` die Adressbereiche
/// aller Arenen des Abbilds.
pub(crate) fn check(image: &Image, m: &Manifest, library: &BTreeSet<String>, arenas: &[(u64, u64)]) -> Vec<Line> {
    let x = &m.prefix;
    if m.bench {
        return if m.xip_flash {
            tick_path_in_ram(image, m)
        } else {
            vec![exact(format!("{x}: ohne `xip_flash` nichts zu pruefen"))]
        };
    }
    let by_name: BTreeMap<&str, &Symbol> = image.symbols.iter().map(|s| (s.name.as_str(), s)).collect();
    let mut out = Vec::new();

    // 1. ABI-Version und Logik-Hash.
    let abi = format!("{x}_abi_{}", m.abi);
    out.push(if by_name.contains_key(abi.as_str()) {
        exact(format!("{x}: ABI {} gebunden", m.abi))
    } else {
        finding(format!("{x}: `{abi}` fehlt — die Bibliothek ist nicht gebunden oder spricht ein anderes ABI"))
    });
    let logic = format!("{x}_logic_{}", m.logic_hash);
    let short = &m.logic_hash[..m.logic_hash.len().min(16)];
    out.push(if by_name.contains_key(logic.as_str()) {
        exact(format!("{x}: Logik-Hash {short}… gebunden"))
    } else {
        let bound: Vec<&str> =
            image.symbols.iter().filter_map(|s| s.name.strip_prefix(&format!("{x}_logic_"))).collect();
        match bound.first() {
            Some(other) => {
                finding(format!("{x}: gebunden ist ein anderes Programm (Logik-Hash {other}), nicht {short}…"))
            }
            None => finding(format!("{x}: `{logic}` fehlt — die Bibliothek ist nicht gebunden")),
        }
    });

    // 2. Die Arena und kein beschreibbares Takt-Symbol ausserhalb.
    match by_name.get(m.arena_symbol.as_str()) {
        None => out.push(finding(format!(
            "{x}: keine Arena `{}` — der Wirt stellt sie unter diesem Namen (12.11)",
            m.arena_symbol
        ))),
        Some(a) => {
            let aligned = m.arena_align == 0 || a.address % m.arena_align == 0;
            if a.size < m.arena_bytes {
                out.push(finding(format!(
                    "{x}: Arena `{}` hat {} Byte, das Programm braucht {}",
                    m.arena_symbol, a.size, m.arena_bytes
                )));
            } else if !aligned {
                out.push(finding(format!(
                    "{x}: Arena `{}` liegt an {:#x}, nicht an {} Byte ausgerichtet",
                    m.arena_symbol, a.address, m.arena_align
                )));
            } else {
                out.push(exact(format!(
                    "{x}: Arena `{}` {} Byte an {:#x}, Ausrichtung {}",
                    m.arena_symbol, a.size, a.address, m.arena_align
                )));
            }
        }
    }
    let in_arena = |s: &Symbol| arenas.iter().any(|(at, len)| s.address >= *at && s.address < at.saturating_add(*len));
    let stray: Vec<&Symbol> = image
        .symbols
        .iter()
        .filter(|s| matches!(s.kind, 'D' | 'd' | 'B' | 'b' | 'G' | 'g' | 'S' | 's'))
        .filter(|s| s.name != m.arena_symbol && !in_arena(s))
        .filter(|s| takt_owned(&s.name, x, library))
        .collect();
    if stray.is_empty() {
        out.push(exact(format!("{x}: kein beschreibbares Symbol aus Takt ausserhalb der Arenen")));
    }
    for s in stray {
        out.push(finding(format!(
            "{x}: beschreibbar ausserhalb der Arena: `{}` ({} Byte an {:#x})",
            s.name, s.size, s.address
        )));
    }

    // 3. Unter `xip_flash` der Tick-Pfad im RAM.
    if m.xip_flash {
        out.extend(tick_path_in_ram(image, m));
    }
    out
}

/// Gehoert ein Symbol zu Takt: definiert in der Bibliothek, mit dem
/// Praefix, aus einem geteilten Crate oder aus dem Modul der Huelle?
fn takt_owned(name: &str, prefix: &str, library: &BTreeSet<String>) -> bool {
    library.contains(name)
        || name.starts_with(&format!("{prefix}_"))
        || TAKT_CRATES.iter().any(|c| name.contains(c))
        || name.contains(&format!("::{prefix}::"))
}

/// Ein Weg, der nur in eine Panik fuehrt: die Panik selbst, die
/// Fehlerausgaenge, die sie rufen (`unwrap_failed`, `panic_bounds_check`,
/// `slice_index_fail`, in jedem Crate), und die der Kernbibliothek.
fn panic_path(name: &str) -> bool {
    let core = ["core::", "<core::", "alloc::", "std::"].iter().any(|p| name.starts_with(p));
    name.ends_with("rust_begin_unwind")
        || name == "abort"
        || name.contains("panic")
        || ["unwrap_failed", "expect_failed", "assert_failed"].iter().any(|f| name.contains(f))
        || (core && (name.ends_with("_fail") || name.ends_with("_failed")))
}

/// Jede Funktion, die von einem Einstieg des Tick-Pfads aus erreichbar ist,
/// laeuft ohne Flash; der Bericht nennt fuer jede im Flash den Weg dorthin.
fn tick_path_in_ram(image: &Image, m: &Manifest) -> Vec<Line> {
    let x = &m.prefix;
    let Some(graph) = &image.graph else {
        return vec![open(format!("{x}: Tick-Pfad nicht geprueft — kein `objdump` liest das Abbild"))];
    };
    let mut from: BTreeMap<u64, Option<u64>> = BTreeMap::new();
    let mut queue: VecDeque<u64> = VecDeque::new();
    // Die Einstiege nach der Symboltabelle: Gleiche Funktionen legt der
    // Linker zusammen (`--icf`), und die Disassemblierung nennt nur einen Namen.
    let roots =
        image.symbols.iter().filter(|s| m.tick_path.contains(&s.name)).filter_map(|s| graph.containing(s.address));
    for root in roots {
        if from.insert(root, None).is_none() {
            queue.push_back(root);
        }
    }
    while let Some(f) = queue.pop_front() {
        for &callee in &graph.functions[&f].calls {
            if from.contains_key(&callee) || panic_path(&graph.functions[&callee].name) {
                continue;
            }
            from.insert(callee, Some(f));
            queue.push_back(callee);
        }
    }
    let name = |at: u64| graph.functions[&at].name.as_str();
    let way = |f: u64| {
        let mut path = vec![name(f)];
        let mut at = f;
        while let Some(Some(caller)) = from.get(&at) {
            path.push(name(*caller));
            at = *caller;
        }
        path.reverse();
        path.join(" → ")
    };
    let mut out = Vec::new();
    let in_flash: Vec<u64> = from.keys().copied().filter(|f| !runs_without_flash(*f, &image.sections)).collect();
    if in_flash.is_empty() {
        out.push(exact(format!("{x}: Tick-Pfad {} Funktionen, alle im RAM oder ROM", from.len())));
    }
    for f in in_flash {
        out.push(finding(format!("{x}: Tick-Pfad im Flash: `{}` ({})", name(f), way(f))));
    }
    let unknown: Vec<String> = from
        .keys()
        .map(|f| &graph.functions[f])
        .filter(|f| f.indirect || !f.unresolved.is_empty())
        .map(|f| f.name.clone())
        .collect();
    if !unknown.is_empty() {
        let mut text = format!(
            "{x}: {} Funktionen des Tick-Pfads rufen, was der Graph nicht kennt (indirekt oder mehrdeutig):",
            unknown.len()
        );
        for f in unknown.iter().take(4) {
            let _ = write!(text, " `{f}`");
        }
        out.push(open(text));
    }
    out
}

/// Der Bericht: je Zeile ihr Ursprung.
pub(crate) fn render(image: &Path, lines: &[Line]) -> String {
    let mut s = format!("takt check-image {}\n", image.display());
    for l in lines {
        let tag = match l.origin {
            Origin::Exact => "exakt ",
            Origin::Open => "offen ",
            Origin::Finding => "BEFUND",
        };
        let _ = writeln!(s, "  [{tag}] {}", l.text);
    }
    let findings = lines.iter().filter(|l| l.origin == Origin::Finding).count();
    let _ = writeln!(s, "{findings} Befunde");
    s
}

/// Die Bibliothek neben dem Manifest (`libP.a`, `P.lib`).
fn library_of(manifest: &Path, prefix: &str) -> Option<PathBuf> {
    let dir = manifest.parent()?;
    [format!("lib{prefix}.a"), format!("{prefix}.lib")].into_iter().map(|f| dir.join(f)).find(|p| p.is_file())
}

/// `takt check-image ABBILD --manifest P.manifest …`.
pub(crate) fn run(image_path: &Path, manifests: &[&str]) -> bool {
    if manifests.is_empty() {
        eprintln!("takt check-image: mindestens ein `--manifest P.manifest`");
        return false;
    }
    let mut parsed = Vec::new();
    for path in manifests {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{path}: {e}");
                return false;
            }
        };
        match Manifest::parse(&text) {
            Ok(m) => parsed.push((Path::new(*path), m)),
            Err(e) => {
                eprintln!("{path}: {e}");
                return false;
            }
        }
    }
    let tools = takt_llvm::inspect::Binutils::llvm().unwrap_or_else(takt_llvm::inspect::Binutils::host);
    let (Some(symbols), Some(sections)) = (tools.symbols_demangled(image_path), tools.section_ranges(image_path))
    else {
        eprintln!("{}: kein `nm`/`objdump` liest das Abbild", image_path.display());
        return false;
    };
    let image = Image { symbols, sections, graph: tools.call_graph(image_path) };
    let arenas: Vec<(u64, u64)> = parsed
        .iter()
        .filter(|(_, m)| !m.bench)
        .filter_map(|(_, m)| image.symbols.iter().find(|s| s.name == m.arena_symbol))
        .map(|s| (s.address, s.size))
        .collect();
    let mut lines = Vec::new();
    for (path, m) in &parsed {
        let library: BTreeSet<String> = match library_of(path, &m.prefix) {
            Some(lib) => tools.symbols_demangled(&lib).unwrap_or_default().into_iter().map(|s| s.name).collect(),
            None => {
                lines.push(open(format!("{}: keine Bibliothek neben {}", m.prefix, path.display())));
                BTreeSet::new()
            }
        };
        lines.extend(check(&image, m, &library, &arenas));
    }
    print!("{}", render(image_path, &lines));
    lines.iter().all(|l| l.origin != Origin::Finding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, address: u64, size: u64, kind: char) -> Symbol {
        Symbol { name: name.to_string(), address, size, kind }
    }

    fn manifest() -> Manifest {
        Manifest {
            bench: false,
            prefix: "app".into(),
            abi: "4".into(),
            logic_hash: "ab12".into(),
            arena_symbol: "app_arena".into(),
            arena_bytes: 256,
            arena_align: 8,
            xip_flash: true,
            tick_path: vec!["app_tick".into(), "app_commit".into()],
        }
    }

    /// Ein Abbild ohne Befund: RAM ab 0x4080_0000, Flash ab 0x4200_0000, ROM
    /// darunter. `app_tick` ruft den Treiber und eine Routine im ROM, eine
    /// Panik liegt im Flash, und dahin fuehrt nur der Panik-Weg.
    fn clean() -> Image {
        let functions: [(u64, &str, &[u64]); 7] = [
            (0x4080_0100, "app_tick", &[0x4080_0300, 0x4000_0100, 0x4200_2000]),
            (0x4080_0200, "app_commit", &[]),
            (0x4080_0300, "app_in_x", &[0x4080_0400]),
            (0x4080_0400, "<takt::Rig as takt::app::Drivers>::in_x", &[]),
            (0x4000_0100, "__divdi3", &[]),
            (0x4200_2000, "core::panicking::panic_bounds_check", &[0x4200_3000]),
            (0x4200_3000, "core::fmt::write", &[]),
        ];
        let graph = CallGraph {
            functions: functions
                .iter()
                .map(|(at, name, calls)| {
                    (
                        *at,
                        Function {
                            name: name.to_string(),
                            calls: calls.iter().copied().collect(),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
        };
        let mut symbols: Vec<Symbol> = functions
            .iter()
            .map(|(at, name, _)| symbol(name, *at, 16, if *at < 0x4200_0000 { 'T' } else { 't' }))
            .collect();
        symbols.extend([
            symbol("app_abi_4", 0x4200_1000, 1, 'R'),
            symbol("app_logic_ab12", 0x4200_1001, 1, 'R'),
            symbol("app_arena", 0x4080_8000, 256, 'B'),
            symbol("takt::RIG", 0x4080_9000, 8, 'd'),
        ]);
        Image {
            symbols,
            sections: vec![
                SectionRange { name: ".rwtext".into(), start: 0x4080_0000, size: 0x1000 },
                SectionRange { name: ".bss".into(), start: 0x4080_8000, size: 0x2000 },
                SectionRange { name: ".text".into(), start: 0x4200_0000, size: 0x10000 },
            ],
            graph: Some(graph),
        }
    }

    fn findings(lines: &[Line]) -> Vec<&str> {
        lines.iter().filter(|l| l.origin == Origin::Finding).map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_clean_image_has_no_finding() {
        let lines = check(&clean(), &manifest(), &BTreeSet::new(), &[(0x4080_8000, 256)]);
        assert_eq!(findings(&lines), Vec::<&str>::new(), "{lines:#?}");
        assert!(lines.iter().any(|l| l.text.contains("Tick-Pfad 5 Funktionen")), "{lines:#?}");
    }

    /// **Ein Treiber im Flash** (12.3): Der Weg vom Einstieg zu ihm steht
    /// im Befund.
    #[test]
    fn a_driver_in_flash_is_reported_with_its_way() {
        let mut image = clean();
        let graph = image.graph.as_mut().expect("Graph");
        let driver = graph.functions.remove(&0x4080_0400).expect("Treiber");
        graph.functions.insert(0x4200_4000, driver);
        graph.functions.get_mut(&0x4080_0300).expect("Kleber").calls = [0x4200_4000].into();
        let lines = check(&image, &manifest(), &BTreeSet::new(), &[(0x4080_8000, 256)]);
        assert_eq!(
            findings(&lines),
            ["app: Tick-Pfad im Flash: `<takt::Rig as takt::app::Drivers>::in_x` (app_tick → app_in_x → \
              <takt::Rig as takt::app::Drivers>::in_x)"]
        );
    }

    /// **Eine zweite beschreibbare Variable** aus Takt ausserhalb der Arena:
    /// aus der Bibliothek (ein `static` des Rahmens), mit dem Praefix und
    /// aus einem geteilten Crate. Die des Wirts zaehlt nicht.
    #[test]
    fn a_writable_takt_symbol_outside_the_arena_is_reported() {
        let mut image = clean();
        image.symbols.push(symbol("g_frame_state", 0x4080_9100, 4, 'b'));
        image.symbols.push(symbol("app_shadow", 0x4080_9200, 4, 'D'));
        image.symbols.push(symbol("takt_rt_core::loopcore::SCRATCH", 0x4080_9300, 4, 'b'));
        let library: BTreeSet<String> = ["g_frame_state".to_string()].into();
        let lines = check(&image, &manifest(), &library, &[(0x4080_8000, 256)]);
        let found = findings(&lines);
        assert_eq!(found.len(), 3, "{found:#?}");
        assert!(found.iter().all(|f| f.contains("beschreibbar ausserhalb der Arena")), "{found:#?}");
        assert!(!found.iter().any(|f| f.contains("takt::RIG")), "der Pruefstand gehoert dem Wirt");
    }

    /// **Eine zu kleine Arena** und eine schief ausgerichtete.
    #[test]
    fn a_too_small_or_misaligned_arena_is_reported() {
        let mut m = manifest();
        m.arena_bytes = 512;
        let lines = check(&clean(), &m, &BTreeSet::new(), &[(0x4080_8000, 256)]);
        assert_eq!(findings(&lines), ["app: Arena `app_arena` hat 256 Byte, das Programm braucht 512"]);
        let mut m = manifest();
        m.arena_align = 0x10000;
        let lines = check(&clean(), &m, &BTreeSet::new(), &[(0x4080_8000, 256)]);
        assert!(findings(&lines)[0].contains("nicht an 65536 Byte ausgerichtet"), "{lines:#?}");
    }

    /// Ein Abbild mit einem anderen Programm oder ohne Bibliothek.
    #[test]
    fn another_program_or_none_is_reported() {
        let mut m = manifest();
        m.logic_hash = "cd34".into();
        let found: Vec<String> =
            findings(&check(&clean(), &m, &BTreeSet::new(), &[])).iter().map(|s| s.to_string()).collect();
        assert!(found.iter().any(|f| f.contains("ein anderes Programm (Logik-Hash ab12)")), "{found:#?}");
        let mut m = manifest();
        m.abi = "5".into();
        assert!(findings(&check(&clean(), &m, &BTreeSet::new(), &[]))[0].contains("`app_abi_5` fehlt"));
    }

    /// Das Manifest des Messprogramms nennt nur Lage und Einstieg; geprueft
    /// wird der Weg vom Messen aus.
    #[test]
    fn the_bench_manifest_checks_only_the_path() {
        let text = "# takt-bench 1\nprefix = takt_bench\nsuite = 0123\nsteps = 44\ntarget = riscv32imac\n\
                    triple = riscv32imac-unknown-none-elf\nxip_flash = true\ntick_path = takt_bench_measure\n";
        let m = Manifest::parse(text).expect("lesbar");
        assert!(m.bench && m.xip_flash);
        assert_eq!(m.tick_path, ["takt_bench_measure"]);
        let lines = check(&Image::default(), &m, &BTreeSet::new(), &[]);
        assert!(lines.iter().all(|l| !l.text.contains("Arena") && !l.text.contains("ABI")), "{lines:?}");
    }

    /// Ohne `xip_flash` gibt es keinen Tick-Pfad zu pruefen.
    #[test]
    fn without_xip_the_tick_path_is_not_checked() {
        let mut image = clean();
        image.sections.iter_mut().for_each(|s| s.name = ".text".into());
        let mut m = manifest();
        m.xip_flash = false;
        let lines = check(&image, &m, &BTreeSet::new(), &[(0x4080_8000, 256)]);
        assert!(!lines.iter().any(|l| l.text.contains("Tick-Pfad")), "{lines:#?}");
    }

    /// Panik-Wege in jedem Crate; der Tick-Pfad selbst ist keiner.
    #[test]
    fn panic_paths_are_named_by_what_they_do() {
        for name in [
            "core::panicking::panic_bounds_check",
            "core::slice::index::slice_end_index_len_fail",
            "esp_hal::fmt::__unwrap_failed::<esp_hal::fmt::NoneError>",
            "esp_sync::panic_lock_not_reentrant",
            "rust_begin_unwind",
        ] {
            assert!(panic_path(name), "{name}");
        }
        for name in ["app_tick", "takt_edge_settle", "<takt_board_esp32c6::led::Ws2812>::set", "core::fmt::write"] {
            assert!(!panic_path(name), "{name}");
        }
    }

    #[test]
    fn the_manifest_names_what_the_check_needs() {
        let text = "# takt-manifest 1\nprefix = app\nabi = 4\nlogic_hash = ab12\narena_symbol = app_arena\n\
                    arena_bytes = 256\narena_align = 8\nxip_flash = true\ntick_path = app_tick, app_commit\n";
        assert_eq!(Manifest::parse(text), Ok(manifest()));
        assert!(Manifest::parse("prefix = app\n").is_err());
    }
}
