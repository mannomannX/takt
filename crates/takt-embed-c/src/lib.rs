//! Takt als Baustein in C (12.11): das SDK-Archiv `libtakt_sdk.a` mit dem
//! Kern ohne Warten, den kuratierten Natives und der korrekt gerundeten
//! Mathematik. Den Kern in C nachzubauen, haette eine zweite Runtime ergeben
//! (plan/m11.md 2.8); der C-Wirt bekommt denselben wie ein Rust-Wirt.
//!
//! **Die Schnittstelle** steht in `include/takt_sdk.h`, und `takt build
//! --emit embed` legt den Kopf neben `P.h`. Eine Programmbibliothek bringt
//! ihre Einstiege als Tabelle `P_ops` ([`Ops`]); der Wirt gibt sie mit
//! Arena und Treiberobjekt an [`takt_core_init`] — hat das Programm
//! `persist`, an [`takt_core_init_journaled`] mit seinem Journal — und ruft
//! dann [`takt_core_service`] zu jeder Frist, die der Kern nennt, und
//! [`takt_core_job_work`] im Job-Kontext, wenn `jobs` gesetzt ist (12.11).
//!
//! **Kein Heap.** Der Kern liegt im Speicher des Wirts ([`Core`],
//! `takt_core` in C) mit fester Groesse [`CORE_BYTES`]; ob er hineinpasst,
//! prueft der Uebersetzer dieses Crates. Die beiden Puffer der Nutzlast des
//! Journals stellt der Wirt ebenso (`P_PERSIST_BOUND` Byte je Puffer).
//!
//! **Was der Wirt zusagt.** `ops`, Arena, Treiberobjekt, Geraet und Puffer
//! leben so lange wie der Kern; `takt_core_service` laeuft nie zweimal
//! zugleich; der Job-Kontext ruft `takt_core_job_work` mit niedrigerer
//! Prioritaet. Der Trace geht ueber die Funktionen `takt_board_trace*`, die
//! der Rahmen ruft und der Wirt stellt (12.5).

#![cfg_attr(not(test), no_std)]
#![allow(unsafe_code, reason = "die Grenze zu C (12.11); 9.5 fuehrt den Kern in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::as_conversions,
        clippy::fn_to_numeric_cast_any,
        clippy::shadow_unrelated,
        clippy::else_if_without_else,
        unreachable_pub,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        unsafe_op_in_unsafe_fn,
        reason = "Stufe 2 des Regelwerks, mit `unsafe` (plan/cert.md Abschnitt 6)"
    )
)]

use core::ffi::c_void;
use core::marker::PhantomData;
use core::mem::{MaybeUninit, align_of, size_of};

use takt_rt_core::{
    Clock, Journal, Memory, NextRun, Nvm, NvmState, Outputs, Persist, Persisting, Policy, Profile, Runtime, Sink, Tick,
    Tolerance, Unloaded, Volatile, Watchdog,
};

// Die Natives und die Mathematik: Ihre Symbole ruft der erzeugte Code, und
// sie gehen mit ins Archiv.
use takt_native_abi as _;

/// Das Archiv bricht bei einer Panik ab: Eine Panik hier ist ein Fehler in
/// der TCB (9.5), und ein C-Wirt faengt keine. Die Tests linken `std` und
/// bringen dessen Handler mit.
#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe extern "C" {
        fn abort() -> !;
    }
    // SAFETY: `abort` der C-Laufzeit nimmt nichts und kehrt nicht zurueck.
    unsafe { abort() }
}

/// Die Personality, auf die das vorgebaute `core` unter ELF verweist; das
/// Archiv wickelt nie ab (`panic = "abort"`, KON2-013).
#[cfg(all(not(test), not(windows)))]
#[unsafe(no_mangle)]
extern "C" fn rust_eh_personality() {}

/// Die Fassung der Schnittstelle (`TAKT_SDK_ABI`); eine Tabelle anderer
/// Fassung nimmt [`takt_core_init`] nicht an.
pub const ABI: u32 = 1;

/// So viele Byte haelt `takt_core` fuer den Kern (`TAKT_CORE_BYTES`).
pub const CORE_BYTES: usize = 1024;

