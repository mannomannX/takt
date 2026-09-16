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
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stimulus {
    /// `cmd <name>`: gilt genau einen Tick (8.5).
    Command {
        /// Tick, an dem das Command anliegt.
        tick: u64,
        /// Name des Commands.
        name: String,
    },
    /// `in <channel> <text>`: ein Element eines Eingabestroms (8.6).
    ///
    /// Es kommt vom Rand und ist sofort sichtbar; der Unit-Delay gilt
    /// nur fuer interne Stroeme (9.6).
    Element {
        /// Tick, an dem der Treiber liefert.
        tick: u64,
        /// Name des Eingabekanals.
        channel: String,
        /// Der Inhalt als Text, wie ihn der Trace schreibt.
        text: String,
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
}

impl Stimulus {
    /// Der Tick, an dem diese Eingabe anliegt.
    pub fn tick(&self) -> u64 {
        match self {
            Stimulus::Command { tick, .. } | Stimulus::Element { tick, .. } | Stimulus::Tune { tick, .. } => *tick,
        }
    }

    /// Ein Command, kurz geschrieben.
    pub fn cmd(tick: u64, name: &str) -> Stimulus {
        Stimulus::Command { tick, name: name.to_string() }
    }

    /// Ein Stromelement, kurz geschrieben.
    pub fn element(tick: u64, channel: &str, text: &str) -> Stimulus {
        Stimulus::Element { tick, channel: channel.to_string(), text: text.to_string() }
    }
}
