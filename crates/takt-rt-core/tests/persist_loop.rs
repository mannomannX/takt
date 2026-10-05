//! Das Journal in der Tickschleife (5.9, 12.1).
//!
//! Ein Programm, das eine Zahl persistiert; die Schleife schreibt sie ins
//! Journal, ein Neustart liest sie zurueck. Dazu die Zusage, die FB-149
//! gebraucht haette: Ein langsames Geraet haelt den Tick nicht auf.

use takt_rt_core::journal::{FakeNvm, Journal, Loaded, Persist};
use takt_rt_core::{Clock, NextRun, Nvm, NvmState, Policy, Profile, Program, Runtime, Sink, Tick, Watchdog};

const SLOT: usize = 256;
const T0: i64 = 1_000_000;
const HASH: u64 = 0x1234;

/// Eine Uhr, die dem Tick ohne Wartezeit folgt.
#[derive(Default)]
struct Instant(i64);

impl Clock for Instant {
    fn now(&self) -> i64 {
        self.0
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

struct Quiet;

impl Watchdog for Quiet {
    fn kick(&mut self) {}
}

impl Sink for Quiet {
    fn record(&mut self, _: &Tick) {}
}

/// Zaehlt Ticks und persistiert den Zaehler als vier Bytes.
struct Counter {
    value: u32,
    restored: Option<u32>,
}

impl Program for Counter {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.value += 1;
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        4
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        if bytes.len() != 4 {
            return 0;
        }
        self.value = u32::from_le_bytes(bytes.try_into().expect("vier Byte"));
        self.restored = Some(self.value);
        1
    }
}

fn runtime(program: Counter) -> Runtime<Counter, Instant, Quiet, Quiet> {
    Runtime::new(program, Instant::default(), Quiet, Quiet, Profile::LINUX_RT, T0, Policy::Fault)
}

#[test]
fn the_loop_writes_the_journal_and_a_restart_reads_it_back() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    assert_eq!(persist.load(&mut rt.program).0, Loaded::Empty);
    rt.run_persisting(50, &mut persist);
    assert!(persist.journal().writes() > 0, "die Schleife hat nie geschrieben");
    assert!(persist.flush(&mut rt.program, || true), "flush scheiterte");
    let value = rt.program.value;

    // Neustart mit demselben Geraet.
    let nvm = persist.into_journal().into_inner();
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(nvm, HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    let (found, applied) = persist.load(&mut rt.program);
    assert!(matches!(found, Loaded::Found { .. }), "nichts gefunden");
    assert_eq!(applied, 1);
    assert_eq!(rt.program.restored, Some(value), "der Zaehler kam nicht zurueck");
}

#[test]
fn min_interval_holds_in_the_loop() {
    // 5.9: hoechstens alle min_interval — bei 10 s und 1-ms-Tick sind das
    // hoechstens ein Schreibvorgang je 10 000 Ticks.
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let ten_s = 10_000 * T0;
    let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, ten_s), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    persist.load(&mut rt.program);
    rt.run_persisting(25_000, &mut persist);
    let writes = persist.journal().writes();
    assert!(writes <= 3, "{writes} Schreibvorgaenge in 25 s bei 10 s Abstand");
    assert!(writes >= 2, "{writes} Schreibvorgaenge: das Journal schreibt nicht mehr");
}

