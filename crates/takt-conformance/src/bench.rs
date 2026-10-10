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
//!
//! **Ein Messprogramm als Baustein** (plan/m11.md 2.12, M11 Schritt 12).
//! Alle Kerne der [`suite`] stehen in einer Bibliothek, die `takt bench
//! --emit embed` baut ([`library`]); der Wirt stellt einen Zyklenzaehler und
//! eine Senke und ruft sie Schritt fuer Schritt. Was die Senke bekommt, liest
//! [`log`], und [`import`] rechnet daraus Tabelle und Bericht — fuer jedes
//! Board, auf dem ein Wirt laeuft. Die eigenen Bring-ups messen auf demselben
//! Weg ([`run`]): ein Abbild je Lage statt eines je Kern.
//!
//! **Die Mathematik** (FB-344) misst den Aufwand eines Aufrufs im erzeugten
//! Code an `pow`, der Funktion mit der teuersten Pruefung ihres
//! Definitionsbereichs, und legt ihn auf den teuersten Einstieg ueber alle
//! Funktionen und Vektoren ([`Calibration::cover_math`]): Welche Funktion
//! am teuersten ist, haengt am Kern, und die Zeit auch am Argument.
//!
//! **Was die Messung selbst kostet**, zwei Lesungen des Zaehlers, misst der
//! erste Schritt; sein Minimum geht von jeder Reihe ab. Auf einem Kern, der
//! aus dem Flash ausfuehrt, haengt eine Messung auch an der Lage des Codes
//! (FB-367): Der Wirt kann sein Abbild verschieben, und mehrere Lagen werden
//! zu einer Messung mit dem jeweils schlechteren Wert; die Gewichte je Lage
//! stehen im Bericht.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use takt_mir::fns::{CostClass, CostVec, Heavy};
use takt_mir::hardware::CTarget;

use crate::board::{Board, Form, Options};

pub mod library;
pub mod log;

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

/// Paare je Kern der Mathematik. Ein Aufruf kostet Tausende Zyklen; acht
/// Aufrufe Unterschied tragen die Messung.
const MATH_PAIRS: (u32, u32) = (1, 5);

/// Die Argumente der Probe `math`: `pow` mit gebrochenem Exponenten, auf
/// dem langsamen Weg. Der Laeufer misst den Einstieg an denselben Bits
/// ([`math_probe_bits`]).
pub const MATH_ARGS: [&str; 2] = ["0.7", "1.3"];

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
/// braucht eine andere Form, siehe [`call_kernel`], die Mathematik weniger
/// Paare, siehe [`MATH_PAIRS`].
pub fn probe_kernels(probe: Probe) -> [String; 2] {
    match probe {
        Probe::Ops(CostClass::Call | CostClass::Native) => [call_kernel(false), call_kernel(true)],
        Probe::Heavy(Heavy::Math, _) => [class_kernel(probe, MATH_PAIRS.0), class_kernel(probe, MATH_PAIRS.1)],
        _ => [class_kernel(probe, PAIRS.0), class_kernel(probe, PAIRS.1)],
    }
}

/// Die Bits der Argumente von [`MATH_ARGS`] in einer Breite, wie der Kern
/// sie rechnet: `MATH_ARGS[0] + b * 0.0` ist `MATH_ARGS[0]`.
pub fn math_probe_bits(wide: bool) -> [u64; 2] {
    MATH_ARGS.map(|a| {
        if wide {
            a.parse::<f64>().map_or(0, f64::to_bits)
        } else {
            a.parse::<f32>().map_or(0, |f| u64::from(f.to_bits()))
        }
    })
}

/// Der Name eines Kerns einer Probe im Protokoll: `i32_small`,
/// `f64_math_large`.
pub fn kernel_name(probe: Probe, large: bool) -> String {
    format!("{}_{}", probe.name(), if large { "large" } else { "small" })
}

/// Ein Kern des Messprogramms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kernel {
    /// Der Name im Protokoll; mit `bench_` davor das Praefix seiner Symbole
    /// (12.11).
    pub name: String,
    /// Der Quelltext.
    pub source: String,
    /// Wozu er gemessen wird.
    pub role: Role,
}

/// Wozu ein Kern gemessen wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// Der leere Kern: sein Maximum ist `T_IO`.
    Frame,
    /// Ein Kern einer Probe, klein oder gross.
    Probe {
        /// Die Probe.
        probe: Probe,
        /// Der grosse Kern des Paars?
        large: bool,
    },
    /// Ein Referenzkern aus 13.8 mit dem Quelltext seiner C-Referenz.
    Reference {
        /// Die C-Referenz.
        c: String,
    },
}

