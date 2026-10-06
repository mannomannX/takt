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

/// Die Vektoren aus den Bloecken ```` ```takt-native ```` und
/// ```` ```takt-crypto ````: die Natives aus `takt-native` und die Jobs aus
/// `takt-crypto` (4.5), die jedes Ziel ebenso rechnen muss.
pub fn vectors(spec: &str) -> Result<Vec<Vector>, String> {
    let mut out = Vec::new();
    let mut inside = false;
    for (i, raw) in spec.lines().enumerate() {
        let (line, at) = (raw.trim(), i + 1);
        if line.starts_with("```") {
            inside = line == "```takt-native" || line == "```takt-crypto";
            continue;
        }
        if !inside || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (head, _) = line.split_once(':').ok_or_else(|| format!("Zeile {at}: kein `:`"))?;
        let mut words = head.split_whitespace();
        let name = words.next().ok_or_else(|| format!("Zeile {at}: keine Funktion"))?;
        let native = Native::by_name(name).ok_or_else(|| format!("Zeile {at}: `{name}` ist keine native Funktion"))?;
        let inputs = words
            .map(|w| hex(w).ok_or_else(|| format!("Zeile {at}: `{w}` ist kein Hex")))
            .collect::<Result<Vec<_>, _>>()?;
        out.push(Vector { line: at, native, inputs });
    }
    Ok(out)
}

