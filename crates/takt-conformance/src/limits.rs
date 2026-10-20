//! Was die Abnahme **nicht** prueft.
//!
//! Ein Test, der „ok" meldet, muss sagen, wofuer. Diese Liste steht im
//! Code und nicht nur im Plan, weil sie dort gelesen wird, wo jemand die
//! Zahl fuer eine Zusage haelt — und weil ein Test sie ausgibt, wenn er
//! laeuft.
//!
//! Jede Zeile nennt, was fehlt, warum es fehlt, womit es kommt und die
//! offene Zeile im Plan, die es verfolgt (`plan/feedback.csv` oder
//! `plan/tests.csv`). Eine Grenze ohne Termin ist eine Ausrede; eine ohne
//! offene Zeile geraet in Vergessenheit, und eine, deren Zeile geschlossen
//! ist, ist keine Grenze mehr (KON1-029).

/// Eine Grenze der Abnahme.
pub struct Limit {
    /// Was nicht geprueft wird.
    pub was: &'static str,
    /// Warum nicht.
    pub warum: &'static str,
    /// Womit es kommt: ein Schritt oder Meilenstein.
    pub wann: &'static str,
    /// Die offene Zeile im Plan, die die Grenze verfolgt; leer, wo es sie
    /// noch nicht gibt.
    pub row: &'static str,
}

/// Die Grenzen, vollstaendig.
///
/// Vollstaendig heisst: Was hier nicht steht, prueft die Abnahme. Wer
/// eine weitere findet, traegt sie ein — eine Liste, die nicht gepflegt
/// wird, ist schlechter als keine, weil ihr jemand glaubt.
pub const LIMITS: &[Limit] = &[
    Limit {
        was: "Das Profil `shared` auf dem ESP32-C6",
        warum: "`shared` wird nur auf dem F401 unter RTIC mit nachgebildeter Funk-ISR geprueft; \
                auf dem C6, dem Board mit Funk, laeuft es nicht.",
        wann: "M11 Schritt 21b: ESP-IDF mit aktivem Funk auf dem C6.",
        row: "KON1-021",
    },
    Limit {
        was: "Fehler, die alle Ausfuehrer teilen, ausserhalb der Relationen",
        warum: "Interpreter, Produktrahmen und Boards teilen Sema, MIR, den Kern des Treiberrands \
                (`takt-hal`) und die Natives. Was dort falsch ist, sehen nur die Relationen \
                (`relations.rs`: Schrittordnung, Schlaf, `sim` gegen `hw`) und die Golden-Traces \
                der Referenzprogramme — wenn es eine Relation bricht oder einen Golden-Trace \
                aendert. IEEE-Umgebung, Praefix und eine gefuellte Arena laufen ueber jeden Fall \
                der Suite `ieee` (`embed.rs`), mehrere Instanzen im SDK nur ueber den Korpus ohne \
                Eingaben (`several_programs.rs`): Das SDK hat eine Leitung je Prozess.",
        wann: "M11: zwei Instanzen im Testwirt, sobald das SDK eine Leitung je Kern hat (FB-691).",
        row: "FB-382",
    },
    Limit {
        was: "aarch64 und armv7 auf einem Geraet (12.8: 64-Bit Linux, 32-Bit mit f64-FPU)",
        warum: "aarch64 laeuft unter qemu gegen x86-64 und den Interpreter (`targets.rs`, im \
                Container aus `tools/Dockerfile.linux`); armv7 belegt die Abnahme nur ueber IR und \
                Bau (`every_target_gets_the_same_ir`, \
                `the_corpus_compiles_for_the_32_bit_class_with_f64`). 12.8 verlangt die Messung je \
                Zielklasse auf einem Geraet.",
        wann: "M11 Schritt 17 (Wirt und `linux_rt`), mit einem Geraet der Klasse: etwa einem \
               Raspberry Pi 3 oder 4 und einem Pi 2.",
        row: "FB-501",
    },
    Limit {
        was: "Pruefstellen und Uebergaenge, die weder ein Lauf erreicht noch der Solver ausschliesst",
        warum: "`UNREACHED` (`differential.rs`) haelt 93 Uebergaenge, 50 nie bestandene und 283 nie verletzte \
                Pruefstellen in 95 Programmen fest, vor allem `range`, `fin`, `valid` und `ovf`: Der Solver findet \
                bis Tiefe 5 in 60 s keinen Pfad, k-Induktion beweist sie nicht unerreichbar, und der lange Lauf \
                ohne Stimulus ueber `MAX_TICKS` (`paths::from_run`) faultet dort nicht. Jede Spalte steht je \
                Programm mit ihren Gruppen als Grund (`GROUPS`). Uebergaenge: ohne Teilmodell 31 (FB-604), Frist \
                27, Fragment 16, Speicher 13, bewiesen und nicht nachgerechnet 4, nicht k-induktiv 2. Nie \
                bestanden: Frist 25, Fragment 6, bewiesene Range 6, ohne Teilmodell 5, in einer Meldung 4, \
                faultet stets 3, unbestaetigt 1. Nie verletzt: Speicher 96, Frist 80, ohne Teilmodell 70, \
                Fragment 12, nicht k-induktiv 12, bewiesen und nicht nachgerechnet 5, Zeitschranke 3 (FB-614), \
                jenseits `MAX_TICKS` 2, ausserhalb der Suite `beweiser` 2, ein bewiesener `check` 1. Eine \
                Funktion, die keine Maschine ruft, zaehlt nicht (13.2, FB-619). Dazu 186 Stellen, die nie faulten koennen (13.2: 167 `proven`, 10 \
                `message`, 9 `observe`).",
        wann: "M11 Schritt 29, Nachtrag (Strang B2): induktive Invarianten je Stelle (Schritt 2), dann der \
               Speicher (FB-612).",
        row: "FB-511",
    },
    Limit {
        was: "Pfade und Beweise des Solvers fuer `45_journal_cut` und `110_journal_log`",
        warum: "Keine Maschine der beiden laesst sich allein kodieren (ein gekoppelter `sim`-Strom, ein \
                zusammengesetzter fremder Output), also fragt der Solver jede Stelle am Gesamtmodell: 112 MB je \
                Anfrage in Tiefe 5, und z3 entscheidet eine solche auch in 300 s nicht. Ihre Stellen haelt \
                `UNREACHED`.",
        wann: "M11 Strang B2: Teilmodelle mit gekoppeltem Strom und zusammengesetztem fremdem Output.",
        row: "FB-604",
    },
];