#[test]
fn a_slow_device_costs_the_tick_nothing() {
    // Ein Geraet, dessen Vorgaenge 200 Ticks dauern: Die Schleife laeuft
    // in logischer Zeit weiter, kein Tick wartet auf das Journal.
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let slow = FakeNvm::<SLOT>::new().with_latency(200);
    let mut persist = Persist::new(Journal::new(slow, HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    persist.load(&mut rt.program);
    for _ in 0..1000 {
        let tick = rt.step_persisting(&mut persist);
        assert!(!tick.overrun, "Tick {} lief ueber", tick.k);
    }
    assert!(persist.journal().writes() >= 1);
}

/// Zaehlt Ticks, persistiert den Zaehler und schlaeft nach jedem Schritt
/// `window` Ticks (9.9), wenn `window > 0`.
struct Sleeper {
    value: u32,
    now: i64,
    window: i64,
    overruns: u32,
}

impl Program for Sleeper {
    fn tick(&mut self, _k: u64, now: i64) {
        self.value += 1;
        self.now = now;
    }

    fn raise_overrun(&mut self) {
        self.overruns += 1;
    }

    fn sleep_allowed(&self) -> bool {
        self.window > 0
    }

    fn next_deadline(&self) -> Option<i64> {
        Some(self.now + self.window * T0)
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        4
    }
}

fn sleeping_runtime(window: i64, policy: Policy) -> Runtime<Sleeper, Instant, Quiet, Quiet> {
    let program = Sleeper { value: 0, now: 0, window, overruns: 0 };
    Runtime::new(program, Instant::default(), Quiet, Quiet, Profile::BAREMETAL, T0, policy)
}

/// Ein Geraet, das den Kern fuenf Ticks lang anhaelt (12.3).
fn blocking() -> FakeNvm<SLOT> {
    FakeNvm::<SLOT>::new().with_blocking_ns(5 * T0)
}

/// **Ein blockierendes Geraet schreibt nur im Schlaffenster** (12.3, 9.9):
/// Mit zehn Ticks Schlaf je Schritt deckt das Fenster den Vorgang, mit
/// zwei nicht.
#[test]
fn a_blocking_device_writes_only_in_a_sleep_window() {
    for (window, expect_writes) in [(10, true), (2, false)] {
        let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
        let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
        let mut rt = sleeping_runtime(window, Policy::Fault);
        persist.load(&mut rt.program);
        rt.run_persisting(100, &mut persist);
        assert_eq!(persist.journal().writes() > 0, expect_writes, "Fenster von {window} Ticks");
    }
}

/// Unter `overrun = alert` schreibt es sofort — das Programm traegt die
/// Ueberlaeufe (7.3).
#[test]
fn under_alert_a_blocking_device_writes_at_once() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
    let mut rt = sleeping_runtime(0, Policy::Alert);
    persist.load(&mut rt.program);
    rt.run_persisting(20, &mut persist);
    assert!(persist.journal().writes() > 0);
}

/// Ohne Fenster und unter `fault` schreibt nur der Flush (5.9: vor dem
/// geordneten Ende eines Laufs, 12.7).
#[test]
fn without_a_window_only_the_flush_writes() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(blocking(), HASH, 0), &mut current, &mut stored);
    let mut rt = sleeping_runtime(0, Policy::Fault);
    persist.load(&mut rt.program);
    rt.run_persisting(100, &mut persist);
    assert_eq!(persist.journal().writes(), 0);
    assert!(persist.flush(&mut rt.program, || true));
    assert_eq!(persist.journal().writes(), 1);
}

// --- Ende des Laufs mit Journal (12.7, 5.9) -----------------------------

/// Zaehlt Ticks, persistiert den Zaehler und beendet den Lauf nach `ends`
/// Ticks; schreibt mit, in welcher Reihenfolge der Kern es ruft.
struct Finishing {
    value: u32,
    ends: u32,
    log: Vec<&'static str>,
}

impl Program for Finishing {
    fn tick(&mut self, _k: u64, _now: i64) {
        self.value += 1;
        self.log.push("tick");
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        out[..4].copy_from_slice(&self.value.to_le_bytes());
        self.log.push("snapshot");
        4
    }

    fn next_run(&self) -> Option<NextRun> {
        (self.value >= self.ends).then_some(NextRun::OnStart)
    }

    fn commit(&mut self) {
        self.log.push("commit");
    }

    fn end(&mut self) {
        self.log.push("end");
    }
}

/// **`next_run` schreibt das Journal vor dem Ende, ohne `min_interval`**
/// (12.7, 5.9): erst der Stand, dann `end` und die `safe`-Werte; der
/// naechste Start findet den Stand des letzten Ticks.
#[test]
fn the_end_of_a_run_flushes_before_it_goes_safe() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let journal = Journal::new(FakeNvm::<SLOT>::new(), HASH, 10_000 * T0);
    let mut persist = Persist::new(journal, &mut current, &mut stored);
    let program = Finishing { value: 0, ends: 3, log: Vec::new() };
    let mut rt = Runtime::new(program, Instant::default(), Quiet, Quiet, Profile::BAREMETAL, T0, Policy::Fault);
    persist.load(&mut rt.program);
    while rt.ended().is_none() {
        rt.clock.0 += T0;
        rt.service_persisting(&mut persist);
    }
    let tail = &rt.program.log[rt.program.log.len() - 5..];
    assert_eq!(tail, ["tick", "commit", "snapshot", "end", "commit"], "{:?}", rt.program.log);
    assert!(rt.finish(Some(&mut persist)), "geschrieben");
    assert_eq!(persist.journal().writes(), 2, "Tick 0 sofort, das Ende trotz Intervall");

    let nvm = persist.into_journal().into_inner();
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(nvm, HASH, 0), &mut current, &mut stored);
    let mut restarted = Counter { value: 0, restored: None };
    assert!(matches!(persist.load(&mut restarted).0, Loaded::Found { .. }));
    assert_eq!(restarted.restored, Some(3), "der Stand des letzten Ticks");
}