/// Die Referenzkerne im Werkzeug: Takt-Programm und C-Referenz, in der
/// Reihenfolge von [`KERNELS`]. Ein Werkzeug misst die Kerne, mit denen es
/// gebaut wurde, und wer sie aendert, baut die Messbibliothek neu.
const REFERENCES: [(&str, &str); 6] = [
    (include_str!("../bench/pid.takt"), include_str!("../bench/pid.c")),
    (include_str!("../bench/thermocouples.takt"), include_str!("../bench/thermocouples.c")),
    (include_str!("../bench/lines.takt"), include_str!("../bench/lines.c")),
    (include_str!("../bench/matrix.takt"), include_str!("../bench/matrix.c")),
    (include_str!("../bench/crc32.takt"), include_str!("../bench/crc32.c")),
    (include_str!("../bench/sequence.takt"), include_str!("../bench/sequence.c")),
];

/// Die Kerne des Messprogramms in der Reihenfolge seiner Schritte: der leere
/// Kern, die Proben in [`Probe::ORDER`], die Referenzkerne aus 13.8.
pub fn suite() -> Vec<Kernel> {
    let mut out = vec![Kernel { name: "frame".into(), source: frame_kernel(), role: Role::Frame }];
    for probe in Probe::ORDER {
        for (large, source) in [false, true].into_iter().zip(probe_kernels(probe)) {
            out.push(Kernel { name: kernel_name(probe, large), source, role: Role::Probe { probe, large } });
        }
    }
    for (name, (takt, c)) in KERNELS.iter().zip(REFERENCES) {
        out.push(Kernel {
            name: name.to_string(),
            source: takt.to_string(),
            role: Role::Reference { c: c.to_string() },
        });
    }
    out
}

/// Die Vektoren der Mathematik, die das Messprogramm rechnet: Stufe 2 aus
/// `grammar/libtaktm.md`, wie das Werkzeug sie beim Bau las.
pub fn math_vectors() -> Result<Vec<crate::math::Vector>, String> {
    crate::math::vectors(include_str!("../../../grammar/libtaktm.md"))
}

