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
/// Tick-Ende"), nicht am Anfang des naechsten: Am Anfang liesse sich ein
/// ueberlanger Schritt nicht von einer verspaeteten Weckung unterscheiden.
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
    /// Perioden, in denen kein Tick beginnen konnte: der Zuwachs des
    /// Rueckstands, nie das Aufholen.
    pub lost: u64,
    last_drift: i64,
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
            self.lost = self.lost.saturating_add(((drift - self.last_drift) / tick_ns) as u64);
        }
        self.last_drift = drift;
    }

    /// Nimmt die Dauer eines Ticks entgegen.
    ///
    /// Die beiden Fragen sind nicht dieselbe: 7.3 protokolliert die
    /// physische Verzoegerung *immer*, aber nur unter `fault` folgt ein
    /// Fault. Ein `bool` fuer beides haette unter `alert` einen Ueberlauf
    /// aus dem Trace verschwinden lassen.
    pub fn observe(&mut self, took: i64, tick_ns: i64) -> Seen {
        if took <= tick_ns {
            return Seen { over: false, fault: false };
        }
        self.count = self.count.saturating_add(1);
        self.worst = self.worst.max(took - tick_ns);
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

    /// Ein frueher Tick (negativer Rueckstand) ist kein Verlust.
    #[test]
    fn an_early_tick_costs_nothing() {
        let mut o = Overrun::new(Policy::Fault);
        o.observe_drift(-T / 2, T);
        o.observe_drift(T / 2, T);
        assert_eq!((o.lost, o.late, o.worst_drift), (0, 0, T / 2));
    }
}
