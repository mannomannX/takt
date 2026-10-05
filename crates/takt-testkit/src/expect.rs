//! Erwartete Diagnosen je Datei, ein Leser fuer alle Korpora (13.8,
//! Schritt 25): `corpus-try/checks/`, die Negativprogramme und jede Datei,
//! die eine Diagnose verlangt.
//!
//! ```text
//! output led : bool @ hw("o/led")   #~ SC-15
//! #~^ SC-13, SC-15@8
//! ```
//!
//! `#~ CODE` erwartet eine Diagnose mit diesem Code in derselben Zeile,
//! `#~^ CODE` in der Zeile davor; mehrere Codes trennt ein Komma, `@n`
//! verlangt die Spalte (1-basiert). Der Formatter schreibt `# ~` (F7);
//! beide Schreibweisen gelten. Die Schwere steht nicht in der Datei: Fuer
//! die Pruefungen nennt sie Tabelle 10, und eine zweite Quelle koennte ihr
//! widersprechen.

/// Eine erwartete Diagnose.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Expected {
    /// Zeile, 1-basiert.
    pub line: u32,
    /// Code (`SC-15`, `E_INDENT`, `P`).
    pub code: String,
    /// Spalte, 1-basiert, wenn die Anmerkung sie verlangt.
    pub col: Option<u32>,
}

/// Eine Diagnose, wie ein Test sie gegen die Erwartungen haelt.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Actual {
    /// Zeile, 1-basiert.
    pub line: u32,
    /// Spalte, 1-basiert.
    pub col: u32,
    /// Code.
    pub code: String,
}

/// Die Erwartungen einer Datei, in der Reihenfolge ihrer Zeilen.
///
/// # Panics
///
/// Wenn eine Spalte keine Zahl ist.
pub fn expectations(src: &str) -> Vec<Expected> {
    let mut out = Vec::new();
    for (i, text) in src.lines().enumerate() {
        let Some(rest) = marker(text) else { continue };
        let (up, rest) = match rest.strip_prefix('^') {
            Some(r) => (true, r),
            None => (false, rest),
        };
        let line = u32::try_from(i).expect("Zeilenzahl") + u32::from(!up);
        for item in rest.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (code, col) = match item.split_once('@') {
                Some((code, n)) => {
                    let col = n.parse().unwrap_or_else(|_| panic!("Zeile {}: Spalte `{n}` ist keine Zahl", i + 1));
                    (code, Some(col))
                }
                None => (item, None),
            };
            out.push(Expected { line, code: code.to_string(), col });
        }
    }
    out
}

/// Der Text hinter `#~` oder `# ~`.
fn marker(text: &str) -> Option<&str> {
    let tight = text.find("#~").map(|p| p + 2);
    let spaced = text.find("# ~").map(|p| p + 3);
    let start = match (tight, spaced) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    Some(text[start..].trim())
}

/// Was nicht stimmt: erwartete Diagnosen, die fehlen, und Diagnosen, die
/// niemand erwartet. Jede Anmerkung steht fuer genau eine Diagnose
/// (Mehrfachmenge): Zwei gleiche Meldungen in einer Zeile brauchen zwei
/// Anmerkungen. Eine verlangte Spalte bindet die Diagnose an dieser Spalte.
pub fn mismatches(expected: &[Expected], actual: &[Actual]) -> Vec<String> {
    let mut free = vec![true; actual.len()];
    let mut missing = vec![false; expected.len()];
    // Erst die Anmerkungen mit Spalte: Eine ohne Spalte soll keine Diagnose
    // belegen, die eine mit Spalte braucht.
    let mut order: Vec<usize> = (0..expected.len()).collect();
    order.sort_by_key(|&i| expected[i].col.is_none());
    for i in order {
        let e = &expected[i];
        let hit = (0..actual.len()).find(|&j| {
            let a = &actual[j];
            free[j] && a.line == e.line && a.code == e.code && e.col.is_none_or(|c| c == a.col)
        });
        match hit {
            Some(j) => free[j] = false,
            None => missing[i] = true,
        }
    }
    let mut out = Vec::new();
    for (e, _) in expected.iter().zip(&missing).filter(|(_, m)| **m) {
        let at = e.col.map(|c| format!(":{c}")).unwrap_or_default();
        out.push(format!("erwartet {} in Zeile {}{at}", e.code, e.line));
    }
    for (a, _) in actual.iter().zip(&free).filter(|(_, f)| **f) {
        out.push(format!("unerwartet {} in Zeile {}:{}", a.code, a.line, a.col));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_spellings_and_the_line_above_are_read() {
        let src = "a #~ SC-1\n# ~^ SC-2, SC-3@5\nb\n";
        let want = vec![
            Expected { line: 1, code: "SC-1".into(), col: None },
            Expected { line: 1, code: "SC-2".into(), col: None },
            Expected { line: 1, code: "SC-3".into(), col: Some(5) },
        ];
        assert_eq!(expectations(src), want);
    }

    #[test]
    fn a_missing_an_unexpected_and_a_misplaced_diagnostic_are_named() {
        let expected = expectations("x #~ SC-1@3\ny #~ SC-2\n");
        let actual =
            vec![Actual { line: 1, col: 4, code: "SC-1".into() }, Actual { line: 3, col: 1, code: "SC-9".into() }];
        let got = mismatches(&expected, &actual);
        // Die verrutschte Diagnose steht mit ihrer wirklichen Spalte da.
        assert_eq!(
            got,
            [
                "erwartet SC-1 in Zeile 1:3",
                "erwartet SC-2 in Zeile 2",
                "unerwartet SC-1 in Zeile 1:4",
                "unerwartet SC-9 in Zeile 3:1"
            ],
            "{got:?}"
        );
    }

    #[test]
    fn matching_diagnostics_leave_nothing() {
        let expected = expectations("x #~ SC-1, SC-1@2\n");
        let actual =
            vec![Actual { line: 1, col: 5, code: "SC-1".into() }, Actual { line: 1, col: 2, code: "SC-1".into() }];
        assert!(mismatches(&expected, &actual).is_empty(), "{:?}", mismatches(&expected, &actual));
    }

    /// Jede Anmerkung steht fuer genau eine Diagnose: Eine Doppelmeldung in
    /// derselben Zeile faellt auf, ebenso eine fehlende zweite.
    #[test]
    fn annotations_count_like_a_multiset() {
        let one = expectations("x #~ SC-2\n");
        let twice =
            vec![Actual { line: 1, col: 1, code: "SC-2".into() }, Actual { line: 1, col: 5, code: "SC-2".into() }];
        assert_eq!(mismatches(&one, &twice), ["unerwartet SC-2 in Zeile 1:5"]);
        let two = expectations("x #~ SC-2, SC-2\n");
        assert!(mismatches(&two, &twice).is_empty());
        assert_eq!(mismatches(&two, &twice[..1]), ["erwartet SC-2 in Zeile 1"]);
    }
}
