//! `takt bench` (13.8): die Kostentabelle eines Ziels, gemessen.
//!
//! **Die Methode** (plan/m10.md 2.2, plan/m5.md 2.4). Je Klasse zwei Kerne,
//! die sich nur in der Zahl der Operationen dieser Klasse unterscheiden;
//! die Kostenanalyse des Compilers bestaetigt, dass der Unterschied rein
//! ist. Das Board misst beide mit dem Zyklenzaehler, und das Gewicht ist
//! der Zeitunterschied je Operation — fester Aufwand um die Operationen
//! herum (Rahmen, Laden und Speichern der Variablen) faellt dabei heraus.
//! Klassen, deren Kerne Hilfsoperationen anderer Klassen brauchen
//! (Speicherzugriffe brauchen einen Index), kommen nach jenen dran und
//! ziehen deren Anteil ab. Gerechnet wird ganzzahlig in Pikosekunden und
//! immer aufgerundet.
//!
//! **Eine Schranke, kein Mittelwert.** Jede Messreihe traegt ihr Maximum
//! bei, und am Ende muss die Tabelle jeden gemessenen Kern decken:
//! `Σ N_c · c_target[c] + T_IO ≥ gemessen`. Deckt sie einen nicht, wird
//! die ganze Tabelle so weit gestreckt, bis sie es tut — was dann im
//! Bericht steht, mit dem Faktor.
//!
//! **`T_IO`** (7.2) ist der Tick eines leeren Programms: Rahmen, Abtastung,
//! Commit, ohne Schrittfunktion von Belang und ohne Telemetrie — die
//! gehoert nach 7.2 nicht dazu.
//!
//! **Die Referenzkerne** (13.8: PID, Thermoelemente, Zeilenparsing, Matrix,
//! CRC32, Sequenzschritt) laufen als Takt-Programm und als C-Referenz im
//! selben Messprogramm. Sie liefern das Verhaeltnis Takt/C und pruefen die
//! Tabelle an Code, wie er wirklich geschrieben wird: Auch sie muessen
//! unter ihrer Schranke liegen, sonst streckt die Tabelle.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use takt_mir::fns::{CostClass, CostVec, Heavy};
use takt_mir::hardware::CTarget;

use crate::board::{Board, Options};

/// Was ein Klassenkern misst.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Gewoehnliche Operationen einer Klasse.
    Ops(CostClass),
    /// Operationen eigenen Gewichts (7.2): Division, `fma`, `sqrt` und die
    /// korrekt gerundete Mathematik in einer Zahlklasse, der Hook der
    /// Runtime unter den Aufrufen.
    Heavy(Heavy, CostClass),
}

impl Probe {
    /// Die Reihenfolge der Kalibrierung: Jede Probe braucht nur Klassen, die
    /// vor ihr stehen.
    pub const ORDER: [Probe; 17] = [
        Probe::Ops(CostClass::I32),
        Probe::Ops(CostClass::I64),
        Probe::Ops(CostClass::F32),
        Probe::Ops(CostClass::F64),
        Probe::Ops(CostClass::Mem),
        Probe::Ops(CostClass::Call),
        Probe::Heavy(Heavy::Hook, CostClass::Call),
        Probe::Heavy(Heavy::Div, CostClass::I32),
        Probe::Heavy(Heavy::Div, CostClass::I64),
        Probe::Heavy(Heavy::Div, CostClass::F32),
        Probe::Heavy(Heavy::Div, CostClass::F64),
        Probe::Heavy(Heavy::Fma, CostClass::F32),
        Probe::Heavy(Heavy::Fma, CostClass::F64),
        Probe::Heavy(Heavy::Sqrt, CostClass::F32),
        Probe::Heavy(Heavy::Sqrt, CostClass::F64),
        Probe::Heavy(Heavy::Math, CostClass::F32),
        Probe::Heavy(Heavy::Math, CostClass::F64),
    ];

    /// Der Name wie in der Hardware-Konfiguration (`i32`, `i32_div`, `f32_fma`).
    pub fn name(self) -> String {
        match self {
            Probe::Ops(c) => c.name().to_string(),
            Probe::Heavy(h, c) => takt_mir::hardware::heavy_key(h, c).unwrap_or_else(|| h.suffix().to_string()),
        }
    }

    /// Eine der Operationen, die die Probe misst.
    fn one(self) -> CostVec {
        match self {
            Probe::Ops(c) => CostVec::op(c),
            Probe::Heavy(h, c) => CostVec::heavy_op(h, c),
        }
    }

    /// Wie viele der Operationen eines Vektors die Probe misst.
    pub fn count(self, v: CostVec) -> u64 {
        match self {
            Probe::Ops(c) => v.ordinary(c),
            Probe::Heavy(h, c) => v.heavy(h, c),
        }
    }
}

/// Paare je Kern: klein und gross. Der Unterschied traegt die Messung;
/// gross genug, dass ein Zyklus Messfehler nichts mehr bedeutet.
const PAIRS: (u32, u32) = (8, 40);

/// Die Referenzkerne aus 13.8, je ein Takt-Programm und seine C-Referenz
/// gleicher Semantik samt Pruefungen unter `bench/`.
pub const KERNELS: [&str; 6] = ["pid", "thermocouples", "lines", "matrix", "crc32", "sequence"];

/// Ein Referenzkern: `takt` fuer das Programm, `c` fuer die Referenz.
pub fn kernel_path(name: &str, extension: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("bench").join(format!("{name}.{extension}"))
}

/// Die beiden Kerne einer Probe, klein und gross.
///
/// Meist dieselbe Anweisungsfolge mit [`PAIRS`] Paaren. Die Probe `call`
/// braucht eine andere Form, siehe [`call_kernel`].
pub fn probe_kernels(probe: Probe) -> [String; 2] {
    match probe {
        Probe::Ops(CostClass::Call | CostClass::Native) => [call_kernel(false), call_kernel(true)],
        _ => [class_kernel(probe, PAIRS.0), class_kernel(probe, PAIRS.1)],
    }
}

/// Die Funktion, die die Probe `math` misst, solange das Board keine
/// teurere gemeldet hat.
pub const MATH_DEFAULT: &str = "pow";

/// Die beiden Kerne der Probe `math` mit der Funktion `fun` (FB-344).
///
/// **Die teuerste Funktion, nicht eine feste.** Ein Gewicht gilt fuer alle
/// zehn, also muss es an der teuersten gemessen sein — und welche das ist,
/// haengt am Kern: In der ersten Messung (2026-10-01) war es auf dem F401
/// `acos` in beiden Breiten, auf dem C6 `asin` in `f64` und `pow` in `f32`.
/// `run` laesst darum zuerst das Messprogramm `natives` die Zyklen jedes
/// Einstiegs zaehlen und misst dann die teuerste Funktion im erzeugten Code,
/// mit ihrer Pruefung des Definitionsbereichs.
pub fn math_kernels(fun: &str, class: CostClass) -> [String; 2] {
    let probe = Probe::Heavy(Heavy::Math, class);
    [kernel(probe, PAIRS.0, fun), kernel(probe, PAIRS.1, fun)]
}

