//! Die Schleife aus 12.1 als Kern ohne Warten (12.11).
//!
//! [`Runtime::service`] rechnet jede faellige Tickgrenze ganz — Schritt,
//! Commit, Aufzeichnung, Watchdog, Schlaf, Journal, Ende des Laufs — und
//! nennt die naechste Frist ([`Next`]); sie wartet nie. Wer zur Frist ruft,
//! ist der Port: eine Timer-ISR, eine Hauptschleife, eine Aufgabe unter
//! einem RTOS. [`Runtime::step`] ist der Port der Form „eigener Kern“
//! (12.3): Er wartet selbst auf die Frist und bestaetigt dabei den
//! Watchdog an jeder Tickgrenze, auch im Schlaf.

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

    /// Der Beginn des Rasters `t0` (7.3): die Frist von Tick 0, ab der die
    /// Schleife jede weitere als `t0 + k·T0` rechnet. Eine Uhr, deren
    /// Tickquelle die Grenzen vorgibt, legt ihn auf deren naechstes Ereignis
    /// (12.3); sonst beginnt das Raster jetzt.
    fn origin(&self) -> i64 {
        self.now()
    }

    /// Die zuletzt gemessene Periode der Tickquelle in Nanosekunden (7.1);
    /// `None`, wenn die Uhr sie nicht misst oder noch nicht kennt. Die
    /// logische Uhr eines Konformitaetslaufs misst nichts.
    fn tick_period(&self) -> Option<i64> {
        None
    }

    /// Gab es seit dem letzten Aufruf ein Weckereignis (9.9): eine
    /// Wake-Quelle, einen Operator-Abort, ein Runtime-Ereignis? Der Kern
    /// fragt im Schlaf an jeder Grenze, die er erreicht, und beendet ihn
    /// dort; eine Uhr ohne Wake-Quellen weckt nie vor der Frist.
    fn woken(&mut self) -> bool {
        false
    }
}

/// `tick_tolerance = p pct for n ticks` (7.1) in der Form der Schleife.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tolerance {
    /// Die erlaubte Abweichung der Periode in Nanosekunden.
    pub ns: i64,
    /// So viele Verletzungen in Folge sind `Runtime(Hardware)`.
    pub runs: u32,
}

/// Der Watchdog (12.3, 12.4).
pub trait Watchdog {
    /// Bestaetigt, dass der Tick durchgelaufen ist.
    fn kick(&mut self);
}

/// Ein Watchdog, der nur im Betrieb wacht: `None` in einem Lauf, der auf
/// die Leitung warten darf (13.8).
impl<W: Watchdog> Watchdog for Option<W> {
    fn kick(&mut self) {
        if let Some(w) = self {
            w.kick();
        }
    }
}

/// Telemetrie und Aufzeichnung (12.1: `record_and_telemeter()`; 12.5).
///
/// 12.2 verlangt: „nie blockierend". Ein Sink, der wartet, verschiebt den
/// naechsten Tick und erzeugt genau den Overrun, den er melden soll —
/// darum ist `record` ohne Rueckgabe: Wer nicht mitkommt, verwirft und
/// zaehlt, statt die Steuerung aufzuhalten.
///
/// Je Tick fragt der Kern zuerst [`Sink::outputs`], gibt dann die
/// Ausgaenge in den Trace ([`Program::trace`]) und meldet zuletzt
/// [`Sink::record`].
pub trait Sink {
    /// Welche Ausgaenge der Trace nach dem Tick `tick` zeigt (9.3); `None`
    /// fragt nach dem Anfangszustand vor dem ersten Tick. Eine Senke, die
    /// eine Zeitzeile schreibt, schreibt sie hier: vor den Ausgaben.
    fn outputs(&mut self, _tick: Option<&Tick>) -> Outputs {
        Outputs::None
    }

    /// Nimmt die Zusammenfassung eines Ticks entgegen, nach seinem Trace.
    fn record(&mut self, tick: &Tick);
}

/// Keine Telemetrie und kein Trace der Ausgaenge.
impl Sink for () {
    fn record(&mut self, _: &Tick) {}
}

/// Welche Ausgaenge ein Tick in den Trace gibt (12.5, 9.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Outputs {
    /// Keine.
    #[default]
    None,
    /// Die, die sich seit der letzten Ausgabe geaendert haben.
    Changed,
    /// Alle.
    All,
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

    /// Holt das Ergebnis eines fertigen Slots nach `into`; liefert seine
    /// Laenge, 0 ohne Ergebnis. Ist es laenger als `into`, kommt nichts an,
    /// und die Laenge sagt es dem Aufrufer. Danach ist der Slot `Idle`.
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
    /// Wie viele Ticks nach diesem uebersprungen wurden (9.9). Das steht
    /// erst fest, wenn der Schlaf endet; bis dahin haelt der Kern die
    /// Zeile des Ticks zurueck.
    pub slept: u64,
}

