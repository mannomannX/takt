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
//! 4. die Stacks, die der Wirt statisch unter ihren Namen stellt
//!    (`tick_stack_symbol`, `job_stack_symbol`): mindestens so gross wie
//!    `TICK_STACK_BYTES` und `JOB_STACK_BYTES`, der Job-Stack an 32 Byte
//!    ausgerichtet, weil sein Waechter es verlangt. Einen Stack, den der
//!    Linker oder ein RTOS stellt, prueft dessen Bau (12.3).
//!
//! Das Manifest des Messprogramms von `takt bench` (`# takt-bench 1`, 13.8)
//! traegt keine Arena und keinen Logik-Hash: Fuer es gilt Punkt 3, mit
//! `takt_bench_measure` als Einstieg — ein Kern, der aus dem Flash liefe,
//! maesse den Cache mit.
//!
//! 5. mit den Manifesten mehrerer Programme eines Kerns: die Auslastung und
//!    je Programm die Antwortzeit mit festen Prioritaeten
//!    (`takt_mir::analysis::response`, 12.11) aus den Schrittbudgets der
//!    Manifeste, ihren Ticks und der fremden Last, die sie aus der
//!    Hardware-Konfiguration mitbringen.
//!
//! Jede Zeile traegt ihren Ursprung (11.5): `exakt` aus Symboltabelle und
//! Disassemblierung, `gemessen` und `Vertrag` wie die Summanden einer
//! Antwortzeit, `offen`, wo das Abbild oder ein Manifest es nicht sagt.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[cfg(test)]
use takt_llvm::inspect::Function;
use takt_llvm::inspect::{CallGraph, SectionRange, Symbol, runs_without_flash};
use takt_mir::analysis::response;

/// Die Crates, die jedes Programm teilt, mit ihrem Pfad im demangelten Namen.
const TAKT_CRATES: [&str; 7] = [
    "takt_rt_core::",
    "takt_rt_baremetal::",
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
    /// Die Stacks: Name im Abbild und Bedarf in Byte (12.3).
    pub(crate) tick_stack: (String, u64),
    pub(crate) job_stack: (String, u64),
    /// Was die Antwortzeiten brauchen (12.11); vor Schritt 16 nannte ein
    /// Manifest es nicht.
    pub(crate) timing: Timing,
}

/// Was ein Manifest fuer die Antwortzeiten nennt (12.11, 11.5).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Timing {
    /// Der Tick in Nanosekunden.
    pub(crate) tick_ns: u64,
    /// Das Schrittbudget in Pikosekunden; `None` ist offen.
    pub(crate) budget_ps: Option<u64>,
    /// So lange haelt ein Journal-Vorgang den Kern an.
    pub(crate) journal_block_ns: u64,
    /// Die fremde Last: die laengste Sperre mit Herkunft und die Stoerer
    /// (`name:kosten:periode:herkunft`), wie die Hardware-Konfiguration sie
    /// nennt.
    pub(crate) foreign_block: Option<(u64, String)>,
    pub(crate) isrs: Vec<String>,
    /// T_IO in Pikosekunden; `None` ist offen.
    pub(crate) t_io_ps: Option<u64>,
    /// `output_timing = boundary` (12.1).
    pub(crate) boundary: bool,
    /// Die Adressen der Ausgaenge an Treibern.
    pub(crate) outputs: Vec<String>,
    /// Die gekoppelten Eingaenge (`from`, 8.10).
    pub(crate) links: Vec<Link>,
}

/// Ein gekoppelter Eingang und was die Hardware-Konfiguration ueber seinen
/// Schreiber sagt (8.10, 12.11).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) input: String,
    pub(crate) from: String,
    pub(crate) tick_ns: u64,
    pub(crate) boundary: bool,
    /// Bei gleichem Tick rechnet der Schreiber zuerst.
    pub(crate) first: bool,
}