/// Ein Paar der Probe `math`: jede Zeile ein Aufruf auf dem langsamen Weg
/// der Funktion — `sin`, `cos` und `tan` mit Reduktion nach Payne und Hanek,
/// `atan` ueber den Kehrwert und mit Tabelle, `pow` mit gebrochenem
/// Exponenten — und ein Fixpunkt im Definitionsbereich.
fn math_pair(fun: &str) -> &'static str {
    match fun {
        "exp" => "            a = exp(b * 0.5 - 1.0)\n            b = exp(a * 0.5 - 1.0)\n",
        "log" => "            a = ln(b + 2.0)\n            b = ln(a + 2.0)\n",
        "sin" => "            a = sin(b + 1.0e10)\n            b = sin(a + 1.0e10)\n",
        "cos" => "            a = cos(b + 1.0e10)\n            b = cos(a + 1.0e10)\n",
        "tan" => "            a = tan(b + 1.0e10) * 0.001\n            b = tan(a + 1.0e10) * 0.001\n",
        "asin" => "            a = asin(b * 0.2 + 0.2)\n            b = asin(a * 0.2 + 0.2)\n",
        "acos" => "            a = acos(b * 0.25)\n            b = acos(a * 0.25)\n",
        "atan" => "            a = atan(b * 0.25 + 1.5)\n            b = atan(a * 0.25 + 1.5)\n",
        "atan2" => "            a = atan2(b + 0.5, b - 2.0)\n            b = atan2(a + 0.5, a - 2.0)\n",
        _ => "            a = pow(b + 0.5, 0.75)\n            b = pow(a + 0.25, 0.75)\n",
    }
}

/// Der Quelltext eines Klassenkerns mit `pairs` Paaren von Anweisungen.
///
/// Jeder Kern haelt zwei Variablen, deren jede aus der anderen entsteht:
/// Nichts davon kann der Compiler vorausrechnen oder zusammenfassen.
/// Integer bleiben in beweisbaren Ranges (keine Pruefung, feste
/// Darstellung), Fliesskomma nahe einem Fixpunkt fern von null (keine
/// Subnormalen, kein Ueberlauf).
///
/// **`i64` rechnet mit 40 Bit.** Eine Maske auf 32 Bit liesse LLVM die
/// Rechnung in 32 Bit fuehren, und die Probe maesse `i32` unter anderem
/// Namen; das hat der erste Lauf auf Board 1 gezeigt (FB-287).
///
/// **`i32` verschmilzt nicht.** `(a * 181 + b) & 65535` wurde auf dem F401
/// ein einziger `MLA`: Multiplikation und Addition in einem Befehl, und die
/// Maske fiel weg, weil die naechste Zeile nur die unteren Bits las. Die
/// Probe mass ein Drittel Zyklus je gezaehlter Operation, und die Tabelle
/// musste um 2,91 gestreckt werden, sobald Code die Maske brauchte
/// (FB-291). Jetzt schiebt jede Zeile um einen Betrag aus einem Register:
/// Thumb-2 kennt dafuer keinen verschmolzenen Operanden, und der Schub
/// liest die oberen Bits, also bleibt die Maske stehen — fuenf
/// Operationen, fuenf Befehle.
pub fn class_kernel(probe: Probe, pairs: u32) -> String {
    kernel(probe, pairs, MATH_DEFAULT)
}

/// [`class_kernel`] mit der Funktion `math` fuer die Probe `math`.
fn kernel(probe: Probe, pairs: u32, math: &str) -> String {
    let float = match probe {
        Probe::Ops(CostClass::F32) | Probe::Heavy(_, CostClass::F32) => "    float    = f32\n",
        _ => "",
    };
    let (decl, pair, prelude, init) = match probe {
        Probe::Ops(CostClass::I32) => (
            "int in 0..65535",
            "            a = ((a * 181) ^ (b >> (a & 7))) & 65535\n            b = ((b * 157) ^ (a >> (b & 7))) & 65535\n",
            "",
            ("1", "7"),
        ),
        Probe::Ops(CostClass::I64) => (
            "int in 0..1099511627775",
            "            a = ((a >> 7) * 12345 + b) & 1099511627775\n            b = ((b >> 5) * 5431 + a) & 1099511627775\n",
            "",
            ("123456789012", "987654321098"),
        ),
        Probe::Ops(CostClass::F32 | CostClass::F64) => {
            ("float", "            a = a * 0.75 + b * 0.25\n            b = b * 0.5 + a * 0.5\n", "", ("1.0", "3.0"))
        }
        Probe::Ops(CostClass::Mem) => (
            "int in 0..65535",
            "            a = t[(a + b) & 15]\n            b = t[(b + a) & 15]\n",
            "    var t : [16] int in 0..65535 = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3]\n",
            ("1", "7"),
        ),
        Probe::Ops(CostClass::Call | CostClass::Native) => return call_kernel(true),
        Probe::Heavy(Heavy::Div, CostClass::I32) => (
            "int in 0..65535",
            "            a = ((b * 16384 + 12345) / ((a & 255) + 1)) & 65535\n            b = ((a * 16384 + 54321) / ((b & 255) + 1)) & 65535\n",
            "",
            ("1", "7"),
        ),
        Probe::Heavy(Heavy::Div, CostClass::I64) => (
            "int in 0..4294967295",
            "            a = ((b * 1073741824 + 12345) / ((a & 255) + 1)) & 4294967295\n            b = ((a * 1073741824 + 54321) / ((b & 255) + 1)) & 4294967295\n",
            "",
            ("1", "7"),
        ),
        Probe::Heavy(Heavy::Div, _) => (
            "float",
            "            a = (b + 1.5) / (a + 1.25)\n            b = (a + 2.5) / (b + 0.75)\n",
            "",
            ("1.0", "3.0"),
        ),
        // Reine `fma`-Ketten mit dem Fixpunkt 1,2.
        Probe::Heavy(Heavy::Fma, _) => {
            ("float", "            a = fma(b, 0.75, 0.3)\n            b = fma(a, 0.75, 0.3)\n", "", ("1.0", "3.0"))
        }
        Probe::Heavy(Heavy::Sqrt, _) => {
            ("float", "            a = sqrt(b + 1.25)\n            b = sqrt(a + 2.5)\n", "", ("1.0", "3.0"))
        }
        // Die Mathematik misst ihre teuerste Funktion (`math_kernels`).
        Probe::Heavy(Heavy::Math, _) => ("float", math_pair(math), "", ("1.0", "3.0")),
        // Jeder Alert kippt in jedem Tick: Die Runtime sieht eine Flanke und
        // meldet sie, der teuerste Weg des Hooks (5.6).
        Probe::Heavy(Heavy::Hook, _) => (
            "int in 0..65535",
            "            alert n == 1, \"bench\"\n            alert n == 0, \"bench\"\n",
            "    var n : int in 0..1 = 0\n",
            ("1", "7"),
        ),
    };
    let mut s = kernel_head(&format!("{} ({pairs} Paare)", probe.name()), float, decl, "", prelude, init);
    if probe == Probe::Heavy(Heavy::Hook, CostClass::Call) {
        s.push_str("            n = 1 - n\n");
    }
    for _ in 0..pairs {
        s.push_str(pair);
    }
    s.push_str("            digest = a\n");
    s
}

