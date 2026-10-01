//! Der Stimulus, den beide Seiten sehen (12.5).
//!
//! **Warum ein eigener Typ.** Der Rahmen nahm den Stimulus bisher als
//! `(Tick, Name)` — genug fuer ein Command, das nur aus seinem Namen
//! besteht (8.5). Ein Stromelement traegt mehr: den Kanal *und* den
//! Wert. Ein drittes Feld im Tupel haette jeden Aufrufer belastet, auch
//! den, der nur Commands schickt, und `("go", "")` waere ein Command mit
//! leerem Wert oder ein Element ohne — am Typ nicht zu unterscheiden.
//!
//! Der Trace des Interpreters kennt die Unterscheidung laengst (`cmd`
//! gegen `in`, 9.3). `Stimulus` ist dieselbe Unterscheidung fuer den
//! Rahmen, damit die Umrechnung aus einem Trace vollstaendig ist und
//! nicht einen Teil unterwegs verliert.

/// Eine Eingabe an einem Tick (12.5).
#[derive(Clone, Debug, PartialEq)]
pub enum Stimulus {
    /// `cmd <name>`: gilt genau einen Tick (8.5).
    Command {
        /// Tick, an dem das Command anliegt.
        tick: u64,
        /// Name des Commands.
        name: String,
    },
    /// `in <channel> …`: eine Lieferung an den Rand (12.6) — eine
    /// Abtastung eines Skalars oder ein Element eines Eingabestroms (8.6),
    /// je nach Kanal, mit Qualitaet, Alter, Zeitstempel und Folgenummer, wie
    /// der Trace sie schreibt.
    ///
    /// Ein Element ist sofort sichtbar; der Unit-Delay gilt nur fuer
    /// interne Stroeme (9.6).
    Input {
        /// Tick, an dem der Treiber liefert.
        tick: u64,
        /// Name des Eingabekanals.
        channel: String,
        /// Wert, Qualitaet, `age`, `t`, `seq`.
        sample: takt_interp::trace::SampleText,
    },
    /// `tune <name> <wert>` (8.4): der Parameter gilt ab diesem Tick.
    Tune {
        /// Tick der Grenze, ab der der Wert gilt.
        tick: u64,
        /// Name des Tunables.
        name: String,
        /// Der Wert als Text, wie ihn der Trace schreibt.
        text: String,
    },
    /// `abort`: Operator-Abort fuer alle Maschinen (5.4).
    Abort {
        /// Tick, an dem er anliegt.
        tick: u64,
    },
    /// `runtime <art> [<output>]`: ein Runtime-Fault (5.3), fuer alle
    /// Maschinen oder den Besitzer des Outputs.
    Runtime {
        /// Tick, an dem er zugestellt wird.
        tick: u64,
        /// Die Art.
        kind: takt_mir::machine::RuntimeKind,
        /// Der Output eines `Driver`-Faults.
        output: Option<String>,
    },
}

impl Stimulus {
    /// Der Tick, an dem diese Eingabe anliegt.
    pub fn tick(&self) -> u64 {
        match self {
            Stimulus::Command { tick, .. }
            | Stimulus::Input { tick, .. }
            | Stimulus::Tune { tick, .. }
            | Stimulus::Abort { tick }
            | Stimulus::Runtime { tick, .. } => *tick,
        }
    }

    /// Ein Command, kurz geschrieben.
    pub fn cmd(tick: u64, name: &str) -> Stimulus {
        Stimulus::Command { tick, name: name.to_string() }
    }

    /// Ein Stromelement ohne eigenen Zeitstempel und ohne Folgenummer, kurz
    /// geschrieben.
    pub fn element(tick: u64, channel: &str, text: &str) -> Stimulus {
        let sample = takt_interp::trace::SampleText {
            value: Some(text.to_string()),
            quality: None,
            reason: None,
            age: None,
            t: None,
            seq: None,
        };
        Stimulus::Input { tick, channel: channel.to_string(), sample }
    }

    /// Lieferungen, Commands, Aborts und Runtime-Faults aus einem Trace.
    pub fn from_trace(trace: &takt_interp::Trace) -> Vec<Stimulus> {
        use takt_interp::trace::LineKind;
        use takt_mir::machine::RuntimeKind;
        trace
            .lines
            .iter()
            .filter_map(|l| match &l.kind {
                LineKind::Input { channel, sample } => {
                    Some(Stimulus::Input { tick: l.tick, channel: channel.clone(), sample: sample.clone() })
                }
                LineKind::Command { name } => Some(Stimulus::cmd(l.tick, name)),
                LineKind::Abort => Some(Stimulus::Abort { tick: l.tick }),
                LineKind::Runtime { kind, output } => {
                    let kind = match kind.as_str() {
                        "Overrun" => RuntimeKind::Overrun,
                        "Driver" => RuntimeKind::Driver,
                        "Hardware" => RuntimeKind::Hardware,
                        _ => RuntimeKind::Node,
                    };
                    Some(Stimulus::Runtime { tick: l.tick, kind, output: output.clone() })
                }
                _ => None,
            })
            .collect()
    }
}
