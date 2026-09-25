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

/// Keine Telemetrie: Wer `step` ruft, bekommt den `Tick` ohnehin zurueck.
impl Sink for () {
    fn record(&mut self, _: &Tick) {}
}

/// Wie weit ein begonnener NVM-Vorgang ist.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NvmState {
    /// Kein Vorgang laeuft.
    #[default]
    Idle,
    /// Ein Vorgang laeuft noch.
    Busy,
    /// Der letzte Vorgang ist fertig.
    Done,
    /// Der letzte Vorgang ist gescheitert.
    Failed,
}

/// Der nichtfluechtige Speicher hinter `persist var` (5.9, 12.3).
///
/// **Warum ein Zustandsautomat und kein `write(…) -> Result`.** Eine
/// Sektorloeschung kostet zweistellige Millisekunden; bei 10 ms Tick sind
/// das mehrere verpasste Ticks. 12.3 verlangt fuer XIP-Targets darum
/// ausdruecklich, dass das Journal „eine niedrig priorisierte Aufgabe"
/// schreibt, „waehrend der Tick aus dem RAM weiterlaeuft". Ein Trait, der
/// das Ergebnis zurueckgibt, laedt zum Blockieren ein — dieser kann es
/// nur in `begin_*` tun, und das zeigt die Tickdauer (13.8).
///
/// FB-149 ist derselbe Fehler eine Stufe kleiner: Die Telemetrie sass an
/// derselben Stelle im Tick, wartete auf `TXE` und machte aus 1,0 s
/// Blinkzyklus 6,0 s.
pub trait Nvm {
    /// Groesse eines Slots in Byte; beide Slots sind gleich gross.
    fn slot_size(&self) -> u32;

    /// Beginnt, einen Slot zu loeschen. `false`, wenn gerade etwas laeuft.
    fn begin_erase(&mut self, slot: u8) -> bool;

    /// Beginnt, `bytes` ab `offset` in den Slot zu schreiben.
    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool;

    /// Fragt den laufenden Vorgang ab; treibt ihn voran.
    fn poll(&mut self) -> NvmState;

    /// Liest aus einem Slot. Laeuft einmal vor dem ersten Tick, darf also
    /// blockieren — dort gibt es keine Deadline.
    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool;

    /// So lange haelt `begin_*` den Kern hoechstens an, in Nanosekunden;
    /// `None`, wenn der Vorgang in der Hardware weiterlaeuft (12.3).
    fn blocking_ns(&self) -> Option<i64> {
        None
    }
}

/// Wie weit ein Job ist (4.5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JobState {
    /// Kein Lauf im Slot.
    #[default]
    Idle,
    /// Der Lauf ist noch nicht fertig.
    Running,
    /// Das Ergebnis liegt bereit (`take`).
    Done,
    /// Der Lauf ist gescheitert: `Err(FAILED)`.
    Failed,
}

/// Die Jobs hinter `job v = f(args)` (4.5, 12.1), in derselben Form wie
/// [`Nvm`]: `begin` uebergibt die Argumente als Folge kanonischer Bloecke
/// (je `u32` Laenge, dann die Bytes; 5.9), `poll` fragt ohne zu warten,
/// `take` holt das Ergebnis in kanonischer Form. Wo der Lauf stattfindet,
/// entscheidet der Aufsatz: `takt-rt-linux` auf einem Arbeiter-Thread mit
/// dem deklarierten `stack`, `baremetal` in der Hauptschleife zwischen den
/// Ticks. Ein Job wird nie unterbrochen — Natives sind `total` —; `cancel`
/// verwirft nur sein Ergebnis (5.3).
pub trait Jobs {
    /// Zahl der Slots.
    fn slots(&self) -> u32;

    /// Beginnt einen Lauf im Slot; `false`, wenn er nicht angenommen wurde
    /// (Slot belegt, Native unbekannt).
    fn begin(&mut self, slot: u32, native: &str, args: &[u8]) -> bool;

    /// Fragt den Slot ab; treibt den Lauf voran, wo er im Tick laeuft.
    fn poll(&mut self, slot: u32) -> JobState;

    /// Holt das Ergebnis eines fertigen Slots nach `into`; liefert die
    /// Laenge, 0 ohne Ergebnis. Danach ist der Slot `Idle`.
    fn take(&mut self, slot: u32, into: &mut [u8]) -> usize;

    /// Verwirft den Lauf des Slots; sein Ergebnis erreicht das Programm nicht.
    fn cancel(&mut self, slot: u32);
}