/// Paare der Probe `call`.
const CALL_PAIRS: u32 = 20;

/// Zeilen im Rumpf von `one`; `two` hat doppelt so viele.
const CALL_BODY: usize = 10;

/// Die Probe `call`: gleiche Rechenarbeit, verschieden viele Aufrufe.
///
/// **Warum nicht wie die anderen Proben.** Der erste Lauf auf Board 1 zog
/// Aufrufe einer kleinen Funktion mit Schleife heran; das Gewicht kam bei
/// null heraus, weil die Analyse Schleifenzaehler und Masken zaehlt, die
/// der Compiler wegfaltet, und der Fehler der Rumpfoperationen den Aufruf
/// verdeckte (FB-287). Hier rufen beide Kerne dieselben Rumpfzeilen auf:
/// `two` ist `one` zweimal hintereinander, der kleine Kern ruft je Paar
/// zweimal `two`, der grosse viermal `one`. Die Rumpfarbeit ist gleich,
/// der Unterschied sind genau die Aufrufe. Die Ruempfe sind so gross,
/// dass `-Os` sie nicht einbettet.
pub fn call_kernel(fine: bool) -> String {
    let line = "    r = (r * 181 + y) & 65535\n";
    let function = |name: &str, lines: usize| {
        format!(
            "fn {name}(x: int in 0..65535, y: int in 0..65535) -> int in 0..65535:\n    var r : int in 0..65535 = x\n{}    \
             return r\n\n",
            line.repeat(lines)
        )
    };
    let functions = function("one", CALL_BODY) + &function("two", 2 * CALL_BODY);
    let pair = if fine {
        "            a = one(a, b)\n            a = one(a, b)\n            b = one(b, a)\n            b = one(b, a)\n"
    } else {
        "            a = two(a, b)\n            b = two(b, a)\n"
    };
    let label = format!("call ({CALL_PAIRS} Paare, {})", if fine { "je Paar vier Aufrufe" } else { "je Paar zwei" });
    let mut s = kernel_head(&label, "", "int in 0..65535", &functions, "", ("1", "7"));
    for _ in 0..CALL_PAIRS {
        s.push_str(pair);
    }
    s.push_str("            digest = a\n");
    s
}

/// Kopf eines Klassenkerns bis zum `loop:` der Maschine.
fn kernel_head(label: &str, float: &str, decl: &str, functions: &str, prelude: &str, init: (&str, &str)) -> String {
    format!(
        "# takt bench: {label}\nsystem:\n    language = 1\n    tick     = 10 ms\n{float}\n\
         output digest : {decl} @ hw(\"bench/digest\") with safe = {}\n\n{functions}machine k:\n{prelude}    \
         var a : {decl} = {}\n    var b : {decl} = {}\n    initial RUN\n    state RUN:\n        loop:\n",
        if decl == "float" { "0.0" } else { "0" },
        init.0,
        init.1
    )
}

/// Der leere Kern: ein Tick ohne Arbeit, fuer `T_IO` und die Stack-Reserve.
pub fn frame_kernel() -> String {
    "# takt bench: Rahmen\nsystem:\n    language = 1\n    tick     = 10 ms\n\noutput digest : int in 0..1 @ \
     hw(\"bench/digest\") with safe = 0\n\nmachine k:\n    initial RUN\n    state RUN:\n        loop:\n            \
     digest = 1\n"
        .to_string()
}

/// Eine Messreihe in Zyklen, wie das Board sie meldet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Series {
    /// Kuerzeste Messung.
    pub min: u64,
    /// Mittelwert.
    pub mean: u64,
    /// Laengste Messung: die Schranke.
    pub max: u64,
    /// Zahl der Messungen.
    pub n: u64,
    /// Was der Kern nach allen Laeufen ergab.
    pub digest: u64,
}

/// Was das Messprogramm meldet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// Kerntakt in Hertz.
    pub core_hz: u32,
    /// Der Takt-Kern.
    pub takt: Series,
    /// Die C-Referenz, wenn gebunden.
    pub c: Option<Series>,
    /// Stack-Tiefe in Byte.
    pub stack: u64,
    /// Faelle des Subnormal-Vektors mit falschem Ergebnis (4.2).
    pub subnormal: u64,
}

/// Liest die `bench`-Zeilen eines Laufs (`takt_rt_baremetal::bench`).
pub fn parse(text: &str) -> Result<Measured, String> {
    let mut m = Measured::default();
    let (mut takt, mut core) = (false, false);
    for line in text.lines().filter_map(|l| l.trim().strip_prefix("bench ")) {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("core_hz") => {
                m.core_hz = number(w.next(), line)? as u32;
                core = true;
            }
            Some("stack") => m.stack = number(w.next(), line)?,
            Some("subnormal") => m.subnormal = number(w.next(), line)?,
            Some(kind @ ("takt" | "c")) => {
                let s = series(w, line)?;
                if kind == "takt" {
                    m.takt = s;
                    takt = true;
                } else {
                    m.c = Some(s);
                }
            }
            _ => return Err(format!("unbekannte Zeile `bench {line}`")),
        }
    }
    if !core || !takt {
        return Err(format!("keine vollstaendige Messung (Kerntakt und `takt`-Reihe fehlen):\n{text}"));
    }
    Ok(m)
}

fn number(word: Option<&str>, line: &str) -> Result<u64, String> {
    word.and_then(|w| w.parse().ok()).ok_or_else(|| format!("keine Zahl in `bench {line}`"))
}

fn series<'a>(words: impl Iterator<Item = &'a str>, line: &str) -> Result<Series, String> {
    let words: Vec<&str> = words.collect();
    let value = |key: &str| {
        words
            .iter()
            .position(|w| *w == key)
            .and_then(|i| words.get(i + 1))
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| format!("`{key}` fehlt in `bench {line}`"))
    };
    Ok(Series {
        min: value("min")?,
        mean: value("mean")?,
        max: value("max")?,
        n: value("n")?,
        digest: value("digest")?,
    })
}

/// Pikosekunden einer Zyklenzahl, aufgerundet.
pub fn ps(cycles: u64, core_hz: u32) -> u64 {
    let hz = u128::from(core_hz.max(1));
    u64::try_from((u128::from(cycles) * 1_000_000_000_000).div_ceil(hz)).unwrap_or(u64::MAX)
}

/// Die Kosten eines Kerns: `B_m + F_m` ueber alle Maschinen, nach der
/// Analyse des Compilers (9.4.3) — ein Tick, in dem jede Maschine ihren
/// teuersten Fall nimmt.
pub fn cost_of(source: &str) -> Result<CostVec, String> {
    analysis_of(source).map(|(cost, _)| cost)
}