/// Rueckgaben von [`takt_core_init`] und [`takt_core_init_journaled`].
pub mod status {
    /// Der Kern steht.
    pub const OK: i32 = 0;
    /// Ein Zeiger war null.
    pub const NULL: i32 = -1;
    /// Die Tabelle spricht eine andere Fassung als das Archiv.
    pub const ABI: i32 = -2;
    /// Der Einstieg passt nicht zum Programm: Mit `persist` beginnt es ueber
    /// sein Journal (`takt_core_init_journaled`, 5.9, FB-385), ohne es nicht.
    pub const MEMORY: i32 = -3;
    /// Kein Profil dieses Namens (12.8).
    pub const PROFILE: i32 = -4;
    /// Die Puffer des Journals fassen die Nutzlast des Programms nicht.
    pub const BOUND: i32 = -5;
}

/// Die Einstiege einer Programmbibliothek, wie `P_ops` sie fuellt
/// (`struct takt_ops`). Die Funktionen sind die `P_*`-Einstiege des Rahmens.
#[repr(C)]
pub struct Ops {
    /// [`ABI`] der Bibliothek.
    pub abi: u32,
    /// Das Programm hat `persist` (5.9).
    pub persists: u8,
    /// `system: overrun = alert` (7.3).
    pub overrun_alert: u8,
    /// Die Tickperiode in Nanosekunden.
    pub tick_ns: i64,
    /// Der Logik-Hash, den das Journal mit jedem Eintrag prueft (5.9).
    pub logic_hash: u64,
    /// `min_interval` der `persist`-Variablen in Nanosekunden (5.9).
    pub persist_min_interval_ns: i64,
    /// Die groesste Nutzlast eines Eintrags in Byte (`P_PERSIST_BOUND`).
    pub persist_bound: u32,
    /// `P_init`.
    pub init: unsafe extern "C" fn(arena: *mut c_void, user: *mut c_void),
    /// `P_init_with`: Tick 0 mit den Werten aus dem Journal.
    pub init_with: unsafe extern "C" fn(arena: *mut c_void, user: *mut c_void, bytes: *const c_void, len: i32) -> i32,
    /// `P_tick`: der Schritt von Tick `k` (der Rahmen zaehlt ab 1).
    pub tick: unsafe extern "C" fn(arena: *mut c_void, k: i64),
    /// `P_commit`.
    pub commit: unsafe extern "C" fn(arena: *mut c_void),
    /// `P_idle`.
    pub idle: unsafe extern "C" fn(arena: *mut c_void) -> u8,
    /// `P_deadline`.
    pub deadline: unsafe extern "C" fn(arena: *mut c_void) -> i64,
    /// `P_advance`.
    pub advance: unsafe extern "C" fn(arena: *mut c_void, n: i64),
    /// `P_tune`.
    pub tune: unsafe extern "C" fn(arena: *mut c_void, k: i64, param: u32, value: *const c_void, len: i32) -> i32,
    /// `P_woken`.
    pub woken: unsafe extern "C" fn(arena: *mut c_void, k: i64) -> u8,
    /// `P_wake_sources`.
    pub wake_sources: unsafe extern "C" fn() -> u8,
    /// `P_persist_snapshot`.
    pub persist_snapshot: unsafe extern "C" fn(arena: *mut c_void, out: *mut c_void, cap: i32) -> i32,
    /// `P_persist_restore`.
    pub persist_restore: unsafe extern "C" fn(arena: *mut c_void, bytes: *const c_void, len: i32) -> i32,
    /// `P_dump`.
    pub dump: unsafe extern "C" fn(arena: *mut c_void, all: i32),
    /// `P_pc`.
    pub pc: unsafe extern "C" fn(arena: *mut c_void, all: i32),
    /// `P_next_run`.
    pub next_run: unsafe extern "C" fn(arena: *mut c_void, delay: *mut i64) -> i32,
    /// `P_end`.
    pub end: unsafe extern "C" fn(arena: *mut c_void),
    /// `P_overrun`.
    pub overrun: unsafe extern "C" fn(arena: *mut c_void),
    /// `P_hardware`.
    pub hardware: unsafe extern "C" fn(arena: *mut c_void),
    /// `P_tolerance`.
    pub tolerance: unsafe extern "C" fn(ns: *mut i64, runs: *mut u32),
    /// `P_output_timing`.
    pub output_timing: unsafe extern "C" fn() -> u8,
    /// `P_job_dispatch`.
    pub job_dispatch: unsafe extern "C" fn(arena: *mut c_void) -> i32,
    /// `P_job_work`.
    pub job_work: unsafe extern "C" fn(arena: *mut c_void),
}

