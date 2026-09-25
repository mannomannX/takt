//! Die Bring-up-Schleife ueber der Runtime (12.1, 12.3): ein Lauf, nicht
//! einer je Board. Das Board liefert Uhr, Leitung und Journal; hier steht,
//! was daraus ein Lauf macht — Ausgaenge im Takt der Leitung, ein Paket
//! je Tick, am Ende die Bilanz.

use takt_rt_core::{Clock, Nvm, Overrun, Persist, Program, Runtime, Sink, Watchdog};

use crate::telemetry::{DRAIN_ROUNDS, Port, Telemetry};

/// Das erzeugte Programm, soweit die Schleife es anspricht.
pub trait Traced: Program {
    /// Gibt den Latch an die Treiber (12.1, Schritt 10).
    fn commit(&self);

    /// Die Ausgaenge als Trace-Zeilen; ohne `all` nur die geaenderten (9.3).
    fn dump(&self, all: bool);

    /// Der Programmzaehler je Maschine (11.2).
    fn pc(&self);
}

/// Kein Hardware-Watchdog angebunden.
pub struct NoWatchdog;

impl Watchdog for NoWatchdog {
    fn kick(&mut self) {}
}

/// Wie oft und wie lange die Schleife ausgibt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cadence {
    /// Alle wie viele Ticks die Ausgaenge gehen. `1` ist der
    /// Konformitaetslauf: nur die Aenderungen, dazu die Zeitzeile —
    /// je Tick, aber hoechstens eine je Millisekunde (FB-271).
    pub every: u64,
    /// Nach so vielen Ticks endet der Lauf; `0` heisst nie.
    pub limit: u64,
    /// Den Programmzaehler mitgeben (11.2, `statements`).
    pub pc: bool,
}

impl Cadence {
    /// Der Konformitaetslauf ueber `limit` Ticks, sonst alle `every` Ticks ein Abzug.
    pub fn of(limit: u64, every: u64, pc: bool) -> Cadence {
        Cadence { every: if limit > 0 { 1 } else { every.max(1) }, limit, pc }
    }

    fn conformance(self) -> bool {
        self.every == 1
    }
}

/// Die Bilanz eines Laufs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Geschlafene Ticks (9.9).
    pub slept: u64,
    /// Ticks ueber der Periode (7.3).
    pub overruns: u64,
    /// Das Journal hat am Ende geschrieben (5.9).
    pub flushed: bool,
}

/// Das Journal in Zahlen, fuer die Bilanz.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStats {
    /// Geschriebene Eintraege.
    pub writes: u32,
    /// Gescheiterte Schreibversuche.
    pub failures: u32,
    /// Gemessene Loeschdauer in ns.
    pub erase_ns: i64,
    /// Gemessene Programmierdauer in ns.
    pub program_ns: i64,
}

/// Laeuft, bis `cadence.limit` erreicht ist.
///
/// `telemetry` holt die Leitung je Aufruf, wie der erzeugte Rahmen sie
/// ueber `takt_board_trace` holt — so gibt es nie zwei Griffe zugleich.
pub fn run<G, C, W, S, N, P, const R: usize>(
    rt: &mut Runtime<G, C, W, S>,
    mut persist: Option<&mut Persist<'_, N>>,
    cadence: Cadence,
    mut telemetry: impl FnMut() -> Option<&'static mut Telemetry<P, R>>,
) -> Stats
where
    G: Traced,
    C: Clock,
    W: Watchdog,
    S: Sink,
    N: Nvm,
    P: Port + 'static,
{
    let mut stats = Stats::default();
    if cadence.conformance() {
        rt.program.dump(true);
    }
    let every = cadence.every.max(1);
    let mut next = every;
    // Unter einer Millisekunde Tick traegt die Leitung keine Zeile je
    // Tick (FB-271); die Zeitzeile ist Statistik und darf duenner werden.
    let time_every = (1_000_000 / rt.tick_ns()).max(1) as u64;
    loop {
        let tick = match persist.as_deref_mut() {
            Some(p) => rt.step_persisting(p),
            None => rt.step(),
        };
        stats.slept += tick.slept;
        stats.overruns += u64::from(tick.overrun);
        rt.program.commit();
        if cadence.conformance()
            && tick.k % time_every == 0
            && let Some(t) = telemetry()
        {
            t.write_time(&tick);
        }
        let k = rt.tick_number();
        if k >= next {
            next = k + every;
            rt.program.dump(!cadence.conformance());
            if cadence.pc {
                rt.program.pc();
            }
        }
        if let Some(t) = telemetry() {
            t.flush();
        }
        if cadence.limit > 0 && k >= cadence.limit {
            break;
        }
    }
    stats.flushed = persist.is_some_and(|p| p.flush(&mut rt.program));
    stats
}

/// Die Bilanz als letzte Zeile, dann `takt end`; leert den Ring.
pub fn report<P: Port, const R: usize>(
    t: &mut Telemetry<P, R>,
    overrun: &Overrun,
    stats: &Stats,
    journal: &JournalStats,
) {
    let dropped = u64::from(t.dropped());
    let counts = [
        ("takt schlief ", stats.slept),
        (" ueberlaeufe ", stats.overruns),
        (" verspaetet ", overrun.late),
        (" verloren ", overrun.lost),
    ];
    for (label, n) in counts {
        t.write(label);
        t.write_u64(n);
    }
    t.write(" rueckstand ");
    t.write_i64(overrun.worst_drift);
    t.write(" ns verworfen ");
    t.write_u64(dropped);
    let journal_counts = [
        (" journal geschrieben ", u64::from(journal.writes)),
        (" fehlgeschlagen ", u64::from(journal.failures)),
        (" flush ", u64::from(stats.flushed)),
    ];
    for (label, n) in journal_counts {
        t.write(label);
        t.write_u64(n);
    }
    t.write(" nvm loeschen ");
    t.write_i64(journal.erase_ns);
    t.write(" ns programmieren ");
    t.write_i64(journal.program_ns);
    t.write(" ns");
    t.newline();
    t.write("takt end");
    t.newline();
    t.drain(DRAIN_ROUNDS);
}