/// Kosten wie [`cost_of`] und die Zahl der impliziten Pruefungen, die im
/// Kern blieben (3.4).
pub fn analysis_of(source: &str) -> Result<(CostVec, u32), String> {
    let options = takt_sema::Options {
        policy: takt_diag::Policy::default(),
        build: takt_sema::Build::Hw,
        profile: None,
        ..Default::default()
    };
    let out = takt_sema::compile(source, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let p = out.program.ok_or("kein Programm")?;
    let report = takt_mir::analysis::budget::report(&p);
    let cost = report.machines.iter().fold(CostVec::default(), |acc, m| acc + m.activation + m.fault_path);
    Ok((cost, out.report.total_checks()))
}

/// Ein Referenzkern aus 13.8 mit seiner C-Referenz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KernelRow {
    /// Name, wie in [`KERNELS`].
    pub name: String,
    /// Der Takt-Kern.
    pub takt: Series,
    /// Die C-Referenz.
    pub c: Series,
    /// Implizite Pruefungen, die im Takt-Kern blieben (3.4).
    pub implicit_checks: u32,
}

impl KernelRow {
    /// Rechneten beide dasselbe? Nur dann sagt das Verhaeltnis etwas ueber
    /// die Sprache (grammar/conformance-report.md, R3).
    pub fn same_digest(&self) -> bool {
        self.takt.digest == self.c.digest
    }
}

/// Eine Zeile der Tabelle mit ihrer Herkunft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRow {
    /// Die Probe.
    pub probe: Probe,
    /// Unterschied der Kostenvektoren zwischen grossem und kleinem Kern.
    pub delta: CostVec,
    /// Die beiden Messreihen, klein und gross.
    pub small: Series,
    /// Siehe `small`.
    pub large: Series,
    /// Das Gewicht in Pikosekunden, vor einer Streckung; bei der Mathematik
    /// mindestens ihr teuerster Einstieg ([`Calibration::cover_math`]).
    pub ps: u64,
}

/// Ein Kern, gegen den die Tabelle geprueft wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Name des Kerns.
    pub name: String,
    /// Gemessen, Pikosekunden (Maximum).
    pub measured_ps: u64,
    /// Die Schranke der gestreckten Tabelle: `Σ N_c · c_target[c] + T_IO`.
    pub bound_ps: u64,
}

/// Eine Kalibrierung: Tabelle, `T_IO` und Herkunft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Calibration {
    /// Kerntakt in Hertz.
    pub core_hz: u32,
    /// Die Tabelle, nach einer Streckung.
    pub c_target: CTarget,
    /// `T_IO` in Pikosekunden.
    pub t_io_ps: u64,
    /// Die Streckung als Bruch `(Zaehler, Nenner)`; `(1, 1)` ohne.
    pub stretch: (u64, u64),
    /// Je Probe die Herkunft ihres Gewichts.
    pub probes: Vec<ProbeRow>,
    /// Je gemessenem Kern, ob die Tabelle ihn deckt.
    pub checks: Vec<Check>,
}

/// Die Tabelle aus den Messungen der Proben (Reihenfolge [`Probe::ORDER`]).
///
/// `frame` ist der leere Kern: sein Maximum ist `T_IO`. Jede Probe traegt
/// die Kostenvektoren ihrer beiden Kerne und deren Messungen. Die Proben
/// muessen rein sein: Der Unterschied ihrer Vektoren darf nur Klassen
/// enthalten, die vorher bestimmt wurden, und die eigene. `references`
/// sind weitere gemessene Kerne mit ihren Kosten — die Referenzkerne aus
/// 13.8 —, gegen die die Tabelle ebenso geprueft und notfalls gestreckt
/// wird.
pub fn calibrate(
    core_hz: u32,
    frame: &Series,
    probes: &[(Probe, [CostVec; 2], [Series; 2])],
    references: &[(String, CostVec, Series)],
) -> Result<Calibration, String> {
    let mut table = CTarget::default();
    let mut known: Vec<Probe> = Vec::new();
    let mut rows = Vec::new();
    for (probe, [small_cost, large_cost], [small, large]) in probes {
        let delta = difference(*large_cost, *small_cost)?;
        let count = probe.count(delta);
        if count == 0 {
            return Err(format!(
                "Probe {}: der Unterschied der Kerne enthaelt keine Operation der Klasse",
                probe.name()
            ));
        }
        // Rein: Jede Komponente des Unterschieds hat ein Gewicht — aus einer
        // frueheren Probe oder aus dieser.
        let weighed = |p: Probe| known.contains(&p) || *probe == p;
        for c in CostClass::ALL {
            let ops = delta.ordinary(c);
            if ops > 0 && !weighed(Probe::Ops(c)) {
                return Err(format!(
                    "Probe {}: {} Operationen der Klasse {} ohne Gewicht",
                    probe.name(),
                    ops,
                    c.name()
                ));
            }
            for h in Heavy::ALL {
                if delta.heavy(h, c) > 0 && !weighed(Probe::Heavy(h, c)) {
                    return Err(format!("Probe {}: `{}` in {} ohne Gewicht", probe.name(), h.suffix(), c.name()));
                }
            }
        }
        let elapsed = ps(large.max.saturating_sub(small.max), core_hz);
        // Der Anteil der bestimmten Klassen, ohne die gemessene.
        let rest = delta.zip(probe.one().times(count), u64::saturating_sub);
        let known_ps = table.duration_ps(rest);
        let weight = elapsed.saturating_sub(known_ps).div_ceil(count).max(1);
        match probe {
            Probe::Ops(c) => table.set(*c, weight),
            Probe::Heavy(h, c) => table.set_heavy(*h, *c, weight),
        }
        known.push(*probe);
        rows.push(ProbeRow { probe: *probe, delta, small: *small, large: *large, ps: weight });
    }

    let t_io_ps = ps(frame.max, core_hz);
    // Jeder gemessene Kern muss unter der Schranke liegen; sonst wird
    // gestreckt, um den groessten noetigen Faktor.
    let kernels: Vec<(String, CostVec, u64)> = probes
        .iter()
        .flat_map(|(p, costs, series)| {
            [0, 1].map(|i| (format!("{} {}", p.name(), ["klein", "gross"][i]), costs[i], ps(series[i].max, core_hz)))
        })
        .chain(references.iter().map(|(name, cost, series)| (name.clone(), *cost, ps(series.max, core_hz))))
        .collect();
    let stretch = stretch_for(&table, t_io_ps, &kernels);
    let table = stretched(&table, stretch);
    let checks = kernels
        .into_iter()
        .map(|(name, cost, measured_ps)| Check { name, measured_ps, bound_ps: table.duration_ps(cost) + t_io_ps })
        .collect();
    Ok(Calibration { core_hz, c_target: table, t_io_ps, stretch, probes: rows, checks })
}

/// Der Faktor, um den `table` gestreckt werden muss, damit jeder Kern
/// unter seiner Schranke liegt: das groesste `(gemessen - T_IO) / Σ`.
pub fn stretch_for(table: &CTarget, t_io_ps: u64, kernels: &[(String, CostVec, u64)]) -> (u64, u64) {
    let mut worst = (1u64, 1u64);
    for (_, cost, measured) in kernels {
        let bound = table.duration_ps(*cost).max(1);
        let need = measured.saturating_sub(t_io_ps);
        // need / bound > worst.0 / worst.1 ?
        if u128::from(need) * u128::from(worst.1) > u128::from(worst.0) * u128::from(bound) {
            worst = (need, bound);
        }
    }
    worst
}

