//! Die kuratierten Natives auf dem Board (4.5, 13.8): bitgleiche
//! Ergebnisse ueber die Ziele und der Stack-Bedarf gegen die Zusage.
//!
//! 13.8 nimmt eine native Funktion erst in die kuratierte Menge, wenn ihre
//! Ergebnisse auf allen Zielen bitgleich sind, und misst den Stack-Bedarf,
//! den sie als `stack` zusagt (4.5, 12.3). Die Vektoren stehen normativ in
//! `grammar/takt-native.md`; `takt-native/tests/vectors.rs` prueft sie auf
//! dem Wirt gegen die erwarteten Werte. Hier laufen dieselben Eingaben im
//! Messprogramm `natives` der Bring-ups, und verglichen wird mit dem Wirt:
//! Was dort der Norm entspricht und hier gleich ist, entspricht ihr auch
//! hier.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use takt_native::{Native, Output};

/// Eine Vektorzeile: Funktion und Eingaben.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vector {
    /// Zeile in der Spezifikation.
    pub line: usize,
    /// Die Funktion.
    pub native: Native,
    /// Die Eingaben in ihrer Reihenfolge.
    pub inputs: Vec<Vec<u8>>,
}

/// Die Spezifikation im Repository.
pub fn spec_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../grammar/takt-native.md")
}

/// Die Vektoren aus dem Block ```` ```takt-native ````, ohne die Natives,
/// deren Implementierung ausserhalb liegt (`takt-crypto`).
pub fn vectors(spec: &str) -> Result<Vec<Vector>, String> {
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in spec.lines().enumerate() {
        let (line, at) = (raw.trim(), i + 1);
        if line.starts_with("```") {
            inside = line == "```takt-native";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, _) = line.split_once(':').ok_or_else(|| format!("Zeile {at}: kein `:`"))?;
        let mut words = head.split_whitespace();
        let name = words.next().ok_or_else(|| format!("Zeile {at}: keine Funktion"))?;
        let native = Native::by_name(name).ok_or_else(|| format!("Zeile {at}: `{name}` ist keine native Funktion"))?;
        if native.external() {
            continue;
        }
        let inputs = words
            .map(|w| hex(w).ok_or_else(|| format!("Zeile {at}: `{w}` ist kein Hex")))
            .collect::<Result<Vec<_>, _>>()?;
        out.push(Vector { line: at, native, inputs });
    }
    Ok(out)
}

/// Eine Hexfolge; ein Bindestrich ist die leere Eingabe.
fn hex(word: &str) -> Option<Vec<u8>> {
    if word == "-" {
        return Some(Vec::new());
    }
    if word.len() % 2 != 0 {
        return None;
    }
    (0..word.len()).step_by(2).map(|i| u8::from_str_radix(word.get(i..i + 2)?, 16).ok()).collect()
}

/// Die Vektoren als Rust-Quelltext fuer das Messprogramm: je Zeile der
/// Name der Funktion und ihre Eingaben.
pub fn table_source(vectors: &[Vector]) -> String {
    let mut s = String::from(
        "/// Die Vektoren aus `grammar/takt-native.md`: Funktion und Eingaben.\n\
         pub static VECTORS: &[(&str, &[&[u8]])] = &[\n",
    );
    for v in vectors {
        let inputs: Vec<String> = v
            .inputs
            .iter()
            .map(|b| format!("&[{}]", b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ")))
            .collect();
        let _ = writeln!(s, "    ({:?}, &[{}]),", v.native.name(), inputs.join(", "));
    }
    s.push_str("];\n");
    s
}

/// Ein Ergebnis in der Schreibweise des Messprogramms
/// (`takt_rt_baremetal::bench::write_native`): eine Pruefsumme als acht
/// Byte, hoechstwertiges zuerst, ein Digest Byte fuer Byte.
pub fn render(out: &Output) -> String {
    match out {
        Output::Scalar(v) => format!("{v:016x}"),
        Output::Digest(d) => d.iter().map(|b| format!("{b:02x}")).collect(),
    }
}

/// Was das Messprogramm zu einem Vektor meldet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    /// Der Platz des Vektors in der Tabelle.
    pub index: usize,
    /// Das Ergebnis wie [`render`].
    pub output: String,
    /// Der Stack-Bedarf des Aufrufs in Byte.
    pub stack: u32,
}

/// Liest die Zeilen `native <i> <ergebnis> stack <byte>`.
pub fn parse(text: &str) -> Result<Vec<Measured>, String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("native "))
        .map(|rest| match rest.split_whitespace().collect::<Vec<_>>().as_slice() {
            [i, output, "stack", n] => Ok(Measured {
                index: i.parse().map_err(|_| format!("`native {rest}`: kein Index"))?,
                output: output.to_string(),
                stack: n.parse().map_err(|_| format!("`native {rest}`: keine Byte-Zahl"))?,
            }),
            _ => Err(format!("unlesbar: `native {rest}`")),
        })
        .collect()
}

