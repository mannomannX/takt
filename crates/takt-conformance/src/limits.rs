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
        was: "Der Korpus mit Eingaben",
        warum: "Die Abnahme faehrt jedes Korpusprogramm ohne Stimulus: Commands, Lieferungen und \
                Tunes kommen nur in den Einzeltests der Abnahme vor (`differential.rs`, \
                `streams.rs`, `driver_edge.rs`). Was ein Programm erst auf eine Eingabe hin tut, \
                vergleicht der Korpuslauf nicht.",
        wann: "M11 Schritte 9 und 29: Stimuli je Programm aus dem Manifest.",
        row: "FB-376",
    },
    Limit {
        was: "Skalare Inputs zusammengesetzter Typen",
        warum: "Skalare Lieferungen gehen mit Wert, Qualitaet, Alter und Zeitstempel durch den \
                Treiberrand beider Seiten (12.6, M10 Schritt 29b). Der Rahmen schreibt den \
                Wert in der C-Form seines Typs; einen Input vom Typ Record, Array oder \
                `samples` liefert er nicht, und kein Korpusprogramm treibt einen solchen.",
        wann: "M11 Schritt 29, mit dem ersten Programm, das einen braucht: die kanonische Form \
               (5.9) aus dem Trace-Text, wie fuer Stromelemente.",
        row: "FB-430",
    },
    Limit {
        was: "Record-Ausgaenge mit `bytes`-Feld",
        warum: "Der Wirtsrahmen schreibt einen Record-Ausgang Feld fuer Feld; ein Feld `bytes<N>` \
                hat dort keine Schreibweise, und der Vergleich sieht den Ausgang nicht.",
        wann: "M11 Schritt 9.",
        row: "FB-402",
    },
    Limit {
        was: "Registerports, deren Modell den Lesekanal als Strom stellt (12.10)",
        warum: "Ein Register, das beim Lesen weiterschaltet (ein FIFO-Datenregister), stellt \
                das Modell als `stream<Regs>`. Der Wirtsrahmen bildet nur einen Pegel ab und \
                bricht den Bau des Rahmens fuer einen Strom mit `#error` ab; kein \
                Korpusprogramm liest ein solches Register.",
        wann: "M11 Schritt 29, mit dem ersten Korpusprogramm, das eines liest: eine \
               Warteschlange je Port, gespeist aus dem Sendepuffer des Modells wie \
               `Image::port_queues`.",
        row: "FB-431",
    },
    Limit {
        was: "Tunables in der Runtime (8.4)",
        warum: "Rahmen, Huelle und Wirtsharness nehmen einen Tune ueber `P_tune` mit den \
                Pruefungen des Interpreters an; die Bring-ups der Boards reichen keinen von der \
                Konsole dorthin weiter.",
        wann: "M11 Schritt 10 (Bring-ups auf takt-embed).",
        row: "FB-389",
    },
    Limit {
        was: "Beobachtungen ohne Gegenstueck im Rahmen",
        warum: "`compare` haelt `log`, `measure`, `verify`, `verdict`, `property` und `end` \
                gegeneinander; `job`, `signal` und `verdict-final` schreibt kein Rahmen, \
                `persist` nur der Wirtsrahmen (`persist_native.rs`). Die Maschine einer \
                Beobachtung nennt der Rahmen mit ihrer Nummer, verglichen wird darum je Tick \
                Art und Ausgang, nicht die Maschine.",
        wann: "M11 Schritt 29: die Zeilen im Rahmen nachziehen.",
        row: "FB-432",
    },
    Limit {
        was: "Das Journal auf dem STM32F401",
        warum: "Das Bring-up des F401 hat keinen Nvm-Treiber: Die `persist`-Programme seines \
                Korpus laufen dort ohne Journal.",
        wann: "M11 Schritt 29.",
        row: "KON1-022",
    },
    Limit {
        was: "Das Profil `shared` auf dem ESP32-C6",
        warum: "`shared` wird nur auf dem F401 unter RTIC mit nachgebildeter Funk-ISR geprueft; \
                auf dem C6, dem Board mit Funk, laeuft es nicht.",
        wann: "M11 Schritt 29.",
        row: "KON1-021",
    },
    Limit {
        was: "Fehler, die alle Ausfuehrer teilen",
        warum: "Interpreter, Wirtsrahmen und Boards teilen den Kern des Treiberrands \
                (`takt-hal`) und die Natives; ein Fehler dort ist auf allen Seiten gleich und \
                fiele keinem Vergleich auf.",
        wann: "M11 Schritt 29: exakte Erwartungen neben dem Vergleich.",
        row: "FB-382",
    },
    Limit {
        was: "Das Modell des Beweisers als Ausfuehrer",
        warum: "Der Beweiser kodiert das Programm in sein eigenes Modell; ob es rechnet wie \
                Interpreter und Codegen, vergleicht nur `takt-prove/tests` an wenigen \
                Programmen.",
        wann: "M11 Schritt 28.",
        row: "FB-381",
    },
    Limit {
        was: "aarch64 und armv7 (12.8: 64-Bit Linux, 32-Bit mit f64-FPU)",
        warum: "Verglichen wird x86-64 gegen den Interpreter, auf den Boards Cortex-M4F und \
                RV32IMAC. Fuer aarch64 und armv7 belegt die Abnahme die gleiche IR und den Bau \
                (`every_target_gets_the_same_ir`, `the_corpus_compiles_for_the_32_bit_class_with_f64`); \
                der Lauf unter qemu braucht die Linux-Kette (`targets.rs`), und 12.8 verlangt \
                die Messung je Zielklasse auf einem Geraet.",
        wann: "M11 Schritt 29 fuer den Lauf unter qemu; die Messung mit einem Geraet der Klasse \
               (etwa einem Raspberry Pi 3 oder 4 und einem Pi 2).",
        row: "KON2-013",
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
        assert!(status("FB-376").is_some_and(|s| s.starts_with("offen")));
        assert!(status("FB-361").is_some_and(|s| s.starts_with("behoben")));
        assert_eq!(status("FB-99999"), None);
    }
}
