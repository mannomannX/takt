//! Die Beweisdatei `.takt-proof` (Referenz 11.3): Pruefstellen, die
//! `takt prove` als unerreichbar bewiesen hat, neben dem Programm.
//!
//! Zeilen: `takt-proof 2`, `program <sha256 der Quelle>`, je Stelle
//! `site <Anfang> <Ende> <Art> k=<Tiefe>`. Der Hash bindet die Datei an genau
//! diese Quelle; der Codegen laesst nur Pruefungen aus, deren Beweis zur
//! Quelle passt. Der Interpreter prueft weiter (`RangeOrigin::Proven`).
//!
//! Fassung 1 nannte nur den Anfang. Zwei Pruefungen gleicher Art mit
//! demselben Anfang (`a + 1 + b`) teilten den Schluessel, und ein Beweis
//! fuer die eine erliess beide (FB-393); eine solche Datei wird abgelehnt.

use crate::analysis::walk::tag_of_name;

/// Die Datei, deren Hash eine Beweisdatei traegt: die Quelle des Programms,
/// nicht das Prelude.
pub const SOURCE: u32 = 0;

/// Eine bewiesene Pruefstelle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Anfang der Stelle in der Quelle (`Span::start`).
    pub start: u32,
    /// Ihr Ende (`Span::end`).
    pub end: u32,
    /// Art der Pruefung, wie `takt check --checks` sie nennt.
    pub kind: String,
    /// Induktionstiefe des Beweises.
    pub k: u32,
}

/// Der Inhalt einer Beweisdatei.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proof {
    /// SHA-256 der Quelle, hexadezimal.
    pub program: String,
    /// Die bewiesenen Stellen.
    pub sites: Vec<Site>,
}

impl Proof {
    /// Die Stellen als Schluessel der Analyse.
    pub fn tags(&self) -> Vec<(u32, u32, u8)> {
        self.sites.iter().filter_map(|s| Some((s.start, s.end, tag_of_name(&s.kind)?))).collect()
    }
}

/// Liest eine Beweisdatei.
pub fn parse(text: &str) -> Result<Proof, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    match lines.next() {
        Some("takt-proof 2") => {}
        Some("takt-proof 1") => {
            return Err("Fassung 1 nennt nur den Anfang einer Stelle und ist mehrdeutig (FB-393); \
                        `takt prove --save-proof` schreibt sie neu"
                .into());
        }
        _ => return Err("erste Zeile muss `takt-proof 2` sein".into()),
    }
    let mut proof = Proof::default();
    for (n, line) in lines.enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["program", _] if !proof.program.is_empty() => {
                return Err(format!("Zeile {}: zweite `program`-Zeile", n + 2));
            }
            ["program", hash] => proof.program = (*hash).to_string(),
            ["site", start, end, kind, k] => {
                let start = start.parse::<u32>().map_err(|e| format!("Zeile {}: Anfang: {e}", n + 2))?;
                let end = end.parse::<u32>().map_err(|e| format!("Zeile {}: Ende: {e}", n + 2))?;
                let k = k.strip_prefix("k=").and_then(|k| k.parse::<u32>().ok());
                let Some(k) = k else { return Err(format!("Zeile {}: `k=<n>` erwartet", n + 2)) };
                if tag_of_name(kind).is_none() {
                    return Err(format!("Zeile {}: unbekannte Art `{kind}`", n + 2));
                }
                proof.sites.push(Site { start, end, kind: (*kind).to_string(), k });
            }
            _ => return Err(format!("Zeile {}: `{line}` unverstanden", n + 2)),
        }
    }
    if proof.program.is_empty() {
        return Err("`program <hash>` fehlt".into());
    }
    Ok(proof)
}

/// Schreibt eine Beweisdatei.
pub fn render(program: &str, sites: &[Site]) -> String {
    let mut out = format!("takt-proof 2\nprogram {program}\n");
    for s in sites {
        out.push_str(&format!("site {} {} {} k={}\n", s.start, s.end, s.kind, s.k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_file_round_trips() {
        let sites = vec![
            Site { start: 120, end: 131, kind: "range".into(), k: 5 },
            Site { start: 7, end: 12, kind: "div".into(), k: 3 },
        ];
        let text = render("abc", &sites);
        let proof = parse(&text).expect("lesbar");
        assert_eq!(proof.program, "abc");
        assert_eq!(proof.sites, sites);
        assert_eq!(proof.tags(), vec![(120, 131, 5), (7, 12, 0)]);
    }

    #[test]
    fn a_wrong_header_or_kind_is_refused() {
        assert!(
            parse(
                "takt-proof 3
program x
"
            )
            .is_err()
        );
        assert!(
            parse(
                "takt-proof 2
program x
site 1 2 magic k=1
"
            )
            .is_err()
        );
        assert!(
            parse(
                "takt-proof 2
site 1 2 range k=1
"
            )
            .is_err()
        );
    }

    /// FB-393: Fassung 1 nannte nur den Anfang; ihr Beweis liess jede
    /// Pruefung derselben Art am selben Anfang fallen. Sie wird abgelehnt,
    /// mit dem Weg zur neuen.
    #[test]
    fn a_proof_of_the_first_edition_is_refused() {
        let e = parse(
            "takt-proof 1
program x
site 1 range k=1
",
        )
        .expect_err("mehrdeutig");
        assert!(e.contains("--save-proof"), "{e}");
    }

    /// SYN-026: Was keine Zeile der Datei ist, lehnt der Leser mit Zeile ab
    /// — auch eine zweite `program`-Zeile, die sonst die erste ersetzte und
    /// die Datei an eine andere Quelle bindete.
    #[test]
    fn every_malformed_line_is_refused() {
        let head = "takt-proof 2
program abc
";
        let cases = [
            format!(
                "{head}site 1 2 range
"
            ),
            format!(
                "{head}site 1 range k=1
"
            ),
            format!(
                "{head}site 1 2 range k=1 extra
"
            ),
            format!(
                "{head}site x 2 range k=1
"
            ),
            format!(
                "{head}site 1 y range k=1
"
            ),
            format!(
                "{head}site -1 2 range k=1
"
            ),
            format!(
                "{head}site 1 2 range k=
"
            ),
            format!(
                "{head}site 1 2 range 5
"
            ),
            format!(
                "{head}program
"
            ),
            format!(
                "{head}program abc def
"
            ),
            format!(
                "{head}program def
"
            ),
            format!(
                "{head}bogus 1
"
            ),
            "takt-proof
program abc
"
            .to_string(),
        ];
        for text in &cases {
            assert!(parse(text).is_err(), "angenommen: {text:?}");
        }
        let ok = format!(
            "# Kommentar
{head}
site 0 0 range k=0
"
        );
        assert_eq!(parse(&ok).map(|p| p.sites.len()), Ok(1), "k = 0 und Versatz 0 sind lesbar");
    }
}