/// Die Tabelle, um `num/den` gestreckt, jedes Gewicht aufgerundet.
pub fn stretched(table: &CTarget, (num, den): (u64, u64)) -> CTarget {
    let scale = |ps: u64| {
        u64::try_from((u128::from(ps) * u128::from(num)).div_ceil(u128::from(den.max(1)))).unwrap_or(u64::MAX)
    };
    let mut out = CTarget::default();
    for c in CostClass::ALL {
        out.set(c, scale(table.of(c)));
        for h in Heavy::ALL {
            if let Some(ps) = table.measured(h, c) {
                out.set_heavy(h, c, scale(ps));
            }
        }
    }
    out
}

/// Der Unterschied zweier Kostenvektoren, komponentenweise.
fn difference(a: CostVec, b: CostVec) -> Result<CostVec, String> {
    if a.max(b) != a {
        return Err(format!(
            "der grosse Kern hat nicht in jeder Klasse mehr Operationen als der kleine: {a:?} gegen {b:?}"
        ));
    }
    Ok(a.zip(b, u64::saturating_sub))
}

/// Wo die erzeugten Kerne liegen: ein fester Ort im Zielverzeichnis,
/// damit der Zwischenspeicher der Abbilder sie wiedererkennt.
fn kernel_dir() -> PathBuf {
    crate::target_dir().join("tmp").join("takt-bench-kernels")
}

/// Schreibt einen Kern und liefert seinen Pfad.
fn write_kernel(name: &str, source: &str) -> Result<PathBuf, String> {
    let dir = kernel_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(format!("{name}.takt"));
    std::fs::write(&path, source).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Baut, schreibt und misst einen Kern auf dem Board.
fn measure(board: &mut dyn Board, program: &Path, reference: Option<PathBuf>, runs: u64) -> Result<Measured, String> {
    let options = Options::bench(runs, reference);
    let elf = board.build(program, &options)?;
    parse(&board.run(&elf, &options)?)
}

/// Ticks des Laufs in der Tickschleife: zehn Sekunden bei 10 ms.
const LOOP_TICKS: u64 = 1000;

/// Was `takt bench` auf einem Board ergibt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Die Tabelle mit ihrer Herkunft.
    pub calibration: Calibration,
    /// Die Referenzkerne aus 13.8 mit ihren C-Referenzen.
    pub kernels: Vec<KernelRow>,
    /// Die kuratierten Natives: Ergebnisse gegen den Wirt, Stack gegen die
    /// Zusage (13.8).
    pub natives: Vec<crate::natives::Row>,
    /// Die korrekt gerundete Mathematik: Ergebnisse gegen die Norm, Stack
    /// und Zyklen je Funktion (4.2).
    pub math: Vec<crate::math::Row>,
    /// Der leere Kern im Messprogramm: Subnormal-Vektor und Kerntakt.
    pub frame: Measured,
    /// Messungen je Kern.
    pub runs: u64,
    /// Stack-Tiefe des leeren Programms in der Tickschleife unter Last: die
    /// Reserve fuer Runtime, Treiber und ISRs (12.3).
    pub stack_reserve: Option<u64>,
    /// Um wie viel der Abstand zweier Tickbeginne die Periode hoechstens
    /// ueberschreitet (7.3).
    pub tick_jitter_ns: Option<i64>,
}

/// Misst alle Proben und die Referenzkerne auf dem Board, rechnet die
/// Tabelle und laesst das leere Programm in der Tickschleife laufen.
///
/// `log` bekommt je Kern eine Zeile, damit ein langer Lauf zeigt, wo er
/// steht.
pub fn run(board: &mut dyn Board, runs: u64, mut log: impl FnMut(&str)) -> Result<Outcome, String> {
    let frame_source = frame_kernel();
    let frame_path = write_kernel("frame", &frame_source)?;
    let frame = measure(board, &frame_path, None, runs)?;
    log(&format!("Rahmen: {} Zyklen hoechstens", frame.takt.max));
    let options = Options::timed(LOOP_TICKS);
    let elf = board.build(&frame_path, &options)?;
    let looped = board.run(&elf, &options)?;
    let (stack_reserve, tick_jitter_ns) = (crate::board::counter(&looped, "stack"), tick_jitter(&looped));
    log(&format!("Tickschleife: Stack {stack_reserve:?} Byte, Jitter {tick_jitter_ns:?} ns"));
    // Die Natives und die Mathematik zuerst: Ihre Zyklen sagen, welche
    // Funktion die Probe `math` messen muss.
    let (natives, math) = natives_on(board, &frame_path)?;
    log(&format!("Natives: {} Funktionen, Mathematik: {} Funktionen", natives.len(), math.len()));
    let slowest = |class: CostClass| {
        math.iter()
            .filter(|r| r.wide == (class == CostClass::F64))
            .max_by_key(|r| r.cycles)
            .map_or(MATH_DEFAULT, |r| r.fun)
    };
    let mut probes = Vec::new();
    for probe in Probe::ORDER {
        let mut costs = [CostVec::default(); 2];
        let mut series = [Series::default(); 2];
        let kernels = match probe {
            Probe::Heavy(Heavy::Math, class) => {
                log(&format!("{}: misst `{}`", probe.name(), slowest(class)));
                math_kernels(slowest(class), class)
            }
            _ => probe_kernels(probe),
        };
        for (i, source) in kernels.into_iter().enumerate() {
            let size = ["klein", "gross"][i];
            costs[i] = cost_of(&source).map_err(|e| format!("Kern {} {size}: {e}", probe.name()))?;
            let m = measure(board, &write_kernel(&format!("{}_{size}", probe.name()), &source)?, None, runs)?;
            series[i] = m.takt;
        }
        log(&format!("{}: {} und {} Zyklen hoechstens", probe.name(), series[0].max, series[1].max));
        probes.push((probe, costs, series));
    }
    let mut references = Vec::new();
    let mut kernels = Vec::new();
    for name in KERNELS {
        let path = kernel_path(name, "takt");
        let source = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (cost, implicit_checks) = analysis_of(&source).map_err(|e| format!("Kern {name}: {e}"))?;
        let m = measure(board, &path, Some(kernel_path(name, "c")), runs)?;
        let c = m.c.ok_or_else(|| format!("Kern {name}: das Messprogramm meldet keine C-Referenz"))?;
        log(&format!("{name}: Takt {} und C {} Zyklen hoechstens", m.takt.max, c.max));
        references.push((name.to_string(), cost, m.takt));
        kernels.push(KernelRow { name: name.to_string(), takt: m.takt, c, implicit_checks });
    }
    let mut calibration = calibrate(frame.core_hz, &frame.takt, &probes, &references)?;
    calibration.cover_math(&math);
    Ok(Outcome { calibration, kernels, natives, math, frame, runs, stack_reserve, tick_jitter_ns })
}