/// Die Aenderungen an Tunables (8.4, v1.1): je Tick-Grenze ein Satz,
/// atomar vor dem Schritt. `poll` liefert die Aenderungen fuer den Tick
/// `k` an `apply` — Parameterindex und Wert in kanonischer Byteform
/// (5.9). Range und Einheit prueft die Quelle beim Lesen der Zeile; die
/// Schleife traegt nur ein, was sie bekommt.
pub trait Tunables {
    /// Die Aenderungen der Tick-Grenze `k`, in Aufzeichnungsreihenfolge.
    fn poll(&mut self, k: u64, apply: &mut dyn FnMut(u32, &[u8]));
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

impl Tick {
    /// Die Metazeile `t=<k> time took=<ns> drift=<ns> slept=<n>`
    /// (grammar/trace.md T1; 12.5: Zeit ausserhalb der Semantik).
    pub fn write_time(&self, w: &mut impl core::fmt::Write) -> core::fmt::Result {
        write!(w, "t={} time took={} drift={} slept={}", self.k, self.took, self.drift, self.slept)
    }
}

/// Die logische Zeit am Ende von Tick `k` (12.1): ein Vielfaches von `T0`
/// und *nicht* die gemessene Zeit — sonst haengt der Trace an der Uhr, und
/// Satz 9.4.1 gilt nicht mehr. Jeder, der [`Program::tick`] ruft, rechnet
/// sie so.
pub fn tick_end(k: u64, tick_ns: i64) -> i64 {
    (k as i64).saturating_add(1).saturating_mul(tick_ns)
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

    /// Schreibt die kanonische Form aller `persist`-Variablen nach `out`
    /// und liefert die Laenge (5.9); 0 heisst: nichts zu sichern.
    fn persist_snapshot(&mut self, _out: &mut [u8]) -> usize {
        0
    }

    /// Uebernimmt eine Journal-Nutzlast und liefert die Zahl der
    /// gueltigen Eintraege; der Rest ist `PersistReset` (5.9).
    fn persist_restore(&mut self, _bytes: &[u8]) -> usize {
        0
    }

    /// Ein Tunable aendert sich (8.4): Parameterindex und Wert in
    /// kanonischer Byteform, gueltig ab dem naechsten Schritt.
    fn tune(&mut self, _param: u32, _value: &[u8]) {}

    /// Ein Job ist fertig (4.5): das Ergebnis in kanonischer Byteform, oder
    /// `None`, wenn der Lauf gescheitert ist. Es geht als `done`/`result`
    /// in das Eingangsbild des naechsten Schritts.
    fn job_done(&mut self, _slot: u32, _result: Option<&[u8]>) {}
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
        self.overrun.observe_drift(drift, self.tick_ns);

        // 7.3: Der Overrun des vorigen Ticks wirkt jetzt. Er kommt vor dem
        // Schritt, damit die Maschinen ihn in diesem Tick sehen.
        if core::mem::take(&mut self.pending_overrun) {
            self.program.raise_overrun();
        }

        // sample_inputs() bis commit_outputs(): die Semantik.
        let now = tick_end(self.k, self.tick_ns);
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

    /// Traegt den Satz der Tick-Grenze in das Programm ein (8.4) — vor
    /// dem Schritt, damit er als Input von I_k gilt.
    pub fn apply_tunables<T: Tunables>(&mut self, tunables: &mut T) {
        let program = &mut self.program;
        tunables.poll(self.k, &mut |param, value| program.tune(param, value));
    }

    /// Ein Tick mit Jobs (4.5): Vor dem Schritt gehen die fertigen Slots
    /// in das Programm — die Fertigstellung ist ein Input von I_k. `buf`
    /// nimmt das Ergebnis auf; er ist Sache des Aufsatzes, weil der Kern
    /// keinen Heap hat.
    pub fn step_with_jobs<J: Jobs>(&mut self, jobs: &mut J, buf: &mut [u8]) -> Tick {
        for slot in 0..jobs.slots() {
            match jobs.poll(slot) {
                JobState::Done => {
                    let n = jobs.take(slot, buf);
                    self.program.job_done(slot, Some(&buf[..n.min(buf.len())]));
                }
                JobState::Failed => {
                    jobs.take(slot, buf);
                    self.program.job_done(slot, None);
                }
                JobState::Idle | JobState::Running => {}
            }
        }
        self.step()
    }

    /// Laesst die Schleife `n` Ticks laufen.
    pub fn run(&mut self, n: u64) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Ein Tick mit Journal (5.9, 12.1).
    ///
    /// Das Journal laeuft *nach* dem Schritt, in der Wartezeit bis zur
    /// naechsten Frist. Ein Geraet, das in der Hardware weiterarbeitet,
    /// wird jeden Tick gefragt; eines, das den Kern anhaelt (12.3), nur
    /// wenn die Wartezeit den Vorgang deckt — nach einem Schlaf ist sie
    /// lang, sonst ein Tick abzueglich des Schritts — oder wenn das
    /// Programm Ueberlaeufe annimmt (`overrun = alert`, 7.3).
    pub fn step_persisting<N: Nvm>(&mut self, persist: &mut crate::journal::Persist<'_, N>) -> Tick {
        let tick = self.step();
        if self.journal_may_run(persist.blocking_ns()) {
            persist.poll(tick.now, &mut self.program);
            // Ein Vorgang ueber die Frist hinaus ist ein Ueberlauf (7.3),
            // auch wenn der Schritt selbst gepasst hat.
            let late = self.clock.now().saturating_sub(self.deadline);
            if late > 0 {
                self.pending_overrun |= self.overrun.observe(self.tick_ns.saturating_add(late), self.tick_ns).fault;
            }
        }
        tick
    }

    fn journal_may_run(&self, blocking_ns: Option<i64>) -> bool {
        match blocking_ns {
            None => true,
            Some(cost) => self.overrun.policy() == Policy::Alert || self.deadline - self.clock.now() >= cost,
        }
    }

    /// `n` Ticks mit Journal.
    pub fn run_persisting<N: Nvm>(&mut self, n: u64, persist: &mut crate::journal::Persist<'_, N>) {
        for _ in 0..n {
            self.step_persisting(persist);
        }
    }

    /// Die Ueberlauf- und Rueckstandszahlen des Laufs (7.3).
    pub fn overrun(&self) -> &Overrun {
        &self.overrun
    }

    /// Die nominale Periode in Nanosekunden (7.1).
    pub fn tick_ns(&self) -> i64 {
        self.tick_ns
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