/// Das Geraet des Journals im Wirt (`struct takt_nvm`, 5.9, 12.3): zwei
/// Slots gleicher Groesse, Vorgaenge, die beginnen und abgefragt werden,
/// wie `takt_rt_core::Nvm`. `ctx` geht an jeden Rueckruf.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NvmOps {
    /// Der Zeiger des Wirts fuer seine Rueckrufe.
    pub ctx: *mut c_void,
    /// Groesse eines Slots in Byte.
    pub slot_size: u32,
    /// So lange haelt ein Vorgang den Kern hoechstens an, in Nanosekunden;
    /// negativ, wenn er in der Hardware weiterlaeuft (12.3).
    pub blocking_ns: i64,
    /// Beginnt, einen Slot zu loeschen; 0, wenn gerade etwas laeuft.
    pub begin_erase: unsafe extern "C" fn(ctx: *mut c_void, slot: u8) -> u8,
    /// Beginnt, `len` Byte ab `offset` in den Slot zu schreiben.
    pub begin_write: unsafe extern "C" fn(ctx: *mut c_void, slot: u8, offset: u32, bytes: *const u8, len: u32) -> u8,
    /// Fragt den laufenden Vorgang ab (`TAKT_NVM_*`); treibt ihn voran.
    pub poll: unsafe extern "C" fn(ctx: *mut c_void) -> i32,
    /// Liest aus einem Slot; vor dem ersten Tick, darf blockieren.
    pub read: unsafe extern "C" fn(ctx: *mut c_void, slot: u8, offset: u32, into: *mut u8, len: u32) -> u8,
}

/// Was `poll` des Geraets meldet (`TAKT_NVM_*`).
pub mod nvm {
    /// Kein Vorgang laeuft.
    pub const IDLE: i32 = 0;
    /// Ein Vorgang laeuft noch.
    pub const BUSY: i32 = 1;
    /// Der letzte Vorgang ist fertig.
    pub const DONE: i32 = 2;
    /// Der letzte Vorgang ist gescheitert.
    pub const FAILED: i32 = 3;
}

/// Das Geraet hinter seiner Tabelle.
struct HostNvm(NvmOps);

impl Nvm for HostNvm {
    fn slot_size(&self) -> u32 {
        self.0.slot_size
    }

    fn begin_erase(&mut self, slot: u8) -> bool {
        // SAFETY: Der Wirt sagt zu, dass das Geraet so lange lebt wie der Kern.
        unsafe { (self.0.begin_erase)(self.0.ctx, slot) != 0 }
    }

    fn begin_write(&mut self, slot: u8, offset: u32, bytes: &[u8]) -> bool {
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        // SAFETY: wie oben; das Geraet liest `len` Byte ab `bytes`.
        unsafe { (self.0.begin_write)(self.0.ctx, slot, offset, bytes.as_ptr(), len) != 0 }
    }

    fn poll(&mut self) -> NvmState {
        // SAFETY: wie oben.
        match unsafe { (self.0.poll)(self.0.ctx) } {
            nvm::BUSY => NvmState::Busy,
            nvm::DONE => NvmState::Done,
            nvm::FAILED => NvmState::Failed,
            _ => NvmState::Idle,
        }
    }

    fn read(&mut self, slot: u8, offset: u32, into: &mut [u8]) -> bool {
        let len = u32::try_from(into.len()).unwrap_or(u32::MAX);
        // SAFETY: wie oben; das Geraet schreibt hoechstens `len` Byte an `into`.
        unsafe { (self.0.read)(self.0.ctx, slot, offset, into.as_mut_ptr(), len) != 0 }
    }

    fn blocking_ns(&self) -> Option<i64> {
        (self.0.blocking_ns >= 0).then_some(self.0.blocking_ns)
    }
}

/// Was [`takt_core_service`] dem Wirt sagt (`takt_next`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NextStep {
    /// Wann der Kern wieder gerufen werden will, in Nanosekunden.
    pub deadline_ns: i64,
    /// Bei `TAKT_NEXT_AFTER` die Dauer bis zum naechsten Lauf (12.7).
    pub next_run_delay_ns: i64,
    /// 0, solange der Lauf geht; sonst das Ende als `TAKT_NEXT_*` (12.7).
    pub ended: i32,
    /// Ein Job wartet: Der Job-Kontext soll [`takt_core_job_work`] rufen.
    pub jobs: u8,
}

impl From<takt_rt_core::Next> for NextStep {
    fn from(next: takt_rt_core::Next) -> NextStep {
        let delay = match next.ended {
            Some(NextRun::After(ns)) => ns,
            _ => 0,
        };
        NextStep {
            deadline_ns: next.deadline,
            next_run_delay_ns: delay,
            ended: next.ended.map_or(0, NextRun::code),
            jobs: u8::from(next.jobs),
        }
    }
}

