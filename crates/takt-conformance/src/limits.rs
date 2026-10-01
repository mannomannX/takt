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
        was: "Skalare Inputs zusammengesetzter Typen",
        warum: "Skalare Lieferungen gehen mit Wert, Qualitaet, Alter und Zeitstempel durch den \
                Treiberrand beider Seiten (12.6, M10 Schritt 29b). Der Rahmen schreibt den \
                Wert in der C-Form seines Typs; einen Input vom Typ Record, Array oder \
                `samples` liefert er nicht, und kein Korpusprogramm treibt einen solchen.",
        wann: "Mit dem ersten Programm, das einen braucht: die kanonische Form (5.9) aus dem \
               Trace-Text, wie fuer Stromelemente.",
    },
    Limit {
        was: "Erzeuger von Runtime-Faults auf dem Wirt (5.3)",
        warum: "Abort-Phase, Operator-Abort und Runtime-Faults stehen in beiden Rahmen \
                (FB-332); auf dem Wirt kommen Runtime-Faults nur als Stimulus (`runtime`), \
                weil der Rahmen keine Uhr und keinen Treiber hat. `Overrun` erhebt die \
                MCU-Schleife selbst.",
        wann: "`Driver` mit dem Treiberrand (M10 Schritt 11), `Hardware` mit Tick-Toleranz \
               und MPU (Schritt 18).",
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
        warum: "Jeder Strom hat im Rahmen einen Ring mit Freigabe unter dem kleinsten Cursor. \
                Laeuft ein Eingabering voll, verdraengt `drop_oldest` die aeltesten, sonst \
                faultet der Leser (`streams.rs`, Test `a_reader_that_falls_behind_overflows_the_ring`); \
                laeuft ein interner Ring voll, faultet der Sender im selben Tick (8.6, FB-326, \
                Korpus 88), und `drop` verwirft auf beiden Wegen. `drop_oldest` kennt der \
                interne Ring nicht, und `s.overflowed` liest kein Korpusprogramm. `s.dropped` \
                zaehlt im erzeugten Code nur den Verwurf im `idle` (5.10, Korpus 92), nicht \
                den des Rings.",
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
        was: "Registerports, deren Modell den Lesekanal als Strom stellt (12.10)",
        warum: "Ein Register, das beim Lesen weiterschaltet (ein FIFO-Datenregister), stellt \
                das Modell als `stream<Regs>`. Der Wirtsrahmen bildet nur einen Pegel ab und \
                bricht den Bau des Rahmens fuer einen Strom mit `#error` ab; kein \
                Korpusprogramm liest ein solches Register.",
        wann: "Mit dem ersten Korpusprogramm, das eines liest: eine Warteschlange je Port, \
               gespeist aus dem Sendepuffer des Modells wie `Image::port_queues`.",
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
