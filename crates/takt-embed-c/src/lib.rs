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
//! Prioritaet.
//!
//! **Die Leitung.** Der Rahmen schreibt den Trace ueber `takt_board_trace*`
//! (12.5); diese Funktionen stellt das Archiv, mit derselben Telemetrie wie
//! die Bring-ups (`takt_rt_baremetal`): ein Ring vor der Leitung des Wirts,
//! je Tick die Zeitzeile, die Marke `takt trace` und am Ende die Bilanz
//! ([`takt_core_report`]). Der Wirt bindet nur seine Leitung
//! ([`takt_line_init`]). So rechnet kein C-Wirt Fliesskommazahlen selbst in
//! Dezimalziffern um (4.2), und jeder C-Port schreibt, was die Portpruefung
//! liest.

#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::todo,
        clippy::unimplemented,
        clippy::dbg_macro,
        clippy::exit,
        deprecated,
        unreachable_code,
        unused_must_use,
        unused_assignments,
        unused_mut,
        unfulfilled_lint_expectations,
        reason = "Stufe 1 des Regelwerks (plan/cert.md Abschnitt 6)"
    )
)]
#![cfg_attr(not(test), no_std)]
#![allow(unsafe_code, reason = "die Grenze zu C (12.11); 9.5 fuehrt den Kern in der TCB")]
#![cfg_attr(
    not(test),
    deny(
        clippy::expect_used,
        clippy::panic,
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

use core::cell::UnsafeCell;
use core::ffi::{c_char, c_void};
use core::marker::PhantomData;
use core::mem::{MaybeUninit, align_of, size_of};

use takt_rt_baremetal::{Cadence, JournalStats, Port, Stacks, Telemetry, Trace};
use takt_rt_core::port_test::{self, ForeignLoad};
use takt_rt_core::{
    Clock, Journal, Memory, NextRun, Nvm, NvmState, Outputs, Persist, Persisting, Policy, Profile, Runtime, Store,
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
    /// Ein Puffer ist zu klein: Die des Journals fassen die Nutzlast des
    /// Programms nicht, oder der Ring der Leitung ist leer.
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

/// Die Leitung des Wirts (`struct takt_line`, 12.2, 12.5): `try_write` nimmt
/// ein Byte, wenn Platz ist, und wartet nie; `flush` schickt ab, was sie
/// gesammelt hat. `ctx` geht an jeden Rueckruf.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LineOps {
    /// Der Zeiger des Wirts fuer seine Rueckrufe.
    pub ctx: *mut c_void,
    /// Nimmt ein Byte an; 0, wenn kein Platz ist.
    pub try_write: unsafe extern "C" fn(ctx: *mut c_void, byte: u8) -> u8,
    /// Schickt ab, was die Leitung gesammelt hat; null, wenn sie nichts sammelt.
    pub flush: Option<unsafe extern "C" fn(ctx: *mut c_void)>,
}

/// Die Leitung hinter ihrer Tabelle.
struct HostLine(LineOps);

impl Port for HostLine {
    fn try_write(&mut self, b: u8) -> bool {
        // SAFETY: Der Wirt sagt zu, dass die Leitung so lange lebt wie der Prozess.
        unsafe { (self.0.try_write)(self.0.ctx, b) != 0 }
    }

    fn flush(&mut self) {
        if let Some(flush) = self.0.flush {
            // SAFETY: wie oben.
            unsafe { flush(self.0.ctx) }
        }
    }
}

/// Die Leitung mit ihrem Ring, eine fuer alle Kerne des Wirts: Der Rahmen
/// ruft `takt_board_trace*` ohne Kern (12.5).
struct Shared(UnsafeCell<Option<Telemetry<HostLine>>>);

// SAFETY: Der Wirt sagt zu, dass nie zwei Kontexte zugleich auf die Leitung
// schreiben (`takt_sdk.h`): der Schrittkontext in `takt_core_service` und
// `takt_core_finish`, der Wirt in `takt_line_init`, `takt_line_mark` und
// `takt_core_report`, waehrend kein Schritt laeuft.
unsafe impl Sync for Shared {}

static LINE: Shared = Shared(UnsafeCell::new(None));

/// Die Leitung, wenn der Wirt eine gebunden hat.
fn line() -> Option<&'static mut Telemetry<HostLine>> {
    // SAFETY: siehe `Shared`; jeder Griff endet, bevor der naechste beginnt.
    unsafe { (*LINE.0.get()).as_mut() }
}

