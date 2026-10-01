//! Der Treibervertrag (12.6, Zeilen 1 und 2) ohne `std` und ohne
//! Allokation.
//!
//! Der Zustand je Kanal ([`Track`]) liegt im Speicher des Aufrufers: im
//! Interpreter in einem `Vec`, im erzeugten C-Rahmen in einem Feld mit so
//! vielen Eintraegen, wie das Programm Kanaele hat. Darum ist er `repr(C)`
//! und kommt ohne `Option` aus.
//!
//! **Der Stand folgt jeder Lieferung, auch einer verletzenden.** Sonst
//! bliebe ein Treiber nach einer einzigen Luecke fuer immer degradiert —
//! jede folgende Folgenummer laege wieder hinter der Luecke —, und 12.6
//! verlangt die Erholung, „sobald der Treiber wieder vertragsgemaess
//! liefert" (plan/m10.md 2.3, Entwurf Schritt 29).

/// Kein Wert: vor der ersten Lieferung eines Kanals.
pub const NONE: i64 = i64::MIN;

/// Welcher Teil des Treibervertrags verletzt wurde (12.6, Zeile 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contract {
    /// Zeitstempel fallend.
    Timestamp,
    /// `seq` nicht streng steigend oder mit Luecke.
    Sequence,
    /// Mehr Elemente als `MAXPT` in einem Tick (8.6).
    TooMany,
    /// Qualitaetsflags widerspruechlich: `Bad` mit Wert, oder der
    /// Messzeitpunkt `t - age` faellt (Alter nicht monoton).
    Flags,
    /// Zeitstempel jenseits der Klemmtoleranz (Zeile 1, zweiter Fall).
    TimeWindow,
}

impl Contract {
    /// Die Verletzung als Wort fuer Meldung und Trace.
    pub fn name(self) -> &'static str {
        match self {
            Contract::Timestamp => "timestamp",
            Contract::Sequence => "seq",
            Contract::TooMany => "maxpt",
            Contract::Flags => "flags",
            Contract::TimeWindow => "window",
        }
    }

    /// Die Verletzung zu ihrem Wort.
    pub fn by_name(name: &str) -> Option<Contract> {
        [Contract::Timestamp, Contract::Sequence, Contract::TooMany, Contract::Flags, Contract::TimeWindow]
            .into_iter()
            .find(|c| c.name() == name)
    }
}

/// Das Zeitfenster eines Ticks (12.6, Zeile 1): `(lo, hi]` in
/// Nanosekunden, dazu die Toleranz, innerhalb derer geklemmt wird.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// `t_(k-1)`, ausschliesslich.
    pub lo: i64,
    /// `t_k`, einschliesslich.
    pub hi: i64,
    /// Klemmtoleranz (Konfiguration, Default ein Tick).
    pub tolerance: i64,
}

/// Wo ein Zeitstempel zum Fenster liegt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Im Fenster.
    Inside,
    /// Daneben, aber in der Toleranz: geklemmt auf diesen Zeitpunkt.
    Warped(i64),
    /// Jenseits der Toleranz: eine Vertragsverletzung (Zeile 1 wie 2).
    Outside,
}

impl Placement {
    /// Der Zeitpunkt, mit dem die Lieferung weitergeht.
    pub fn time(self, t: i64) -> i64 {
        match self {
            Placement::Warped(c) => c,
            Placement::Inside | Placement::Outside => t,
        }
    }
}

impl Window {
    /// Das Fenster des Ticks `k` bei der Periode `tick`: `((k-1)·T0, k·T0]`.
    pub fn of_tick(k: i64, tick: i64, tolerance: i64) -> Window {
        let hi = k.saturating_mul(tick);
        Window { lo: hi.saturating_sub(tick), hi, tolerance }
    }

    /// Ordnet einen Zeitstempel ein.
    pub fn place(&self, t: i64) -> Placement {
        if t > self.lo && t <= self.hi {
            return Placement::Inside;
        }
        if t <= self.lo.saturating_sub(self.tolerance) || t > self.hi.saturating_add(self.tolerance) {
            return Placement::Outside;
        }
        Placement::Warped(t.clamp(self.lo.saturating_add(1), self.hi))
    }
}

/// Was der Rand je Kanal zwischen den Ticks weiss (12.6, Zeilen 1 und 2).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Track {
    /// Letzter gelieferter Zeitstempel; [`NONE`] vor der ersten Lieferung.
    pub last_t: i64,
    /// Letzte Folgenummer eines Stroms; [`NONE`] vor dem ersten Element.
    pub last_seq: i64,
    /// Letzter Messzeitpunkt `t - age` eines Skalars; [`NONE`] zu Beginn.
    pub last_measured: i64,
    /// `MAXPT` eines Stroms (8.6); 0 heisst ohne Schranke.
    pub maxpt: u32,
    /// Elemente im laufenden Stapel.
    pub count: u32,
}