/// Die Vektoren der kuratierten Natives und der korrekt gerundeten
/// Mathematik auf dem Board (13.8): Das Messprogramm `natives` rechnet sie
/// mit dem Stack-Bedarf je Aufruf, die Mathematik dazu mit den Zyklen;
/// verglichen wird mit dem Wirt und mit der Norm. `program` bindet das
/// Bring-up nur, weil sein Bau eines verlangt.
pub fn natives_on(
    board: &mut dyn Board,
    program: &Path,
) -> Result<(Vec<crate::natives::Row>, Vec<crate::math::Row>), String> {
    let read =
        |spec: std::path::PathBuf| std::fs::read_to_string(&spec).map_err(|e| format!("{}: {e}", spec.display()));
    let natives = crate::natives::vectors(&read(crate::natives::spec_path())?)?;
    let math = crate::math::vectors(&read(crate::math::spec_path())?)?;
    let options = Options::natives();
    let elf = board.build(program, &options)?;
    let text = board.run(&elf, &options)?;
    let natives = crate::natives::judge(&natives, &crate::natives::parse(&text)?)?;
    let math = crate::math::judge(&math, &crate::math::parse(&text)?)?;
    if !crate::natives::painted(&natives, &math) {
        return Err("jede Funktion meldet 0 Byte Stack: Das Malen des Stacks lief nicht (KON1-033)".into());
    }
    Ok((natives, math))
}

/// Um wie viel der Abstand zweier Tickbeginne die Periode hoechstens
/// ueberschreitet, aus den Zeitzeilen (`t=… time took=… drift=…`,
/// grammar/trace.md): das groesste `drift[k] - drift[k-1]`, nie unter null.
///
/// Pruefung 59 rechnet mit `P_m + jitter`, dem laengsten Abstand zweier
/// Aktivierungen. Ein fester Versatz aller Tickbeginne verschiebt keinen
/// Abstand, und ein spaeter Anlauf von Tick 0 verkuerzt nur den ersten;
/// Board 1 meldete beides als Jitter, erst 121, dann 157 µs (FB-291).
fn tick_jitter(text: &str) -> Option<i64> {
    let drifts: Vec<i64> = text
        .lines()
        .filter_map(|l| l.split_whitespace().find_map(|w| w.strip_prefix("drift="))?.parse().ok())
        .collect();
    drifts.windows(2).map(|w| (w[1] - w[0]).max(0)).max()
}

impl Outcome {
    /// Die Kalibrierung als Ziel der Hardware-Konfiguration (8.10).
    pub fn target(&self, name: &str) -> takt_mir::hardware::Target {
        let mut t = takt_mir::hardware::Target {
            name: name.to_string(),
            core_hz: Some(self.calibration.core_hz),
            cost_model: Some(takt_mir::analysis::cost::MODEL_VERSION),
            c_target: self.calibration.c_target,
            t_io_ps: self.calibration.t_io_ps,
            tick_jitter_ns: self.tick_jitter_ns,
            ..takt_mir::hardware::Target::default()
        };
        t.memory.stack_reserve = self.stack_reserve;
        t
    }

    /// Der Konformitaetsbericht dieses Laufs (13.8).
    pub fn report(&self, board: &str, target: &str, tool: &str) -> crate::report::Report {
        use crate::report::{
            Calibration as Summary, Check as CheckEntry, Kernel, Probe as ProbeEntry, Report, Run, Spread,
        };
        let spread = |s: &Series| Spread { min: s.min, mean: s.mean, max: s.max };
        let c = &self.calibration;
        Report {
            format_version: crate::report::FORMAT_VERSION,
            run: Run {
                date: crate::report::today(),
                board: board.to_string(),
                target: target.to_string(),
                profile: "baremetal".to_string(),
                tool: tool.to_string(),
                core_hz: c.core_hz,
                runs: self.runs,
            },
            calibration: Some(Summary {
                t_io_ps: c.t_io_ps,
                stretch: c.stretch,
                stack_reserve: self.stack_reserve,
                subnormal_failures: self.frame.subnormal,
            }),
            probes: c
                .probes
                .iter()
                .map(|r| ProbeEntry {
                    name: r.probe.name(),
                    ps: r.ps,
                    ops: r.probe.count(r.delta),
                    small: spread(&r.small),
                    large: spread(&r.large),
                })
                .collect(),
            checks: c
                .checks
                .iter()
                .map(|k| CheckEntry { name: k.name.clone(), measured_ps: k.measured_ps, bound_ps: k.bound_ps })
                .collect(),
            kernels: self
                .kernels
                .iter()
                .map(|k| Kernel {
                    name: k.name.clone(),
                    takt: spread(&k.takt),
                    c: spread(&k.c),
                    same_digest: k.same_digest(),
                    implicit_checks: k.implicit_checks,
                })
                .collect(),
            natives: self
                .natives
                .iter()
                .map(|n| crate::report::NativeEntry {
                    name: n.native.name().to_string(),
                    vectors: n.vectors,
                    same_result: n.same_result(),
                    stack: n.stack,
                    contract: n.contract,
                })
                .collect(),
            corpus: None,
        }
    }
}

impl Outcome {
    /// Je Referenzkern das Verhaeltnis Takt/C (13.8) und je Native ihr
    /// Urteil, als Zeilen.
    pub fn kernel_lines(&self) -> Vec<String> {
        let natives = self.natives.iter().map(|n| {
            format!(
                "  {:<14} {} Vektoren {}, Stack {} von {} Byte{}",
                n.native.name(),
                n.vectors,
                if n.same_result() { "bitgleich" } else { "WEICHEN AB" },
                n.stack,
                n.contract,
                if n.within_contract() { "" } else { "  UEBER DER ZUSAGE" }
            )
        });
        let math = self.math.iter().map(|m| {
            format!(
                "  {:<14} {} Vektoren {}, Stack {} von {} Byte, {} Zyklen{}",
                m.name(),
                m.vectors,
                if m.same_result() { "bitgleich" } else { "WEICHEN AB" },
                m.stack,
                takt_mir::analysis::stack::MATH_STACK,
                m.cycles,
                if m.within_contract() { "" } else { "  UEBER DEM VERTRAG" }
            )
        });
        self.kernels
            .iter()
            .map(|k| {
                // Hundertstel, ganzzahlig und aufgerundet: ein Verhaeltnis
                // ist eine Aussage, und sie faellt nicht zugunsten von Takt.
                let ratio = (u128::from(k.takt.max) * 100).div_ceil(u128::from(k.c.max.max(1)));
                format!(
                    "  {:<14} Takt {:>9}  C {:>9} Zyklen  {}.{:02}  {}  {} implizite Pruefungen",
                    k.name,
                    k.takt.max,
                    k.c.max,
                    ratio / 100,
                    ratio % 100,
                    if k.same_digest() { "gleicher Digest" } else { "DIGEST WEICHT AB" },
                    k.implicit_checks
                )
            })
            .chain(natives)
            .chain(math)
            .collect()
    }
}

