//! Der defensive Treiberrand (12.6): die sieben Pruefungen.
//!
//! | # | Pruefung | Bei Verletzung |
//! |---|---|---|
//! | 1 | Zeitstempel im Tickfenster | in Toleranz geklemmt, `time_warped`, Alert; darueber wie 2 |
//! | 2 | Zeitstempel/`seq` steigend, `seq` lueckenlos, Menge <= `MAXPT`, Flags konsistent | alle Inputs des Treibers `Bad`/`Driver` |
//! | 3 | Werte in deklarierten Ranges | `Bad`/`OutOfRange` (mit `debounce` erst `Suspect`) |
//! | 4 | `max_slew` | `Bad`/`Implausible` (mit `debounce` erst `Suspect`) |
//! | 5 | Record-Stroeme: `decode` erfolgreich | Element verworfen, `malformed`, Alert |
//! | 6 | Schreiben bestaetigt, Heartbeat, Sendepuffer nicht ueberfahren | `Runtime(Driver)` fuer die Besitzer |
//! | 7 | Tick-Periode in `tick_tolerance` | `Runtime(Hardware)` fuer alle Maschinen |
//!
//! Die Pruefungen 3 und 4 stehen in `quality`, weil sie einen Zustand je
//! Input haben: den letzten guten Wert. Die uebrigen stehen hier.
//!
//! **Warum ein Vertragsbruch alle Kanaele des Treibers trifft.** 12.6
//! Zeile 2 sagt es so, und der Grund ist, dass die Pruefung nicht den Wert
//! misst, sondern den Treiber: Wer Folgenummern durcheinanderbringt, bei
//! dem ist auch der unauffaellige Kanal nur zufaellig unauffaellig.

use takt_mir::ChannelId;
use takt_mir::program::{Channel, Direction, Program};

use crate::driver::{Delivery, Element, Reading, Writing};
use crate::quality::{Gate, Limits, Quality, Scalar, Verdict};

/// Was der Rand meldet, ohne die Steuerung zu beeinflussen (12.6).
///
/// Ein Alert ist eine Beobachtung, kein Fault: 12.6 trennt beides
/// ausdruecklich, und nur die Zeilen 6 und 7 erzeugen Faults.
#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    /// Zeitstempel lag ausserhalb des Fensters und wurde geklemmt (Zeile 1).
    TimeWarped {
        /// Betroffener Channel.
        channel: ChannelId,
        /// Der gelieferte Zeitstempel.
        got: i64,
        /// Worauf geklemmt wurde.
        clamped: i64,
    },
    /// Der Treiber haelt seinen Vertrag nicht (Zeile 2).
    DriverDegraded {
        /// Name des Treibers.
        driver: String,
        /// Was verletzt wurde.
        what: Contract,
    },
    /// Der Treiber liefert wieder vertragsgemaess (Zeile 2, Erholung).
    DriverRecovered {
        /// Name des Treibers.
        driver: String,
    },
    /// Ein Stromelement liess sich nicht decodieren (Zeile 5).
    Malformed {
        /// Betroffener Channel.
        channel: ChannelId,
    },
}

/// Welcher Teil des Treibervertrags verletzt wurde (12.6, Zeile 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contract {
    /// Zeitstempel nicht streng steigend.
    Timestamp,
    /// `seq` nicht streng steigend oder mit Luecke.
    Sequence,
    /// Mehr Elemente als `MAXPT` in einem Tick (8.6).
    TooMany,
    /// Qualitaetsflags widerspruechlich: `Bad` mit Wert.
    Flags,
    /// Zeitstempel jenseits der Klemmtoleranz (Zeile 1, zweiter Fall).
    TimeWindow,
}

impl Contract {
    /// Die Verletzung als Text fuer Meldung und Trace.
    pub fn name(self) -> &'static str {
        match self {
            Contract::Timestamp => "Zeitstempel",
            Contract::Sequence => "Folgenummer",
            Contract::TooMany => "zu viele Elemente",
            Contract::Flags => "Qualitaetsflags",
            Contract::TimeWindow => "Zeitfenster",
        }
    }
}