/// Eine Uhr, die jedes Ablesen eine Mikrosekunde weiterlaufen laesst, wie
/// eine echte waehrend eines Wartens auf das Geraet.
struct Running(core::cell::Cell<i64>);

impl Clock for Running {
    fn now(&self) -> i64 {
        let now = self.0.get();
        self.0.set(now + 1_000);
        now
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0.set(self.0.get().max(deadline));
    }
}

/// **Ein haengendes Geraet haelt das Ende nicht auf** (12.7, 12.11): Mit
/// einer Frist fuer das Ende gibt das Journal auf, der Lauf endet trotzdem
/// mit `end` und den `safe`-Werten, und die Bilanz sagt: nicht geschrieben.
#[test]
fn a_hanging_device_does_not_stop_the_end_of_a_run() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let journal = Journal::new(FakeNvm::<SLOT>::new(), HASH, 0);
    let mut persist = Persist::new(journal, &mut current, &mut stored);
    let program = Finishing { value: 0, ends: 3, log: Vec::new() };
    let clock = Running(core::cell::Cell::new(0));
    let mut rt =
        Runtime::new(program, clock, Quiet, Quiet, Profile::BAREMETAL, T0, Policy::Alert).with_end_budget(50 * T0);
    persist.load(&mut rt.program);
    rt.step_persisting(&mut persist);
    persist_device(&mut persist).cut_at(0);
    while rt.ended().is_none() {
        rt.step_persisting(&mut persist);
    }
    assert!(!rt.finish(Some(&mut persist)), "nichts geschrieben");
    assert!(persist.journal().failures() > 0);
    let tail = &rt.program.log[rt.program.log.len() - 2..];
    assert_eq!(tail, ["end", "commit"], "das Ende lief trotzdem: {:?}", rt.program.log);
}

/// Das Geraet unter einem `Persist`, um ihm den Strom zu nehmen.
fn persist_device<'p>(persist: &'p mut Persist<'_, FakeNvm<SLOT>>) -> &'p mut FakeNvm<SLOT> {
    persist.journal_mut().device_mut()
}

// --- Journal ueber die Frist ist ein Ueberlauf (7.3) --------------------

/// Eine Uhr, die das Geraet vorstellt.
struct Shared<'a>(&'a core::cell::Cell<i64>);

impl Clock for Shared<'_> {
    fn now(&self) -> i64 {
        self.0.get()
    }

    fn wait_until(&mut self, deadline: i64) {
        self.0.set(self.0.get().max(deadline));
    }
}

/// Ein Geraet, das den Kern je begonnenem Vorgang `cost` lang anhaelt und
/// `declared` als seine Dauer meldet (12.3).
struct Stalling<'a> {
    nvm: FakeNvm<SLOT>,
    clock: &'a core::cell::Cell<i64>,
    cost: i64,
    declared: i64,
}

impl Stalling<'_> {
    fn stall(&self) {
        self.clock.set(self.clock.get() + self.cost);
    }
}

impl Nvm for Stalling<'_> {
    fn slot_size(&self) -> u32 {
        self.nvm.slot_size()
    }

    fn begin_erase(&mut self, slot: u8) -> bool {
        self.stall();
        self.nvm.begin_erase(slot)
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        self.stall();
        self.nvm.begin_write(slot, offset, bytes)
    }

    fn poll(&mut self) -> NvmState {
        self.nvm.poll()
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        self.nvm.read(slot, offset, into)
    }

    fn blocking_ns(&self) -> Option<i64> {
        Some(self.declared)
    }
}