impl Calibration {
    /// Hebt das Gewicht der Mathematik je Breite auf den teuersten Aufruf,
    /// den das Messprogramm `natives` am Einstieg gezaehlt hat (FB-344).
    ///
    /// Die Probe misst eine Funktion an einem Fixpunkt; die Laufzeit haengt
    /// aber auch am Argument — auf dem F401 lag `acos` in `f64` an einem
    /// Vektor 13 % ueber dem Fixpunkt der Probe. Das Gewicht gilt fuer jeden
    /// Aufruf, also traegt es das Maximum aus beidem, gestreckt wie die
    /// Tabelle.
    pub fn cover_math(&mut self, math: &[crate::math::Row]) {
        for class in [CostClass::F32, CostClass::F64] {
            let wide = class == CostClass::F64;
            let Some(cycles) = math.iter().filter(|r| r.wide == wide).map(|r| u64::from(r.cycles)).max() else {
                continue;
            };
            let floor = ps(cycles, self.core_hz);
            let Some(row) = self.probes.iter_mut().find(|r| r.probe == Probe::Heavy(Heavy::Math, class)) else {
                continue;
            };
            if floor > row.ps {
                row.ps = floor;
                let (num, den) = self.stretch;
                let scaled = (u128::from(floor) * u128::from(num)).div_ceil(u128::from(den.max(1)));
                self.c_target.set_heavy(Heavy::Math, class, u64::try_from(scaled).unwrap_or(u64::MAX));
            }
        }
    }