/// Bindet die Leitung des Wirts (12.5), eine fuer alle Kerne: Ueber sie gehen
/// der Trace des Rahmens, je Tick die Zeitzeile, die Marke und die Bilanz.
/// `ring` haelt `len` Byte zwischen Schritt und Leitung (12.2): Ist er voll
/// und nimmt die Leitung nichts, wird verworfen und gezaehlt.
///
/// # Safety
///
/// `line_ops` zeigt auf eine gueltige Tabelle, deren Rueckrufe so lange
/// gelten wie der Prozess, `ring` auf `len` beschreibbare Byte, die nur das
/// Archiv benutzt; kein Schritt laeuft.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_line_init(line_ops: *const LineOps, ring: *mut u8, len: u32) -> i32 {
    if line_ops.is_null() || ring.is_null() {
        return status::NULL;
    }
    let Ok(len) = usize::try_from(len) else { return status::BOUND };
    if len == 0 {
        return status::BOUND;
    }
    // SAFETY: nicht null; der Wirt stellt eine gueltige Tabelle.
    let ops = unsafe { *line_ops };
    // SAFETY: `len` Byte an `ring`, die nur das Archiv benutzt und die so
    // lange leben wie der Prozess.
    let ring = unsafe { core::slice::from_raw_parts_mut(ring, len) };
    // SAFETY: siehe `Shared`; kein Schritt laeuft.
    unsafe { *LINE.0.get() = Some(Telemetry::new(HostLine(ops), ring)) };
    status::OK
}

/// Schreibt die Marke `takt trace`, ab der die Bilanz zaehlt, was gesendet
/// wurde (FB-304); davor darf der Wirt einen Kopf schreiben.
///
/// # Safety
///
/// Kein Schritt laeuft.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_line_mark() {
    if let Some(t) = line() {
        t.mark();
    }
}

/// Vom Rahmen gerufen: ein Text im Trace (12.5).
///
/// # Safety
///
/// `text` ist null oder nullterminiert; der Rahmen uebergibt nur Literale
/// und Namen aus seinen Tabellen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_board_trace(text: *const c_char) {
    let Some(t) = line() else { return };
    if text.is_null() {
        return;
    }
    // SAFETY: siehe oben.
    let text = unsafe { core::ffi::CStr::from_ptr(text) };
    for &b in text.to_bytes() {
        t.write_byte(b);
    }
}

/// Vom Rahmen gerufen: eine Zahl im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_i64(value: i64) {
    if let Some(t) = line() {
        t.write_i64(value);
        t.write_byte(b' ');
    }
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen im Trace.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_u64(value: u64) {
    if let Some(t) = line() {
        t.write_u64(value);
        t.write_byte(b' ');
    }
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige
/// Ziffernfolge (4.2), wie in den Bring-ups.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_f64(value: f64) {
    use core::fmt::Write as _;
    if let Some(t) = line() {
        let _ = write!(t, "{value:?} ");
    }
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_board_trace_hex8(value: u8) {
    if let Some(t) = line() {
        t.write_hex8(value);
    }
}

/// Die fremde Last der Portpruefung (`takt_foreign`, 12.11, 13.8):
/// gerechnete Runden und die falschen darunter.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Foreign {
    /// Gerechnete Runden.
    pub rounds: u64,
    /// Runden mit falschem Ergebnis.
    pub errors: u64,
}

/// Eine Runde der fremden Last (`takt_rt_core::port_test`): Der Wirt ruft
/// sie in seiner fremden Aufgabe, so oft er kann. Rechnet eine falsch, hat
/// der Port die FPU-Register des unterbrochenen Codes nicht gesichert.
///
/// # Safety
///
/// `foreign` ist null oder zeigt auf Zaehler, die nur dieser Kontext schreibt.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_foreign_round(foreign: *mut Foreign) {
    // SAFETY: siehe oben.
    let Some(f) = (unsafe { foreign.as_mut() }) else { return };
    let mut load = ForeignLoad::new();
    load.round();
    f.rounds = f.rounds.wrapping_add(load.rounds());
    f.errors = f.errors.wrapping_add(load.errors());
}

/// Die Stacks eines Laufs, wie der Port sie misst (`struct takt_stacks`,
/// 12.3): die Tiefe unter Last und die Schranke des Programms
/// (`P_TICK_STACK_BYTES`, `P_JOB_STACK_BYTES`); 0 heisst nicht gemessen.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StackDepths {
    /// Die Tiefe des Schritt-Stacks in Byte.
    pub tick: u32,
    /// `P_TICK_STACK_BYTES`.
    pub tick_bound: u32,
    /// Der Anteil des Programms an der Tiefe.
    pub tick_program: u32,
    /// Die Tiefe des Job-Stacks in Byte.
    pub job: u32,
    /// `P_JOB_STACK_BYTES`.
    pub job_bound: u32,
    /// Die Jobs rechnen auf dem Schritt-Stack (FB-459).
    pub job_shared: u8,
}