/// **Unter `alert` zaehlt ein Journal-Vorgang ueber die Frist als
/// Ueberlauf, ohne zu faulten** (7.3 letzter Satz): Der Schritt selbst passt
/// in die Periode, der Vorgang nicht. Der Tick danach beginnt darum spaet und
/// endet nach seinem Raster; am Raster gemessen (RT-003) zaehlt auch er.
#[test]
fn under_alert_a_journal_over_the_deadline_counts_as_an_overrun() {
    let now = core::cell::Cell::new(0);
    let device = Stalling { nvm: FakeNvm::new(), clock: &now, cost: 5 * T0, declared: 5 * T0 };
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(device, HASH, 0), &mut current, &mut stored);
    let program = Sleeper { value: 0, now: 0, window: 0, overruns: 0 };
    let mut rt = Runtime::new(program, Shared(&now), Quiet, Quiet, Profile::BAREMETAL, T0, Policy::Alert);
    persist.load(&mut rt.program);
    let mut over = 0;
    for _ in 0..20 {
        over += u64::from(rt.step_persisting(&mut persist).overrun);
    }
    assert!(persist.journal().writes() > 0);
    assert!(rt.overrun().count > over, "die Journal-Vorgaenge zaehlen neben den spaeten Ticks: {over}");
    assert_eq!(rt.program.overruns, 0, "unter `alert` kein Fault");
}

/// **Unter `fault` wird er im naechsten Tick `Runtime(Overrun)`** (7.3):
/// Das Geraet meldet fuenf Perioden, braucht aber zwoelf; das Schlaffenster
/// von zehn deckt nur die gemeldete Dauer.
#[test]
fn under_fault_a_journal_over_the_deadline_faults_the_next_tick() {
    let now = core::cell::Cell::new(0);
    let device = Stalling { nvm: FakeNvm::new(), clock: &now, cost: 12 * T0, declared: 5 * T0 };
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(device, HASH, 0), &mut current, &mut stored);
    let program = Sleeper { value: 0, now: 0, window: 10, overruns: 0 };
    let mut rt = Runtime::new(program, Shared(&now), Quiet, Quiet, Profile::BAREMETAL, T0, Policy::Fault);
    persist.load(&mut rt.program);
    rt.step_persisting(&mut persist);
    assert_eq!((rt.overrun().count, rt.program.overruns), (1, 0), "der Vorgang lief ueber die Frist");
    rt.step_persisting(&mut persist);
    assert_eq!(rt.program.overruns, 1, "im naechsten Tick");
}

// --- Stromausfall in der Schleife (5.9, 8.11; KON2-022) ----------------

/// Eine Kopie des Geraets, wie sie ein Neustart vorfaende.
fn copy_of(nvm: &FakeNvm<SLOT>) -> FakeNvm<SLOT> {
    let mut copy = FakeNvm::<SLOT>::new();
    for slot in 0..2u8 {
        for raw in [None, Some(nvm.slot(slot))] {
            let started = match raw {
                None => copy.begin_erase(slot),
                Some(bytes) => copy.begin_write(slot, 0, bytes),
            };
            assert!(started);
            while copy.poll() == NvmState::Busy {}
        }
    }
    copy
}

/// Was ein Neustart auf `nvm` mit dem Typ-Hash `hash` in den Zaehler laedt.
fn restart(nvm: FakeNvm<SLOT>, hash: u64) -> Option<u32> {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(nvm, hash, 0), &mut current, &mut stored);
    let mut program = Counter { value: 0, restored: None };
    persist.load(&mut program);
    program.restored
}

/// So viele Ticks laufen die Schnitt-Laeufe: genug fuer mehrere
/// Slotwechsel, denn ein Slot fasst nur wenige Eintraege.
const CUT_TICKS: u64 = 80;