    /// Die Tabelle als Zeilen fuer Meldungen.
    pub fn lines(&self) -> Vec<String> {
        let mut out = vec![format!("  Kerntakt {} Hz, T_IO {} ps", self.core_hz, self.t_io_ps)];
        for row in &self.probes {
            let mut line = String::new();
            let _ = write!(
                line,
                "  {:<8} {:>10} ps   (Unterschied {} Operationen, {} -> {} Zyklen)",
                row.probe.name(),
                row.ps,
                row.delta.sum(),
                row.small.max,
                row.large.max
            );
            out.push(line);
        }
        if self.stretch != (1, 1) {
            out.push(format!("  gestreckt um {}/{}", self.stretch.0, self.stretch.1));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(max: u64) -> Series {
        Series { min: max, mean: max, max, n: 10, digest: 0 }
    }

    /// Stack und Jitter kommen aus dem Lauf in der Tickschleife.
    #[test]
    fn the_loop_run_yields_stack_and_jitter() {
        let text = "t=1 time took=120 drift=-3 slept=0\nt=2 time took=121 drift=15 slept=0\n\
                    takt schlief 0 ueberlaeufe 0 verspaetet 0 verloren 0 rueckstand 0 ns verworfen 0 journal \
                    geschrieben 0 fehlgeschlagen 0 flush 0 nvm loeschen 0 ns programmieren 0 ns stack 1432\ntakt end\n";
        assert_eq!(crate::board::counter(text, "stack"), Some(1432));
        assert_eq!(tick_jitter(text), Some(18));
    }

    /// Ein fester Versatz aller Tickbeginne ist kein Jitter, und ein spaeter
    /// Anlauf auch nicht: Er verkuerzt den ersten Abstand, statt ihn zu
    /// verlaengern — so lief es auf Board 1 (FB-291).
    #[test]
    fn a_constant_drift_is_no_jitter() {
        let text = "t=1 time took=120 drift=-120000 slept=0\nt=2 time took=121 drift=-120000 slept=0\n";
        assert_eq!(tick_jitter(text), Some(0));
        let late_start = "t=0 time took=15000 drift=35000 slept=0\nt=1 time took=10000 drift=-122000 slept=0\n\
                          t=2 time took=10000 drift=-122000 slept=0\n";
        assert_eq!(tick_jitter(late_start), Some(0));
        assert_eq!(tick_jitter("takt end\n"), None);
    }

    #[test]
    fn the_board_lines_read_back() {
        let text = "takt bench stm32f401\r\nbench core_hz 84000000\r\nbench takt min 10 mean 11 max 12 n 5 digest 7\r\n\
                    bench c min 9 mean 9 max 10 n 5 digest 7\r\nbench stack 1432\r\nbench subnormal 0\r\ntakt end\r\n";
        let m = parse(text).expect("lesbar");
        assert_eq!(m.core_hz, 84_000_000);
        assert_eq!(m.takt, Series { min: 10, mean: 11, max: 12, n: 5, digest: 7 });
        assert_eq!(m.c.map(|c| c.max), Some(10));
        assert_eq!((m.stack, m.subnormal), (1432, 0));
        assert!(parse("bench core_hz 1\n").is_err(), "ohne Reihe keine Messung");
    }

    /// 84 MHz: ein Zyklus sind 11904,76 ps, aufgerundet 11905.
    #[test]
    fn cycles_become_picoseconds_rounded_up() {
        assert_eq!(ps(1, 84_000_000), 11_905);
        assert_eq!(ps(84, 84_000_000), 1_000_000);
    }

    /// Jede Probe ergibt ein reines Kernpaar: `calibrate` nimmt die
    /// Kostenvektoren aller Kerne an. Die Zeiten sind erfunden; geprueft
    /// wird nur, dass kein Kern fremde Klassen traegt.
    /// Ein teurerer Einstieg hebt das Gewicht der Mathematik, gestreckt wie die
    /// Tabelle; ein billigerer laesst es stehen.
    #[test]
    fn the_costliest_entry_covers_the_math_weight() {
        let row = |wide: bool, cycles: u32| crate::math::Row {
            fun: "acos",
            wide,
            vectors: 1,
            deviations: Vec::new(),
            stack: 0,
            cycles,
        };
        let probe = |class| ProbeRow {
            probe: Probe::Heavy(Heavy::Math, class),
            delta: CostVec::default(),
            small: series(1),
            large: series(2),
            ps: 1_000_000,
        };
        let mut c = calibrate(84_000_000, &series(1), &[], &[]).expect("leer");
        c.stretch = (2, 1);
        c.probes = vec![probe(CostClass::F32), probe(CostClass::F64)];
        c.cover_math(&[row(true, 84_000), row(false, 42)]);
        assert_eq!(c.probes[1].ps, 1_000_000_000, "84 000 Zyklen bei 84 MHz");
        assert_eq!(c.c_target.measured(Heavy::Math, CostClass::F64), Some(2_000_000_000));
        assert_eq!(c.probes[0].ps, 1_000_000);
        assert_eq!(c.c_target.measured(Heavy::Math, CostClass::F32), None);
    }

    /// Jede Funktion taugt als Kern der Probe `math`: Er uebersetzt, und der
    /// Unterschied seiner Kerne zaehlt genau die zusaetzlichen Aufrufe.
    #[test]
    fn every_math_kernel_counts_its_calls() {
        for fun in crate::math::FUNCTIONS {
            for class in [CostClass::F32, CostClass::F64] {
                let [small, large] = math_kernels(fun, class);
                let cost = |s: &str| cost_of(s).unwrap_or_else(|e| panic!("{fun}: {e}"));
                let d = difference(cost(&large), cost(&small)).unwrap_or_else(|e| panic!("{fun}: {e}"));
                let calls = Probe::Heavy(Heavy::Math, class).count(d);
                assert_eq!(calls, u64::from(2 * (PAIRS.1 - PAIRS.0)), "{fun} in {}", class.name());
            }
        }
    }

    #[test]
    fn every_probe_is_pure() {
        let probes: Vec<(Probe, [CostVec; 2], [Series; 2])> = Probe::ORDER
            .iter()
            .map(|probe| {
                let cost = |source: &str| cost_of(source).unwrap_or_else(|e| panic!("{}: {e}", probe.name()));
                let [small, large] = probe_kernels(*probe);
                (*probe, [cost(&small), cost(&large)], [series(1_000), series(9_000)])
            })
            .collect();
        let c = calibrate(84_000_000, &series(100), &probes, &[]).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            c.c_target.is_complete() || c.c_target.missing() == vec![CostClass::Native],
            "{:?}",
            c.c_target.missing()
        );
        for probe in Probe::ORDER {
            let row = c.probes.iter().find(|r| r.probe == probe).expect("Zeile");
            assert!(row.ps > 0, "{}", probe.name());
        }
    }

    /// Das Gewicht ist der Zeitunterschied je Operation.
    #[test]
    fn a_weight_is_the_time_per_operation_difference() {
        let small = CostVec { i32: 48, ..CostVec::default() };
        let large = CostVec { i32: 240, ..CostVec::default() };
        // 192 Operationen mehr, 192 Zyklen mehr bei 84 MHz: je Operation ein Zyklus.
        let probes = [(Probe::Ops(CostClass::I32), [small, large], [series(100), series(292)])];
        let c = calibrate(84_000_000, &series(40), &probes, &[]).expect("rein");
        assert_eq!(c.probes[0].ps, 11_905);
        assert_eq!(c.t_io_ps, ps(40, 84_000_000));
        assert!(c.checks.iter().all(|k| k.bound_ps >= k.measured_ps), "{:?}", c.checks);
    }

    /// Ein Kern, den die Tabelle nicht deckt, streckt sie.
    #[test]
    fn an_uncovered_kernel_stretches_the_table() {
        let mut t = CTarget::default();
        t.set(CostClass::I32, 1_000);
        let kernels = vec![("k".to_string(), CostVec { i32: 10, ..CostVec::default() }, 25_000)];
        // Gemessen 25000, T_IO 5000: 20000 noetig, 10000 da — Faktor 2.
        let s = stretch_for(&t, 5_000, &kernels);
        assert_eq!(s, (20_000, 10_000));
        assert_eq!(stretched(&t, s).of(CostClass::I32), 2_000);
    }

    /// Ein Referenzkern ueber seiner Schranke streckt die Tabelle wie ein
    /// Klassenkern, und er steht unter den Pruefungen.
    #[test]
    fn a_reference_kernel_is_checked_and_stretches() {
        let small = CostVec { i32: 48, ..CostVec::default() };
        let large = CostVec { i32: 240, ..CostVec::default() };
        let probes = [(Probe::Ops(CostClass::I32), [small, large], [series(100), series(292)])];
        let reference = ("crc32".to_string(), CostVec { i32: 100, ..CostVec::default() }, series(1_040));
        let c = calibrate(84_000_000, &series(40), &probes, &[reference]).expect("rein");
        let check = c.checks.iter().find(|k| k.name == "crc32").expect("Pruefung des Referenzkerns");
        assert!(check.measured_ps <= check.bound_ps, "{check:?}");
        assert!(c.stretch.0 > c.stretch.1, "1000 Zyklen Arbeit fuer 100 Operationen streckt: {:?}", c.stretch);
    }

    /// Das Verhaeltnis Takt/C steht je Kern, aufgerundet.
    #[test]
    fn the_ratio_is_rounded_up() {
        let row = KernelRow { name: "pid".into(), takt: series(103), c: series(100), implicit_checks: 12 };
        assert!(row.same_digest());
        let outcome = Outcome {
            calibration: calibrate(84_000_000, &series(1), &[], &[]).expect("leer"),
            kernels: vec![row],
            natives: Vec::new(),
            math: Vec::new(),
            frame: Measured::default(),
            runs: 10,
            stack_reserve: None,
            tick_jitter_ns: None,
        };
        let line = outcome.kernel_lines().join("\n");
        assert!(line.contains("1.03") && line.contains("gleicher Digest"), "{line}");
    }

    /// Die Probe `call` unterscheidet sich nur in den Aufrufen: gleiche
    /// Rumpfarbeit, doppelt so viele Aufrufe.
    #[test]
    fn the_call_probe_differs_only_in_calls() {
        let [small, large] = probe_kernels(Probe::Ops(CostClass::Call));
        let (s, l) = (cost_of(&small).expect("klein"), cost_of(&large).expect("gross"));
        assert_eq!(l.call - s.call, u64::from(2 * CALL_PAIRS), "{s:?} {l:?}");
        assert_eq!(difference(l, s).map(|d| d.i32 + d.i64 + d.mem), Ok(0), "{s:?} {l:?}");
    }

    /// Die Proben fuer `fma` und `sqrt` zaehlen je Zeile genau eine
    /// Operation ihrer Art; was sonst dazukommt, hat ein Gewicht aus einer
    /// frueheren Probe.
    #[test]
    fn the_fma_and_sqrt_probes_count_one_per_line() {
        for (h, c) in [(Heavy::Fma, CostClass::F32), (Heavy::Sqrt, CostClass::F64)] {
            let probe = Probe::Heavy(h, c);
            let [small, large] = probe_kernels(probe);
            let d = difference(cost_of(&large).expect("gross"), cost_of(&small).expect("klein")).expect("waechst");
            assert_eq!(probe.count(d), u64::from(2 * (PAIRS.1 - PAIRS.0)), "{}: {d:?}", probe.name());
            assert_eq!(d.heavy(Heavy::Div, c), 0, "{}: {d:?}", probe.name());
        }
    }

    /// Die Probe `i64` rechnet ueber 32 Bit hinaus, ohne Pruefung, und
    /// zwei Kerngroessen unterscheiden sich nur in `i64`.
    #[test]
    fn the_i64_probe_needs_64_bits() {
        let (small, checks) = analysis_of(&class_kernel(Probe::Ops(CostClass::I64), 2)).expect("uebersetzt");
        let (large, _) = analysis_of(&class_kernel(Probe::Ops(CostClass::I64), 4)).expect("uebersetzt");
        assert_eq!(checks, 0, "keine implizite Pruefung");
        let d = difference(large, small).expect("waechst");
        assert!(d.i64 > 0 && d.i32 == 0 && d.mem == 0, "{d:?}");
    }

    /// Eine Probe mit fremden Operationen wird abgewiesen.
    #[test]
    fn an_impure_probe_is_refused() {
        let small = CostVec { mem: 2, i32: 4, ..CostVec::default() };
        let large = CostVec { mem: 10, i32: 20, ..CostVec::default() };
        let probes = [(Probe::Ops(CostClass::Mem), [small, large], [series(10), series(50)])];
        let e = calibrate(84_000_000, &series(1), &probes, &[]).expect_err("i32 unbekannt");
        assert!(e.contains("ohne Gewicht"), "{e}");
    }
}