/// Was eine Randpruefung fuer die Runtime ergibt.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Abtastungen, die ins Prozessabbild gehen.
    pub readings: Vec<(ChannelId, Verdict)>,
    /// Indizes der Elemente, die zugestellt werden duerfen.
    pub accepted: Vec<usize>,
    /// Beobachtungen fuer Telemetrie und Trace.
    pub alerts: Vec<Alert>,
    /// Outputs, deren Besitzer `Runtime(Driver)` bekommen (Zeile 6).
    pub driver_faults: Vec<ChannelId>,
}

/// Der Rand eines Treibers: sein Zustand zwischen den Ticks.
#[derive(Debug)]
pub struct Edge {
    /// Qualitaetsmaschine je Channel (3.5).
    gates: Vec<Gate>,
    /// Ausgewertete Grenzen je Channel.
    limits: Vec<Limits>,
    /// Letzter akzeptierter Zeitstempel je Channel (Zeile 2).
    last_t: Vec<Option<i64>>,
    /// Letzte akzeptierte Folgenummer je Stream-Channel (Zeile 2).
    last_seq: Vec<Option<i64>>,
    /// `MAXPT` je Stream-Channel (8.6).
    maxpt: Vec<Option<usize>>,
    /// Laeuft der Treiber gerade degradiert (Zeile 2)?
    degraded: bool,
    /// Toleranz fuer Pruefung 1 in Nanosekunden.
    warp_tolerance: i64,
    /// Wie oft ein Zeitstempel geklemmt wurde (`s.time_warped`).
    pub time_warped: u64,
    /// Wie viele Elemente verworfen wurden (`s.malformed`).
    pub malformed: u64,
    /// Wie viele Ticks in Folge die Periode verletzt haben (7.1).
    off_period: u32,
}

impl Edge {
    /// Baut den Rand fuer ein Programm.
    ///
    /// `limits` kommt ausgewertet vom Aufrufer: `max_slew` und die
    /// Range-Grenzen stehen als Ausdruecke in der MIR, und ihre Auswertung
    /// gehoert in den Start, nicht in den Tick (12.1 verlangt im
    /// Tick-Thread keine Arbeit, die sich vorziehen laesst).
    ///
    /// `warp_tolerance` ist die Konfiguration aus 12.6 Zeile 1; der
    /// Default ist ein Tick.
    pub fn new(p: &Program, limits: Vec<Limits>, warp_tolerance: i64) -> Edge {
        let n = p.channels.len();
        debug_assert_eq!(limits.len(), n, "je Channel eine Grenze");
        Edge {
            gates: vec![Gate::default(); n],
            limits,
            last_t: vec![None; n],
            last_seq: vec![None; n],
            maxpt: p.channels.iter().map(|c| maxpt_of(c, p.config.tick)).collect(),
            degraded: false,
            warp_tolerance,
            time_warped: 0,
            malformed: 0,
            off_period: 0,
        }
    }