impl Tick {
    /// Die Metazeile `t=<k> time took=<ns> drift=<ns> slept=<n>`
    /// (grammar/trace.md T1; 12.5: Zeit ausserhalb der Semantik).
    pub fn write_time(&self, w: &mut impl core::fmt::Write) -> core::fmt::Result {
        write!(w, "t={} time took={} drift={} slept={}", self.k, self.took, self.drift, self.slept)
    }
}

/// Leiht die Tunables erneut, fuer einen Aufruf.
fn reborrow<'a>(tunables: &'a mut Option<&mut dyn Tunables>) -> Option<&'a mut dyn Tunables> {
    match tunables {
        Some(t) => Some(&mut **t),
        None => None,
    }
}

/// Die logische Zeit am Ende von Tick `k` (12.1): ein Vielfaches von `T0`
/// und *nicht* die gemessene Zeit — sonst haengt der Trace an der Uhr, und
/// Satz 9.4.1 gilt nicht mehr. Jeder, der [`Program::tick`] ruft, rechnet
/// sie so.
pub fn tick_end(k: u64, tick_ns: i64) -> i64 {
    i64::try_from(k).unwrap_or(i64::MAX).saturating_add(1).saturating_mul(tick_ns)
}

/// Wann der naechste Lauf beginnen soll (12.7, `next_run`); der laufende
/// endet damit nach dem Commit. Was dazwischen geschieht — Neustart,
/// Schlaf ohne RAM, ein Host, der spaeter wieder startet —, entscheidet
/// die Plattform; der naechste Lauf meldet `previous_run = ENDED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextRun {
    /// `NOW`: sofort.
    Now,
    /// `AFTER(delay)`: nach der Dauer in Nanosekunden, ab dem Ende
    /// gezaehlt, oder frueher, wenn eine Wake-Quelle weckt.
    After(i64),
    /// `ON_WAKE`: wenn eine Wake-Quelle weckt.
    OnWake,
    /// `ON_START`: mit dem naechsten Start der Plattform.
    OnStart,
}

impl NextRun {
    /// Die Nummer, mit der `P_next_run` des Rahmens dieses Ende meldet
    /// (12.7); 0 heisst weiter. Der Rahmen schreibt sie, jede Huelle liest
    /// sie mit [`NextRun::from_code`] zurueck: eine Zuordnung statt dreier
    /// Fassungen von Hand (GEN-029).
    pub const fn code(self) -> i32 {
        match self {
            NextRun::Now => 1,
            NextRun::After(_) => 2,
            NextRun::OnWake => 3,
            NextRun::OnStart => 4,
        }
    }

    /// Das Ende zu einer Nummer von `P_next_run`; `delay` gilt fuer `AFTER`.
    pub fn from_code(code: i32, delay: i64) -> Option<NextRun> {
        [NextRun::Now, NextRun::After(delay), NextRun::OnWake, NextRun::OnStart].into_iter().find(|n| n.code() == code)
    }
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

    /// Meldet allen Maschinen `Runtime(Hardware)` (12.3): Der Speicherschutz
    /// hat einen Zugriff der TCB auf den Programmzustand abgewiesen, oder
    /// die Tickquelle haelt ihre Periode nicht (12.6 Zeile 7). Der Fault
    /// wirkt im naechsten Schritt.
    fn raise_hardware(&mut self) {}

    /// `tick_tolerance` des Programms (7.1); `None` prueft die Periode nicht.
    fn tick_tolerance(&self) -> Option<Tolerance> {
        None
    }

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
    /// als absoluter Zeitpunkt. `None` heisst: keine Frist; der Kern
    /// schlaeft dann nicht, denn ohne Frist wecken koennte ihn nur ein
    /// Ereignis.
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
    /// kanonischer Byteform, gueltig ab dem Schritt von Tick `k`. Der
    /// Golden-Trace zeichnet die Zeile auf, auch eine verworfene
    /// (`grammar/trace.md`); dafuer nennt der Kern den Tick.
    fn tune(&mut self, _k: u64, _param: u32, _value: &[u8]) {}

