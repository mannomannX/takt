//! Die Schleife aus 12.1.

use crate::overrun::{Overrun, Policy};
use crate::profile::Profile;

/// Die Zeitquelle (7.1).
///
/// `wait_until` ist der einzige blockierende Aufruf der Schleife; 12.2
/// verlangt dafuer absolute Deadlines (`clock_nanosleep` TIMER_ABSTIME),
/// damit sich ein zu spaeter Tick nicht auf die folgenden fortpflanzt.
pub trait Clock {
    /// Jetzt, in Nanosekunden seit dem Start des Laufs.
    fn now(&self) -> i64;

    /// Wartet bis zum absoluten Zeitpunkt `deadline`.
    ///
    /// Kehrt sofort zurueck, wenn er schon vergangen ist — der Tick ist
    /// dann ueberfaellig, und [`Overrun`] entscheidet, was daraus folgt.
    fn wait_until(&mut self, deadline: i64);
}

/// Der Watchdog (12.4).
pub trait Watchdog {
    /// Bestaetigt, dass der Tick durchgelaufen ist.
    fn kick(&mut self);
}

/// Telemetrie und Aufzeichnung (12.1: `record_and_telemeter()`).
///
/// 12.2 verlangt: „nie blockierend". Ein Sink, der wartet, verschiebt den
/// naechsten Tick und erzeugt genau den Overrun, den er melden soll —
/// darum ist `record` ohne Rueckgabe: Wer nicht mitkommt, verwirft und
/// zaehlt, statt die Steuerung aufzuhalten.
pub trait Sink {
    /// Nimmt die Zusammenfassung eines Ticks entgegen.
    fn record(&mut self, tick: &Tick);
}

/// Was ein Tick gekostet hat und was er ergab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tick {
    /// Nummer des Ticks, bei 0 beginnend.
    pub k: u64,
    /// Logische Zeit am Ende des Ticks in Nanosekunden.
    pub now: i64,
    /// Wie lange der Schritt gedauert hat, in Nanosekunden.
    pub took: i64,
    /// Wie weit die Periode danebenlag (gemessen minus nominal).
    pub drift: i64,
    /// Der Tick hat seine Periode ueberschritten (7.3).
    pub overrun: bool,
    /// Wie viele Ticks uebersprungen wurden (9.9).
    pub slept: u64,
}

/// Das Programm, das die Schleife ausfuehrt.
///
/// Die Semantik liegt hinter diesem Trait: Der Interpreter fuehrt sie ueber
/// `Value`, der erzeugte Code ueber getypte Strukturen (11.2). Der Kern
/// kennt nur „ein Tick ist vergangen".
pub trait Program {
    /// Fuehrt die Schritte 2 bis 10 aus 12.1 aus — vom Abbild bis zum
    /// Commit. `now` ist die logische Zeit des Tickendes.
    fn tick(&mut self, k: u64, now: i64);

    /// Meldet allen Maschinen `Runtime(Overrun)` (7.3).
    ///
    /// Der Fault wirkt im *naechsten* Tick, nicht in diesem: 7.3 sagt „als
    /// Fault fuer alle Maschinen im naechsten Tick", und der laufende Tick
    /// ist bereits zu Ende, wenn die Ueberschreitung feststeht.
    fn raise_overrun(&mut self) {}

    /// Darf geschlafen werden (9.9)?
    ///
    /// Der Default ist `false`: Ein Programm, das die Bedingung nicht
    /// prueft, schlaeft nie — das ist langsamer, aber nie falsch. Die
    /// Bedingung ist lang (alle Maschinen idle, alle `sched[o]` leer, kein
    /// `pending`, kein `raised`, keine laufenden Jobs), und wer sie
    /// versehentlich zu schwach formuliert, verliert Ticks.
    fn sleep_allowed(&self) -> bool {
        false
    }

    /// Bis wann darf geschlafen werden (9.9)?
    ///
    /// Die frueheste `after`-Frist ueber alle Maschinen, in Nanosekunden
    /// als absoluter Zeitpunkt. `None` heisst: keine Frist, es weckt nur
    /// ein Ereignis.
    fn next_deadline(&self) -> Option<i64> {
        None
    }

    /// Traegt `n` uebersprungene Ticks nach (9.9).
    ///
    /// „fuer jede Maschine: time_in_state += n*T0". Ohne das feuerte jede
    /// `after`-Frist um die geschlafenen Ticks zu spaet, und Satz 9.9.1
    /// (Trace mit und ohne Schlaf gleich) waere verletzt.
    fn advance(&mut self, _ticks: u64) {}
}

