//! Der defensive Treiberrand ueber einem Programm (12.6): die sieben
//! Pruefungen fuer die Lieferungen eines Ticks.
//!
//! | # | Pruefung | Bei Verletzung |
//! |---|---|---|
//! | 1 | Zeitstempel im Tickfenster | in Toleranz geklemmt, `time_warped`, Alert; darueber wie 2 |
//! | 2 | Zeitstempel monoton, `seq` streng steigend und lueckenlos, Menge <= `MAXPT`, Flags konsistent | alle Inputs des Treibers `Bad`/`Driver` |
//! | 3 | Werte in deklarierten Ranges | `Bad`/`OutOfRange` (mit `debounce` erst `Suspect`) |
//! | 4 | `max_slew` | `Bad`/`Implausible` (mit `debounce` erst `Suspect`) |
//! | 5 | Record-Stroeme: `decode` erfolgreich | Element verworfen, `malformed`, Alert |
//! | 6 | Schreiben bestaetigt, Heartbeat, Sendepuffer nicht ueberfahren | `Runtime(Driver)` fuer die Besitzer |
//! | 7 | Tick-Periode in `tick_tolerance` | `Runtime(Hardware)` fuer alle Maschinen |
//!
//! Die Regeln stehen im Kern (`contract`, `quality`), den auch die
//! erzeugten Rahmen ueber ihre C-Einstiege rufen; hier steht, was ein
//! Programm dazu beitraegt: welcher Kanal zu welchem Treiber gehoert, seine
//! Grenzen und `MAXPT`.
//!
//! **Warum ein Vertragsbruch alle Kanaele des Treibers trifft.** 12.6 Zeile
//! 2 sagt es so, und der Grund ist, dass die Pruefung nicht den Wert misst,
//! sondern den Treiber: Wer Folgenummern durcheinanderbringt, bei dem ist
//! auch der unauffaellige Kanal nur zufaellig unauffaellig.

use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

use takt_mir::ChannelId;
use takt_mir::program::{Binding, Channel, Direction, Program};

use crate::contract::{Contract, Device, Period, Placement, Track, Turn, Window};
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

/// Was die Zeilen 1 und 2 fuer die Lieferungen eines Ticks ergeben.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settled {
    /// Je Skalar-Lieferung der Zeitpunkt, mit dem sie weitergeht; `None`,
    /// wenn ihr Treiber den Vertrag verletzt hat.
    pub readings: Vec<Option<i64>>,
    /// Je Stromelement dasselbe; ein `None`-Element wird nicht zugestellt.
    pub elements: Vec<Option<i64>>,
    /// Die Inputs der degradierten Treiber: `Bad` mit Grund `Driver`.
    pub degraded: Vec<ChannelId>,
    /// Beobachtungen fuer Telemetrie und Trace.
    pub alerts: Vec<Alert>,
}

/// Der Rand eines Programms: sein Zustand zwischen den Ticks.
#[derive(Debug)]
pub struct Edge {
    /// Qualitaetsmaschine je Channel (3.5).
    gates: Vec<Gate>,
    /// Ausgewertete Grenzen je Channel.
    limits: Vec<Limits>,
    /// Vertragsstand je Channel (Zeilen 1, 2).
    tracks: Vec<Track>,
    /// Je Channel der Index seines Treibers in `devices`.
    device: Vec<usize>,
    /// Die Treiber mit Namen und Zustand.
    devices: Vec<(String, Device)>,
    /// Toleranz fuer Pruefung 1 in Nanosekunden.
    tolerance: i64,
    /// Zeile 7: Verletzungen der Periode in Folge.
    period: Period,
    /// Wie oft ein Zeitstempel geklemmt wurde (`s.time_warped`).
    pub time_warped: u64,
    /// Wie viele Elemente verworfen wurden (`s.malformed`).
    pub malformed: u64,
}