    /// Hat eine Wake-Quelle des Programms ausgeloest (5.10, 9.9)? Der Kern
    /// fragt im Schlaf an jeder geschlafenen Grenze, in Reihenfolge, mit dem
    /// Tick `k`, der dort rechnen wuerde; wahr beendet den Schlaf dort. Das
    /// Programm tastet seine Wake-Quellen dafuer so ab, wie der Schritt von
    /// Tick `k` es taete, und sagt, ob er etwas anderes saehe als der Tick
    /// vor dem Schlaf. Frueher zu wecken ist nie falsch: Die uebersprungenen
    /// Ticks waeren leere Schritte gewesen (Satz 9.9.1).
    fn woken(&mut self, _k: u64) -> bool {
        false
    }

    /// Hat das Programm Wake-Quellen, die [`Program::woken`] abtastet? Ein
    /// Port, der den Kern ueber [`Runtime::service`] faehrt, kommt dann im
    /// Schlaf an jeder Grenze wieder ([`Next::deadline`]); ohne sie schlaeft
    /// er bis zur Frist.
    fn wake_sources(&self) -> bool {
        false
    }

    /// Ein Job ist fertig (4.5): das Ergebnis in kanonischer Byteform, oder
    /// `None`, wenn der Lauf gescheitert ist. Es geht als `done`/`result`
    /// in das Eingangsbild des naechsten Schritts.
    fn job_done(&mut self, _slot: u32, _result: Option<&[u8]>) {}

    /// Verlangt `next_run` nach dem Commit einen naechsten Lauf (12.7)? Der
    /// laufende endet dann: `persist` synchron, danach alle Ausgaenge auf
    /// `safe`.
    fn next_run(&self) -> Option<NextRun> {
        None
    }

    /// Gibt den Latch an die Treiber (12.1, Schritt 10).
    fn commit(&mut self) {}

    /// `output_timing = boundary` (1.4): Der Latch eines Ticks geht erst an
    /// der naechsten Tickgrenze an die Treiber, jitterfrei, wie lange der
    /// Schritt auch rechnete (LET). Die Semantik aendert das nicht: Das
    /// Programm liest seine Ausgaenge erst im naechsten Tick.
    fn commit_at_boundary(&self) -> bool {
        false
    }

    /// Gibt die Ausgaenge in den Trace (12.5, 9.3).
    fn trace(&mut self, _outputs: Outputs) {}

    /// Beendet den Lauf (12.7): die Zeile `end` in den Trace, dann alle
    /// Ausgaenge auf `safe`.
    fn end(&mut self) {}

    /// Gibt dem Job-Kontext den aeltesten wartenden Job (4.5); wahr, wenn
    /// er zu rechnen hat.
    fn dispatch_job(&mut self) -> bool {
        false
    }
}

/// Was [`Runtime::service`] dem Port sagt (12.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Next {
    /// Wann `service` wieder gerufen werden will, absolut in Nanosekunden:
    /// die naechste Tickgrenze, mit erlaubtem Schlaf (9.9) die Frist des
    /// Schlafs; haelt der Kern einen Latch (1.4) oder tastet das Programm
    /// im Schlaf Wake-Quellen ab ([`Program::wake_sources`]), die naechste
    /// Grenze.
    pub deadline: i64,
    /// Ein Job wartet (4.5): Der Job-Kontext soll rechnen.
    pub jobs: bool,
    /// Der Lauf hat geendet (12.7): `next_run` als Befehl an den Wirt.
    pub ended: Option<NextRun>,
}