/// Das Urteil ueber eine Funktion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Die Funktion.
    pub native: Native,
    /// Gerechnete Vektoren.
    pub vectors: u32,
    /// Zeilen der Spezifikation, deren Ergebnis vom Wirt abweicht.
    pub deviations: Vec<usize>,
    /// Der groesste gemessene Stack-Bedarf in Byte.
    pub stack: u32,
    /// Die Zusage `stack` (4.5).
    pub contract: u32,
}

impl Row {
    /// Bitgleich mit dem Wirt in jedem Vektor?
    pub fn same_result(&self) -> bool {
        self.deviations.is_empty()
    }

    /// Haelt die Funktion ihre Zusage?
    pub fn within_contract(&self) -> bool {
        self.stack <= self.contract
    }
}

/// Vergleicht die Messung mit dem Wirt und der Zusage; je Funktion eine
/// Zeile, in der Reihenfolge von [`Native::ALL`].
pub fn judge(vectors: &[Vector], measured: &[Measured]) -> Result<Vec<Row>, String> {
    if measured.len() != vectors.len() {
        return Err(format!("das Board meldet {} von {} Vektoren", measured.len(), vectors.len()));
    }
    let mut rows: Vec<Row> = Vec::new();
    for (i, v) in vectors.iter().enumerate() {
        let m = measured.iter().find(|m| m.index == i).ok_or_else(|| format!("Vektor {i} fehlt"))?;
        let inputs: Vec<&[u8]> = v.inputs.iter().map(Vec::as_slice).collect();
        let want = takt_native::call(v.native, &inputs)
            .map(|o| render(&o))
            .ok_or_else(|| format!("Zeile {}: `{}` nimmt nicht {} Eingaben", v.line, v.native.name(), inputs.len()))?;
        let at = match rows.iter().position(|r| r.native == v.native) {
            Some(at) => at,
            None => {
                let contract = takt_native::cost_of(v.native).stack;
                rows.push(Row { native: v.native, vectors: 0, deviations: Vec::new(), stack: 0, contract });
                rows.len() - 1
            }
        };
        let row = &mut rows[at];
        row.vectors += 1;
        row.stack = row.stack.max(m.stack);
        if m.output != want {
            row.deviations.push(v.line);
        }
    }
    rows.sort_by_key(|r| Native::ALL.iter().position(|n| *n == r.native));
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> String {
        std::fs::read_to_string(spec_path()).expect("grammar/takt-native.md lesbar")
    }

    /// Die Spezifikation traegt Vektoren fuer jede Funktion, die hier
    /// laeuft; die aus `takt-crypto` bleiben draussen.
    #[test]
    fn the_spec_yields_the_vectors() {
        let v = vectors(&spec()).expect("lesbar");
        assert!(v.len() >= 32, "{}", v.len());
        assert!(v.iter().all(|v| !v.native.external()));
        assert!(v.iter().any(|v| v.native == Native::Crc32 && v.inputs == vec![Vec::<u8>::new()]), "leere Eingabe");
    }

    /// Die Tabelle nennt jede Zeile mit Namen und Bytes.
    #[test]
    fn the_table_carries_names_and_bytes() {
        let v = vec![Vector { line: 1, native: Native::Crc16, inputs: vec![vec![0xff], Vec::new()] }];
        let s = table_source(&v);
        assert!(s.contains("(\"crc16\", &[&[0xff], &[]]),"), "{s}");
    }

    /// Gleiche Ergebnisse, Stack unter der Zusage: kein Befund; ein
    /// anderes Ergebnis nennt seine Zeile.
    #[test]
    fn a_deviation_names_its_line() {
        let v = vec![
            Vector { line: 44, native: Native::Crc32, inputs: vec![Vec::new()] },
            Vector { line: 48, native: Native::Crc32, inputs: vec![vec![0]] },
        ];
        let good = |i: usize| Measured {
            index: i,
            output: render(&takt_native::call(Native::Crc32, &[v[i].inputs[0].as_slice()]).expect("rechnet")),
            stack: 12,
        };
        let rows = judge(&v, &[good(0), good(1)]).expect("vollstaendig");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].same_result() && rows[0].within_contract(), "{rows:?}");
        let bad = Measured { output: "0000000000000001".into(), ..good(1) };
        let rows = judge(&v, &[good(0), bad]).expect("vollstaendig");
        assert_eq!(rows[0].deviations, vec![48]);
    }

    /// Das Board meldet, was `parse` liest.
    #[test]
    fn the_board_lines_read_back() {
        let text = "takt natives stm32f401\r\nnative 0 00000000d202ef8d stack 24\r\ntakt end\r\n";
        assert_eq!(
            parse(text).expect("lesbar"),
            vec![Measured { index: 0, output: "00000000d202ef8d".into(), stack: 24 }]
        );
        assert!(parse("native 0 zu kurz\n").is_err());
    }
}