/// Das Programm hinter seiner Tabelle, wie die Huelle `P.rs` es einem
/// Rust-Wirt gibt (`takt_frame::embed`); `M` sagt, ob es ein Journal hat.
struct CProgram<M> {
    ops: *const Ops,
    arena: *mut c_void,
    user: *mut c_void,
    initialized: bool,
    memory: PhantomData<M>,
}

impl<M> CProgram<M> {
    fn new(ops: *const Ops, arena: *mut c_void, user: *mut c_void) -> CProgram<M> {
        CProgram { ops, arena, user, initialized: false, memory: PhantomData }
    }

    fn ops(&self) -> &Ops {
        // SAFETY: Der Wirt sagt zu, dass die Tabelle so lange lebt wie der Kern.
        unsafe { &*self.ops }
    }

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    fn ensure_init(&mut self) {
        if !self.initialized {
            // SAFETY: die Arena des Programms und das Treiberobjekt des Wirts.
            unsafe { (self.ops().init)(self.arena, self.user) };
            self.initialized = true;
        }
    }
}

/// Der Rahmen zaehlt Ticks ab 1 (12.11).
fn frame_tick(k: u64) -> i64 {
    i64::try_from(k).unwrap_or(i64::MAX).saturating_add(1)
}

impl<M: Memory> takt_rt_core::Program for CProgram<M> {
    type Memory = M;

    fn tick(&mut self, k: u64, _now: i64) {
        self.ensure_init();
        // SAFETY: die Arena aus `takt_core_init`; der Rahmen zaehlt ab 1.
        unsafe { (self.ops().tick)(self.arena, frame_tick(k)) }
    }

    fn raise_overrun(&mut self) {
        self.ensure_init();
        // SAFETY: wie oben.
        unsafe { (self.ops().overrun)(self.arena) }
    }

    fn raise_hardware(&mut self) {
        self.ensure_init();
        // SAFETY: wie oben.
        unsafe { (self.ops().hardware)(self.arena) }
    }

    fn tick_tolerance(&self) -> Option<Tolerance> {
        let (mut ns, mut runs) = (0i64, 0u32);
        // SAFETY: schreibt zwei Zahlen an die uebergebenen Stellen.
        unsafe { (self.ops().tolerance)(&mut ns, &mut runs) };
        Some(Tolerance { ns, runs })
    }

    fn commit_at_boundary(&self) -> bool {
        // SAFETY: liest eine Konstante der Bibliothek.
        unsafe { (self.ops().output_timing)() == 1 }
    }

    fn sleep_allowed(&self) -> bool {
        // SAFETY: liest die Arena nach `init`.
        self.initialized && unsafe { (self.ops().idle)(self.arena) != 0 }
    }

    fn next_deadline(&self) -> Option<i64> {
        if !self.initialized {
            return None;
        }
        // SAFETY: liest die Arena nach `init`.
        let ns = unsafe { (self.ops().deadline)(self.arena) };
        (ns >= 0).then_some(ns)
    }

    fn advance(&mut self, ticks: u64) {
        self.ensure_init();
        // SAFETY: die Arena.
        unsafe { (self.ops().advance)(self.arena, i64::try_from(ticks).unwrap_or(i64::MAX)) }
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        self.ensure_init();
        let cap = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: Der Rahmen schreibt hoechstens `cap` Byte an `out`.
        let n = unsafe { (self.ops().persist_snapshot)(self.arena, out.as_mut_ptr().cast(), cap) };
        usize::try_from(n).unwrap_or(0)
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        let n = if self.initialized {
            // SAFETY: Der Rahmen liest `len` Byte ab `bytes` und ersetzt nur
            // die Werte.
            unsafe { (self.ops().persist_restore)(self.arena, bytes.as_ptr().cast(), len) }
        } else {
            self.initialized = true;
            // SAFETY: Der Rahmen liest `len` Byte ab `bytes` und beginnt damit.
            unsafe { (self.ops().init_with)(self.arena, self.user, bytes.as_ptr().cast(), len) }
        };
        usize::try_from(n).unwrap_or(0)
    }

    fn tune(&mut self, k: u64, param: u32, value: &[u8]) {
        self.ensure_init();
        let len = i32::try_from(value.len()).unwrap_or(i32::MAX);
        // SAFETY: Der Rahmen liest `len` Byte ab `value` (8.4).
        let _ = unsafe { (self.ops().tune)(self.arena, frame_tick(k), param, value.as_ptr().cast(), len) };
    }

    fn woken(&mut self, k: u64) -> bool {
        if !self.initialized {
            return false;
        }
        // SAFETY: Der Rahmen tastet die Treiber ab wie der Schritt von Tick `k + 1`.
        unsafe { (self.ops().woken)(self.arena, frame_tick(k)) != 0 }
    }