/// Die Schleife.
pub struct Runtime<P, C, W, S> {
    /// Das Programm.
    pub program: P,
    /// Die Zeitquelle.
    pub clock: C,
    /// Der Watchdog.
    pub watchdog: W,
    /// Telemetrie.
    pub sink: S,
    /// Das Laufzeitprofil (12.8).
    pub profile: Profile,
    /// Basis-Tick T0 in Nanosekunden.
    tick_ns: i64,
    /// Die Overrun-Erkennung (7.3).
    overrun: Overrun,
    /// Nummer des naechsten Ticks.
    k: u64,
    /// Absoluter Zeitpunkt, an dem der naechste Tick beginnt.
    deadline: i64,
    /// Im vorigen Tick ist die Periode uebergelaufen (7.3: der Fault wirkt
    /// im naechsten Tick).
    pending_overrun: bool,
}

impl<P: Program, C: Clock, W: Watchdog, S: Sink> Runtime<P, C, W, S> {
    /// Baut die Schleife.
    ///
    /// `tick_ns` ist T0 aus dem `system:`-Block, `policy` die Reaktion auf
    /// eine Ueberschreitung (7.3, Default `fault`).
    pub fn new(program: P, clock: C, watchdog: W, sink: S, profile: Profile, tick_ns: i64, policy: Policy) -> Self {
        let start = clock.now();
        Runtime {
            program,
            clock,
            watchdog,
            sink,
            profile,
            tick_ns: tick_ns.max(1),
            overrun: Overrun::new(policy),
            k: 0,
            deadline: start,
            pending_overrun: false,
        }
    }

    /// Ein Durchlauf der Schleife aus 12.1.
    ///
    /// Die Reihenfolge ist die der Referenz und nicht verhandelbar: Der
    /// Watchdog wird *nach* dem Schritt bestaetigt, nicht davor — sonst
    /// bestaetigte er einen Tick, der noch nicht durchgelaufen ist, und
    /// haette seinen Zweck verloren (12.4).
    pub fn step(&mut self) -> Tick {
        // wait_for_tick_boundary()
        self.clock.wait_until(self.deadline);
        let began = self.clock.now();
        let drift = began - self.deadline;

        // 7.3: Der Overrun des vorigen Ticks wirkt jetzt. Er kommt vor dem
        // Schritt, damit die Maschinen ihn in diesem Tick sehen.
        if core::mem::take(&mut self.pending_overrun) {
            self.program.raise_overrun();
        }

        // sample_inputs() bis commit_outputs(): die Semantik. Die logische
        // Zeit ist ein Vielfaches von T0 und *nicht* die gemessene — sonst
        // haengt der Trace an der Uhr, und Satz 9.4.1 gilt nicht mehr.
        let now = (self.k as i64).saturating_add(1).saturating_mul(self.tick_ns);
        self.program.tick(self.k, now);
        let took = self.clock.now() - began;

        // record_and_telemeter(): nie blockierend (12.2).
        let seen = self.overrun.observe(took, self.tick_ns);
        self.pending_overrun = seen.fault;
        let mut tick = Tick { k: self.k, now, took, drift, overrun: seen.over, slept: 0 };

        // kick_watchdog()
        self.watchdog.kick();

        // maybe_sleep() (9.9)
        tick.slept = self.sleep(now);
        if tick.slept > 0 {
            self.program.advance(tick.slept);
        }
        self.sink.record(&tick);

        self.k = self.k.saturating_add(1).saturating_add(tick.slept);
        // Der naechste Tick beginnt eine Periode nach diesem — absolut
        // gerechnet, damit ein zu spaeter Tick die folgenden nicht
        // verschiebt (12.2). Der Tick wird nie uebersprungen (7.3).
        self.deadline = self.deadline.saturating_add(self.tick_ns.saturating_mul(1 + tick.slept as i64));
        tick
    }

    /// Laesst die Schleife `n` Ticks laufen.
    pub fn run(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Nummer des naechsten Ticks.
    pub fn tick_number(&self) -> u64 {
        self.k
    }

    /// Das Programm, das die Schleife treibt.
    pub fn program(&self) -> &P {
        &self.program
    }

    /// `maybe_sleep()` nach 9.9: Wie viele Ticks werden uebersprungen?
    ///
    /// Satz 9.9.1 sagt, dass der Schlaf unsichtbar ist — die uebersprungenen
    /// Ticks waeren leere Schritte gewesen. Die Schleife rueckt `now` und
    /// die Tickzahl exakt um sie vor, statt sie auszufuehren.
    fn sleep(&mut self, now: i64) -> u64 {
        if !self.profile.may_sleep || !self.program.sleep_allowed() {
            return 0;
        }
        let Some(deadline) = self.program.next_deadline() else { return 0 };
        let d = deadline.saturating_sub(now);
        if d <= self.tick_ns {
            return 0;
        }
        // n = d / T0, und der Tick an der Frist selbst wird ausgefuehrt.
        u64::try_from(d / self.tick_ns).unwrap_or(0).saturating_sub(1)
    }
}