impl From<StackDepths> for Stacks {
    fn from(s: StackDepths) -> Stacks {
        let measured = |n: u32| (n != 0).then_some(n);
        Stacks {
            tick: measured(s.tick),
            tick_bound: measured(s.tick_bound),
            tick_program: measured(s.tick_program),
            job: measured(s.job),
            job_bound: measured(s.job_bound),
            job_shared: s.job_shared != 0,
        }
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

/// Der Trace des Wirts (12.5): der Anfangszustand ganz, danach je Tick die
/// Aenderungen (9.3) und die Zeitzeile, ueber die Leitung aus
/// [`takt_line_init`]; ohne Leitung nichts.
type HostSink = Trace<fn() -> Option<&'static mut Telemetry<HostLine>>, Telemetry<HostLine>>;

fn host_sink(tick_ns: i64) -> HostSink {
    let lead: fn() -> Option<&'static mut Telemetry<HostLine>> = line;
    // Die Grenze des Laufs setzt der Wirt, nicht die Senke.
    Trace::new(Cadence::of(0, 1), tick_ns, lead)
}

/// Was die Bilanz vom Journal nennt (5.9).
fn journal_stats<J: Store>(store: &J) -> JournalStats {
    store.persist().map_or_else(JournalStats::default, |p| JournalStats {
        writes: p.journal().writes(),
        failures: p.journal().failures(),
        ..JournalStats::default()
    })
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

    /// Die Bilanz ueber die Leitung, davor die Zeile der fremden Last.
    fn report(&mut self, stacks: &Stacks, foreign: Option<Foreign>) {
        // Erst der Abschluss, der noch Trace schreiben kann (12.7, 9.9), dann
        // ein Griff auf die Leitung fuer die Bilanz.
        let (stats, overrun, journal) = match self {
            Engine::Volatile(rt) => (takt_rt_baremetal::stats(rt), *rt.overrun(), journal_stats(rt.journal())),
            Engine::Persisting(rt) => (takt_rt_baremetal::stats(rt), *rt.overrun(), journal_stats(rt.journal())),
        };
        let Some(t) = line() else { return };
        if let Some(f) = foreign {
            t.write(port_test::LINE);
            t.write_u64(f.rounds);
            t.write(port_test::ERRORS);
            t.write_u64(f.errors);
            t.newline();
        }
        takt_rt_baremetal::report(t, &overrun, &stats, &journal, stacks);
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
    let rt = Runtime::new(program, HostClock(0), HostWatchdog, host_sink(tick_ns), profile, tick_ns, policy);
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
    let sink = host_sink(o.tick_ns);
    let rt = Runtime::journaled(program, persist, HostClock(0), HostWatchdog, sink, profile, o.tick_ns, policy);
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

/// Beendet den Lauf wie [`takt_core_finish`] und schreibt die Bilanz als
/// letzte Zeile ueber die Leitung (13.8): Schlaf, Ueberlaeufe, Rueckstand,
/// verworfene und gesendete Byte, das Journal und die Stacks `stacks`; davor
/// die Zeile der fremden Last `foreign`, wenn sie nicht null ist, danach
/// `takt end`. Leert den Ring.
///
/// # Safety
///
/// Wie [`takt_core_service`], und kein Schritt laeuft; `stacks` und
/// `foreign` sind null oder gueltig.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_report(core: *mut Core, stacks: *const StackDepths, foreign: *const Foreign) {
    // SAFETY: wie in `takt_core_service`.
    let engine = unsafe { &mut (*core.cast::<Parts>()).engine };
    // SAFETY: null oder gueltig.
    let stacks = unsafe { stacks.as_ref() }.copied().unwrap_or_default();
    // SAFETY: ebenso.
    let foreign = unsafe { foreign.as_ref() }.copied();
    engine.report(&Stacks::from(stacks), foreign);
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

    use super::{ABI, CORE_BYTES, Foreign, LineOps, NextStep, NvmOps, Ops, StackDepths, nvm, status};

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
        let line = in_order(vec![
            ("ctx", offset_of!(LineOps, ctx)),
            ("try_write", offset_of!(LineOps, try_write)),
            ("flush", offset_of!(LineOps, flush)),
        ]);
        assert_eq!(fields("struct takt_line {"), line);
        let stacks = in_order(vec![
            ("tick", offset_of!(StackDepths, tick)),
            ("tick_bound", offset_of!(StackDepths, tick_bound)),
            ("tick_program", offset_of!(StackDepths, tick_program)),
            ("job", offset_of!(StackDepths, job)),
            ("job_bound", offset_of!(StackDepths, job_bound)),
            ("job_shared", offset_of!(StackDepths, job_shared)),
        ]);
        assert_eq!(fields("struct takt_stacks {"), stacks);
        let foreign = in_order(vec![("rounds", offset_of!(Foreign, rounds)), ("errors", offset_of!(Foreign, errors))]);
        assert_eq!(fields("typedef struct takt_foreign {"), foreign);
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
