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

    /// Der Stimulus eines Traces: jede Zeile, die der Interpreter als
    /// Eingabe nimmt (`apply_stimulus`), ohne eine zu verlieren.
    ///
    /// Lieferungen, Commands, angenommene Tunes, Aborts und Runtime-Faults
    /// werden zu Eingaben. Ein verworfener Tune bleibt wie im Interpreter
    /// ohne Wirkung, ebenso die Beobachtungszeilen eines Laufs. Eine
    /// aufgezeichnete Job-Fertigstellung kann der Rahmen nicht nachspielen,
    /// und eine unbekannte Runtime-Art lehnt der Interpreter ab: beides ist
    /// ein Fehler, kein stilles Weglassen.
    pub fn from_trace(trace: &takt_interp::Trace) -> Result<Vec<Stimulus>, String> {
        use takt_interp::trace::LineKind;
        use takt_mir::machine::RuntimeKind;
        let mut out = Vec::new();
        for l in &trace.lines {
            let tick = l.tick;
            match &l.kind {
                LineKind::Input { channel, sample } => {
                    out.push(Stimulus::Input { tick, channel: channel.clone(), sample: sample.clone() });
                }
                LineKind::Command { name } => out.push(Stimulus::cmd(tick, name)),
                LineKind::Tune { name, value, accepted: true } => {
                    out.push(Stimulus::Tune { tick, name: name.clone(), text: value.clone() });
                }
                LineKind::Abort => out.push(Stimulus::Abort { tick }),
                LineKind::Runtime { kind, output } => {
                    let kind = match kind.as_str() {
                        "Overrun" => RuntimeKind::Overrun,
                        "Driver" => RuntimeKind::Driver,
                        "Hardware" => RuntimeKind::Hardware,
                        "Node" => RuntimeKind::Node,
                        other => return Err(format!("t={tick}: Runtime-Fault `{other}` gibt es nicht")),
                    };
                    out.push(Stimulus::Runtime { tick, kind, output: output.clone() });
                }
                LineKind::Job { machine, handle } => {
                    return Err(format!("t={tick}: `job {machine} {handle} done` kann der Rahmen nicht nachspielen"));
                }
                LineKind::Tune { accepted: false, .. }
                | LineKind::Output { .. }
                | LineKind::State { .. }
                | LineKind::Published { .. }
                | LineKind::Signal { .. }
                | LineKind::Fault { .. }
                | LineKind::Log { .. }
                | LineKind::Alert { .. }
                | LineKind::Measure { .. }
                | LineKind::Verify { .. }
                | LineKind::Verdict { .. }
                | LineKind::Property { .. }
                | LineKind::Stream { .. }
                | LineKind::Driver { .. }
                | LineKind::Final { .. }
                | LineKind::End { .. }
                | LineKind::Persist { .. }
                | LineKind::Time { .. }
                | LineKind::Record { .. } => {}
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takt_mir::machine::RuntimeKind;

    fn stimulus(text: &str) -> Result<Vec<Stimulus>, String> {
        Stimulus::from_trace(&takt_interp::Trace::parse(text).expect("lesbar"))
    }

    /// Jede Eingabezeile wird eine Eingabe, in ihrer Reihenfolge; Beobachtungen
    /// und ein verworfener Tune bleiben ohne Wirkung wie im Interpreter.
    #[test]
    fn every_input_line_becomes_a_stimulus() {
        let got = stimulus(
            "t=1 in rx READY\nt=2 cmd go\nt=3 tune GAIN 5\nt=3 tune GAIN 900 rejected\nt=4 abort\n\
             t=5 runtime Overrun\nt=6 runtime Driver o\nt=7 runtime Hardware\nt=8 runtime Node\n\
             t=9 out o 1\nt=9 state m RUN\nt=9 fault m Range \"x\" -> SAFE\nt=9 log m \"x\"\n\
             t=9 time took=0 drift=0 slept=0\n",
        )
        .expect("Stimulus");
        let kinds: Vec<(u64, String)> = got
            .iter()
            .map(|s| {
                let what = match s {
                    Stimulus::Input { channel, sample, .. } => format!("in {channel} {:?}", sample.value),
                    Stimulus::Command { name, .. } => format!("cmd {name}"),
                    Stimulus::Tune { name, text, .. } => format!("tune {name} {text}"),
                    Stimulus::Abort { .. } => "abort".into(),
                    Stimulus::Runtime { kind, output, .. } => format!("runtime {kind:?} {output:?}"),
                };
                (s.tick(), what)
            })
            .collect();
        let want: Vec<(u64, String)> = [
            (1, "in rx Some(\"READY\")"),
            (2, "cmd go"),
            (3, "tune GAIN 5"),
            (4, "abort"),
            (5, "runtime Overrun None"),
            (6, "runtime Driver Some(\"o\")"),
            (7, "runtime Hardware None"),
            (8, "runtime Node None"),
        ]
        .into_iter()
        .map(|(t, w)| (t, w.to_string()))
        .collect();
        assert_eq!(kinds, want);
        assert!(matches!(got[6], Stimulus::Runtime { kind: RuntimeKind::Hardware, .. }));
    }

    /// Was der Rahmen nicht nachspielen kann oder der Interpreter ablehnt,
    /// ist ein Fehler und kein stilles Weglassen.
    #[test]
    fn a_line_the_frame_cannot_replay_is_an_error() {
        assert!(stimulus("t=3 job m h done\n").is_err_and(|e| e.contains("job m h done")));
        assert!(stimulus("t=3 runtime Brownout\n").is_err_and(|e| e.contains("Brownout")));
    }

    /// Eine Lieferung mit Wert und Qualitaet liest sich so zurueck, wie der
    /// Interpreter sie schreibt (`grammar/trace.md`: `t=2 in lox_temp 90 K
    /// stale age=120 ms`): Wert, Qualitaet und Alter getrennt.
    #[test]
    fn a_value_with_its_quality_reads_back_as_written() {
        let got = stimulus("t=2 in p 12.0 bar stale age=60 ms\n").expect("Stimulus");
        let Some(Stimulus::Input { sample, .. }) = got.first() else { panic!("{got:?}") };
        assert_eq!(
            (sample.value.as_deref(), sample.quality.as_deref(), sample.age.as_deref()),
            (Some("12.0 bar"), Some("stale"), Some("60 ms")),
            "{sample:?}"
        );
    }
}