/// Ein Schlaf, der noch laeuft (9.9): Seine Ticks sind dem Programm noch
/// nicht nachgetragen, und die Zeile des Ticks davor wartet auf ihre Zahl.
#[derive(Clone, Copy, Debug)]
struct Asleep {
    /// Der Tick vor dem Schlaf.
    tick: Tick,
    /// Der erste geschlafene Tick.
    from: u64,
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
    /// Nummer des naechsten Ticks; im Schlaf der an seiner Frist.
    k: u64,
    /// Absoluter Zeitpunkt, an dem der naechste Tick beginnt.
    deadline: i64,
    /// Der Beginn von Tick 0; Tick `k` beginnt `k * T0` danach.
    start: i64,
    /// Der laufende Schlaf.
    asleep: Option<Asleep>,
    /// Die erste Grenze, deren Tunables noch nicht abgefragt sind (8.4).
    tuned: u64,
    /// Die erste geschlafene Grenze, deren Wake-Quellen noch nicht
    /// abgetastet sind (9.9).
    polled: u64,
    /// Die erste Tickgrenze, an der die Schleife auf dem Weg zur Frist den
    /// Watchdog bestaetigt: nach virtuellen Ticks die erste geschlafene,
    /// sonst die Frist selbst.
    beat_from: i64,
    /// Im vorigen Tick ist die Periode uebergelaufen (7.3: der Fault wirkt
    /// im naechsten Tick).
    pending_overrun: bool,
    /// Der Latch des letzten Ticks wartet auf die naechste Grenze
    /// (`output_timing = boundary`, 1.4).
    held: bool,
    /// Verletzungen der Periode in Folge (12.6 Zeile 7).
    period: takt_hal::contract::Period,
    /// Der Anfangszustand ist im Trace, oder das Programm hat ihn beendet.
    begun: bool,
    /// Der zuletzt gerechnete Tick.
    last: Tick,
    /// Das Ende des Laufs, wenn das Programm ihn beendet hat (12.7).
    ended: Option<NextRun>,
    /// Das Journal am Ende des Laufs: geschrieben oder nicht.
    flushed: Option<bool>,
    /// So lange darf das Journal am Ende des Laufs schreiben, in
    /// Nanosekunden (12.3: das geordnete Ende hat eine eigene Frist);
    /// `None` wartet, bis das Geraet fertig ist.
    end_budget: Option<i64>,
    /// Die Zahl der Ticks eines Laufs mit Ende ([`Runtime::end_at`]).
    horizon: Option<u64>,
}

