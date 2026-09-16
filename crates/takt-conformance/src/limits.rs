//! Was die Abnahme **nicht** prueft.
//!
//! Ein Test, der „ok" meldet, muss sagen, wofuer. Diese Liste steht im
//! Code und nicht nur im Plan, weil sie dort gelesen wird, wo jemand die
//! Zahl fuer eine Zusage haelt — und weil ein Test sie ausgibt, wenn er
//! laeuft.
//!
//! Jede Zeile nennt, was fehlt, warum es fehlt und womit es kommt. Eine
//! Grenze ohne Termin ist eine Ausrede; eine mit Termin ist ein Plan.

/// Eine Grenze der Abnahme.
pub struct Limit {
    /// Was nicht geprueft wird.
    pub was: &'static str,
    /// Warum nicht.
    pub warum: &'static str,
    /// Womit es kommt.
    pub wann: &'static str,
}

/// Die Grenzen, vollstaendig.
///
/// Vollstaendig heisst: Was hier nicht steht, prueft die Abnahme. Wer
/// eine weitere findet, traegt sie ein — eine Liste, die nicht gepflegt
/// wird, ist schlechter als keine, weil ihr jemand glaubt.
pub const LIMITS: &[Limit] = &[
    Limit {
        was: "Record-Muster auf Stroemen (8.7): `when s matches Edge(rising = true) as e`",
        warum: "Der Codegen senkt Muster nur als Text ueber den DFA (11.2); ein Record-Element \
                hat im Byte-Ring der Runtime noch keine Form, also nichts, wogegen ein Guard \
                oder Handler vergleichen koennte. Der Interpreter prueft die Felder auf `Value`.",
        wann: "M6 mit den Geraeteprofilen (8.10) und dem Treiberrand fuer Stroeme (12.6): Dort \
               entsteht die Elementform, die ein Treiber liefert — `Edge` mit Zeitstempel aus \
               dem Timer-Capture (12.3). 14.7 steht bis dahin in `examples.rs` unter OPEN.",
    },
    Limit {
        was: "Skalare Inputs ueber die Zeit",
        warum: "Commands prueft die Abnahme seit Schritt 10 (ein Puls, ein Byte, 8.5); \
                skalare Lieferungen mit Wert, Qualitaet und Alter brauchen die Umrechnung \
                von Trace-Text in Abbild-Bytes, und die gehoert zum Treiber.",
        wann: "Schritt 5 (`takt-rt-linux`): Der Simulationstreiber aus `takt-hal` liefert \
               sie beiden Seiten; er ist bereits die erste HAL-Implementierung (Prinzip 4).",
    },
    Limit {
        was: "Die Abort-Phase (5.4)",
        warum: "`abort` und Runtime-Faults wirken auf *alle* Maschinen im selben Tick. Der \
                Rahmen fuehrt nur eine Maschine und kennt die Phase nicht; der erzeugte Code \
                ruft `takt_abort`, und der Rahmen schreibt es in den Trace, mehr nicht.",
        wann: "Schritt 5 (`takt-rt-linux`): Die Phase gehoert in die Runtime, nicht in den \
               Testrahmen.",
    },
    Limit {
        was: "Mehrere Maschinen und ihre Perioden (7.2)",
        warum: "Der Rahmen ruft den Schritt *einer* Maschine je Tick. Multirate, Ψ mit \
                Unit-Delay und die Reihenfolge der Schritte (Satz 9.4.1) bleiben ungeprueft.",
        wann: "Schritt 5: Die Tickschleife aus 12.1 fuehrt alle Maschinen; `takt-rt-core` hat \
               sie bereits, nur die Bindung an den erzeugten Code fehlt.",
    },
    Limit {
        was: "Ueberlaufende Stroeme (8.6)",
        warum: "Der Rahmen liefert Stromelemente (`streams.rs`), prueft `capacity` und \
                `capacity_bytes` aber je Tick — unter der Annahme eines Konsumenten, der sein \
                Fenster leert. Ein Puffer, der ueber mehrere Ticks volllaeuft, weil niemand \
                liest, braeuchte den Cursor des Konsumenten, und den kennt erst der Lauf. \
                `s.overflowed` und der `StreamOverflow` bleiben darum ungeprueft.",
        wann: "Mit der Runtime: `takt-rt-core::stream` haelt den Ring samt Verdraengung und \
               Eviction; wo der Rahmen rechnet, wuerde sie messen.",
    },
    Limit {
        was: "Faults im Fuzzer",
        warum: "Der Fuzzer meidet sie per Konstruktion: Ein gefaultetes Programm hat nichts \
                mehr zu vergleichen. Die Pruefungen aus 4.1 deckt darum `19_faults.takt` ab, \
                von Hand geschrieben — und FB-86 zeigt, dass genau dort ein Fehler sass.",
        wann: "Offen. Ein Fuzzer, der Faults erzeugt, muesste den Zeitpunkt des Faults \
               vergleichen statt der Outputs danach; das ist ein eigener Vergleich.",
    },
    Limit {
        was: "aarch64",
        warum: "Verglichen wird x86-64 gegen den Interpreter. Satz 9.4.4 verlangt auch \
                x86-64 gegen aarch64 — dieselbe Rechnung auf anderer Hardware.",
        wann: "Schritt 11.",
    },
];

/// Die Grenzen als Text, fuer die Ausgabe eines Testlaufs.
pub fn report() -> String {
    let mut out = String::from("Die Abnahme prueft diese Punkte NICHT:\n");
    for l in LIMITS {
        out.push_str(&format!("  - {}\n      warum: {}\n      wann:  {}\n", l.was, l.warum, l.wann));
    }
    out
}
