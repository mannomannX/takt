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
}

impl Overrun {
    /// Mit einer Policy.
    pub fn new(policy: Policy) -> Overrun {
        Overrun { policy, count: 0, worst: 0 }
    }

    /// Die Policy.
    pub fn policy(&self) -> Policy {
        self.policy
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