    fn wake_sources(&self) -> bool {
        // SAFETY: liest eine Konstante der Bibliothek.
        unsafe { (self.ops().wake_sources)() != 0 }
    }

    fn next_run(&self) -> Option<NextRun> {
        if !self.initialized {
            return None;
        }
        let mut delay = 0i64;
        // SAFETY: liest die Arena und schreibt `delay`.
        let code = unsafe { (self.ops().next_run)(self.arena, &mut delay) };
        NextRun::from_code(code, delay)
    }

    fn commit(&mut self) {
        self.ensure_init();
        // SAFETY: gibt den Latch der Arena an die Treiber.
        unsafe { (self.ops().commit)(self.arena) }
    }

    fn trace(&mut self, outputs: Outputs) {
        self.ensure_init();
        let all = match outputs {
            Outputs::None => return,
            Outputs::Changed => 0,
            Outputs::All => 1,
        };
        // SAFETY: liest die Arena und schreibt ueber die Leitung des Wirts.
        unsafe { (self.ops().dump)(self.arena, all) };
        // SAFETY: ebenso, die Zeile der Programmzaehler.
        unsafe { (self.ops().pc)(self.arena, all) };
    }

    fn end(&mut self) {
        self.ensure_init();
        // SAFETY: schreibt die Zeile `end` und die `safe`-Werte in den Latch.
        unsafe { (self.ops().end)(self.arena) }
    }

    fn dispatch_job(&mut self) -> bool {
        self.ensure_init();
        // SAFETY: die Arena.
        unsafe { (self.ops().job_dispatch)(self.arena) != 0 }
    }
}

/// Die Zeit des Wirts: der Augenblick, den er `takt_core_service` nennt.
struct HostClock(i64);

impl Clock for HostClock {
    fn now(&self) -> i64 {
        self.0
    }

    /// Der Kern ohne Warten wartet nie; gerufen, stellt sie die Uhr nur vor.
    fn wait_until(&mut self, deadline: i64) {
        self.0 = self.0.max(deadline);
    }
}

/// Den Watchdog fuettert der Wirt (12.11, Wem was gehoert).
struct HostWatchdog;

impl Watchdog for HostWatchdog {
    fn kick(&mut self) {}
}

/// Der Trace des Wirts: der Anfangszustand ganz, danach je Tick die
/// Aenderungen (9.3), ueber die Leitung `takt_board_trace*`.
struct HostSink;

impl Sink for HostSink {
    fn outputs(&mut self, tick: Option<&Tick>) -> Outputs {
        if tick.is_some() { Outputs::Changed } else { Outputs::All }
    }

    fn record(&mut self, _tick: &Tick) {}
}

/// Der Kern eines Programms ohne und mit Journal (5.9): Welches er hat,
/// sagt die Tabelle erst zur Laufzeit.
enum Engine {
    Volatile(Runtime<CProgram<Volatile>, HostClock, HostWatchdog, HostSink>),
    Persisting(Runtime<CProgram<Persisting>, HostClock, HostWatchdog, HostSink, Persist<'static, HostNvm>>),
}

impl Engine {
    fn service(&mut self, now: i64) -> takt_rt_core::Next {
        match self {
            Engine::Volatile(rt) => {
                rt.clock.0 = now;
                rt.service()
            }
            Engine::Persisting(rt) => {
                rt.clock.0 = now;
                rt.service()
            }
        }
    }