impl Edge {
    /// Baut den Rand fuer ein Programm.
    ///
    /// `limits` kommt ausgewertet vom Aufrufer: `max_slew` und die
    /// Range-Grenzen stehen als Ausdruecke in der MIR, und ihre Auswertung
    /// gehoert in den Start, nicht in den Tick (12.1 verlangt im
    /// Tick-Thread keine Arbeit, die sich vorziehen laesst).
    ///
    /// `tolerance` ist die Konfiguration aus 12.6 Zeile 1; der Default ist
    /// ein Tick.
    pub fn new(p: &Program, limits: Vec<Limits>, tolerance: i64) -> Edge {
        let n = p.channels.len();
        debug_assert_eq!(limits.len(), n, "je Channel eine Grenze");
        let mut devices: Vec<(String, Device)> = Vec::new();
        let device = p
            .channels
            .iter()
            .map(|c| {
                let name = driver_of(c);
                match devices.iter().position(|(d, _)| *d == name) {
                    Some(i) => i,
                    None => {
                        devices.push((name, Device::default()));
                        devices.len() - 1
                    }
                }
            })
            .collect();
        Edge {
            gates: vec![Gate::default(); n],
            limits,
            tracks: p.channels.iter().map(|c| Track::new(maxpt_of(c, p.config.tick).unwrap_or(0))).collect(),
            device,
            devices,
            tolerance,
            period: Period::default(),
            time_warped: 0,
            malformed: 0,
        }
    }

    /// Die Zeilen 1 und 2 fuer alle Lieferungen eines Ticks
    /// (12.1: `validate_and_bound()`).
    ///
    /// Erst entscheidet der Vertrag je Treiber ueber *alle* seine Kanaele,
    /// dann gehen die Lieferungen der vertragstreuen Treiber weiter — mit
    /// dem geklemmten Zeitpunkt, wo Zeile 1 klemmt. Die Werte selbst prueft
    /// danach [`Edge::gate`] (Zeilen 3, 4).
    pub fn contract<V>(
        &mut self,
        readings: &[Reading<V>],
        elements: &[Element<V>],
        window: (i64, i64),
        p: &Program,
    ) -> Settled {
        let w = Window { lo: window.0, hi: window.1, tolerance: self.tolerance };
        let mut broken: Vec<Option<Contract>> = vec![None; self.devices.len()];
        let mut delivered = vec![false; self.devices.len()];
        let mut first = |d: usize, r: Result<Placement, Contract>| match r {
            Ok(placed) => placed,
            Err(c) => {
                broken[d].get_or_insert(c);
                Placement::Outside
            }
        };
        let mut placed_readings = Vec::with_capacity(readings.len());
        for r in readings {
            let (i, d) = (r.channel.index(), self.device[r.channel.index()]);
            delivered[d] = true;
            let bad_with_value = r.quality == Quality::Bad && r.value.is_some();
            placed_readings.push(first(d, self.tracks[i].reading(r.t, r.age, bad_with_value, &w)));
        }
        let mut placed_elements = Vec::with_capacity(elements.len());
        for e in elements {
            let (i, d) = (e.channel.index(), self.device[e.channel.index()]);
            delivered[d] = true;
            placed_elements.push(first(d, self.tracks[i].element(e.t, e.seq, &w)));
        }
        for (i, track) in self.tracks.iter_mut().enumerate() {
            if track.count > 0
                && let Err(c) = track.finish()
            {
                broken[self.device[i]].get_or_insert(c);
            }
        }

        let mut out = Settled::default();
        for (d, (name, device)) in self.devices.iter_mut().enumerate() {
            match device.settle(delivered[d], broken[d]) {
                Turn::Degraded(what) => out.alerts.push(Alert::DriverDegraded { driver: name.clone(), what }),
                Turn::Recovered => out.alerts.push(Alert::DriverRecovered { driver: name.clone() }),
                Turn::Steady => {}
            }
        }
        let down: Vec<bool> = self.device.iter().map(|d| self.devices[*d].1.degraded).collect();
        for (r, placed) in readings.iter().zip(placed_readings) {
            out.readings.push(self.pass(r.channel, r.t, placed, down[r.channel.index()], &mut out.alerts));
        }
        for (e, placed) in elements.iter().zip(placed_elements) {
            out.elements.push(self.pass(e.channel, e.t, placed, down[e.channel.index()], &mut out.alerts));
        }
        out.degraded = p
            .channels
            .iter()
            .enumerate()
            .filter(|(i, c)| c.dir == Direction::Input && down[*i])
            .map(|(i, _)| ChannelId(i as u32))
            .collect();
        out
    }

    /// Eine Lieferung eines vertragstreuen Treibers geht weiter; Zeile 1
    /// zaehlt und meldet, was sie geklemmt hat.
    fn pass(
        &mut self,
        c: ChannelId,
        t: i64,
        placed: Placement,
        degraded: bool,
        alerts: &mut Vec<Alert>,
    ) -> Option<i64> {
        if degraded {
            return None;
        }
        if let Placement::Warped(clamped) = placed {
            self.time_warped = self.time_warped.saturating_add(1);
            alerts.push(Alert::TimeWarped { channel: c, got: t, clamped });
        }
        Some(placed.time(t))
    }