impl Link {
    /// `eingang<-ausgang tick timing zuerst`.
    fn parse(text: &str) -> Option<Link> {
        let (input, rest) = text.split_once("<-")?;
        let words: Vec<&str> = rest.split_whitespace().collect();
        let [from, tick, timing, first] = words.as_slice() else { return None };
        Some(Link {
            input: input.trim().to_string(),
            from: (*from).to_string(),
            tick_ns: tick.parse().ok()?,
            boundary: match *timing {
                "boundary" => true,
                "asap" => false,
                _ => return None,
            },
            first: first.parse().ok()?,
        })
    }
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
        // Vor Schritt 13 nannte ein Manifest keinen Schritt-Stack.
        let stack = |key: &str, default: String| -> Result<(String, u64), String> {
            let symbol = values.get(format!("{key}_symbol").as_str()).map_or(default, |s| s.to_string());
            let bytes = values.get(format!("{key}_bytes").as_str()).map(|v| v.parse::<u64>()).transpose();
            Ok((symbol, bytes.map_err(|e| format!("`{key}_bytes`: {e}"))?.unwrap_or(0)))
        };
        let list = |key: &str| -> Vec<String> {
            values
                .get(key)
                .filter(|v| **v != "none")
                .map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
                .unwrap_or_default()
        };
        let optional = |key: &str| -> Result<Option<u64>, String> {
            match values.get(key) {
                None | Some(&"none") => Ok(None),
                Some(v) => v.parse::<u64>().map(Some).map_err(|e| format!("`{key}`: {e}")),
            }
        };
        let timing = Timing {
            tick_ns: optional("tick_ns")?.unwrap_or(0),
            budget_ps: optional("step_budget_ps")?.filter(|_| values.get("step_budget_origin") == Some(&"gemessen")),
            journal_block_ns: optional("journal_block_ns")?.unwrap_or(0),
            foreign_block: optional("foreign_block_ns")?
                .map(|n| (n, values.get("foreign_block_origin").map_or("Vertrag", |o| *o).to_string())),
            isrs: list("isrs"),
            t_io_ps: optional("t_io_ps")?.filter(|_| values.get("step_budget_origin") == Some(&"gemessen")),
            boundary: values.get("output_timing") == Some(&"boundary"),
            outputs: list("outputs"),
            links: list("links")
                .iter()
                .map(|l| {
                    Link::parse(l)
                        .ok_or_else(|| format!("`links`: `{l}` ist nicht `eingang<-ausgang tick timing zuerst`"))
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(Manifest {
            bench: false,
            timing,
            tick_stack: stack("tick_stack", format!("{prefix}_tick_stack"))?,
            job_stack: stack("job_stack", format!("{prefix}_job_stack"))?,
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
    Measured,
    Contract,
    Open,
    Finding,
}

impl From<response::Origin> for Origin {
    fn from(o: response::Origin) -> Origin {
        match o {
            response::Origin::Exact => Origin::Exact,
            response::Origin::Measured => Origin::Measured,
            response::Origin::Contract => Origin::Contract,
            response::Origin::Open => Origin::Open,
        }
    }
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
        .filter(|s| s.name != m.arena_symbol && s.name != m.tick_stack.0 && s.name != m.job_stack.0 && !in_arena(s))
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

    // 4. Die Stacks, die der Wirt unter ihren Namen stellt.
    for ((symbol, bytes), what, align) in [(&m.tick_stack, "Schritt-Stack", 1), (&m.job_stack, "Job-Stack", 32)] {
        if *bytes == 0 {
            continue;
        }
        out.push(match by_name.get(symbol.as_str()) {
            Some(s) if s.size < *bytes => {
                finding(format!("{x}: {what} `{symbol}` hat {} Byte, das Programm braucht {bytes} (12.3)", s.size))
            }
            Some(s) if s.address % align != 0 => {
                finding(format!("{x}: {what} `{symbol}` liegt an {:#x}, nicht an {align} Byte ausgerichtet", s.address))
            }
            Some(s) => exact(format!("{x}: {what} `{symbol}` {} Byte an {:#x}, gebraucht {bytes}", s.size, s.address)),
            None => open(format!(
                "{x}: {what} ohne Symbol `{symbol}`; den Stack stellt der Linker oder ein RTOS, und dessen Bau prueft \
                 {bytes} Byte"
            )),
        });
    }
    out
}

/// Eine Herkunft der fremden Last aus ihrem Namen (8.10).
fn provenance(name: &str) -> response::Origin {
    if name == "gemessen" { response::Origin::Measured } else { response::Origin::Contract }
}

/// Pikosekunden als Nanosekunden, aufgerundet: eine Schranke bleibt eine.
fn ns(ps: u64) -> u64 {
    ps.div_ceil(1000)
}

/// Punkt 5: Auslastung und Antwortzeiten der Programme eines Kerns (12.11);
/// mit weniger als zwei Programmen nichts.
pub(crate) fn responses(manifests: &[&Manifest]) -> Vec<Line> {
    let programs: Vec<&Manifest> = manifests.iter().copied().filter(|m| !m.bench).collect();
    if programs.len() < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Die fremde Last gehoert dem Kern: Jedes Manifest muss dieselbe nennen.
    let first = &programs[0].timing;
    let differs: Vec<&str> = programs
        .iter()
        .filter(|m| m.timing.foreign_block != first.foreign_block || m.timing.isrs != first.isrs)
        .map(|m| m.prefix.as_str())
        .collect();
    if !differs.is_empty() {
        out.push(finding(format!(
            "fremde Last: {} nennt eine andere als {}; sie gehoert dem Kern, nicht einem Programm (12.11)",
            differs.join(", "),
            programs[0].prefix
        )));
        return out;
    }
    let mut isrs = Vec::new();
    for entry in &first.isrs {
        let parts: Vec<&str> = entry.split(':').collect();
        let parsed = match parts.as_slice() {
            [name, cost, period, origin] => {
                cost.parse::<u64>().ok().zip(period.parse::<u64>().ok()).map(|(c, p)| response::Load {
                    name: (*name).to_string(),
                    cost_ps: c.saturating_mul(1000),
                    period_ps: p.saturating_mul(1000),
                    origin: provenance(origin),
                })
            }
            _ => None,
        };
        match parsed {
            Some(load) => isrs.push(load),
            None => {
                out.push(finding(format!("fremde Last: Stoerer `{entry}` ist nicht `name:kosten:periode:herkunft`")));
                return out;
            }
        }
    }
    // B: das Groessere aus der fremden Sperre und der Sperre eines Journals.
    let journal = programs.iter().map(|m| m.timing.journal_block_ns).max().unwrap_or(0);
    let foreign = first.foreign_block.as_ref().map(|(n, o)| (*n, provenance(o)));
    let blocking = match foreign {
        Some((n, o)) if n >= journal => Some(response::Load {
            name: "fremde Sperre".into(),
            cost_ps: n.saturating_mul(1000),
            period_ps: 0,
            origin: o,
        }),
        _ if journal > 0 => Some(response::Load {
            name: "Journal".into(),
            cost_ps: journal.saturating_mul(1000),
            period_ps: 0,
            origin: response::Origin::Measured,
        }),
        _ => None,
    };
    if first.foreign_block.is_none() && first.isrs.is_empty() {
        out.push(open("fremde Last: keine Angabe in der Hardware-Konfiguration (8.10)".to_string()));
    }
    let tasks: Vec<response::Program> = programs
        .iter()
        .map(|m| response::Program {
            name: m.prefix.clone(),
            period_ps: m.timing.tick_ns.saturating_mul(1000),
            budget_ps: m.timing.budget_ps,
            origin: if m.timing.budget_ps.is_some() { response::Origin::Measured } else { response::Origin::Open },
        })
        .collect();
    match response::utilisation_ppm(&tasks, &isrs) {
        Some(ppm) => {
            let permille = ppm.div_ceil(1000);
            let text = format!("Auslastung {},{} %", permille / 10, permille % 10);
            let origin = tasks
                .iter()
                .map(|t| t.origin)
                .chain(isrs.iter().map(|k| k.origin))
                .fold(response::Origin::Exact, Ord::max);
            out.push(if ppm > 1_000_000 {
                finding(format!("{text}: mehr als der Kern hat (12.11)"))
            } else {
                Line { origin: origin.into(), text }
            });
        }
        None => out.push(open("Auslastung offen: ein Schrittbudget fehlt".to_string())),
    }
    out.extend(couplings(&programs, &tasks, blocking.as_ref(), &isrs));
    for r in response::responses(&tasks, blocking.as_ref(), &isrs) {
        let tick = programs.iter().find(|m| m.prefix == r.name).map_or(0, |m| m.timing.tick_ns);
        out.push(match r.ps {
            None => open(format!(
                "{}: Antwortzeit offen: ein Schrittbudget fehlt (Kalibrierung zum Kostenmodell, 7.2)",
                r.name
            )),
            Some(ps) if !r.fits => finding(format!(
                "{}: Antwortzeit ueber {} ns, ueber dem Tick von {tick} ns (7.3, 12.11)",
                r.name,
                ns(ps)
            )),
            Some(ps) => Line {
                origin: r.origin.into(),
                text: format!("{}: Antwortzeit {} ns bei einem Tick von {tick} ns", r.name, ns(ps)),
            },
        });
    }
    out
}

/// Der groesste gemeinsame Teiler zweier Perioden.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Die Kopplungen zweier Programme (12.11, „An einer gemeinsamen Grenze"):
/// Liest das hoehere einen Ausgang des niedrigeren, braucht das niedrigere
/// `output_timing = boundary`; und in jeder Richtung muss das niedrigere S
/// freigeben und abtasten, bevor die naechste Grenze des hoeheren F kommt:
/// `R_io(S) < ggT(T_S, T_F)`. Rang: der kuerzere Tick, bei gleichem die
/// Reihenfolge der Manifeste.
fn couplings(
    programs: &[&Manifest],
    tasks: &[response::Program],
    blocking: Option<&response::Load>,
    isrs: &[response::Load],
) -> Vec<Line> {
    let mut out = Vec::new();
    let higher = |a: usize, b: usize| (programs[a].timing.tick_ns, a) < (programs[b].timing.tick_ns, b);
    for (reader, m) in programs.iter().enumerate() {
        for link in &m.timing.links {
            let (input, from) = (&link.input, &link.from);
            let Some(writer) = programs.iter().position(|w| w.timing.outputs.contains(from)) else {
                out.push(finding(format!(
                    "{}: `{input}` liest `{from}`, aber kein Manifest nennt diesen Ausgang (12.11)",
                    m.prefix
                )));
                continue;
            };
            let what = format!("{}:{input} <- {}:{from}", m.prefix, programs[writer].prefix);
            if writer == reader {
                out.push(finding(format!("{what}: ein Programm koppelt sich mit sich selbst (12.11)")));
                continue;
            }
            // Was die Konfiguration ueber den Schreiber sagt, prueft der Leser
            // zur Laufzeit; es muss stimmen (12.11).
            let w = &programs[writer].timing;
            let first = higher(writer, reader) && w.tick_ns == m.timing.tick_ns;
            if (link.tick_ns, link.boundary) != (w.tick_ns, w.boundary)
                || (w.tick_ns == m.timing.tick_ns && link.first != first)
            {
                out.push(finding(format!(
                    "{what}: die Hardware-Konfiguration sagt Tick {} ns, `{}`{}, das Manifest von {} Tick {} ns, `{}`{} \
                     (12.11)",
                    link.tick_ns,
                    if link.boundary { "boundary" } else { "asap" },
                    if link.first { ", zuerst" } else { "" },
                    programs[writer].prefix,
                    w.tick_ns,
                    if w.boundary { "boundary" } else { "asap" },
                    if first { ", zuerst" } else { "" },
                )));
                continue;
            }
            let (low, high) = if higher(reader, writer) { (writer, reader) } else { (reader, writer) };
            if low == writer && !programs[writer].timing.boundary {
                out.push(finding(format!(
                    "{what}: das hoehere Programm liest einen Ausgang des niedrigeren, das dafuer \
                     `output_timing = boundary` braucht (12.11)"
                )));
                continue;
            }
            let period = gcd(programs[low].timing.tick_ns, programs[high].timing.tick_ns).saturating_mul(1000);
            let Some(t_io) = programs[low].timing.t_io_ps else {
                out.push(open(format!("{what}: offen, T_IO von {} fehlt (Kalibrierung)", programs[low].prefix)));
                continue;
            };
            let mut probe = tasks.to_vec();
            probe[low].budget_ps = Some(t_io);
            let r = &response::responses(&probe, blocking, isrs)[low];
            out.push(match r.ps {
                None => open(format!("{what}: offen, ein Schrittbudget fehlt")),
                Some(ps) if ps >= period => finding(format!(
                    "{what}: {} gibt erst nach {} ns frei und tastet ab, die naechste Grenze von {} kommt nach {} ns \
                     (12.11)",
                    programs[low].prefix,
                    ns(ps),
                    programs[high].prefix,
                    period / 1000
                )),
                Some(ps) => Line {
                    origin: r.origin.into(),
                    text: format!(
                        "{what}: {} gibt frei und tastet ab nach {} ns, vor der naechsten Grenze von {} nach {} ns",
                        programs[low].prefix,
                        ns(ps),
                        programs[high].prefix,
                        period / 1000
                    ),
                },
            });
        }
    }
    out
}

/// Die Adressbereiche, die der Wirt einem Programm stellt: Arena und, wo er
/// sie unter ihren Namen stellt, die Stacks. Was darin liegt, ist kein
/// beschreibbares Symbol ausserhalb (Punkt 2).
pub(crate) fn regions(image: &Image, manifests: &[&Manifest]) -> Vec<(u64, u64)> {
    let names: Vec<&str> = manifests
        .iter()
        .filter(|m| !m.bench)
        .flat_map(|m| [m.arena_symbol.as_str(), m.tick_stack.0.as_str(), m.job_stack.0.as_str()])
        .collect();
    image.symbols.iter().filter(|s| names.contains(&s.name.as_str())).map(|s| (s.address, s.size)).collect()
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
            Origin::Exact => "exakt   ",
            Origin::Measured => "gemessen",
            Origin::Contract => "Vertrag ",
            Origin::Open => "offen   ",
            Origin::Finding => "BEFUND  ",
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
    let arenas = regions(&image, &parsed.iter().map(|(_, m)| m).collect::<Vec<_>>());
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
    lines.extend(responses(&parsed.iter().map(|(_, m)| m).collect::<Vec<_>>()));
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
            tick_stack: ("app_tick_stack".into(), 0),
            job_stack: ("app_job_stack".into(), 0),
            prefix: "app".into(),
            abi: "4".into(),
            logic_hash: "ab12".into(),
            arena_symbol: "app_arena".into(),
            arena_bytes: 256,
            arena_align: 8,
            xip_flash: true,
            tick_path: vec!["app_tick".into(), "app_commit".into()],
            timing: Timing::default(),
        }
    }

    /// Ein Programm mit Tick und Schrittbudget, ohne fremde Last.
    fn timed(prefix: &str, tick_ns: u64, budget_ns: Option<u64>) -> Manifest {
        Manifest {
            prefix: prefix.into(),
            timing: Timing { tick_ns, budget_ps: budget_ns.map(|n| n * 1000), ..Timing::default() },
            ..manifest()
        }
    }

    fn texts(lines: &[Line]) -> Vec<(Origin, &str)> {
        lines.iter().map(|l| (l.origin, l.text.as_str())).collect()
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

    /// **Punkt 4:** Ein Stack unter seinem Namen ist mindestens so gross wie
    /// gebraucht, der Job-Stack ausgerichtet; ohne Symbol bleibt er offen.
    #[test]
    fn a_named_stack_is_checked_by_size_and_alignment() {
        let mut m = manifest();
        m.tick_stack.1 = 2048;
        m.job_stack.1 = 1056;
        let image = |tick: u64, job_at: u64| Image {
            symbols: vec![symbol("app_tick_stack", 0x2000_1000, tick, 'B'), symbol("app_job_stack", job_at, 1056, 'B')],
            ..Image::default()
        };
        let lines = |i: &Image| check(i, &m, &BTreeSet::new(), &regions(i, &[&m]));
        let fine = lines(&image(2048, 0x2000_2000));
        assert!(
            fine.iter().any(|l| l.origin == Origin::Exact && l.text.contains("Schritt-Stack `app_tick_stack` 2048"))
        );
        assert!(fine.iter().any(|l| l.origin == Origin::Exact && l.text.contains("Job-Stack `app_job_stack`")));
        assert!(!fine.iter().any(|l| l.text.contains("beschreibbar ausserhalb")), "Stacks sind Bereiche des Wirts");
        let small = lines(&image(1024, 0x2000_2000));
        assert!(small.iter().any(|l| l.origin == Origin::Finding && l.text.contains("hat 1024 Byte")), "{small:?}");
        let skewed = lines(&image(2048, 0x2000_2010));
        assert!(
            skewed.iter().any(|l| l.origin == Origin::Finding && l.text.contains("nicht an 32 Byte")),
            "{skewed:?}"
        );
        let none = check(&Image::default(), &m, &BTreeSet::new(), &[]);
        assert!(none.iter().any(|l| l.origin == Origin::Open && l.text.contains("ohne Symbol `app_tick_stack`")));
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

    /// **Das Manifest nennt, was die Antwortzeiten brauchen** (12.11): ein
    /// offenes Budget ist keines, auch wenn eine Zahl dasteht.
    #[test]
    fn the_manifest_names_budget_and_foreign_load() {
        let text = "# takt-manifest 1\nprefix = app\nabi = 6\nlogic_hash = ab12\narena_bytes = 256\n\
                    arena_align = 8\nxip_flash = false\ntick_path = app_tick\ntick_ns = 1000000\n\
                    step_budget_ps = 250000000\nstep_budget_origin = gemessen\njournal_block_ns = 0\n\
                    foreign_block_ns = 2000\nforeign_block_origin = Vertrag\nisrs = uart:1100:10850:gemessen\n";
        let m = Manifest::parse(text).expect("lesbar");
        assert_eq!(
            m.timing,
            Timing {
                tick_ns: 1_000_000,
                budget_ps: Some(250_000_000),
                journal_block_ns: 0,
                foreign_block: Some((2000, "Vertrag".into())),
                isrs: vec!["uart:1100:10850:gemessen".into()],
                ..Timing::default()
            }
        );
        let open = Manifest::parse(&text.replace("origin = gemessen", "origin = offen")).expect("lesbar");
        assert_eq!(open.timing.budget_ps, None);
    }

    /// **Zwei Programme eines Kerns** (12.11): Auslastung und Antwortzeiten
    /// mit ihrer Herkunft; ein einzelnes Programm rechnet nichts.
    #[test]
    fn two_programs_have_utilisation_and_response_times() {
        let (fast, slow) = (timed("fast", 1_000_000, Some(250_000)), timed("slow", 10_000_000, Some(2_000_000)));
        assert!(responses(&[&fast]).is_empty());
        let lines = responses(&[&fast, &slow]);
        assert_eq!(
            texts(&lines),
            [
                (Origin::Open, "fremde Last: keine Angabe in der Hardware-Konfiguration (8.10)"),
                (Origin::Measured, "Auslastung 45,0 %"),
                (Origin::Measured, "fast: Antwortzeit 250000 ns bei einem Tick von 1000000 ns"),
                (Origin::Measured, "slow: Antwortzeit 2750000 ns bei einem Tick von 10000000 ns"),
            ]
        );
    }

    /// Mit angegebener fremder Last ist die Antwortzeit `Vertrag`; ein
    /// Programm ueber seinem Tick und eine Auslastung ueber 1 sind Befunde,
    /// ein offenes Budget laesst die Antwortzeit offen.
    #[test]
    fn foreign_load_overload_and_open_budgets_are_reported() {
        let mut fast = timed("fast", 1_000_000, Some(250_000));
        let mut slow = timed("slow", 10_000_000, Some(2_000_000));
        for m in [&mut fast, &mut slow] {
            m.timing.foreign_block = Some((2000, "Vertrag".into()));
            m.timing.isrs = vec!["uart:1100:10850:gemessen".into()];
        }
        let lines = responses(&[&fast, &slow]);
        assert!(
            lines.iter().filter(|l| l.text.contains("Antwortzeit")).all(|l| l.origin == Origin::Contract),
            "{lines:?}"
        );
        let heavy = timed("slow", 10_000_000, Some(9_000_000));
        let lines = responses(&[&timed("fast", 1_000_000, Some(250_000)), &heavy]);
        assert!(
            lines.iter().any(|l| l.origin == Origin::Finding && l.text.starts_with("Auslastung 115,0 %")),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.origin == Origin::Finding && l.text.starts_with("slow: Antwortzeit ueber")));
        let open = responses(&[&timed("fast", 1_000_000, None), &timed("slow", 10_000_000, Some(2_000_000))]);
        assert!(open.iter().any(|l| l.origin == Origin::Open && l.text.starts_with("slow: Antwortzeit offen")));
        assert!(open.iter().all(|l| l.origin != Origin::Finding));
    }

    /// Die fremde Last gehoert dem Kern: Zwei Manifeste mit verschiedener
    /// sind ein Befund.
    #[test]
    fn different_foreign_loads_are_a_finding() {
        let fast = timed("fast", 1_000_000, Some(250_000));
        let mut slow = timed("slow", 10_000_000, Some(2_000_000));
        slow.timing.foreign_block = Some((5000, "Vertrag".into()));
        let lines = responses(&[&fast, &slow]);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].origin, Origin::Finding);
    }

    /// Das Paar aus Schritt 16: `fast` schreibt `pair/count`, `slow` liest
    /// ihn als `pair/seen` und gibt `pair/last` frei.
    fn pair(fast_budget_ns: u64, slow_t_io_ns: u64) -> (Manifest, Manifest) {
        let mut fast = timed("fast", 1_000_000, Some(fast_budget_ns));
        fast.timing.t_io_ps = Some(20_000_000);
        fast.timing.outputs = vec!["pair/count".into()];
        let mut slow = timed("slow", 10_000_000, Some(2_000_000));
        slow.timing.t_io_ps = Some(slow_t_io_ns * 1000);
        slow.timing.outputs = vec!["pair/last".into()];
        slow.timing.links = vec![link("pair/seen", "pair/count", 1_000_000, false)];
        (fast, slow)
    }

    /// Ein gekoppelter Eingang mit dem, was die Konfiguration ueber seinen
    /// Schreiber sagt.
    fn link(input: &str, from: &str, tick_ns: u64, boundary: bool) -> Link {
        Link { input: input.into(), from: from.into(), tick_ns, boundary, first: false }
    }

    fn coupling(lines: &[Line]) -> Vec<(Origin, &str)> {
        texts(lines).into_iter().filter(|(_, t)| t.contains(" <- ")).collect()
    }

    /// **Das niedrigere Programm gibt frei und tastet ab, bevor die naechste
    /// Grenze des hoeheren kommt** (12.11): `R_io(slow)` = T_IO + ein Schritt
    /// von `fast` = 300 us, unter ggT(10 ms, 1 ms) = 1 ms.
    #[test]
    fn a_coupling_holds_when_the_lower_program_samples_in_time() {
        let (fast, slow) = pair(250_000, 50_000);
        assert_eq!(
            coupling(&responses(&[&fast, &slow])),
            [(
                Origin::Measured,
                "slow:pair/seen <- fast:pair/count: slow gibt frei und tastet ab nach 300000 ns, vor der naechsten \
                 Grenze von fast nach 1000000 ns"
            )]
        );
        // 200 us T_IO, und je Millisekunde 900 us von `fast`: Bis zum Ende des
        // Abtastens vergehen zwei Schritte von `fast`, 2 ms, und das ist zu spaet.
        let (fast, slow) = pair(900_000, 200_000);
        let found = coupling(&responses(&[&fast, &slow]))
            .into_iter()
            .filter(|(o, t)| *o == Origin::Finding && t.contains("erst nach 2000000 ns"))
            .count();
        assert_eq!(found, 1);
    }

    /// Liest das hoehere einen Ausgang des niedrigeren, braucht das niedrigere
    /// `output_timing = boundary` (12.11); ohne ist es ein Befund. Ein
    /// Eingang ohne schreibendes Programm ist ebenso einer.
    #[test]
    fn the_lower_program_publishes_at_its_boundary() {
        let (mut fast, slow) = pair(250_000, 50_000);
        fast.timing.links = vec![link("pair/back", "pair/last", 10_000_000, false)];
        let lines = responses(&[&fast, &slow]);
        assert!(coupling(&lines).iter().any(|(o, t)| *o == Origin::Finding && t.contains("boundary` braucht")));
        let mut slow = slow;
        slow.timing.boundary = true;
        // Die Konfiguration sagt noch `asap`: Das stimmt nicht mit dem Manifest.
        let lines = responses(&[&fast, &slow]);
        assert!(
            coupling(&lines).iter().any(|(o, t)| *o == Origin::Finding && t.contains("Hardware-Konfiguration sagt"))
        );
        fast.timing.links = vec![link("pair/back", "pair/last", 10_000_000, true)];
        let lines = responses(&[&fast, &slow]);
        assert!(coupling(&lines).iter().all(|(o, _)| *o == Origin::Measured), "{lines:?}");
        fast.timing.links = vec![link("pair/back", "pair/nobody", 10_000_000, true)];
        let lines = responses(&[&fast, &slow]);
        assert!(lines.iter().any(|l| l.origin == Origin::Finding && l.text.contains("kein Manifest nennt")));
    }

    /// Das Manifest nennt Zeitverhalten und Kopplungen (12.11).
    #[test]
    fn the_manifest_names_its_couplings() {
        let text = "# takt-manifest 1\nprefix = slow\nabi = 6\nlogic_hash = ab12\narena_bytes = 256\n\
                    arena_align = 8\nxip_flash = false\ntick_path = slow_tick\ntick_ns = 10000000\n\
                    step_budget_ps = 1\nstep_budget_origin = gemessen\nt_io_ps = 5\noutput_timing = boundary\n\
                    outputs = pair/last, pair/step\nlinks = pair/seen<-pair/count 1000000 asap false\n";
        let m = Manifest::parse(text).expect("lesbar");
        assert_eq!(m.timing.t_io_ps, Some(5));
        assert!(m.timing.boundary);
        assert_eq!(m.timing.outputs, ["pair/last", "pair/step"]);
        assert_eq!(m.timing.links, [link("pair/seen", "pair/count", 1_000_000, false)]);
        assert!(Manifest::parse(&text.replace("pair/seen<-", "pair/seen=")).is_err());
        assert!(Manifest::parse(&text.replace(" asap false", " sofort false")).is_err());
    }
}
