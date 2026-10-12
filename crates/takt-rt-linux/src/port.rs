//! Der Port `linux_rt` (12.2, 12.11): ein Tick-Faden, der den Kern ohne
//! Warten zu jeder Frist ruft, und ein Job-Faden neben ihm.
//!
//! **Der Tick-Faden** ([`spawn`]) hat den Stack, den der Wirt fuer das
//! Programm nennt (`TICK_STACK_BYTES`), und darueber [`HOST_STACK_BYTES`]
//! fuer `std` und den Wirt. Er bindet sich an seinen Kern und sperrt den
//! Speicher, bevor das Programm entsteht ([`prepare`]); was er erreicht hat,
//! steht in der [`Guarantee`], die der Lauf-Header traegt (11.3). Das
//! Programm baut der Wirt im Faden selbst: Die Huelle haelt Arena und
//! Treiberobjekt geliehen und wechselt den Faden nicht.
//!
//! **Die Schleife** ([`serve`]) ruft `service` zu jeder Frist und wartet
//! dazwischen mit `clock_nanosleep` auf die absolute Frist (12.2). Meldet
//! `service` einen Job, rechnet ihn der Job-Faden; ist er fertig, weckt er
//! den Tick-Faden, und `service` gibt vor der Frist den naechsten wartenden
//! Job aus (12.11, Portvertrag 3). Solange ein Job rechnet, wartet der
//! Tick-Faden darum auf Frist *oder* Wecker
//! ([`RealtimeClock::wait_until_or`]).
//!
//! **Prioritaet.** Der Job-Kontext liegt unter dem Schritt (12.11). Unter
//! `chrt` erbt der Job-Faden die Echtzeitklasse des Prozesses, und abgeben
//! koennte er sie nur mit `sched_setscheduler`, das `rustix` nicht anbietet
//! (FB-90). Der Port legt ihn darum auf die Kerne des Prozesses ohne die des
//! Tick-Fadens; bleibt keiner, sagt es die Zusage ([`Guarantee::apart`]).
//!
//! **Keine Allokation im Tick.** Faden, Stack und Uebergabe entstehen vor
//! dem ersten Tick; danach tauschen die Faeden nur Zaehler ueber Atomics
//! und wecken sich mit `unpark` (12.2).

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;

use takt_rt_core::{Clock, NextRun, Program, Runtime, Sink, Store, Watchdog};

use crate::clock::RealtimeClock;
use crate::guarantee::{Guarantee, prepare};

/// Was `std` und der Wirt in jedem Faden ueber den Stack des Programms
/// hinaus brauchen, in Bytes: den Start des Fadens, die Schleife des Ports,
/// die Leitung des Wirts.
///
/// Die Reserve des Ports nennt 12.3 aus der Hardware-Konfiguration; die gilt
/// dem, was der Schritt ruft. Was ein Faden von `std` selbst braucht, haengt
/// an der Toolchain des Wirts und bekommt hier reichlich Platz.
pub const HOST_STACK_BYTES: usize = 64 * 1024;

/// Wie der Port laeuft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Der Stack des Schritts: `TICK_STACK_BYTES` des Programms (12.3).
    pub tick_stack: usize,
    /// Der Stack des Job-Kontexts: `JOB_STACK_BYTES` des Programms (4.5).
    pub job_stack: usize,
    /// Der Kern des Tick-Fadens (12.2); `None` laesst die Bindung, wie sie
    /// ist (etwa, weil `taskset` sie gesetzt hat).
    pub cpu: Option<usize>,
    /// Nach so vielen Ticks ab dem Aufruf von [`serve`] endet der Lauf; 0
    /// laeuft, bis das Programm ihn beendet (12.7).
    pub ticks: u64,
}

impl Config {
    /// Ein Lauf mit den Stacks des Programms, ungebunden und ohne Ende.
    pub fn new(tick_stack: usize, job_stack: usize) -> Config {
        Config { tick_stack, job_stack, cpu: None, ticks: 0 }
    }

    /// Bindet den Tick-Faden an den Kern `cpu`.
    #[must_use]
    pub fn cpu(mut self, cpu: usize) -> Config {
        self.cpu = Some(cpu);
        self
    }

    /// Endet nach `ticks` Ticks, wie ein Konformitaetslauf (13.8).
    #[must_use]
    pub fn ticks(mut self, ticks: u64) -> Config {
        self.ticks = ticks;
        self
    }
}

/// Was ein Lauf ergab (7.3, 12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Served {
    /// Die Nummer des naechsten Ticks; so weit reicht der Lauf.
    pub ticks: u64,
    /// Wie das Programm den Lauf beendet hat (12.7); `None`, wenn er nach
    /// den Ticks der [`Config`] endete.
    pub ended: Option<NextRun>,
    /// Das Journal schrieb am Ende (5.9).
    pub flushed: bool,
    /// Fristen, die beim Warten schon vorbei waren (7.3).
    pub late: u64,
    /// Ticks, die ihre Periode ueberschritten (7.3).
    pub overruns: u64,
    /// Auftraege, die der Job-Faden bekam (4.5).
    pub jobs: u64,
}