impl Default for Track {
    fn default() -> Track {
        Track::new(0)
    }
}

impl Track {
    /// Ein Kanal vor seiner ersten Lieferung.
    pub const fn new(maxpt: u32) -> Track {
        Track { last_t: NONE, last_seq: NONE, last_measured: NONE, maxpt, count: 0 }
    }

    /// Eine Skalar-Lieferung: Zeitstempel `t`, Alter `age` des Werts,
    /// `bad_with_value` fuer `Bad` mit Wert.
    ///
    /// Zeitstempel duerfen gleich bleiben, nicht fallen: Ein grober
    /// Zeitgeber stempelt zwei Lieferungen gleich, und die Reihenfolge
    /// steht ohnehin fest.
    pub fn reading(&mut self, t: i64, age: i64, bad_with_value: bool, w: &Window) -> Result<Placement, Contract> {
        let placed = w.place(t);
        let measured = t.saturating_sub(age);
        let broken = if placed == Placement::Outside {
            Some(Contract::TimeWindow)
        } else if bad_with_value {
            Some(Contract::Flags)
        } else if self.last_t != NONE && t < self.last_t {
            Some(Contract::Timestamp)
        } else if self.last_measured != NONE && measured < self.last_measured {
            Some(Contract::Flags)
        } else {
            None
        };
        self.last_t = t;
        self.last_measured = measured;
        broken.map_or(Ok(placed), Err)
    }

    /// Ein Stromelement: `seq` streng steigend und lueckenlos, Zeitstempel
    /// nicht fallend. Die Menge prueft [`Track::finish`].
    pub fn element(&mut self, t: i64, seq: i64, w: &Window) -> Result<Placement, Contract> {
        let placed = w.place(t);
        let broken = if placed == Placement::Outside {
            Some(Contract::TimeWindow)
        } else if self.last_seq != NONE && Some(seq) != self.last_seq.checked_add(1) {
            Some(Contract::Sequence)
        } else if self.last_t != NONE && t < self.last_t {
            Some(Contract::Timestamp)
        } else {
            None
        };
        self.last_t = t;
        self.last_seq = seq;
        self.count = self.count.saturating_add(1);
        broken.map_or(Ok(placed), Err)
    }

    /// Schliesst den Stapel eines Ticks: `len(D) <= MAXPT` (8.6).
    pub fn finish(&mut self) -> Result<(), Contract> {
        let n = core::mem::take(&mut self.count);
        if self.maxpt != 0 && n > self.maxpt { Err(Contract::TooMany) } else { Ok(()) }
    }
}

/// Wie sich ein Treiber zwischen zwei Ticks veraendert hat (Zeile 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Wie zuvor.
    Steady,
    /// Er haelt den Vertrag nicht mehr: Alert `DriverDegraded`.
    Degraded(Contract),
    /// Er liefert wieder vertragsgemaess: die Erholung.
    Recovered,
}

/// Der Zustand eines Treibers zwischen den Ticks.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// Laeuft er gerade degradiert?
    pub degraded: bool,
}

impl Device {
    /// Das Ergebnis eines Stapels: die erste Verletzung oder keine. Ein
    /// Treiber, der in diesem Tick nichts geliefert hat, bleibt, wie er
    /// war — Schweigen ist keine Erholung.
    pub fn settle(&mut self, delivered: bool, broken: Option<Contract>) -> Turn {
        match (broken, self.degraded) {
            (Some(c), false) => {
                self.degraded = true;
                Turn::Degraded(c)
            }
            (None, true) if delivered => {
                self.degraded = false;
                Turn::Recovered
            }
            _ => Turn::Steady,
        }
    }
}

/// Die Tick-Periode (12.6 Zeile 7, 7.1): Erst nach `runs`
/// aufeinanderfolgenden Verletzungen ist es `Runtime(Hardware)` — ein
/// einzelner Ausreisser ist Jitter, kein Hardwarefehler.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Period {
    /// Verletzungen in Folge.
    pub off: u32,
}

impl Period {
    /// Nimmt eine gemessene Periode auf; `true`, wenn der Fault faellig ist.
    pub fn observe(&mut self, measured: i64, nominal: i64, tolerance: i64, runs: u32) -> bool {
        if measured.abs_diff(nominal) <= tolerance.unsigned_abs() {
            self.off = 0;
            return false;
        }
        self.off = self.off.saturating_add(1);
        self.off >= runs.max(1)
    }
}

/// Die Ausgabeseite (12.6, Zeile 6): Ein nicht bestaetigter
/// Schreibvorgang, ein stiller Heartbeat oder ein ueberfahrener
/// Sendepuffer (`free[o] > capacity`) geben den Besitzern
/// `Runtime(Driver)`.
pub fn output_fails(confirmed: bool, alive: bool, free: Option<u32>, capacity: Option<u32>) -> bool {
    let overrun = free.zip(capacity).is_some_and(|(free, cap)| free > cap);
    !confirmed || !alive || overrun
}