impl<P: Program, C: Clock, W: Watchdog, S: Sink> Runtime<P, C, W, S> {
    /// Baut die Schleife.
    ///
    /// `tick_ns` ist T0 aus dem `system:`-Block, `policy` die Reaktion auf
    /// eine Ueberschreitung (7.3, Default `fault`).
    pub fn new(program: P, clock: C, watchdog: W, sink: S, profile: Profile, tick_ns: i64, policy: Policy) -> Self {
        let start = clock.origin();
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
            start,
            asleep: None,
            tuned: 0,
            polled: 0,
            beat_from: start,
            pending_overrun: false,
            held: false,
            period: takt_hal::contract::Period::default(),
            begun: false,
            last: Tick::default(),
            ended: None,
            flushed: None,
            end_budget: None,
            horizon: None,
        }
    }

    /// Ein Lauf ueber `ticks` Ticks (13.8: ein Konformitaetslauf endet nach
    /// Tick `ticks - 1`). Ein Schlaf reicht dann hoechstens bis zum letzten
    /// Tick: Jede Grenze davor wird abgetastet, und ein Weckereignis vor dem
    /// Ende wirkt (9.9). Der Tick an dieser verkuerzten Frist ist ein leerer
    /// Schritt — frueher zu wecken ist nie falsch (Satz 9.9.1).
    pub fn end_at(&mut self, ticks: u64) {
        self.horizon = Some(ticks);
    }

    /// Begrenzt das synchrone Schreiben des Journals am Ende des Laufs auf
    /// `ns` (12.3, 12.7): Ein haengendes Geraet laesst das Ende dann ohne
    /// geschriebenen Stand zu, statt es — und unter `shared` den Wirt —
    /// aufzuhalten (12.11: kein Einstieg blockiert). Der Port waehlt `ns`
    /// unter der Frist seines Watchdogs fuer das Ende.
    pub fn with_end_budget(mut self, ns: i64) -> Self {
        self.end_budget = Some(ns);
        self
    }

    /// Rechnet jede Tickgrenze bis jetzt nach 12.1 und nennt die naechste
    /// Frist (12.11). Wartet nie; ist nichts faellig, rechnet sie nichts.
    /// „Jetzt“ ist die Zeit beim Eintritt: Ein Tick, der laenger dauert als
    /// die Periode, laesst die naechste Grenze fuer den naechsten Aufruf.
    pub fn service(&mut self) -> Next {
        self.service_with(None::<&mut crate::journal::Persist<'_, crate::journal::FakeNvm<0>>>, None)
    }

    /// Wie [`Runtime::service`], mit Journal (5.9): nach jedem Tick in der
    /// Wartezeit bis zur naechsten Frist, am Ende des Laufs synchron.
    pub fn service_persisting<N: Nvm>(&mut self, persist: &mut crate::journal::Persist<'_, N>) -> Next {
        self.service_with(Some(persist), None)
    }

    /// Die Form „eigener Kern“ (12.11, 12.3): wartet auf die Frist, rechnet
    /// mit [`Runtime::service`] und liefert den zuletzt gerechneten Tick.
    ///
    /// Die Reihenfolge ist die der Referenz und nicht verhandelbar: Der
    /// Watchdog wird *nach* dem Schritt bestaetigt, nicht davor — sonst
    /// bestaetigte er einen Tick, der noch nicht durchgelaufen ist, und
    /// haette seinen Zweck verloren (12.4).
    pub fn step(&mut self) -> Tick {
        self.step_with(None::<&mut crate::journal::Persist<'_, crate::journal::FakeNvm<0>>>, None)
    }

    /// Wie [`Runtime::step`], mit Journal.
    pub fn step_persisting<N: Nvm>(&mut self, persist: &mut crate::journal::Persist<'_, N>) -> Tick {
        self.step_with(Some(persist), None)
    }

    /// Der eigene Kern mit Journal und Tunables (8.4): ein Tick je Aufruf,
    /// an seiner Frist. Die Aenderungen einer Grenze gehen vor ihrem Schritt
    /// in das Programm; im Schlaf beendet eine Aenderung ihn an ihrer
    /// Grenze (9.9).
    pub fn step_with<N: Nvm>(
        &mut self,
        mut persist: Option<&mut crate::journal::Persist<'_, N>>,
        mut tunables: Option<&mut dyn Tunables>,
    ) -> Tick {
        self.begin(persist.as_deref_mut());
        if self.ended.is_none() {
            self.wait(reborrow(&mut tunables));
            self.tick_now(persist, tunables);
        }
        self.last
    }

    /// `wait_for_tick_boundary()`: Nach virtuellen Ticks (9.9) liegt die
    /// Frist mehrere Perioden voraus. Der eigene Kern wartet Periode fuer
    /// Periode und bestaetigt den Watchdog an jeder Grenze — er sieht auch
    /// im Schlaf, dass die Tickquelle lebt (12.3). Ein Weckereignis der Uhr
    /// oder einer Wake-Quelle oder eine Tunable-Aenderung an einer Grenze
    /// beendet den Schlaf dort; ihr Tick bestaetigt den Watchdog nach
    /// seinem Schritt.
    fn wait(&mut self, mut tunables: Option<&mut dyn Tunables>) {
        while self.beat_from < self.deadline {
            self.clock.wait_until(self.beat_from);
            self.release();
            if self.asleep.is_some() {
                let j = self.boundary_at(self.beat_from);
                self.wake_asleep(reborrow(&mut tunables), self.beat_from);
                if self.clock.woken() {
                    self.wake_at(j);
                }
                if self.deadline <= self.beat_from {
                    break;
                }
            }
            self.watchdog.kick();
            self.beat_from = self.beat_from.saturating_add(self.tick_ns);
        }
        self.clock.wait_until(self.deadline);
        self.end_sleep();
    }

    /// Beendet den Lauf an einer Grenze, die der Port setzt, etwa nach so
    /// vielen Ticks eines Konformitaetslaufs: Das Journal schreibt synchron
    /// (5.9). Hat das Programm den Lauf selbst beendet, ist das schon
    /// geschehen; die Rueckgabe sagt, ob geschrieben wurde.
    pub fn finish<N: Nvm>(&mut self, persist: Option<&mut crate::journal::Persist<'_, N>>) -> bool {
        if let Some(flushed) = self.flushed {
            return flushed;
        }
        // Ein Schlaf bis ueber das Ende des Laufs gilt bis zu seiner Frist.
        self.end_sleep();
        let limit = self.end_budget.map(|ns| self.clock.now().saturating_add(ns));
        let clock = &self.clock;
        let flushed = persist.is_some_and(|p| p.flush(&mut self.program, || limit.is_none_or(|l| clock.now() < l)));
        self.flushed = Some(flushed);
        flushed
    }

    /// [`Runtime::service`] mit Journal und Tunables (8.4): Die Aenderungen
    /// einer Grenze gehen vor ihrem Schritt in das Programm. Im Schlaf
    /// fragt der Kern die Grenzen bis jetzt in Reihenfolge ab, je Grenze
    /// Tunables und Wake-Quellen ([`Program::woken`]), und die erste mit
    /// einer Aenderung beendet ihn (9.9); ebenso ein Weckereignis der Uhr
    /// ([`Clock::woken`]) an der Grenze nach dem Aufruf.
    pub fn service_with<N: Nvm>(
        &mut self,
        mut persist: Option<&mut crate::journal::Persist<'_, N>>,
        mut tunables: Option<&mut dyn Tunables>,
    ) -> Next {
        self.begin(persist.as_deref_mut());
        let now = self.clock.now();
        if self.asleep.is_some() {
            self.wake_asleep(reborrow(&mut tunables), now);
            if self.clock.woken() {
                let j = self.boundary_at(now);
                self.wake_at(j);
            }
        }
        // 12.3: Eine Grenze im Schlaf (9.9) bestaetigt den Watchdog, auch
        // ohne Tick; ein Port, der an jeder Grenze ruft, haelt ihn so wach.
        while self.beat_from < self.deadline && self.beat_from <= now {
            self.release();
            self.watchdog.kick();
            self.beat_from = self.beat_from.saturating_add(self.tick_ns);
        }
        // `i64::MAX` ist die gesaettigte Frist am Ende der Zeitachse, keine
        // Grenze: Dort waechst sie nicht mehr, und die Schleife kaeme nie zurueck.
        while self.ended.is_none() && self.deadline <= now && self.deadline < i64::MAX {
            self.end_sleep();
            self.tick_now(persist.as_deref_mut(), reborrow(&mut tunables));
            self.wake_asleep(reborrow(&mut tunables), now);
        }
        let polling = self.asleep.is_some() && self.program.wake_sources();
        let deadline = if self.held || polling { self.beat_from.min(self.deadline) } else { self.deadline };
        Next { deadline, jobs: self.program.dispatch_job(), ended: self.ended }
    }

    /// Der Beginn von Tick `j`.
    fn boundary(&self, j: u64) -> i64 {
        let periods = i64::try_from(j).unwrap_or(i64::MAX);
        self.start.saturating_add(self.tick_ns.saturating_mul(periods))
    }

    /// Der Commit nach einem Schritt (12.1): sofort, unter `output_timing =
    /// boundary` (1.4) an der naechsten Grenze ([`Runtime::release`]).
    fn commit_or_hold(&mut self) {
        if self.program.commit_at_boundary() {
            self.held = true;
        } else {
            self.program.commit();
        }
    }

    /// An einer Grenze geht ein gehaltener Latch an die Treiber, vor dem
    /// Abtasten des naechsten Ticks (1.4), im Schlaf an der ersten
    /// geschlafenen Grenze.
    fn release(&mut self) {
        if core::mem::take(&mut self.held) {
            self.program.commit();
        }
    }

    /// Der erste Tick, der nicht vor `t` beginnt.
    fn boundary_at(&self, t: i64) -> u64 {
        let since = t.saturating_sub(self.start).max(0);
        u64::try_from(since.div_euclid(self.tick_ns) + i64::from(since.rem_euclid(self.tick_ns) != 0)).unwrap_or(0)
    }

    /// Beendet den laufenden Schlaf vorzeitig an Grenze `j`: Dort rechnet
    /// der naechste Tick (9.9: d = min(Frist, Weckereignis)).
    fn wake_at(&mut self, j: u64) {
        if let Some(a) = self.asleep
            && j < self.k
        {
            self.k = j.max(a.from);
            self.deadline = self.boundary(self.k);
        }
    }

    /// Traegt die Tunables der Grenze `j` ein, wenn sie noch nicht
    /// abgefragt ist; wahr, wenn sich etwas aenderte.
    fn tune_at(&mut self, tunables: &mut dyn Tunables, j: u64) -> bool {
        if j < self.tuned {
            return false;
        }
        self.tuned = j.saturating_add(1);
        let program = &mut self.program;
        let mut changed = false;
        tunables.poll(j, &mut |param, value| {
            changed = true;
            program.tune(j, param, value);
        });
        changed
    }

    /// Tastet die Wake-Quellen der geschlafenen Grenze `j` ab, wenn sie noch
    /// nicht abgetastet ist; wahr, wenn eine ausgeloest hat.
    fn poll_at(&mut self, j: u64) -> bool {
        if j < self.polled || !self.program.wake_sources() {
            return false;
        }
        self.polled = j.saturating_add(1);
        self.program.woken(j)
    }

    /// Im Schlaf: die geschlafenen Grenzen bis `now` in Reihenfolge, je
    /// Grenze erst die Tunables, dann die Wake-Quellen; die erste mit einer
    /// Aenderung beendet den Schlaf dort (9.9). So sieht kein Tick vor
    /// dieser Grenze einen Tune, der erst an ihr gilt.
    fn wake_asleep(&mut self, mut tunables: Option<&mut dyn Tunables>, now: i64) {
        let Some(a) = self.asleep else { return };
        let polling = self.program.wake_sources();
        if tunables.is_none() && !polling {
            return;
        }
        let first = match (tunables.is_some(), polling) {
            (true, true) => self.tuned.min(self.polled),
            (true, false) => self.tuned,
            _ => self.polled,
        };
        let mut j = a.from.max(first);
        while j < self.k && self.boundary(j) <= now {
            let changed = reborrow(&mut tunables).is_some_and(|t| self.tune_at(t, j));
            if changed || self.poll_at(j) {
                self.wake_at(j);
                return;
            }
            j = j.saturating_add(1);
        }
    }

    /// Das Ende des Schlafs an der Frist `k`: Die geschlafenen Ticks gehen
    /// in das Programm (9.9), dann die zurueckgehaltene Zeile des Ticks
    /// davor mit ihrer Zahl.
    fn end_sleep(&mut self) {
        let Some(mut a) = self.asleep.take() else { return };
        a.tick.slept = self.k.saturating_sub(a.from);
        if a.tick.slept > 0 {
            self.program.advance(a.tick.slept);
        }
        self.publish(a.tick);
    }

    /// Die Zeile eines Ticks: Zeitzeile und Ausgaenge (12.5), dann die
    /// Zusammenfassung.
    fn publish(&mut self, tick: Tick) -> Outputs {
        let shown = self.sink.outputs(Some(&tick));
        self.program.trace(shown);
        self.sink.record(&tick);
        self.last = tick;
        shown
    }

    /// Vor dem ersten Tick: der Anfangszustand (Tick 0, 9.4) in den Trace;
    /// setzt schon er `next_run`, endet der Lauf ohne Tick.
    fn begin<N: Nvm>(&mut self, persist: Option<&mut crate::journal::Persist<'_, N>>) {
        if core::mem::replace(&mut self.begun, true) {
            return;
        }
        // 1.5, 9.4: Tick 0 committet wie jeder Tick — was der Anfangszustand
        // in den Latch schrieb, erreicht die Treiber vor Tick 1.
        self.commit_or_hold();
        let shown = self.sink.outputs(None);
        self.program.trace(shown);
        if let Some(next) = self.program.next_run() {
            self.end_run(next, shown, persist);
        }
    }

    /// Das geordnete Ende (12.7): das Journal synchron, dann der Tick, der
    /// das Ende verlangt, falls der Trace ihn noch nicht zeigt, dann `end`,
    /// die `safe`-Werte an die Treiber und in den Trace.
    fn end_run<N: Nvm>(&mut self, next: NextRun, shown: Outputs, persist: Option<&mut crate::journal::Persist<'_, N>>) {
        self.ended = Some(next);
        self.finish(persist);
        if shown == Outputs::None {
            self.program.trace(Outputs::All);
        }
        self.program.end();
        self.program.commit();
        self.program.trace(if shown == Outputs::Changed { Outputs::Changed } else { Outputs::All });
    }

    /// Der Tick an der Frist: ab `sample_inputs()` wie in 12.1.
    fn tick_now<N: Nvm>(
        &mut self,
        persist: Option<&mut crate::journal::Persist<'_, N>>,
        tunables: Option<&mut dyn Tunables>,
    ) {
        let began = self.clock.now();
        self.release();
        // Was vor dem Abtasten geschah, sieht der Schritt selbst (9.9).
        self.clock.woken();
        let drift = began - self.deadline;
        self.overrun.observe_drift(drift, self.tick_ns);

        // 7.3: Der Overrun des vorigen Ticks wirkt jetzt. Er kommt vor dem
        // Schritt, damit die Maschinen ihn in diesem Tick sehen.
        if core::mem::take(&mut self.pending_overrun) {
            self.program.raise_overrun();
        }
        // 12.6 Zeile 7: die Periode, mit der die Tickquelle diesen Tick
        // gebracht hat, gegen `tick_tolerance`; die Regel ist die des Rands.
        if let (Some(measured), Some(t)) = (self.clock.tick_period(), self.program.tick_tolerance())
            && self.period.observe(measured, self.tick_ns, t.ns, t.runs)
        {
            self.program.raise_hardware();
        }

        // 8.4: der Satz der Grenze vor dem Schritt, als Input von I_k.
        if let Some(t) = tunables {
            self.tune_at(t, self.k);
        }

        // sample_inputs() bis commit_outputs(): die Semantik.
        let now = tick_end(self.k, self.tick_ns);
        self.program.tick(self.k, now);
        self.commit_or_hold();
        let took = self.clock.now() - began;

        // 7.3: am Raster gemessen — der Schritt endet `drift + took` nach
        // dem nominalen Beginn dieses Ticks, und eine Periode danach beginnt
        // der naechste. Ein spaeter Beginn verkuerzt, was dem Schritt bleibt.
        let seen = self.overrun.observe(drift.saturating_add(took), self.tick_ns);
        self.pending_overrun = seen.fault;
        let tick = Tick { k: self.k, now, took, drift, overrun: seen.over, slept: 0 };

        // kick_watchdog()
        self.watchdog.kick();

        // maybe_sleep() (9.9); ein Lauf, der endet, schlaeft nicht mehr, ein
        // vorgemerkter Ueberlauf ist ein `raised`: Er wirkt im naechsten
        // Tick, nicht an der Frist des Schlafs; ebenso ein Weckereignis
        // nach dem Abtasten.
        let ending = self.program.next_run();
        let planned =
            if ending.is_none() && !self.pending_overrun && !self.clock.woken() { self.sleep(now) } else { 0 };
        self.last = tick;

        // Der naechste Tick beginnt eine Periode nach diesem — absolut
        // gerechnet, damit ein zu spaeter Tick die folgenden nicht
        // verschiebt (12.2). Der Tick wird nie uebersprungen (7.3). Im
        // Schlaf rechnet der naechste an der Frist; erst wenn er endet,
        // stehen seine Ticks fest (ohne Vorgriff, FB-388).
        self.k = self.k.saturating_add(1);
        self.beat_from = self.boundary(self.k);
        let shown = if planned > 0 {
            self.asleep = Some(Asleep { tick, from: self.k });
            self.k = self.k.saturating_add(planned);
            Outputs::None
        } else {
            // record_and_telemeter(): nie blockierend (12.2).
            self.publish(tick)
        };
        self.deadline = self.boundary(self.k);

        match ending {
            Some(next) => self.end_run(next, shown, persist),
            None => {
                if let Some(p) = persist {
                    self.journal(p);
                }
            }
        }
    }

    /// Ein Tick mit Jobs (4.5): Vor dem Schritt gehen die fertigen Slots
    /// in das Programm — die Fertigstellung ist ein Input von I_k. `buf`
    /// nimmt das Ergebnis auf; er ist Sache des Aufsatzes, weil der Kern
    /// keinen Heap hat.
    ///
    /// Ein Ergebnis, das `buf` uebersteigt, wird verweigert und kommt als
    /// `Err(FAILED)` an: Gekuerzt waere es gueltig und falsch. Nach dem Ende
    /// des Laufs (12.7) erreicht kein Ergebnis das Programm mehr.
    pub fn step_with_jobs<J: Jobs>(&mut self, jobs: &mut J, buf: &mut [u8]) -> Tick {
        let open = if self.ended.is_none() { jobs.slots() } else { 0 };
        for slot in 0..open {
            match jobs.poll(slot) {
                JobState::Done => {
                    let n = jobs.take(slot, buf);
                    self.program.job_done(slot, buf.get(..n));
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

    /// Das Journal nach dem Schritt (5.9), in der Wartezeit bis zur naechsten
    /// Frist. Ein Geraet, das in der Hardware weiterarbeitet, wird jeden Tick
    /// gefragt; eines, das den Kern anhaelt (12.3), nur wenn die Wartezeit
    /// den Vorgang deckt — nach einem Schlaf ist sie lang, sonst ein Tick
    /// abzueglich des Schritts — oder wenn das Programm Ueberlaeufe annimmt
    /// (`overrun = alert`, 7.3).
    fn journal<N: Nvm>(&mut self, persist: &mut crate::journal::Persist<'_, N>) {
        if self.journal_may_run(persist.blocking_ns()) {
            persist.poll(self.last.now, &mut self.program);
            // Ein Vorgang ueber die Frist hinaus ist ein Ueberlauf (7.3),
            // auch wenn der Schritt selbst gepasst hat.
            let late = self.clock.now().saturating_sub(self.deadline);
            if late > 0 {
                self.pending_overrun |= self.overrun.observe(self.tick_ns.saturating_add(late), self.tick_ns).fault;
            }
        }
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

    /// Die naechste Tickgrenze, an der `service` rechnet (12.11).
    pub fn deadline(&self) -> i64 {
        self.deadline
    }

    /// Das Ende des Laufs, wenn das Programm ihn beendet hat (12.7).
    pub fn ended(&self) -> Option<NextRun> {
        self.ended
    }

    /// Der zuletzt gerechnete Tick.
    pub fn last(&self) -> Tick {
        self.last
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
        let n = u64::try_from(d / self.tick_ns).unwrap_or(0).saturating_sub(1);
        // Mit Ende rechnet der letzte Tick `horizon - 1` an der Frist.
        self.horizon.map_or(n, |h| n.min(h.saturating_sub(self.k.saturating_add(2))))
    }
}
