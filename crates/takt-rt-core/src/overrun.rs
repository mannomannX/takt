//! Ueberlauf zur Laufzeit (7.3).
//!
//! 7.3 woertlich: „Ueberschreitet ein Tick trotz Budget die Periode
//! (Treiberstoerung, Cache-Effekte auf Linux), erzeugt die Runtime
//! `Runtime(Overrun)` als Fault fuer alle Maschinen im naechsten Tick
//! (Policy konfigurierbar: `fault` (Default) oder `alert` fuer unkritische
//! Systeme). Der Tick wird nie uebersprungen; die logische Zeit bleibt
//! konsistent, die physische Verzoegerung wird protokolliert."
//!
//! Drei Saetze, drei Entscheidungen: Der Fault wirkt *spaeter*, die Policy
//! ist konfigurierbar, und die logische Zeit bleibt von der physischen
//! unberuehrt. Die dritte ist die wichtigste — sie ist der Grund, warum
//! Satz 9.4.1 auch auf einer Box gilt, die einmal zu spaet kommt.

/// Was aus einer Ueberschreitung folgt (7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Policy {
    /// `Runtime(Overrun)` fuer alle Maschinen (Default).
    #[default]
    Fault,
    /// Nur ein Alert — fuer unkritische Systeme.
    Alert,
}

/// Die Ueberlauferkennung.
///
/// Sie misst am Tick-Ende (12.2: „Overrun-Erkennung per Zeitstempel am
/// Tick-Ende") gegen das feste Raster `t0 + (k+1)·T0` (7.3): Uebergelaufen
/// ist ein Tick, dessen Schritt erst nach dem nominalen Beginn des naechsten
/// endet. Ein verspaeteter Beginn zaehlt mit, sonst summierte sich eine
/// Verspaetung unbemerkt.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overrun {
    policy: Policy,
    /// Wie oft die Periode ueberschritten wurde.
    pub count: u64,
    /// Die groesste gemessene Ueberschreitung in Nanosekunden.
    pub worst: i64,
    /// Ticks, die mindestens eine Periode zu spaet begannen.
    pub late: u64,
    /// Der groesste Rueckstand beim Tickbeginn in Nanosekunden.
    pub worst_drift: i64,
    /// Perioden, in denen kein Tick beginnen konnte: die Summe der Zuwaechse
    /// des Rueckstands, einmal durch T0 geteilt, nie das Aufholen.
    pub lost: u64,
    last_drift: i64,
    /// Die Summe der Zuwaechse in Nanosekunden.
    grown: i64,
}

impl Overrun {
    /// Mit einer Policy.
    pub fn new(policy: Policy) -> Overrun {
        Overrun { policy, ..Overrun::default() }
    }

    /// Die Policy.
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Nimmt den Rueckstand beim Tickbeginn entgegen (12.3, plan/esp32c6.md 6.1).
    ///
    /// `drift` und `took` je Tick sind die einzige Quelle; die drei Zahlen
    /// hier sind eine Faltung darueber, kein zweiter Zaehler.
    pub fn observe_drift(&mut self, drift: i64, tick_ns: i64) {
        let drift = drift.max(0);
        if drift >= tick_ns {
            self.late = self.late.saturating_add(1);
        }
        self.worst_drift = self.worst_drift.max(drift);
        if drift > self.last_drift && tick_ns > 0 {
            self.grown = self.grown.saturating_add(drift - self.last_drift);
            self.lost = (self.grown / tick_ns) as u64;
        }
        self.last_drift = drift;
    }

    /// Nimmt entgegen, wie lange nach dem nominalen Beginn seines Ticks ein
    /// Schritt endete (`drift + took`); mehr als eine Periode ist ein
    /// Ueberlauf.
    ///
    /// Die beiden Fragen sind nicht dieselbe: 7.3 protokolliert die
    /// physische Verzoegerung *immer*, aber nur unter `fault` folgt ein
    /// Fault. Ein `bool` fuer beides haette unter `alert` einen Ueberlauf
    /// aus dem Trace verschwinden lassen.
    pub fn observe(&mut self, elapsed: i64, tick_ns: i64) -> Seen {
        if elapsed <= tick_ns {
            return Seen { over: false, fault: false };
        }
        self.count = self.count.saturating_add(1);
        self.worst = self.worst.max(elapsed - tick_ns);
        Seen { over: true, fault: self.policy == Policy::Fault }
    }
}

/// Was eine Messung ergab (7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// Der Tick hat seine Periode ueberschritten; das wird immer
    /// protokolliert.
    pub over: bool,
    /// Daraus folgt `Runtime(Overrun)` im naechsten Tick — nur unter der
    /// Policy `fault`.
    pub fault: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_000_000;

    /// Ein Stillstand von fuenf Perioden, aufgeholt, dann drei weitere:
    /// acht verlorene Perioden, nicht 5 + 4 + 3 + 2 + 1 + 3.
    #[test]
    fn lost_periods_count_the_growth_of_the_lag_only() {
        let mut o = Overrun::new(Policy::Fault);
        for drift in [0, 0, 5 * T, 4 * T, 3 * T, 2 * T, T, 0, 3 * T, 2 * T] {
            o.observe_drift(drift, T);
        }
        assert_eq!(o.lost, 8);
        assert_eq!(o.late, 7, "sieben Ticks begannen mindestens eine Periode zu spaet");
        assert_eq!(o.worst_drift, 5 * T);
    }

    /// **Verloren ist die Summe der Zuwaechse, geteilt durch T0** (7.3):
    /// Waechst der Rueckstand je Tick um eine halbe Periode, sind nach zehn
    /// Ticks viereinhalb Perioden verloren, abgerundet vier — wie `takt
    /// timing` rechnet, das die Zuwaechse in Nanosekunden summiert und
    /// einmal teilt. Ein Abrunden je Zuwachs meldete null.
    #[test]
    fn lost_periods_divide_the_summed_growth_once() {
        let mut o = Overrun::new(Policy::Fault);
        for k in 0..10 {
            o.observe_drift(k * T / 2, T);
        }
        assert_eq!(o.lost, 4, "Zuwachs 9 * T/2 = 4,5 T");
        o.observe_drift(5 * T, T);
        assert_eq!(o.lost, 5, "der naechste Zuwachs um T/2 vollendet die fuenfte Periode");
    }

    /// Ein frueher Tick (negativer Rueckstand) ist kein Verlust.
    #[test]
    fn an_early_tick_costs_nothing() {
        let mut o = Overrun::new(Policy::Fault);
        o.observe_drift(-T / 2, T);
        o.observe_drift(T / 2, T);
        assert_eq!((o.lost, o.late, o.worst_drift), (0, 0, T / 2));
    }
}