    fn finish(&mut self) {
        match self {
            Engine::Volatile(rt) => rt.finish(),
            Engine::Persisting(rt) => rt.finish(),
        };
    }
}

/// Was im Speicher des Wirts liegt. Tabelle und Arena stehen vor dem Kern:
/// Der Job-Kontext liest nur sie, waehrend der Schrittkontext den Kern haelt.
#[repr(C)]
struct Parts {
    ops: *const Ops,
    arena: *mut c_void,
    engine: Engine,
}

const _: () = assert!(size_of::<Parts>() <= CORE_BYTES && align_of::<Parts>() <= 8, "`CORE_BYTES` zu klein");

/// Der Speicher des Kerns im Wirt (`takt_core`): [`CORE_BYTES`] Byte, auf
/// acht ausgerichtet.
#[repr(C, align(8))]
pub struct Core {
    bytes: [MaybeUninit<u8>; CORE_BYTES],
}

/// Prueft, was beide Einstiege brauchen, und nennt Profil und Politik.
///
/// # Safety
///
/// `ops` ist null oder zeigt auf eine gueltige Tabelle.
unsafe fn prepare(
    core: *mut Core,
    ops: *const Ops,
    arena: *mut c_void,
    profile: i32,
    persisting: bool,
) -> Result<(Profile, Policy), i32> {
    if core.is_null() || ops.is_null() || arena.is_null() {
        return Err(status::NULL);
    }
    // SAFETY: nicht null, und der Wirt sagt zu, dass sie gilt.
    let o = unsafe { &*ops };
    if o.abi != ABI {
        return Err(status::ABI);
    }
    if (o.persists != 0) != persisting {
        return Err(status::MEMORY);
    }
    let profile = match profile {
        0 => Profile::BAREMETAL,
        1 => Profile::SHARED,
        2 => Profile::LINUX_RT,
        _ => return Err(status::PROFILE),
    };
    Ok((profile, if o.overrun_alert != 0 { Policy::Alert } else { Policy::Fault }))
}

/// Baut den Kern eines Programms ohne `persist` in `core`: `ops` sind seine
/// Einstiege (`P_ops`), `arena` seine Arena, `user` das Treiberobjekt,
/// `profile` das Laufzeitprofil (12.8: 0 `baremetal`, 1 `shared`, 2
/// `linux_rt`). Das Programm steht danach in Tick 0 (9.4).
///
/// # Safety
///
/// `core`, `ops` und `arena` zeigen auf gueltigen Speicher, der so lange lebt
/// wie der Kern; `arena` ist die Arena des Programms von `ops`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_init(
    core: *mut Core,
    ops: *const Ops,
    arena: *mut c_void,
    user: *mut c_void,
    profile: i32,
) -> i32 {
    // SAFETY: siehe oben.
    let (profile, policy) = match unsafe { prepare(core, ops, arena, profile, false) } {
        Ok(p) => p,
        Err(code) => return code,
    };
    // SAFETY: von `prepare` geprueft.
    let tick_ns = unsafe { (*ops).tick_ns };
    let mut program = CProgram::new(ops, arena, user);
    program.ensure_init();
    let rt = Runtime::new(program, HostClock(0), HostWatchdog, HostSink, profile, tick_ns, policy);
    // SAFETY: `core` fasst `Parts` (Pruefung oben) und ist ausgerichtet.
    unsafe { core.cast::<Parts>().write(Parts { ops, arena, engine: Engine::Volatile(rt) }) };
    status::OK
}

/// Wie [`takt_core_init`], fuer ein Programm mit `persist` (5.9): Es beginnt
/// ueber sein Journal auf dem Geraet `nvm`, mit den beiden Puffern `current`
/// und `stored` von je `bound` Byte (`P_PERSIST_BOUND`). Was das Journal
/// enthaelt, steht danach in der Arena; ohne gueltigen Eintrag beginnt das
/// Programm bei den Vorgaben.
///
/// # Safety
///
/// Wie [`takt_core_init`]; `nvm` zeigt auf eine gueltige Tabelle, deren
/// Rueckrufe so lange gelten wie der Kern, und `current` und `stored` auf je
/// `bound` beschreibbare Byte, die nur der Kern benutzt.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments, reason = "die Teile des Journals; jedes stellt der Wirt (12.11)")]
pub unsafe extern "C" fn takt_core_init_journaled(
    core: *mut Core,
    ops: *const Ops,
    arena: *mut c_void,
    user: *mut c_void,
    profile: i32,
    nvm: *const NvmOps,
    current: *mut u8,
    stored: *mut u8,
    bound: u32,
) -> i32 {
    // SAFETY: siehe oben.
    let (profile, policy) = match unsafe { prepare(core, ops, arena, profile, true) } {
        Ok(p) => p,
        Err(code) => return code,
    };
    if nvm.is_null() || current.is_null() || stored.is_null() {
        return status::NULL;
    }
    // SAFETY: `prepare` hat die Tabelle geprueft; sie lebt so lange wie der Kern.
    let o = unsafe { &*ops };
    // SAFETY: oben auf null geprueft; der Wirt stellt eine gueltige Tabelle.
    let device = unsafe { *nvm };
    let Ok(len) = usize::try_from(bound) else { return status::BOUND };
    if bound < o.persist_bound {
        return status::BOUND;
    }
    // SAFETY: Der Wirt stellt `bound` Byte an `current`, die nur der Kern
    // benutzt und die so lange leben wie er.
    let current = unsafe { core::slice::from_raw_parts_mut(current, len) };
    // SAFETY: ebenso `stored`, ein eigener Puffer neben `current`.
    let stored = unsafe { core::slice::from_raw_parts_mut(stored, len) };
    let journal = Journal::new(HostNvm(device), o.logic_hash, o.persist_min_interval_ns);
    let mut program = CProgram::new(ops, arena, user);
    let (persist, _, _) = Unloaded::new(journal, current, stored).load(&mut program);
    program.ensure_init();
    let rt = Runtime::journaled(program, persist, HostClock(0), HostWatchdog, HostSink, profile, o.tick_ns, policy);
    // SAFETY: `core` fasst `Parts` (Pruefung oben) und ist ausgerichtet.
    unsafe { core.cast::<Parts>().write(Parts { ops, arena, engine: Engine::Persisting(rt) }) };
    status::OK
}

