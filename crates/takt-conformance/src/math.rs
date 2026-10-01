//! Die korrekt gerundete Mathematik auf dem Board (4.2, 13.8): bitgleiche
//! Ergebnisse ueber die Ziele, der Stack gegen den Vertrag und die Zyklen
//! eines Aufrufs.
//!
//! Die Vektoren der Stufe 2 stehen normativ in `grammar/libtaktm.md`;
//! `libtaktm/tests/vectors.rs` prueft sie auf dem Wirt. Das Messprogramm
//! `natives` der Bring-ups ruft dieselben Einstiege `takt_m_*` aus
//! `takt-native-abi`, die der erzeugte Code ruft, und schreibt je Vektor
//! `math <i> <ergebnis> stack <byte> cycles <zyklen>`. Ein Ergebnis gleich
//! der Norm ist das korrekt gerundete; der Stack gilt gegen
//! [`takt_mir::analysis::stack::MATH_STACK`], die Zyklen gegen das Gewicht
//! `math` der Kalibrierung (FB-344).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use takt_mir::analysis::stack::MATH_STACK;

/// Die Funktionen der Stufe 2, wie die Vektoren sie nennen.
pub const FUNCTIONS: [&str; 10] = ["exp", "log", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "pow"];

/// Eine Vektorzeile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// Zeile in der Spezifikation.
    pub line: usize,
    /// Die Funktion.
    pub fun: &'static str,
    /// `f64` oder `f32`.
    pub wide: bool,
    /// Die Argumente als Bitmuster; das zweite nur bei `atan2` und `pow`.
    pub args: [u64; 2],
    /// Das korrekt gerundete Ergebnis.
    pub want: u64,
}

/// Die Spezifikation im Repository.
pub fn spec_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../grammar/libtaktm.md")
}

/// Die Vektoren der Stufe 2 aus dem Block ```` ```libtaktm ````.
pub fn vectors(spec: &str) -> Result<Vec<Vector>, String> {
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in spec.lines().enumerate() {
        let (line, at) = (raw.trim(), i + 1);
        if line.starts_with("```") {
            inside = line == "```libtaktm";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || format!("Zeile {at}: `{line}` ist keine Vektorzeile");
        let (head, tail) = line.split_once(':').ok_or_else(bad)?;
        let (width, name) = head.split_once(' ').ok_or_else(bad)?;
        let Some(fun) = FUNCTIONS.iter().copied().find(|f| *f == name.trim()) else { continue };
        let (args, want) = tail.split_once("->").ok_or_else(bad)?;
        let hex = |s: &str| u64::from_str_radix(s.trim(), 16).map_err(|_| bad());
        let args: Vec<u64> = args.split_whitespace().map(hex).collect::<Result<_, _>>()?;
        let wide = match width {
            "f64" => true,
            "f32" => false,
            _ => return Err(bad()),
        };
        let (Some(&x), y) = (args.first(), args.get(1).copied().unwrap_or(0)) else { return Err(bad()) };
        out.push(Vector { line: at, fun, wide, args: [x, y], want: hex(want)? });
    }
    Ok(out)
}

/// Die Tabelle fuer das Messprogramm: Funktion, Breite und Argumente.
pub fn table_source(vectors: &[Vector]) -> String {
    let mut s = String::from(
        "/// Die Vektoren der Stufe 2 aus `grammar/libtaktm.md`: Funktion, `f64`, Argumente.\n\
         pub static VECTORS: &[(&str, bool, u64, u64)] = &[\n",
    );
    for v in vectors {
        let _ = writeln!(s, "    ({:?}, {}, {:#x}, {:#x}),", v.fun, v.wide, v.args[0], v.args[1]);
    }
    s.push_str("];\n");
    s
}

/// Eine Zeile des Messprogramms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    /// Der Platz des Vektors in der Tabelle.
    pub index: usize,
    /// Das Ergebnis als Bitmuster.
    pub result: u64,
    /// Der Stack-Bedarf des Einstiegs in Byte.
    pub stack: u32,
    /// Die Zyklen eines Aufrufs.
    pub cycles: u32,
}