    /// Die naechste Folgenummer der lueckenlosen Folge eines Stroms: die
    /// Voreinstellung fuer ein Element ohne eigene.
    pub fn next_seq(&self, c: ChannelId) -> i64 {
        match self.tracks[c.index()].last_seq {
            crate::contract::NONE => 0,
            seq => seq.saturating_add(1),
        }
    }

    /// Prueft einen Wert gegen Range und `max_slew` (Zeilen 3, 4); `t` ist
    /// der Zeitpunkt, den [`Edge::contract`] geliefert hat.
    pub fn gate<V: Scalar>(&mut self, c: ChannelId, v: &V, t: i64) -> Verdict {
        let limits = self.limits[c.index()];
        self.gates[c.index()].check(v, t, &limits)
    }

    /// Ein Kanal ist vom Treiber degradiert oder meldet selbst `Bad`
    /// (Zeile 2): Der Bezugspunkt faellt weg.
    pub fn driver_bad(&mut self, c: ChannelId) -> Verdict {
        self.gates[c.index()].driver_bad()
    }

    /// Ein Element liess sich nicht decodieren (Zeile 5): `s.malformed`
    /// zaehlt, ein Alert geht heraus. Verworfen hat es der Aufrufer — ob
    /// ein `decode` gelingt, weiss nur, wer den Record kennt (8.6).
    pub fn malformed(&mut self, channel: ChannelId) -> Alert {
        self.malformed = self.malformed.saturating_add(1);
        Alert::Malformed { channel }
    }

    /// Prueft die Output-Seite (12.6, Zeile 6) und nennt die Outputs,
    /// deren Besitzer `Runtime(Driver)` bekommen.
    ///
    /// Ein nicht bestaetigter Schreibvorgang, ein stiller Heartbeat oder
    /// ein ueberfahrener Sendepuffer sind ein Fault und keine Degradierung:
    /// Die Geraete stehen dann bereits auf `safe` (12.4), das Stellen selbst
    /// ist gescheitert, und darueber kann kein `.or()` im Programm
    /// entscheiden.
    pub fn confirm<V, D>(&self, driver: &D, writes: &[(Writing<V>, Delivery)], p: &Program) -> Vec<ChannelId>
    where
        D: Heartbeat + Capacity + ?Sized,
    {
        let alive = driver.alive();
        writes
            .iter()
            .filter(|(w, d)| {
                let capacity = p.channels[w.channel.index()].attrs.capacity_bytes;
                crate::contract::output_fails(*d == Delivery::Acked, alive, driver.free(w.channel), capacity)
            })
            .map(|(w, _)| w.channel)
            .collect()
    }

    /// Prueft die Tick-Periode (12.6 Zeile 7, 7.1); `true`, wenn
    /// `Runtime(Hardware)` faellig ist.
    pub fn period(&mut self, measured: i64, nominal: i64, tolerance: i64, runs: u32) -> bool {
        self.period.observe(measured, nominal, tolerance, runs)
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

/// Der Treiber eines Kanals: das Geraet, also das erste Segment seiner
/// Adresse (`hw("adc1/ch0")` gehoert zu `adc1`). Ein ungebundener Kanal
/// ist sein eigener Treiber und heisst wie er.
pub fn driver_of(c: &Channel) -> String {
    match &c.binding {
        Binding::Hw(a) | Binding::Sim(a) => a.segments.first().map_or_else(|| c.name.clone(), |s| s.name.to_string()),
        Binding::None => c.name.clone(),
    }
}

/// `MAXPT = ceil(max_rate * T0)`: wie viele Elemente ein Stream in einem
/// Tick hoechstens liefern darf (8.6).
///
/// Ohne `max_rate` gibt es keine Schranke — dann ist die Menge durch die
/// Kapazitaet begrenzt, und Ueberlauf ist ein anderes, definiertes
/// Ereignis (8.6), keine Vertragsverletzung.
pub fn maxpt_of(c: &Channel, tick_ns: i64) -> Option<u32> {
    let hz = match &c.attrs.max_rate.as_ref()?.kind {
        takt_mir::expr::ExprKind::Int(n) => u64::try_from(*n).ok()?,
        takt_mir::expr::ExprKind::Float(f) if *f >= 0.0 => *f as u64,
        _ => return None,
    };
    let tick = u64::try_from(tick_ns).ok()?;
    // ceil(hz * tick_ns / 1e9), ganzzahlig gerechnet.
    let per_tick = hz.checked_mul(tick)?.div_ceil(1_000_000_000);
    u32::try_from(per_tick.max(1)).ok()
}