/// Rechnet alle Tickgrenzen bis `now_ns` (12.1) und sagt, wann der Kern
/// wieder gerufen werden will, ob Jobs warten und ob der Lauf endete.
/// Nach jedem Auftrag des Job-Kontexts ruft der Schrittkontext ihn erneut:
/// Vor der Frist rechnet er keine Grenze und verteilt nur (12.11).
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`]; nie zwei Aufrufe zugleich.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_service(core: *mut Core, now_ns: i64) -> NextStep {
    // SAFETY: nur der Kern, nicht Tabelle und Arena, die der Job-Kontext liest.
    let engine = unsafe { &mut (*core.cast::<Parts>()).engine };
    engine.service(now_ns).into()
}

/// Beendet den Lauf an einer Grenze, die der Wirt setzt (12.7): Das Journal
/// schreibt synchron, und ein Schlaf ueber das Ende hinaus gibt die Zeile
/// des Ticks davor frei (9.9). Hat das Programm den Lauf selbst beendet, ist
/// das schon geschehen.
///
/// # Safety
///
/// Wie [`takt_core_service`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_finish(core: *mut Core) {
    // SAFETY: wie in `takt_core_service`.
    let engine = unsafe { &mut (*core.cast::<Parts>()).engine };
    engine.finish();
}

/// Rechnet die Jobs, die der Kern verteilt hat (4.5); unterbrechbar durch
/// [`takt_core_service`].
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`]; nur der Job-Kontext ruft.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_job_work(core: *mut Core) {
    // Feld fuer Feld ueber den Zeiger: Eine Referenz auf den ganzen Kern
    // ueberschnitte sich mit dem `&mut` des unterbrochenen `service`.
    let parts = core.cast::<Parts>();
    // SAFETY: die Tabelle, gesetzt in `takt_core_init`.
    let ops = unsafe { (*parts).ops };
    // SAFETY: die Arena, gesetzt in `takt_core_init`.
    let arena = unsafe { (*parts).arena };
    // SAFETY: Die Tabelle lebt so lange wie der Kern.
    let job_work = unsafe { (*ops).job_work };
    // SAFETY: Der Job-Kontext rechnet seine Auftraege in der Arena; den Kern
    // beruehrt er nicht.
    unsafe { job_work(arena) };
}

/// Der Kopf `include/takt_sdk.h` gegen das Archiv (12.11): Die Felder der
/// Tabellen und des Ergebnisses stehen in der Reihenfolge, in der der Kern
/// sie liest, und jede Zahl des Kopfs ist die des Archivs. Mit `repr(C)`
/// legt dieselbe Reihenfolge dieselben Versaetze fest; dass C sie so liest,
/// zeigt der Lauf des C-Wirts (`examples/c-host`).
#[cfg(test)]
mod tests {
    use std::mem::offset_of;
    use std::string::String;
    use std::vec::Vec;

    use super::{ABI, CORE_BYTES, NextStep, NvmOps, Ops, nvm, status};

    const HEADER: &str = include_str!("../include/takt_sdk.h");

    /// Die Felder eines `struct`- oder `typedef struct`-Blocks des Kopfs.
    fn fields(block: &str) -> Vec<String> {
        let start = HEADER.find(block).expect("Block im Kopf");
        let body = &HEADER[start..];
        let body = &body[body.find('{').expect("{") + 1..body.find('}').expect("}")];
        body.lines()
            .map(|l| l.split("/*").next().unwrap_or("").trim())
            .filter(|l| l.ends_with(';'))
            .map(|l| {
                let name = match l.find("(*") {
                    Some(at) => &l[at + 2..l[at..].find(')').map_or(l.len(), |e| at + e)],
                    None => l.trim_end_matches(';').rsplit([' ', '*']).next().unwrap_or(""),
                };
                String::from(name.split('[').next().unwrap_or(name))
            })
            .collect()
    }