    /// Prueft die Lieferungen eines Ticks (12.1: `validate_and_bound()`).
    ///
    /// `window` ist das halboffene Tickfenster in Nanosekunden: `lo`
    /// ausschliesslich, `hi` einschliesslich. Die Reihenfolge folgt der
    /// Tabelle — erst der Treibervertrag (1, 2), der ueber *alle* Kanaele
    /// entscheidet, dann Wert fuer Wert (3, 4).
    pub fn validate<V: Scalar>(
        &mut self,
        name: &str,
        readings: &[Reading<V>],
        elements: &[Element<V>],
        window: (i64, i64),
        p: &Program,
    ) -> Outcome {
        let mut out = Outcome::default();
        if let Some(what) = self.contract(readings, elements, window) {
            if !self.degraded {
                out.alerts.push(Alert::DriverDegraded { driver: name.to_string(), what });
            }
            self.degraded = true;
            for r in readings {
                out.readings.push((r.channel, self.gates[r.channel.index()].driver_bad()));
            }
            return out;
        }
        if self.degraded {
            // 12.6: "Erholung, sobald der Treiber wieder vertragsgemaess
            // liefert."
            out.alerts.push(Alert::DriverRecovered { driver: name.to_string() });
            self.degraded = false;
        }

        for r in readings {
            let c = &p.channels[r.channel.index()];
            if c.dir != Direction::Input {
                continue;
            }
            let t = self.clamp(r, window, &mut out);
            let verdict = match (&r.value, r.quality) {
                // Ein Treiber darf abwerten; der Rand wertet nicht auf.
                (_, Quality::Bad) | (None, _) => self.gates[r.channel.index()].driver_bad(),
                (Some(v), _) => {
                    let limits = self.limits[r.channel.index()];
                    self.gates[r.channel.index()].check(v, c, t, &limits)
                }
            };
            self.last_t[r.channel.index()] = Some(t);
            out.readings.push((r.channel, verdict));
        }

        // Zeile 5 entscheidet der Aufrufer: Ob ein `decode` gelingt, weiss
        // nur, wer den Record kennt (8.6). Hier wird durchgereicht; was
        // scheitert, meldet er mit `malformed`.
        for (i, e) in elements.iter().enumerate() {
            self.last_seq[e.channel.index()] = Some(e.seq);
            out.accepted.push(i);
        }
        out
    }

    /// Meldet ein Element als nicht decodierbar (12.6, Zeile 5): Es faellt
    /// aus `accepted`, `s.malformed` zaehlt, ein Alert geht heraus.
    ///
    /// `index` ist der Platz des Elements in der Liste, die `validate`
    /// bekommen hat.
    pub fn malformed(&mut self, channel: ChannelId, index: usize, out: &mut Outcome) {
        self.malformed = self.malformed.saturating_add(1);
        out.accepted.retain(|i| *i != index);
        out.alerts.push(Alert::Malformed { channel });
    }

    /// Prueft die Output-Seite (12.6, Zeile 6).
    ///
    /// Ein nicht bestaetigter Schreibvorgang, ein stiller Heartbeat oder
    /// ein ueberfahrener Sendepuffer geben den Besitzern der betroffenen
    /// Outputs `Runtime(Driver)`. Die Geraete stehen dann bereits auf
    /// `safe` (12.4) — deshalb ist es ein Fault und keine Degradierung:
    /// Das Stellen selbst ist gescheitert, und darueber kann kein
    /// `.or()` im Programm entscheiden.
    pub fn confirm<V, D>(&mut self, driver: &D, writes: &[(Writing<V>, Delivery)], p: &Program) -> Outcome
    where
        D: Heartbeat + Capacity + ?Sized,
    {
        let mut out = Outcome::default();
        let alive = driver.alive();
        for (w, d) in writes {
            let overrun = p.channels[w.channel.index()]
                .attrs
                .capacity_bytes
                .zip(driver.free(w.channel))
                .is_some_and(|(cap, free)| free > cap);
            if *d == Delivery::Unconfirmed || !alive || overrun {
                out.driver_faults.push(w.channel);
            }
        }
        out
    }

    /// Prueft die Tick-Periode (12.6 Zeile 7, 7.1).
    ///
    /// `tick_tolerance ... for N`: Erst nach `runs` aufeinanderfolgenden
    /// Verletzungen ist es `Runtime(Hardware)` — ein einzelner Ausreisser
    /// ist Jitter, kein Hardwarefehler. Liefert `true`, wenn der Fault
    /// faellig ist.
    pub fn period(&mut self, measured: i64, nominal: i64, tolerance: i64, runs: u32) -> bool {
        if measured.abs_diff(nominal) <= tolerance.unsigned_abs() {
            self.off_period = 0;
            return false;
        }
        self.off_period = self.off_period.saturating_add(1);
        self.off_period >= runs.max(1)
    }

    /// Zeile 1: Zeitstempel ins Tickfenster klemmen.
    fn clamp<V>(&mut self, r: &Reading<V>, window: (i64, i64), out: &mut Outcome) -> i64 {
        let (lo, hi) = window;
        if r.t > lo && r.t <= hi {
            return r.t;
        }
        let clamped = r.t.clamp(lo.saturating_add(1), hi);
        self.time_warped = self.time_warped.saturating_add(1);
        out.alerts.push(Alert::TimeWarped { channel: r.channel, got: r.t, clamped });
        clamped
    }