/// Die Kennung des Messprogramms: 16 Hexziffern aus SHA-256 ueber Kerne,
/// C-Referenzen, Vektoren der Mathematik und die Version des Kostenmodells.
///
/// Das Protokoll nennt sie, und `takt bench --import` nimmt nur ein
/// Protokoll mit seiner eigenen: Die Kostenvektoren rechnet der Import aus
/// den Quelltexten, und sie gehoeren zu genau den Kernen, die gemessen wurden.
pub fn suite_id(kernels: &[Kernel], vectors: &[crate::math::Vector]) -> String {
    let mut bytes = format!("takt-bench 1 model {}\n", takt_mir::analysis::cost::MODEL_VERSION).into_bytes();
    for k in kernels {
        bytes.extend_from_slice(k.name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(k.source.as_bytes());
        bytes.push(0);
        if let Role::Reference { c } = &k.role {
            bytes.extend_from_slice(c.as_bytes());
        }
        bytes.push(0);
    }
    for v in vectors {
        bytes.extend_from_slice(format!("{} {} {:x} {:x}\n", v.fun, v.wide, v.args[0], v.args[1]).as_bytes());
    }
    takt_mir::hash::sha256(&bytes).to_string()[..16].to_string()
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
///
/// **Die Mathematik ruft `pow` immer an derselben Stelle** ([`MATH_ARGS`]):
/// `b * 0.0` ist null, haengt aber am Ergebnis davor, und so faellt kein
/// Aufruf weg, obwohl jeder dasselbe rechnet.
pub fn class_kernel(probe: Probe, pairs: u32) -> String {
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
        Probe::Heavy(Heavy::Math, _) => ("float", MATH_LINES, "", ("1.0", "3.0")),
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

/// Ein Paar der Probe `math` mit den Argumenten aus [`MATH_ARGS`].
const MATH_LINES: &str = "            a = pow(0.7 + b * 0.0, 1.3)\n            b = pow(0.7 + a * 0.0, 1.3)\n";

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

/// Der leere Kern: ein Tick ohne Arbeit, fuer `T_IO` und den Tick-Jitter.
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
    let (p, checks) = program_of(source)?;
    let report = takt_mir::analysis::budget::report(&p);
    let cost = report.machines.iter().fold(CostVec::default(), |acc, m| acc + m.activation + m.fault_path);
    Ok((cost, checks))
}

/// Ein Kern, uebersetzt fuer die Hardware (8.3), mit der Zahl der
/// impliziten Pruefungen, die in ihm blieben: dasselbe Programm, das
/// `takt bench --emit embed` baut und dessen Kosten [`analysis_of`] zaehlt.
pub fn program_of(source: &str) -> Result<(takt_mir::Program, u32), String> {
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
    let checks = out.report.total_checks();
    Ok((out.program.ok_or("kein Programm")?, checks))
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

/// Ein Kernpaar einer Probe mit seinen Messungen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    /// Die Probe.
    pub probe: Probe,
    /// Die Kostenvektoren, klein und gross.
    pub costs: [CostVec; 2],
    /// Die Messreihen, klein und gross.
    pub series: [Series; 2],
}

/// Eine Zeile der Tabelle mit ihrer Herkunft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeRow {
    /// Die Probe.
    pub probe: Probe,
    /// Bei der Mathematik die Funktion, an deren teuerstem Einstieg das
    /// Gewicht haengt ([`Calibration::cover_math`]).
    pub fun: Option<&'static str>,
    /// Unterschied der Kostenvektoren zwischen grossem und kleinem Kern.
    pub delta: CostVec,
    /// Die beiden Messreihen, klein und gross.
    pub small: Series,
    /// Siehe `small`.
    pub large: Series,
    /// Das Gewicht in Pikosekunden, vor einer Streckung; bei der Mathematik
    /// mindestens ihr teuerster Einstieg ([`Calibration::cover_math`]).
    pub ps: u64,
    /// Das Gewicht in jeder Lage, wenn in mehreren gemessen wurde (FB-367).
    pub placements: Vec<u64>,
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
/// `frame` ist der leere Kern: sein Maximum ist `T_IO`. Jedes Paar traegt
/// die Kostenvektoren seiner beiden Kerne und deren Messungen. Die Proben
/// muessen rein sein: Der Unterschied ihrer Vektoren darf nur Klassen
/// enthalten, die vorher bestimmt wurden, und die eigene. `references` sind
/// weitere gemessene Kerne mit ihren Kosten — die Referenzkerne aus 13.8 —,
/// gegen die die Tabelle ebenso geprueft und notfalls gestreckt wird.
pub fn calibrate(
    core_hz: u32,
    frame: &Series,
    pairs: &[Pair],
    references: &[(String, CostVec, Series)],
) -> Result<Calibration, String> {
    let mut table = CTarget::default();
    let mut known: Vec<Probe> = Vec::new();
    let mut rows: Vec<ProbeRow> = Vec::new();
    for pair in pairs {
        let probe = pair.probe;
        if known.contains(&probe) {
            return Err(format!("Probe {}: zwei Paare", probe.name()));
        }
        let [small_cost, large_cost] = pair.costs;
        let [small, large] = pair.series;
        let delta = difference(large_cost, small_cost)?;
        let count = probe.count(delta);
        if count == 0 {
            return Err(format!(
                "Probe {}: der Unterschied der Kerne enthaelt keine Operation der Klasse",
                probe.name()
            ));
        }
        // Rein: Jede Komponente des Unterschieds hat ein Gewicht — aus einer
        // frueheren Probe oder aus dieser.
        let weighed = |p: Probe| known.contains(&p) || probe == p;
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
            Probe::Ops(c) => table.set(c, weight),
            Probe::Heavy(h, c) => table.set_heavy(h, c, weight),
        }
        known.push(probe);
        rows.push(ProbeRow { probe, fun: None, delta, small, large, ps: weight, placements: Vec::new() });
    }

    let t_io_ps = ps(frame.max, core_hz);
    // Jeder gemessene Kern muss unter der Schranke liegen; sonst wird
    // gestreckt, um den groessten noetigen Faktor.
    let kernels: Vec<(String, CostVec, u64)> = pairs
        .iter()
        .flat_map(|p| {
            [false, true].map(|large| {
                let i = usize::from(large);
                (kernel_name(p.probe, large), p.costs[i], ps(p.series[i].max, core_hz))
            })
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

/// Wo der leere Kern fuer Tickschleife und Natives liegt: ein fester Ort
/// im Zielverzeichnis, damit der Zwischenspeicher der Abbilder ihn
/// wiedererkennt.
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

/// Ticks des Laufs in der Tickschleife: zehn Sekunden bei 10 ms.
const LOOP_TICKS: u64 = 1000;

/// Der Lastkern der Stack-Reserve (12.3, 13.8): Ausgaben jeder Art in jedem
/// Tick, das Journal so oft es fertig wird, Log, Alert und ein
/// Zustandswechsel — die Wege der Runtime, die der leere Kern nicht nimmt.
pub const LOAD_KERNEL: &str = include_str!("../bench/load.takt");

/// Wie der Lastkern laeuft: in Echtzeit und in logischer Zeit, wo der Trace
/// nichts verwirft, und beides in jeder Form, die das Board kennt.
fn load_runs(forms: &[Form]) -> Vec<Options> {
    let runs = [Options::timed(LOOP_TICKS), Options::fresh(LOOP_TICKS)].map(Options::calibrating);
    forms.iter().flat_map(|&form| runs.clone().map(|o| o.in_form(form))).collect()
}

/// Die Stack-Reserve (12.3) aus Laeufen in der Tickschleife: je Lauf die
/// Tiefe ohne den Anteil des Programms, den die Bilanz nennt; die tiefste.
fn stack_reserve<'a>(runs: impl IntoIterator<Item = &'a str>) -> Option<u64> {
    let counter = crate::board::counter;
    runs.into_iter()
        .filter_map(|t| Some(counter(t, "stack")?.saturating_sub(counter(t, "programm").unwrap_or(0))))
        .max()
}

/// Was `takt bench` auf einem Board ergibt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Die Tabelle mit ihrer Herkunft.
    pub calibration: Calibration,
    /// Die Referenzkerne aus 13.8 mit ihren C-Referenzen.
    pub kernels: Vec<KernelRow>,
    /// Die kuratierten Natives: Ergebnisse gegen den Wirt, Stack gegen die
    /// Zusage (13.8); leer ohne das Messprogramm `natives`.
    pub natives: Vec<crate::natives::Row>,
    /// Die korrekt gerundete Mathematik: Ergebnisse gegen die Norm, Zyklen
    /// je Funktion und, mit dem Messprogramm `natives`, ihr Stack (4.2).
    pub math: Vec<crate::math::Row>,
    /// Die Kennung des Messprogramms.
    pub suite: String,
    /// Die Lagen, in denen gemessen wurde, als Verschiebung in Byte.
    pub placements: Vec<u32>,
    /// Was die Messung selbst kostet, in Zyklen; abgezogen von jeder Reihe.
    pub overhead: u64,
    /// Messungen je Kern und Lage.
    pub runs: u64,
    /// Faelle des Subnormal-Vektors mit falschem Ergebnis (4.2).
    pub subnormal: u64,
    /// Die Reserve fuer Runtime, Treiber und ISRs (12.3): die tiefste
    /// Stack-Tiefe in der Tickschleife ohne den Anteil des Programms, aus dem
    /// leeren Kern und dem Lastkern.
    pub stack_reserve: Option<u64>,
    /// Um wie viel der Abstand zweier Tickbeginne die Periode hoechstens
    /// ueberschreitet (7.3).
    pub tick_jitter_ns: Option<i64>,
}

/// Was `takt bench --import` liest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Logs {
    /// Die Protokolle des Messprogramms, je Lage eines.
    pub bench: Vec<String>,
    /// Der leere Kern in der Tickschleife: Jitter und Stack-Reserve.
    pub looped: Option<String>,
    /// Der Lastkern in der Tickschleife, je Lauf ein Protokoll: die
    /// Stack-Reserve unter Last.
    pub loads: Vec<String>,
    /// Das Messprogramm `natives`: Ergebnisse und Stack der Natives und der
    /// Mathematik.
    pub natives: Option<String>,
}

/// Rechnet Tabelle, Referenzkerne und Urteile aus den Protokollen (13.8).
///
/// Die Kostenvektoren kommen aus den Quelltexten der [`suite`]; ein
/// Protokoll mit fremder Kennung wird abgelehnt. Mehrere Lagen werden zu
/// einer Messung ([`log::merge`]); das Gewicht jeder Probe in jeder Lage
/// steht dazu in [`ProbeRow::placements`].
pub fn import(logs: &Logs) -> Result<Outcome, String> {
    let kernels = suite();
    let vectors = math_vectors()?;
    let id = suite_id(&kernels, &vectors);
    let mut runs = Vec::new();
    for text in &logs.bench {
        runs.extend(log::runs(text)?);
    }
    if runs.is_empty() {
        return Err("kein Lauf des Messprogramms im Protokoll (`takt bench <kennung>` bis `takt end`)".into());
    }
    if let Some(r) = runs.iter().find(|r| r.suite != id) {
        return Err(format!(
            "das Protokoll stammt von den Kernen {}, dieses Werkzeug misst {id}: die Bibliothek neu bauen und messen",
            r.suite
        ));
    }
    let runs: Vec<log::Run> = runs.iter().map(log::Run::without_overhead).collect();
    let merged = log::merge(&runs)?;
    let costs = kernels
        .iter()
        .map(|k| analysis_of(&k.source).map(|a| (k.name.clone(), a)).map_err(|e| format!("Kern {}: {e}", k.name)))
        .collect::<Result<std::collections::BTreeMap<_, _>, _>>()?;
    let painted = logs.natives.as_deref().map(crate::math::parse).transpose()?.unwrap_or_default();
    // Je Lauf die Tabelle samt dem Gewicht der Mathematik (`cover_math`).
    let calibrated = |run: &log::Run| -> Result<(Calibration, Vec<crate::math::Row>), String> {
        let taken = |name: &str| run.takt.get(name).copied().ok_or_else(|| format!("Kern `{name}` fehlt im Protokoll"));
        let mut pairs = Vec::new();
        for probe in Probe::ORDER {
            let names = [false, true].map(|large| kernel_name(probe, large));
            pairs.push(Pair {
                probe,
                costs: [costs[&names[0]].0, costs[&names[1]].0],
                series: [taken(&names[0])?, taken(&names[1])?],
            });
        }
        let references = KERNELS
            .iter()
            .map(|name| taken(name).map(|s| (name.to_string(), costs[*name].0, s)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut calibration = calibrate(run.core_hz, &taken("frame")?, &pairs, &references)?;
        let math = crate::math::judge(&vectors, &crate::math::joined(&run.math, &painted)?)?;
        let entry = |wide: bool| {
            let name = if wide { "f64" } else { "f32" };
            run.entries.get(name).copied().ok_or_else(|| format!("der Einstieg der Probe `math` in {name} fehlt"))
        };
        calibration.cover_math(&math, [entry(false)?, entry(true)?]);
        Ok((calibration, math))
    };
    let (mut calibration, math) = calibrated(&merged)?;
    if runs.len() > 1 {
        let each = runs.iter().map(calibrated).collect::<Result<Vec<_>, _>>()?;
        for row in &mut calibration.probes {
            row.placements =
                each.iter().map(|(c, _)| c.probes.iter().find(|r| r.probe == row.probe).map_or(0, |r| r.ps)).collect();
        }
    }
    let natives = match logs.natives.as_deref() {
        Some(text) => {
            let spec = crate::natives::spec_path();
            let read = std::fs::read_to_string(&spec).map_err(|e| format!("{}: {e}", spec.display()))?;
            let natives = crate::natives::judge(&crate::natives::vectors(&read)?, &crate::natives::parse(text)?)?;
            if !crate::natives::painted(&natives, &math) {
                return Err("jede Funktion meldet 0 Byte Stack: Das Malen des Stacks lief nicht (KON1-033)".into());
            }
            natives
        }
        None => Vec::new(),
    };
    let reference_rows = KERNELS
        .iter()
        .map(|name| {
            let (takt, c) = (merged.takt.get(*name), merged.c.get(*name));
            match (takt, c) {
                (Some(takt), Some(c)) => {
                    Ok(KernelRow { name: name.to_string(), takt: *takt, c: *c, implicit_checks: costs[*name].1 })
                }
                _ => Err(format!("Kern {name}: das Protokoll nennt keine C-Referenz")),
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    let looped = logs.looped.as_deref();
    Ok(Outcome {
        calibration,
        kernels: reference_rows,
        natives,
        math,
        suite: id,
        placements: runs.iter().map(|r| r.shift).collect(),
        overhead: runs.iter().map(|r| r.overhead.min).max().unwrap_or(0),
        runs: merged.takt.get("frame").map_or(0, |s| s.n / runs.len() as u64),
        subnormal: merged.subnormal,
        stack_reserve: stack_reserve(looped.into_iter().chain(logs.loads.iter().map(String::as_str))),
        tick_jitter_ns: looped.and_then(tick_jitter),
    })
}

/// Misst auf dem Board: das Messprogramm in jeder Lage des Boards, den leeren
/// Kern und den Lastkern in der Tickschleife und die Natives; liefert die
/// Protokolle, aus denen [`import`] rechnet.
///
/// `log` bekommt je Abbild eine Zeile, damit ein langer Lauf zeigt, wo er
/// steht.
pub fn run(board: &mut dyn Board, runs: u64, mut log: impl FnMut(&str)) -> Result<Logs, String> {
    let frame = write_kernel("frame", &frame_kernel())?;
    let mut logs = Logs::default();
    for &shift in board.placements() {
        let options = Options::bench(runs, shift);
        let elf = board.build(&frame, &options)?;
        let text = board.run(&elf, &options)?;
        let n = log::runs(&text)?.first().map_or(0, |r| r.takt.len());
        log(&format!("Messprogramm in Lage {shift}: {n} Kerne"));
        logs.bench.push(text);
    }
    let options = Options::timed(LOOP_TICKS).calibrating();
    let elf = board.build(&frame, &options)?;
    let looped = board.run(&elf, &options)?;
    log(&format!(
        "Tickschleife: Stack {:?} Byte, Jitter {:?} ns",
        crate::board::counter(&looped, "stack"),
        tick_jitter(&looped)
    ));
    logs.looped = Some(looped);
    let load = write_kernel("load", LOAD_KERNEL)?;
    for options in load_runs(board.forms()) {
        let elf = board.build(&load, &options)?;
        let text = board.run(&elf, &options)?;
        log(&format!(
            "Lastkern{}{}: Stack {:?} Byte, davon Programm {:?}",
            match options.form {
                Form::Own => "",
                Form::Interrupt => " in der Interruptform",
                Form::Poll => " in der Pollform",
                Form::Rtos => " unter RTOS",
            },
            if options.timed { " in Echtzeit" } else { " in logischer Zeit" },
            crate::board::counter(&text, "stack"),
            crate::board::counter(&text, "programm")
        ));
        logs.loads.push(text);
    }
    let natives = natives_text(board, &frame)?;
    log("Natives und Mathematik gerechnet");
    logs.natives = Some(natives);
    Ok(logs)
}

/// Das Protokoll des Messprogramms `natives`: die Vektoren der kuratierten
/// Natives und der korrekt gerundeten Mathematik mit dem Stack-Bedarf je
/// Aufruf (13.8). `program` bindet das Bring-up nur, weil sein Bau eines
/// verlangt.
fn natives_text(board: &mut dyn Board, program: &Path) -> Result<String, String> {
    let options = Options::natives();
    let elf = board.build(program, &options)?;
    board.run(&elf, &options)
}

/// Die Natives und die Mathematik auf dem Board, verglichen mit dem Wirt und
/// mit der Norm (13.8).
pub fn natives_on(
    board: &mut dyn Board,
    program: &Path,
) -> Result<(Vec<crate::natives::Row>, Vec<crate::math::Row>), String> {
    let read =
        |spec: std::path::PathBuf| std::fs::read_to_string(&spec).map_err(|e| format!("{}: {e}", spec.display()));
    let natives = crate::natives::vectors(&read(crate::natives::spec_path())?)?;
    let math = crate::math::vectors(&read(crate::math::spec_path())?)?;
    let text = natives_text(board, program)?;
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
                suite: self.suite.clone(),
                placements: self.placements.clone(),
            },
            calibration: Some(Summary {
                t_io_ps: c.t_io_ps,
                stretch: c.stretch,
                stack_reserve: self.stack_reserve,
                subnormal_failures: self.subnormal,
                overhead: self.overhead,
            }),
            probes: c
                .probes
                .iter()
                .map(|r| ProbeEntry {
                    name: r.probe.name(),
                    function: r.fun.map(str::to_string),
                    ps: r.ps,
                    ops: r.probe.count(r.delta),
                    small: spread(&r.small),
                    large: spread(&r.large),
                    placements: r.placements.clone(),
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
            let stack = match m.stack {
                Some(bytes) => format!("Stack {bytes} von {} Byte", takt_mir::analysis::stack::MATH_STACK),
                None => "Stack ungemessen".to_string(),
            };
            format!(
                "  {:<14} {} Vektoren {}, {stack}, {} Zyklen{}",
                m.name(),
                m.vectors,
                if m.same_result() { "bitgleich" } else { "WEICHEN AB" },
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
    /// Das Gewicht der Mathematik je Breite (FB-344): der Aufwand eines
    /// Aufrufs im erzeugten Code und der teuerste Einstieg.
    ///
    /// Die Probe misst `pow` an [`MATH_ARGS`]; `entries` sind die Zyklen
    /// desselben Einstiegs an denselben Argumenten (`f32`, `f64`), ohne den
    /// erzeugten Code drumherum. Ihr Unterschied ist der Aufwand: Aufruf,
    /// Argumente und die Pruefung des Definitionsbereichs, die bei `pow` die
    /// teuerste ist. Der teuerste Einstieg kommt aus den Vektoren aller
    /// Funktionen: Welche am teuersten ist, haengt am Kern, und die Zeit auch
    /// am Argument — auf dem F401 lag `acos` in `f64` an einem Vektor 13 %
    /// ueber dem Fixpunkt der alten Probe. Das Gewicht gilt fuer jeden Aufruf,
    /// also traegt es beides, gestreckt wie die Tabelle.
    pub fn cover_math(&mut self, math: &[crate::math::Row], entries: [Series; 2]) {
        for (class, entry) in [CostClass::F32, CostClass::F64].into_iter().zip(entries) {
            let wide = class == CostClass::F64;
            let worst = math.iter().filter(|r| r.wide == wide).max_by_key(|r| r.cycles);
            let Some(row) = self.probes.iter_mut().find(|r| r.probe == Probe::Heavy(Heavy::Math, class)) else {
                continue;
            };
            let overhead = row.ps.saturating_sub(ps(entry.min, self.core_hz));
            let (cycles, fun) = match worst {
                Some(w) if u64::from(w.cycles) > entry.max => (u64::from(w.cycles), w.fun),
                _ => (entry.max, "pow"),
            };
            let weight = overhead + ps(cycles, self.core_hz);
            row.fun = Some(fun);
            if weight > row.ps {
                row.ps = weight;
                let (num, den) = self.stretch;
                let scaled = (u128::from(weight) * u128::from(num)).div_ceil(u128::from(den.max(1)));
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
            stack: None,
            cycles,
        };
        let probe = |class| ProbeRow {
            probe: Probe::Heavy(Heavy::Math, class),
            fun: Some("acos"),
            delta: CostVec::default(),
            small: series(1),
            large: series(2),
            ps: 1_000_000,
            placements: Vec::new(),
        };
        let mut c = calibrate(84_000_000, &series(1), &[], &[]).expect("leer");
        c.stretch = (2, 1);
        c.probes = vec![probe(CostClass::F32), probe(CostClass::F64)];
        // Die Probe: 1 000 000 ps je Aufruf, davon 500 000 ps der Einstieg
        // selbst (42 Zyklen); der Aufwand drumherum sind 500 000 ps.
        c.cover_math(&[row(true, 84_000), row(false, 42)], [series(42), series(42)]);
        assert_eq!(c.probes[1].ps, 1_000_500_000, "84 000 Zyklen bei 84 MHz und der Aufwand");
        assert_eq!(c.probes[1].fun, Some("acos"));
        assert_eq!(c.c_target.measured(Heavy::Math, CostClass::F64), Some(2_001_000_000));
        assert_eq!((c.probes[0].ps, c.probes[0].fun), (1_000_000, Some("pow")), "kein Einstieg teurer als die Probe");
        assert_eq!(c.c_target.measured(Heavy::Math, CostClass::F32), None);
    }

    /// Der Kern der Probe `math` uebersetzt in beiden Breiten, und der
    /// Unterschied seiner Kerne zaehlt genau die zusaetzlichen Aufrufe.
    #[test]
    fn the_math_kernel_counts_its_calls() {
        for class in [CostClass::F32, CostClass::F64] {
            let [small, large] = probe_kernels(Probe::Heavy(Heavy::Math, class));
            let cost = |s: &str| cost_of(s).unwrap_or_else(|e| panic!("{}: {e}", class.name()));
            let d = difference(cost(&large), cost(&small)).unwrap_or_else(|e| panic!("{e}"));
            let calls = Probe::Heavy(Heavy::Math, class).count(d);
            assert_eq!(calls, u64::from(2 * (MATH_PAIRS.1 - MATH_PAIRS.0)), "{}", class.name());
            assert!(small.contains("pow(0.7 + b * 0.0, 1.3)"), "{small}");
        }
        assert_eq!(math_probe_bits(true), [0.7f64.to_bits(), 1.3f64.to_bits()]);
        assert_eq!(math_probe_bits(false), [u64::from(0.7f32.to_bits()), u64::from(1.3f32.to_bits())]);
    }

    #[test]
    fn every_probe_is_pure() {
        let probes: Vec<Pair> = Probe::ORDER
            .iter()
            .map(|&probe| {
                let cost = |source: &str| cost_of(source).unwrap_or_else(|e| panic!("{}: {e}", probe.name()));
                let [small, large] = probe_kernels(probe);
                Pair { probe, costs: [cost(&small), cost(&large)], series: [series(1_000), series(9_000)] }
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
        let probes = [i32_pair(small, large, 100, 292)];
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
        let probes = [i32_pair(small, large, 100, 292)];
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
            suite: String::new(),
            placements: vec![0],
            overhead: 0,
            runs: 10,
            subnormal: 0,
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
        let probes =
            [Pair { probe: Probe::Ops(CostClass::Mem), costs: [small, large], series: [series(10), series(50)] }];
        let e = calibrate(84_000_000, &series(1), &probes, &[]).expect_err("i32 unbekannt");
        assert!(e.contains("ohne Gewicht"), "{e}");
    }

    fn i32_pair(small: CostVec, large: CostVec, small_max: u64, large_max: u64) -> Pair {
        Pair {
            probe: Probe::Ops(CostClass::I32),
            costs: [small, large],
            series: [series(small_max), series(large_max)],
        }
    }

    /// Jeder Kern der Suite hat einen eigenen Namen, und mit `bench_` davor
    /// ist er ein Praefix (12.11); die Kennung haengt an den Quelltexten.
    #[test]
    fn the_suite_names_every_kernel_once() {
        let kernels = suite();
        let mut names: Vec<&str> = kernels.iter().map(|k| k.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), kernels.len(), "doppelte Namen");
        for k in &kernels {
            takt_llvm::symbols::Prefix::new(&library::prefix(k)).unwrap_or_else(|e| panic!("{e}"));
        }
        assert_eq!(kernels.iter().filter(|k| matches!(k.role, Role::Reference { .. })).count(), KERNELS.len());
        let vectors = math_vectors().expect("Vektoren");
        let id = suite_id(&kernels, &vectors);
        assert_eq!(id.len(), 16);
        let mut changed = kernels.clone();
        changed[1].source.push('\n');
        assert_ne!(suite_id(&changed, &vectors), id);
    }

    /// Die Tabelle, aus der [`synthetic`] die Zeiten rechnet: jedes Gewicht
    /// ein Vielfaches von 1000 ps, bei 1 GHz also ganze Zyklen.
    fn table() -> CTarget {
        let mut t = CTarget::default();
        for (c, ps) in [
            (CostClass::I32, 1_000),
            (CostClass::I64, 3_000),
            (CostClass::F32, 2_000),
            (CostClass::F64, 40_000),
            (CostClass::Mem, 2_000),
            (CostClass::Call, 5_000),
        ] {
            t.set(c, ps);
        }
        for (h, c, ps) in [
            (Heavy::Hook, CostClass::Call, 30_000),
            (Heavy::Div, CostClass::I32, 10_000),
            (Heavy::Div, CostClass::I64, 50_000),
            (Heavy::Div, CostClass::F32, 20_000),
            (Heavy::Div, CostClass::F64, 400_000),
            (Heavy::Fma, CostClass::F32, 3_000),
            (Heavy::Fma, CostClass::F64, 80_000),
            (Heavy::Sqrt, CostClass::F32, 15_000),
            (Heavy::Sqrt, CostClass::F64, 500_000),
            (Heavy::Math, CostClass::F32, 2_000_000),
            (Heavy::Math, CostClass::F64, 9_000_000),
        ] {
            t.set_heavy(h, c, ps);
        }
        t
    }

    /// Ein Protokoll, wie ein Board mit der Tabelle [`table`] bei 1 GHz es
    /// schriebe: jeder Kern `T_IO` plus seine Kosten, dazu der Messaufwand.
    fn synthetic(shift: u32, overhead: u64) -> String {
        let kernels = suite();
        let vectors = math_vectors().expect("Vektoren");
        let (t, frame) = (table(), 500);
        let mut s = format!(
            "boot\ntakt bench {}\nbench core_hz 1000000000\nbench shift {shift}\n\
             bench overhead min {overhead} mean {overhead} max {overhead} n 4 digest 0\n",
            suite_id(&kernels, &vectors)
        );
        for k in &kernels {
            let work =
                if k.role == Role::Frame { 0 } else { t.duration_ps(cost_of(&k.source).expect("Kosten")) / 1000 };
            let c = overhead + frame + work;
            let _ = writeln!(s, "bench takt {} min {c} mean {c} max {c} n 4 digest 7", k.name);
            if matches!(k.role, Role::Reference { .. }) {
                let _ = writeln!(s, "bench c {} min {c} mean {c} max {c} n 4 digest 7", k.name);
            }
        }
        for (i, v) in vectors.iter().enumerate() {
            let _ = writeln!(s, "math {i} {:016x} cycles {}", v.want, overhead + 100);
        }
        // Der Einstieg der Probe an ihren Argumenten: die Haelfte des Gewichts.
        for (name, ps) in [("f32", 2_000_000), ("f64", 9_000_000)] {
            let c = overhead + ps / 2 / 1000;
            let _ = writeln!(s, "bench entry {name} min {c} mean {c} max {c} n 4 digest 0");
        }
        s.push_str("bench subnormal 0\ntakt end\n");
        s
    }

    /// **Die Reserve ist die tiefste Last ohne ihr Programm** (12.3): Der
    /// Lastkern reicht tiefer als der leere Kern, aber ein Teil davon ist
    /// sein eigener Rahmen, den die Bilanz nennt. Ein Protokoll ohne
    /// `programm` zaehlt ganz.
    #[test]
    fn the_reserve_is_the_deepest_load_without_its_program() {
        let summary = |tail: &str| format!("takt schlief 0 ueberlaeufe 0 {tail}\ntakt end\n");
        let looped = summary("stack 1000 schranke 4096 programm 16");
        let loads = [summary("stack 3000 schranke 4096 programm 200"), summary("stack 2500 schranke 4096")];
        assert_eq!(stack_reserve([looped.as_str()]), Some(984));
        assert_eq!(stack_reserve(loads.iter().map(String::as_str).chain([looped.as_str()])), Some(2800));
        assert_eq!(stack_reserve(["takt schlief 0\n"]), None);
        let logs = Logs { bench: vec![synthetic(0, 6)], looped: Some(looped), loads: loads.to_vec(), natives: None };
        assert_eq!(import(&logs).unwrap_or_else(|e| panic!("{e}")).stack_reserve, Some(2800));
    }

    /// **Der Import findet die Tabelle wieder**, aus der das Protokoll
    /// entstand: jedes Gewicht, ohne Streckung, jeder Kern unter seiner
    /// Schranke; zwei Lagen geben ihre Gewichte in den Bericht.
    #[test]
    fn an_import_recovers_the_table_it_was_measured_with() {
        let logs = Logs { bench: vec![synthetic(0, 6), synthetic(8, 9)], ..Logs::default() };
        let o = import(&logs).unwrap_or_else(|e| panic!("{e}"));
        let c = &o.calibration;
        assert_eq!(c.stretch, (1, 1), "{:?}", c.checks.iter().find(|k| k.measured_ps > k.bound_ps));
        for class in CostClass::ALL.into_iter().filter(|c| *c != CostClass::Native) {
            assert_eq!(c.c_target.of(class), table().of(class), "{}", class.name());
            for h in Heavy::ALL.into_iter().filter(|h| h.exists_in(class)) {
                assert_eq!(
                    c.c_target.measured(h, class),
                    table().measured(h, class),
                    "{}_{}",
                    class.name(),
                    h.suffix()
                );
            }
        }
        assert_eq!(c.t_io_ps, 500_000);
        assert_eq!((o.placements.clone(), o.overhead, o.runs), (vec![0, 8], 9, 4));
        let i32_row = c.probes.iter().find(|r| r.probe == Probe::Ops(CostClass::I32)).expect("i32");
        assert_eq!(i32_row.placements, vec![1_000, 1_000]);
        assert!(o.kernels.iter().all(KernelRow::same_digest));
        assert!(o.math.iter().all(crate::math::Row::same_result), "die Vektoren tragen das Ergebnis der Norm");
        let report = crate::report::render(&o.report("stm32f401", "thumbv7em", "takt 0.1.0"));
        assert_eq!(crate::report::parse(&report).map(|r| r.probes.len()), Ok(Probe::ORDER.len()));
    }

    /// Ein Protokoll anderer Kerne passt nicht zu den Kostenvektoren dieses
    /// Werkzeugs und wird abgelehnt.
    #[test]
    fn an_import_needs_its_own_suite() {
        let good = synthetic(0, 6);
        let id = good.lines().find_map(|l| l.strip_prefix("takt bench ")).expect("Kopf").to_string();
        let text = good.replace(&format!("takt bench {id}"), "takt bench 0000000000000000");
        let e = import(&Logs { bench: vec![text], ..Logs::default() }).expect_err("fremde Kennung");
        assert!(e.contains("neu bauen"), "{e}");
        assert!(import(&Logs::default()).is_err(), "ohne Lauf");
    }
}