/// Eine Hexfolge; ein Bindestrich ist die leere Eingabe, `@f32/SEED` und
/// `@f64/SEED` die erzeugten Eingaben von `fft256`.
fn hex(word: &str) -> Option<Vec<u8>> {
    if word == "-" {
        return Some(Vec::new());
    }
    if word.starts_with('@') {
        let mut out = [0u8; takt_native::fft::BYTES_F64];
        let len = takt_native::fft::generated(word, &mut out)?;
        return Some(out[..len].to_vec());
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
/// Byte, hoechstwertiges zuerst, ein Digest Byte fuer Byte, die Werte von
/// `fft256` als SHA-256 ihrer kanonischen Form.
pub fn render(out: &Output) -> String {
    match out.digest_or_scalar() {
        Ok(v) => format!("{v:016x}"),
        Err(d) => hex_of(&d),
    }
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Was der Wirt zu einem Vektor rechnet, in der Schreibweise des
/// Messprogramms. Die Jobs aus `takt-crypto` rechnet `takt-crypto`: ein
/// `bool` als Pruefsumme, ein Klartext als SHA-256, `-` ohne Ergebnis.
pub fn expected(v: &Vector) -> Option<String> {
    let inputs: Vec<&[u8]> = v.inputs.iter().map(Vec::as_slice).collect();
    let bit = |ok: bool| render(&Output::Scalar(u64::from(ok)));
    match (v.native, inputs.as_slice()) {
        (Native::EcdsaP256Verify, [key, digest, sig]) => {
            match (<&[u8; 64]>::try_from(*key), <&[u8; 32]>::try_from(*digest), <&[u8; 64]>::try_from(*sig)) {
                (Ok(key), Ok(digest), Ok(sig)) => takt_crypto::ecdsa_p256_verify(key, digest, sig).ok().map(bit),
                _ => Some(bit(false)),
            }
        }
        (Native::Rsa3072Verify, [key, digest, sig]) => takt_crypto::rsa3072_verify(key, digest, sig).ok().map(bit),
        (Native::AesGcmDecrypt, [key, nonce, aad, data, tag]) => {
            let mut out = vec![0u8; data.len()];
            Some(match takt_crypto::aes_gcm_decrypt(key, nonce, aad, data, tag, &mut out).ok()? {
                Some(n) => hex_of(&takt_native::sha256::sha256(&out[..n])),
                None => "-".to_string(),
            })
        }
        _ => takt_native::call(v.native, &inputs).map(|o| render(&o)),
    }
}

/// Was das Messprogramm zu einem Vektor meldet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Measured {
    /// Der Platz des Vektors in der Tabelle.
    pub index: usize,
    /// Das Ergebnis wie [`render`].
    pub output: String,
    /// Der Stack-Bedarf seines Einstiegs in Byte.
    pub stack: u32,
    /// Die weiteren Einstiege, die der Vektor ruft, mit ihrem Stack-Bedarf:
    /// Anfang und Ende einer Kette `sha256_init/update/final`.
    pub others: Vec<(Native, u32)>,
}

/// Liest die Zeilen `native <i> <ergebnis> stack <byte> [<name> <byte>]...`.
pub fn parse(text: &str) -> Result<Vec<Measured>, String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("native "))
        .map(|rest| {
            let bytes = |n: &str| n.parse::<u32>().map_err(|_| format!("`native {rest}`: keine Byte-Zahl"));
            let words: Vec<&str> = rest.split_whitespace().collect();
            let [i, output, "stack", n, others @ ..] = words.as_slice() else {
                return Err(format!("unlesbar: `native {rest}`"));
            };
            if others.len() % 2 != 0 {
                return Err(format!("unlesbar: `native {rest}`"));
            }
            let others = others
                .chunks_exact(2)
                .map(|pair| {
                    let f =
                        Native::by_name(pair[0]).ok_or_else(|| format!("`native {rest}`: `{}` unbekannt", pair[0]))?;
                    Ok((f, bytes(pair[1])?))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Measured {
                index: i.parse().map_err(|_| format!("`native {rest}`: kein Index"))?,
                output: output.to_string(),
                stack: bytes(n)?,
                others,
            })
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

    /// Haelt die Funktion ihre Zusage? Null Byte ist eine Messung: Eine
    /// Blattfunktion wie `crc32` braucht auf dem C6 keinen eigenen Rahmen. Ob
    /// das Malen ueberhaupt lief, prueft [`painted`] am ganzen Lauf.
    pub fn within_contract(&self) -> bool {
        self.stack <= self.contract
    }
}

/// Lief das Malen des Stacks? Ein Lauf, in dem jede Native und jede Funktion
/// der Mathematik null Byte meldet, hat nichts gemessen: Die grossen unter
/// ihnen (`sha256`, `fft256`, `exp_f64`) brauchen Hunderte Byte.
pub fn painted(natives: &[Row], math: &[crate::math::Row]) -> bool {
    natives.iter().any(|r| r.stack > 0) || math.iter().any(|r| r.stack.is_some_and(|s| s > 0))
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
        let want = expected(v).ok_or_else(|| {
            format!("Zeile {}: `{}` nimmt nicht {} Eingaben", v.line, v.native.name(), v.inputs.len())
        })?;
        let row = row_of(&mut rows, v.native);
        row.vectors += 1;
        row.stack = row.stack.max(m.stack);
        if m.output != want {
            row.deviations.push(v.line);
        }
        for &(f, stack) in &m.others {
            let row = row_of(&mut rows, f);
            row.vectors += 1;
            row.stack = row.stack.max(stack);
        }
    }
    rows.sort_by_key(|r| Native::ALL.iter().position(|n| *n == r.native));
    Ok(rows)
}

/// Die Zeile einer Funktion, angelegt mit ihrer Zusage, wenn es sie noch
/// nicht gibt.
fn row_of(rows: &mut Vec<Row>, native: Native) -> &mut Row {
    let at = match rows.iter().position(|r| r.native == native) {
        Some(at) => at,
        None => {
            let contract = takt_native::cost_of(native).stack;
            rows.push(Row { native, vectors: 0, deviations: Vec::new(), stack: 0, contract });
            rows.len() - 1
        }
    };
    &mut rows[at]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> String {
        std::fs::read_to_string(spec_path()).expect("grammar/takt-native.md lesbar")
    }

    /// Die Spezifikation traegt Vektoren fuer jede Funktion der Menge, auch
    /// fuer die Jobs aus `takt-crypto`, und der Wirt rechnet zu jedem einen
    /// Erwartungswert.
    #[test]
    fn the_spec_yields_the_vectors() {
        let v = vectors(&spec()).expect("lesbar");
        assert!(v.len() >= 32, "{}", v.len());
        for f in Native::ALL {
            assert!(v.iter().any(|v| v.native.vector_name() == f.vector_name()), "{} ohne Vektor", f.name());
        }
        assert!(v.iter().all(|v| expected(v).is_some()), "ein Vektor ohne Erwartungswert");
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
            others: Vec::new(),
        };
        let rows = judge(&v, &[good(0), good(1)]).expect("vollstaendig");
        assert_eq!(rows.len(), 1);
        assert!(rows[0].same_result() && rows[0].within_contract(), "{rows:?}");
        let bad = Measured { output: "0000000000000001".into(), ..good(1) };
        let rows = judge(&v, &[good(0), bad]).expect("vollstaendig");
        assert_eq!(rows[0].deviations, vec![48]);
    }

    /// Ein Stack ueber der Zusage bricht sie, null Byte nicht (Blattfunktion).
    #[test]
    fn a_stack_above_the_contract_breaks_it() {
        let row = |stack| Row { native: Native::Crc32, vectors: 1, deviations: Vec::new(), stack, contract: 64 };
        assert!(row(64).within_contract());
        assert!(!row(65).within_contract());
        assert!(row(0).within_contract());
    }

    /// Meldet jede Funktion null Byte, lief das Malen nicht (KON1-033);
    /// eine einzelne Null mit gemessenen Nachbarn ist eine Messung.
    #[test]
    fn a_run_where_every_stack_is_zero_measured_nothing() {
        let row = |stack| Row { native: Native::Crc32, vectors: 1, deviations: Vec::new(), stack, contract: 64 };
        assert!(painted(&[row(0), row(40)], &[]));
        assert!(!painted(&[row(0), row(0)], &[]));
    }

    /// Das Board meldet, was `parse` liest.
    #[test]
    fn the_board_lines_read_back() {
        let text = "takt natives stm32f401\r\nnative 0 00000000d202ef8d stack 24\r\ntakt end\r\n";
        assert_eq!(
            parse(text).expect("lesbar"),
            vec![Measured { index: 0, output: "00000000d202ef8d".into(), stack: 24, others: Vec::new() }]
        );
        assert!(parse("native 0 zu kurz\n").is_err());
        assert!(parse("native 0 00 stack 24 sha256_init\n").is_err());
    }

    /// Eine Kette misst Anfang und Ende mit; jede der drei Funktionen hat
    /// ihre Zeile.
    #[test]
    fn a_chain_reports_each_entry() {
        let v = vec![Vector { line: 70, native: Native::Sha256Update, inputs: vec![b"abc".to_vec()] }];
        let digest = render(&takt_native::call(Native::Sha256Update, &[b"abc".as_slice()]).expect("rechnet"));
        let text = format!("native 0 {digest} stack 400 sha256_init 40 sha256_final 420\n");
        let rows = judge(&v, &parse(&text).expect("lesbar")).expect("vollstaendig");
        let stacks: Vec<(Native, u32)> = rows.iter().map(|r| (r.native, r.stack)).collect();
        assert_eq!(stacks, [(Native::Sha256Init, 40), (Native::Sha256Update, 400), (Native::Sha256Final, 420)]);
        assert!(rows.iter().all(Row::same_result), "{rows:?}");
    }
}