/// Die Grenzen als Text, fuer die Ausgabe eines Testlaufs.
pub fn report() -> String {
    let mut out = String::from("Die Abnahme prueft diese Punkte NICHT:\n");
    for l in LIMITS {
        let row = if l.row.is_empty() { "ohne Zeile" } else { l.row };
        out.push_str(&format!("  - {} ({row})\n      warum: {}\n      wann:  {}\n", l.was, l.warum, l.wann));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suites::csv_rows;

    /// Der Status der Zeile `id` in `plan/feedback.csv` oder `plan/tests.csv`.
    fn status(id: &str) -> Option<String> {
        let plan = crate::repo_root().join("plan");
        ["feedback.csv", "tests.csv"].iter().find_map(|file| {
            let text = std::fs::read_to_string(plan.join(file)).ok()?;
            let mut rows = csv_rows(&text);
            let head = rows.next()?;
            let at = head.iter().position(|c| c == "Status")?;
            rows.find(|r| r.first().is_some_and(|c| c == id)).and_then(|r| r.get(at).cloned())
        })
    }

    /// **Jede Grenze hat einen Termin und eine offene Zeile im Plan**
    /// (KON1-029): Der Termin nennt einen Schritt oder Meilenstein, die Zeile
    /// steht in `plan/feedback.csv` oder `plan/tests.csv` und ist offen. Eine
    /// Grenze, deren Zeile geschlossen ist, gehoert aus der Liste; eine ohne
    /// Zeile braucht eine.
    #[test]
    fn every_limit_names_a_step_and_an_open_row() {
        let mut failed = Vec::new();
        for l in LIMITS {
            assert!(!l.was.is_empty() && !l.warum.is_empty(), "eine Grenze ohne Begruendung");
            if !(l.wann.contains("Schritt")
                || l.wann.split_whitespace().any(|w| w.starts_with('M') && w[1..].starts_with(char::is_numeric)))
            {
                failed.push(format!("`{}`: der Termin nennt keinen Schritt und keinen Meilenstein", l.was));
            }
            match status(l.row) {
                _ if l.row.is_empty() => failed.push(format!("`{}`: keine Zeile im Plan", l.was)),
                Some(s) if s.starts_with("offen") || s.starts_with("teilweise") => {}
                Some(s) => failed.push(format!("`{}`: {} ist `{s}`", l.was, l.row)),
                None => failed.push(format!("`{}`: {} steht nicht im Plan", l.was, l.row)),
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// Die Pruefung liest den Plan richtig: eine offene, eine geschlossene und
    /// eine fehlende Zeile.
    #[test]
    fn the_status_comes_from_the_plan() {
        assert!(status("FB-501").is_some_and(|s| s.starts_with("offen")));
        assert!(status("FB-361").is_some_and(|s| s.starts_with("behoben")));
        assert_eq!(status("FB-99999"), None);
    }
}