    /// Die Felder in der Reihenfolge ihrer Versaetze im Archiv.
    fn in_order(mut fields: Vec<(&'static str, usize)>) -> Vec<String> {
        fields.sort_by_key(|(_, at)| *at);
        fields.into_iter().map(|(n, _)| String::from(n)).collect()
    }

    /// Der Wert eines `#define` des Kopfs ohne Klammern und `u`.
    fn define(name: &str) -> i64 {
        let line = HEADER.lines().find(|l| l.starts_with(&format!("#define {name} "))).expect(name);
        let value = line[format!("#define {name} ").len()..].split("/*").next().unwrap_or("").trim();
        value.trim_matches(|c| c == '(' || c == ')').trim_end_matches('u').parse().expect(name)
    }

    #[test]
    fn the_header_lays_out_what_the_archive_reads() {
        let ops = in_order(vec![
            ("abi", offset_of!(Ops, abi)),
            ("persists", offset_of!(Ops, persists)),
            ("overrun_alert", offset_of!(Ops, overrun_alert)),
            ("tick_ns", offset_of!(Ops, tick_ns)),
            ("logic_hash", offset_of!(Ops, logic_hash)),
            ("persist_min_interval_ns", offset_of!(Ops, persist_min_interval_ns)),
            ("persist_bound", offset_of!(Ops, persist_bound)),
            ("init", offset_of!(Ops, init)),
            ("init_with", offset_of!(Ops, init_with)),
            ("tick", offset_of!(Ops, tick)),
            ("commit", offset_of!(Ops, commit)),
            ("idle", offset_of!(Ops, idle)),
            ("deadline", offset_of!(Ops, deadline)),
            ("advance", offset_of!(Ops, advance)),
            ("tune", offset_of!(Ops, tune)),
            ("woken", offset_of!(Ops, woken)),
            ("wake_sources", offset_of!(Ops, wake_sources)),
            ("persist_snapshot", offset_of!(Ops, persist_snapshot)),
            ("persist_restore", offset_of!(Ops, persist_restore)),
            ("dump", offset_of!(Ops, dump)),
            ("pc", offset_of!(Ops, pc)),
            ("next_run", offset_of!(Ops, next_run)),
            ("end", offset_of!(Ops, end)),
            ("overrun", offset_of!(Ops, overrun)),
            ("hardware", offset_of!(Ops, hardware)),
            ("tolerance", offset_of!(Ops, tolerance)),
            ("output_timing", offset_of!(Ops, output_timing)),
            ("job_dispatch", offset_of!(Ops, job_dispatch)),
            ("job_work", offset_of!(Ops, job_work)),
        ]);
        assert_eq!(fields("struct takt_ops {"), ops);
        let device = in_order(vec![
            ("ctx", offset_of!(NvmOps, ctx)),
            ("slot_size", offset_of!(NvmOps, slot_size)),
            ("blocking_ns", offset_of!(NvmOps, blocking_ns)),
            ("begin_erase", offset_of!(NvmOps, begin_erase)),
            ("begin_write", offset_of!(NvmOps, begin_write)),
            ("poll", offset_of!(NvmOps, poll)),
            ("read", offset_of!(NvmOps, read)),
        ]);
        assert_eq!(fields("struct takt_nvm {"), device);
        let next = in_order(vec![
            ("deadline_ns", offset_of!(NextStep, deadline_ns)),
            ("next_run_delay_ns", offset_of!(NextStep, next_run_delay_ns)),
            ("ended", offset_of!(NextStep, ended)),
            ("jobs", offset_of!(NextStep, jobs)),
        ]);
        assert_eq!(fields("typedef struct takt_next {"), next);
        assert_eq!(define("TAKT_SDK_ABI"), i64::from(ABI));
        assert_eq!(define("TAKT_CORE_BYTES"), CORE_BYTES as i64);
        for (name, value) in [
            ("TAKT_OK", status::OK),
            ("TAKT_ERR_NULL", status::NULL),
            ("TAKT_ERR_ABI", status::ABI),
            ("TAKT_ERR_MEMORY", status::MEMORY),
            ("TAKT_ERR_PROFILE", status::PROFILE),
            ("TAKT_ERR_BOUND", status::BOUND),
            ("TAKT_NVM_IDLE", nvm::IDLE),
            ("TAKT_NVM_BUSY", nvm::BUSY),
            ("TAKT_NVM_DONE", nvm::DONE),
            ("TAKT_NVM_FAILED", nvm::FAILED),
        ] {
            assert_eq!(define(name), i64::from(value), "{name}");
        }
    }
}