/// Liest die Zeilen `math <i> <ergebnis> stack <byte> cycles <zyklen>`.
pub fn parse(text: &str) -> Result<Vec<Measured>, String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("math "))
        .map(|rest| {
            let bad = || format!("unlesbar: `math {rest}`");
            let words: Vec<&str> = rest.split_whitespace().collect();
            let [i, result, "stack", stack, "cycles", cycles] = words.as_slice() else { return Err(bad()) };
            Ok(Measured {
                index: i.parse().map_err(|_| bad())?,
                result: u64::from_str_radix(result, 16).map_err(|_| bad())?,
                stack: stack.parse().map_err(|_| bad())?,
                cycles: cycles.parse().map_err(|_| bad())?,
            })
        })
        .collect()
}

/// Das Urteil ueber eine Funktion in einer Breite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Die Funktion.
    pub fun: &'static str,
    /// `f64` oder `f32`.
    pub wide: bool,
    /// Gerechnete Vektoren.
    pub vectors: u32,
    /// Zeilen der Spezifikation, deren Ergebnis abweicht.
    pub deviations: Vec<usize>,
    /// Der groesste gemessene Stack-Bedarf in Byte.
    pub stack: u32,
    /// Die meisten Zyklen eines Aufrufs.
    pub cycles: u32,
}

impl Row {
    /// Der Name mit Breite, wie ihn Bericht und Meldungen schreiben.
    pub fn name(&self) -> String {
        format!("{}_{}", self.fun, if self.wide { "f64" } else { "f32" })
    }

    /// Bitgleich mit der Norm in jedem Vektor?
    pub fn same_result(&self) -> bool {
        self.deviations.is_empty()
    }

    /// Unter dem Stackvertrag der Mathematik?
    pub fn within_contract(&self) -> bool {
        self.stack <= MATH_STACK
    }
}

/// Vergleicht die Messung mit der Norm; je Funktion und Breite eine Zeile,
/// `f64` zuerst, in der Reihenfolge von [`FUNCTIONS`].
pub fn judge(vectors: &[Vector], measured: &[Measured]) -> Result<Vec<Row>, String> {
    if measured.len() != vectors.len() {
        return Err(format!("das Board meldet {} von {} Vektoren der Mathematik", measured.len(), vectors.len()));
    }
    let mut rows: Vec<Row> = Vec::new();
    for (i, v) in vectors.iter().enumerate() {
        let m = measured.iter().find(|m| m.index == i).ok_or_else(|| format!("Vektor {i} fehlt"))?;
        let at = match rows.iter().position(|r| r.fun == v.fun && r.wide == v.wide) {
            Some(at) => at,
            None => {
                rows.push(Row { fun: v.fun, wide: v.wide, vectors: 0, deviations: Vec::new(), stack: 0, cycles: 0 });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.vectors += 1;
        row.stack = row.stack.max(m.stack);
        row.cycles = row.cycles.max(m.cycles);
        if m.result != v.want {
            row.deviations.push(v.line);
        }
    }
    rows.sort_by_key(|r| (!r.wide, FUNCTIONS.iter().position(|f| *f == r.fun)));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Spezifikation traegt jede Funktion in beiden Breiten.
    #[test]
    fn the_spec_yields_every_function_in_both_widths() {
        let spec = std::fs::read_to_string(spec_path()).expect("grammar/libtaktm.md lesbar");
        let v = vectors(&spec).expect("lesbar");
        for f in FUNCTIONS {
            for wide in [true, false] {
                assert!(v.iter().any(|v| v.fun == f && v.wide == wide), "{f} ohne Vektor (f64: {wide})");
            }
        }
        assert!(table_source(&v).contains("(\"pow\", true, 0x4000000000000000, 0xc090cc0000000000),"));
    }

    /// Eine Abweichung nennt ihre Zeile, Stack und Zyklen sind das Maximum.
    #[test]
    fn a_deviation_names_its_line() {
        let v = vec![
            Vector { line: 7, fun: "exp", wide: true, args: [0, 0], want: 0x3ff0_0000_0000_0000 },
            Vector { line: 8, fun: "exp", wide: true, args: [1, 0], want: 0x3ff0_0000_0000_0000 },
        ];
        let text = "math 0 3ff0000000000000 stack 400 cycles 9000\nmath 1 3ff0000000000001 stack 420 cycles 8000\n";
        let rows = judge(&v, &parse(text).expect("lesbar")).expect("vollstaendig");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].deviations, vec![8]);
        assert_eq!((rows[0].stack, rows[0].cycles), (420, 9000));
        assert_eq!(rows[0].name(), "exp_f64");
    }
}