/// Startet den Tick-Faden und rechnet `body` darin (12.2); liefert, was
/// `body` liefert.
///
/// Vor `body` bindet sich der Faden an den Kern der [`Config`] und sperrt den
/// Speicher; `body` bekommt den Befund, baut das Programm mit seiner Uhr
/// ([`RealtimeClock::under`]) und faehrt es mit [`serve`]. Eine Panik in
/// `body` kommt beim Aufrufer an.
///
/// # Errors
///
/// Wenn sich der Faden nicht anlegen laesst.
pub fn spawn<T, B>(config: &Config, body: B) -> io::Result<T>
where
    T: Send,
    B: FnOnce(&Guarantee) -> T + Send,
{
    let cpu = config.cpu;
    let stack = config.tick_stack.saturating_add(HOST_STACK_BYTES);
    thread::scope(|scope| {
        let tick =
            thread::Builder::new().name("takt-tick".into()).stack_size(stack).spawn_scoped(scope, move || {
                let guarantee = prepare(cpu);
                body(&guarantee)
            })?;
        match tick.join() {
            Ok(value) => Ok(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

/// Die Uebergabe zwischen Tick- und Job-Faden: Zaehler statt Sperren.
#[derive(Debug, Default)]
struct Handoff {
    /// Der letzte Auftrag, den der Tick-Faden gab.
    given: AtomicU32,
    /// Der letzte Auftrag, den der Job-Faden gerechnet hat.
    done: AtomicU32,
    /// Der Lauf ist vorbei.
    stop: AtomicBool,
}

/// Haelt den Job-Faden an, sobald die Schleife endet, auch durch eine Panik
/// im Tick-Faden: Sonst wartete der Scope ewig auf ihn.
struct Stop<'a> {
    handoff: &'a Handoff,
    worker: Option<thread::Thread>,
}

impl Drop for Stop<'_> {
    fn drop(&mut self) {
        self.handoff.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = &self.worker {
            worker.unpark();
        }
    }
}

/// Die Kerne des Job-Fadens (4.5, 12.2): die des Prozesses ohne die des
/// Tick-Fadens, gelesen im Tick-Faden.
#[derive(Clone, Copy, Debug)]
struct Cores {
    #[cfg(target_os = "linux")]
    set: Option<rustix::thread::CpuSet>,
}

impl Cores {
    /// Die Kerne des Prozesses ohne die dieses Fadens.
    fn besides_this_thread() -> Cores {
        Cores {
            #[cfg(target_os = "linux")]
            set: rustix::thread::sched_getaffinity(None).ok().and_then(crate::guarantee::others),
        }
    }

    /// Bindet den rufenden Faden an diese Kerne; bleibt keiner, rechnet er,
    /// wo er geerbt hat, und die Zusage sagt es ([`Guarantee::apart`]).
    fn bind(self) {
        #[cfg(target_os = "linux")]
        if let Some(set) = self.set {
            let _ = rustix::thread::sched_setaffinity(None, &set);
        }
    }
}

/// Faehrt die Schleife im Tick-Faden (12.11): `service` zu jeder Frist,
/// die Jobs auf einem eigenen Faden, und am Ende das Journal (5.9).
///
/// `jobs` ist der Job-Kontext des Programms: Er rechnet den Auftrag, den
/// `service` gab. Ohne ihn laeuft kein Job-Faden. Der Lauf endet, wenn das
/// Programm ihn beendet (12.7) oder nach den Ticks der [`Config`]; ein Schlaf ueber
/// dieses Ende hinaus endet ihn an seinem Beginn, wie in logischer Zeit.
///
/// # Errors
///
/// Wenn sich der Job-Faden nicht anlegen laesst.
pub fn serve<P, W, S, J, K>(
    rt: &mut Runtime<P, RealtimeClock, W, S, J>,
    jobs: Option<K>,
    config: &Config,
) -> io::Result<Served>
where
    P: Program,
    W: Watchdog,
    S: Sink,
    J: Store,
    K: FnMut() + Send,
{
    let span = i64::try_from(config.ticks).unwrap_or(i64::MAX).saturating_mul(rt.tick_ns());
    let end = (config.ticks > 0).then(|| rt.deadline().saturating_add(span));
    let handoff = Handoff::default();
    let cores = Cores::besides_this_thread();
    let tick = thread::current();
    let stack = config.job_stack.saturating_add(HOST_STACK_BYTES);
    let dispatched = thread::scope(|scope| {
        let handoff = &handoff;
        let worker = jobs
            .map(|mut work| {
                thread::Builder::new().name("takt-job".into()).stack_size(stack).spawn_scoped(scope, move || {
                    cores.bind();
                    let mut taken = 0u32;
                    loop {
                        let wanted = handoff.given.load(Ordering::SeqCst);
                        if wanted != taken {
                            taken = wanted;
                            work();
                            handoff.done.store(taken, Ordering::SeqCst);
                            tick.unpark();
                        } else if handoff.stop.load(Ordering::SeqCst) {
                            break;
                        } else {
                            thread::park();
                        }
                    }
                })
            })
            .transpose()?;
        let stop = Stop { handoff, worker: worker.as_ref().map(|w| w.thread().clone()) };
        let mut given = 0u32;
        let mut count = 0u64;
        loop {
            let next = rt.service();
            if next.jobs
                && let Some(worker) = &worker
            {
                given = given.wrapping_add(1);
                count = count.saturating_add(1);
                handoff.given.store(given, Ordering::SeqCst);
                worker.thread().unpark();
            }
            if next.ended.is_some() || end.is_some_and(|e| next.deadline >= e) {
                break;
            }
            if handoff.done.load(Ordering::SeqCst) == given {
                rt.clock.wait_until(next.deadline);
            } else {
                rt.clock.wait_until_or(next.deadline, || handoff.done.load(Ordering::SeqCst) == given);
            }
        }
        drop(stop);
        if let Some(worker) = worker
            && let Err(panic) = worker.join()
        {
            std::panic::resume_unwind(panic);
        }
        Ok::<u64, io::Error>(count)
    })?;
    let flushed = rt.finish();
    Ok(Served {
        ticks: rt.tick_number(),
        ended: rt.ended(),
        flushed,
        late: rt.clock.late,
        overruns: rt.overrun().count,
        jobs: dispatched,
    })
}
