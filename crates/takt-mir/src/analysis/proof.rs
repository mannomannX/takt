//! Die Beweisdatei `.takt-proof` (Referenz 11.3): Pruefstellen, die
//! `takt prove` als unerreichbar bewiesen hat, neben dem Programm.
//!
//! Zeilen: `takt-proof 1`, `program <sha256 der Quelle>`, je Stelle
//! `site <Versatz> <Art> k=<Tiefe>`. Der Hash bindet die Datei an genau
//! diese Quelle; der Codegen laesst nur Pruefungen aus, deren Beweis zur
//! Quelle passt. Der Interpreter prueft weiter (`RangeOrigin::Proven`).

use crate::analysis::walk::tag_of_name;

/// Eine bewiesene Pruefstelle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    /// Versatz der Stelle in der Quelle (`Span::start`).
    pub start: u32,
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
    pub fn tags(&self) -> Vec<(u32, u8)> {
        self.sites.iter().filter_map(|s| Some((s.start, tag_of_name(&s.kind)?))).collect()
    }
}

/// Liest eine Beweisdatei.
pub fn parse(text: &str) -> Result<Proof, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#'));
    if lines.next() != Some("takt-proof 1") {
        return Err("erste Zeile muss `takt-proof 1` sein".into());
    }
    let mut proof = Proof::default();
    for (n, line) in lines.enumerate() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["program", hash] => proof.program = (*hash).to_string(),
            ["site", start, kind, k] => {
                let start = start.parse::<u32>().map_err(|e| format!("Zeile {}: Versatz: {e}", n + 2))?;
                let k = k.strip_prefix("k=").and_then(|k| k.parse::<u32>().ok());
                let Some(k) = k else { return Err(format!("Zeile {}: `k=<n>` erwartet", n + 2)) };
                if tag_of_name(kind).is_none() {
                    return Err(format!("Zeile {}: unbekannte Art `{kind}`", n + 2));
                }
                proof.sites.push(Site { start, kind: (*kind).to_string(), k });
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
    let mut out = format!("takt-proof 1\nprogram {program}\n");
    for s in sites {
        out.push_str(&format!("site {} {} k={}\n", s.start, s.kind, s.k));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_file_round_trips() {
        let sites = vec![Site { start: 120, kind: "range".into(), k: 5 }, Site { start: 7, kind: "div".into(), k: 3 }];
        let text = render("abc", &sites);
        let proof = parse(&text).expect("lesbar");
        assert_eq!(proof.program, "abc");
        assert_eq!(proof.sites, sites);
        assert_eq!(proof.tags().len(), 2);
    }

    #[test]
    fn a_wrong_header_or_kind_is_refused() {
        assert!(parse("takt-proof 2\nprogram x\n").is_err());
        assert!(parse("takt-proof 1\nprogram x\nsite 1 magic k=1\n").is_err());
        assert!(parse("takt-proof 1\nsite 1 range k=1\n").is_err());
    }
}