    /// Zeile 2: der Treibervertrag. `None` heisst eingehalten.
    fn contract<V>(&self, readings: &[Reading<V>], elements: &[Element<V>], window: (i64, i64)) -> Option<Contract> {
        for r in readings {
            if self.out_of_tolerance(r.t, window) {
                return Some(Contract::TimeWindow);
            }
            // Flags konsistent: `Bad` traegt keinen Wert (12.6, Zeile 2).
            if r.quality == Quality::Bad && r.value.is_some() {
                return Some(Contract::Flags);
            }
            if let Some(prev) = self.last_t[r.channel.index()]
                && r.t < prev
            {
                return Some(Contract::Timestamp);
            }
        }
        // Je Stream: `seq` streng steigend und lueckenlos, Menge <= MAXPT.
        let mut seen: Vec<(ChannelId, i64, usize)> = Vec::new();
        for e in elements {
            if self.out_of_tolerance(e.t, window) {
                return Some(Contract::TimeWindow);
            }
            match seen.iter_mut().find(|(c, _, _)| *c == e.channel) {
                Some(slot) => {
                    if e.seq != slot.1 + 1 {
                        return Some(Contract::Sequence);
                    }
                    slot.1 = e.seq;
                    slot.2 += 1;
                }
                None => {
                    if let Some(prev) = self.last_seq[e.channel.index()]
                        && e.seq != prev + 1
                    {
                        return Some(Contract::Sequence);
                    }
                    seen.push((e.channel, e.seq, 1));
                }
            }
        }
        seen.iter().any(|(c, _, n)| self.maxpt[c.index()].is_some_and(|m| *n > m)).then_some(Contract::TooMany)
    }

    /// Liegt der Zeitstempel so weit neben dem Fenster, dass Klemmen nicht
    /// mehr vertretbar ist (Zeile 1, zweiter Fall)?
    fn out_of_tolerance(&self, t: i64, window: (i64, i64)) -> bool {
        t <= window.0.saturating_sub(self.warp_tolerance) || t > window.1.saturating_add(self.warp_tolerance)
    }
}

/// Der Heartbeat eines Treibers (12.6, Zeile 6).
///
/// Getrennt vom `Driver`-Trait, damit `confirm` ohne die Wertdarstellung
/// auskommt: Ob ein Geraet antwortet, haengt nicht daran, was es liefert.
pub trait Heartbeat {
    /// Ist die Verbindung zum Geraet intakt?
    fn alive(&self) -> bool;
}

/// Der Sendepuffer eines Treibers (8.8).
pub trait Capacity {
    /// Freier Platz in Bytes (`free[o]`).
    fn free(&self, channel: ChannelId) -> Option<u32>;
}

/// `MAXPT = ceil(max_rate * T0)`: wie viele Elemente ein Stream in einem
/// Tick hoechstens liefern darf (8.6).
///
/// Ohne `max_rate` gibt es keine Schranke — dann ist die Menge durch die
/// Kapazitaet begrenzt, und Ueberlauf ist ein anderes, definiertes
/// Ereignis (8.6), keine Vertragsverletzung.
fn maxpt_of(c: &Channel, tick_ns: i64) -> Option<usize> {
    let hz = match &c.attrs.max_rate.as_ref()?.kind {
        takt_mir::expr::ExprKind::Int(n) => u64::try_from(*n).ok()?,
        takt_mir::expr::ExprKind::Float(f) if *f >= 0.0 => *f as u64,
        _ => return None,
    };
    let tick = u64::try_from(tick_ns).ok()?;
    // ceil(hz * tick_ns / 1e9), ganzzahlig gerechnet.
    let per_tick = hz.checked_mul(tick)?.div_ceil(1_000_000_000);
    usize::try_from(per_tick.max(1)).ok()
}