/// **Ein Stromausfall an jedem Byte laesst den alten oder den neuen Stand
/// zurueck** (5.9, 8.11; KON2-022): Die Schleife schreibt das Journal
/// (`Persist::poll`) ueber mehrere Slotwechsel; bricht der Strom nach `n`
/// Bytes ab, laedt ein Neustart den Zaehler, den ein Neustart vor dem
/// angefangenen Vorgang geladen haette, oder den danach — nie einen
/// halben, nie einen aelteren. Ein `flush` auf dem abgebrochenen Geraet
/// gibt auf, statt zu haengen.
#[test]
fn a_power_cut_in_the_loop_restores_the_old_or_the_new_count() {
    // Der Lauf ohne Schnitt: nach jedem Tick, wie viele Bytes geschrieben
    // sind und was ein Neustart dann laede.
    let mut reference = vec![(0u32, None)];
    {
        let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
        let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, 0), &mut current, &mut stored);
        let mut rt = runtime(Counter { value: 0, restored: None });
        persist.load(&mut rt.program);
        for _ in 0..CUT_TICKS {
            rt.step_persisting(&mut persist);
            let nvm = persist.journal().device();
            reference.push((nvm.bytes_written(), restart(copy_of(nvm), HASH)));
        }
        let (e, w) = (persist.journal().device().erases(), persist.journal().writes());
        assert!(e >= 3, "der Lauf wechselt die Slots: {e} Loeschungen, {w} Eintraege");
    }
    let total = reference.last().map_or(0, |(bytes, _)| *bytes);
    for cut in 0..total {
        let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
        let mut nvm = FakeNvm::<SLOT>::new();
        nvm.cut_at(cut);
        let mut persist = Persist::new(Journal::new(nvm, HASH, 0), &mut current, &mut stored);
        let mut rt = runtime(Counter { value: 0, restored: None });
        persist.load(&mut rt.program);
        rt.run_persisting(CUT_TICKS, &mut persist);
        let mut patience = 8;
        let flushed = persist.flush(&mut rt.program, || {
            patience -= 1;
            patience > 0
        });
        assert!(!flushed, "Schnitt bei {cut}: ohne Strom wird nichts geschrieben");
        let mut nvm = persist.into_journal().into_inner();
        nvm.power_on();
        let got = restart(nvm, HASH);
        // Der Tick, in dem der Schnitt lag: sein Vorgang hatte begonnen.
        let s = reference.iter().position(|(bytes, _)| *bytes > cut).unwrap_or(reference.len() - 1);
        let (old, new) = (reference[s - 1].1, reference[s].1);
        assert!(got == old || got == new, "Schnitt bei {cut}: {got:?}, erwartet {old:?} oder {new:?}");
    }
}

/// **Fremder Typ-Hash, verfaelschter CRC, geloeschtes Flash** (5.9, 11.3;
/// KON2-022), je durch die Schleife bis ins Programm: Ein Journal eines
/// anderen Programms laedt nichts, ein verfaelschter neuester Eintrag
/// laesst den davor gelten, ein geloeschtes Geraet ist leer und wird
/// danach beschrieben.
#[test]
fn foreign_hash_broken_crc_and_erased_flash_reach_the_program_as_specified() {
    let (mut current, mut stored) = ([0u8; SLOT], [0u8; SLOT]);
    let mut persist = Persist::new(Journal::new(FakeNvm::<SLOT>::new(), HASH, 0), &mut current, &mut stored);
    let mut rt = runtime(Counter { value: 0, restored: None });
    assert_eq!(persist.load(&mut rt.program).0, Loaded::Empty, "geloeschtes Flash: leer, kein Fehler");
    assert_eq!(rt.program.restored, None);
    rt.run_persisting(3, &mut persist);
    assert!(persist.flush(&mut rt.program, || true));
    let newest = rt.program.value;
    let nvm = persist.into_journal().into_inner();
    assert_eq!(restart(copy_of(&nvm), HASH), Some(newest));
    assert_eq!(restart(copy_of(&nvm), HASH ^ 1), None, "fremder Typ-Hash: nichts geladen");

    // Den neuesten Eintrag finden und ein Byte seiner Nutzlast kippen.
    let slot = (0..2u8).find(|s| nvm.slot(*s).windows(4).any(|w| w == newest.to_le_bytes())).expect("Eintrag");
    let raw = nvm.slot(slot);
    let at = raw.windows(4).rposition(|w| w == newest.to_le_bytes()).expect("Nutzlast");
    let mut broken = *raw;
    broken[at] ^= 0x01;
    let mut tampered = copy_of(&nvm);
    assert!(tampered.begin_erase(slot));
    while tampered.poll() == NvmState::Busy {}
    assert!(tampered.begin_write(slot, 0, &broken));
    while tampered.poll() == NvmState::Busy {}
    let got = restart(tampered, HASH);
    assert!(got.is_some_and(|v| v < newest), "verfaelschter CRC: der Eintrag davor gilt, nicht {got:?}");
}
