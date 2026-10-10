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
        was: "Record-Ausgaenge mit `bytes`-Feld",
        warum: "Der Wirtsrahmen schreibt einen Record-Ausgang Feld fuer Feld; ein Feld `bytes<N>` \
                hat dort keine Schreibweise, und der Vergleich sieht den Ausgang nicht.",
        wann: "M11 Schritt 9.",
        row: "FB-402",
    },
    Limit {
        was: "Die Maschine von `log`, `measure`, `verify` und `verdict`",
        warum: "`compare` haelt diese Beobachtungen je Tick nach Art und Ausgang gegeneinander; \
                der Rahmen nennt ihre Maschine mit der Nummer, nicht mit dem Namen. `job` und \
                `signal` tragen Maschine und Namen (FB-432), `persist` schreibt nur der \
                Wirtsrahmen (`persist_native.rs`).",
        wann: "M11 Schritt 9: Der Produktrahmen schreibt die Beobachtungen mit dem Namen der Maschine.",
        row: "FB-391",
    },
    Limit {
        was: "Das Journal auf dem STM32F401",
        warum: "Das Bring-up des F401 hat keinen Nvm-Treiber: Die `persist`-Programme seines \
                Korpus laufen dort ohne Journal.",
        wann: "M11 Schritt 15: Die Portpruefung faehrt `persist` auf jedem Port; der Port des F401 \
               braucht dafuer den Nvm-Treiber.",
        row: "KON1-022",
    },
    Limit {
        was: "Das Profil `shared` auf dem ESP32-C6",
        warum: "`shared` wird nur auf dem F401 unter RTIC mit nachgebildeter Funk-ISR geprueft; \
                auf dem C6, dem Board mit Funk, laeuft es nicht.",
        wann: "M11 Schritt 21b: ESP-IDF mit aktivem Funk auf dem C6.",
        row: "KON1-021",
    },
    Limit {
        was: "Fehler, die alle Ausfuehrer teilen, ausserhalb der Relationen",
        warum: "Interpreter, Wirtsrahmen und Boards teilen Sema, MIR, den Kern des Treiberrands \
                (`takt-hal`) und die Natives. Was dort falsch ist, sehen nur die Relationen \
                (`relations.rs`: Schrittordnung, Schlaf, `sim` gegen `hw`) und die Golden-Traces \
                der Referenzprogramme — wenn es eine Relation bricht oder einen Golden-Trace \
                aendert. IEEE-Umgebung (`embed.rs`) sowie Praefix und Instanzen \
                (`several_programs.rs`) laufen nur ueber den Korpus ohne Eingaben.",
        wann: "M11 Schritt 9: IEEE-Umgebung, Praefix und Instanzen ueber den erzeugten Eingaben, \
               sobald der Produktrahmen den Stimulus treibt.",
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
        warum: "`UNREACHED` (`differential.rs`) haelt 94 Uebergaenge, 72 nie bestandene und 522 nie verletzte \
                Pruefstellen in 98 Programmen fest, vor allem `range`, `fin`, `valid` und `ovf`: Der Solver findet \
                bis Tiefe 5 in 10 s keinen Pfad, und k-Induktion beweist sie nicht unerreichbar.",
        wann: "M11 Schritt 29, Nachtrag: induktive Invarianten je Stelle (Spacer) und tiefere Pfade.",
        row: "FB-511",
    },
    Limit {
        was: "Das Flash-Modell aus 14.8 im Modell des Beweisers",
        warum: "Interpreter und erzeugter Code rechnen jedes Szenario aus Kapitel 14, das Modell \
                alle bis auf die von 14.8: Der Zustand des Flash-Modells liegt ueber `STATE_LIMIT`, \
                und in `fallback` und `no_image` ordnet der Kodierer seinen Sendestrom keiner \
                kodierten Maschine zu (`MODEL_GAPS` in `examples.rs`).",
        wann: "M11 Schritt 29, Nachtrag: Maps und Puffer als Arrays der SMT-Theorie.",
        row: "FB-497",
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
        let plan = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plan");
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
        assert!(status("FB-402").is_some_and(|s| s.starts_with("offen")));
        assert!(status("FB-361").is_some_and(|s| s.starts_with("behoben")));
        assert_eq!(status("FB-99999"), None);
    }
}
