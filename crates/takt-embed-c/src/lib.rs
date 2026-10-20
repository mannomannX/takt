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
//! **Die Senke.** Der Rahmen schreibt den Trace ueber die Haken
//! `takt_trace_*` (12.5), jeden Aufruf mit der Senke, die sein Kern bei
//! `init` nannte; die Haken stellt das Archiv, mit derselben Telemetrie wie
//! die Bring-ups (`takt_rt_baremetal`): ein Ring vor der Leitung des Wirts,
//! je Tick die Zeitzeile, die Marke `takt trace` und am Ende die Bilanz
//! ([`takt_core_report`]). Der Wirt bindet je Senke seine Leitung
//! ([`takt_sink_init`]) und gibt jedem Kern eine: Zwei Kerne mit zwei
//! Senken schreiben nie ineinander (FB-691). So rechnet kein C-Wirt
//! Fliesskommazahlen selbst in Dezimalziffern um (4.2), und jeder C-Port
//! schreibt, was die Portpruefung liest.
//!
//! **Der Schutz.** Gehoert dem Wirt eine Schutzeinheit, gibt er dem Kern vor
//! dem ersten Tick seine Haken ([`takt_core_protect`]): Der Kern oeffnet
//! den Programmbereich der Arena nur um die Aufrufe, die ihn schreiben, wie
//! die Rust-Formen (`takt_rt_baremetal::Guarded`), und stellt eine gemeldete
//! Verletzung im naechsten Tick als `Runtime(Hardware)` zu (12.3, FB-755).

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

use core::ffi::{c_char, c_void};
use core::marker::PhantomData;
use core::mem::{MaybeUninit, align_of, size_of};
use core::ptr::null;
use core::sync::atomic::{AtomicBool, Ordering};

use takt_rt_baremetal::{Cadence, Guarded, JournalStats, Lead, Port, Protection, Stacks, Telemetry, Trace, Violation};
use takt_rt_core::port_test::{self, ForeignLoad};
use takt_rt_core::protection::Case;
use takt_rt_core::{
    Clock, Journal, Memory, NextRun, Nvm, NvmState, Outputs, Persist, Persisting, Policy, Profile, Runtime, Store,
    Tolerance, Tunables, Unloaded, Volatile, Watchdog,
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
/// Fassung nimmt [`takt_core_init`] nicht an. Fassung 2: Der Wirt nennt
/// seinen Watchdog bei `takt_core_init` (12.11). Fassung 3: Die Tabelle
/// traegt `P_operator_abort`, den der Kern nach [`takt_core_abort`] meldet
/// (5.4). Fassung 4: die Senke je Kern ([`takt_sink_init`],
/// [`takt_core_init`] mit ihr), `init` und `init_with` der Tabelle mit ihr
/// und die Haken `takt_trace_*` mit ihr; die Leitung je Prozess
/// (`takt_line_*`) entfaellt (12.11, FB-691). Fassung 5: der Schutz des
/// Wirts ([`takt_core_protect`], 12.3, FB-755).
pub const ABI: u32 = 5;

/// So viele Byte haelt `takt_core` fuer den Kern (`TAKT_CORE_BYTES`).
pub const CORE_BYTES: usize = 1024;

/// So viele Byte haelt `takt_sink` fuer eine Senke (`TAKT_SINK_BYTES`).
pub const SINK_BYTES: usize = 128;

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
    /// Programms nicht, oder der Ring der Senke ist leer.
    pub const BOUND: i32 = -5;
    /// Keine Senke ([`takt_sink_init`](super::takt_sink_init)): Die Senke
    /// des Trace stellt der Wirt ausdruecklich (12.11), einen Trace, der
    /// still verloren geht, gibt es nicht.
    pub const LINE: i32 = -6;
    /// Der Lauf hat begonnen: Seine Haken gibt der Wirt dem Kern vor dem
    /// ersten Tick ([`takt_core_protect`](super::takt_core_protect)), nicht
    /// mitten im Lauf.
    pub const STARTED: i32 = -7;
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
    /// `P_init`: mit dem Treiberobjekt und der Senke des Wirts.
    pub init: unsafe extern "C" fn(arena: *mut c_void, user: *mut c_void, sink: *mut c_void),
    /// `P_init_with`: Tick 0 mit den Werten aus dem Journal.
    pub init_with: unsafe extern "C" fn(
        arena: *mut c_void,
        user: *mut c_void,
        sink: *mut c_void,
        bytes: *const c_void,
        len: i32,
    ) -> i32,
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
    /// `P_operator_abort`: ein Operator-Abort fuer den naechsten Tick (5.4).
    pub operator_abort: unsafe extern "C" fn(arena: *mut c_void),
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

/// Der Speicher einer Senke im Wirt (`takt_sink`, 12.5, 12.11): eine
/// Leitung mit ihrem Ring. [`SINK_BYTES`] Byte, auf acht ausgerichtet;
/// [`takt_sink_init`] beschreibt sie. Jeder Kern bekommt eine; teilen sich
/// Kerne eine, sagt der Wirt zu, dass nie zwei zugleich schreiben.
#[repr(C, align(8))]
pub struct TraceSink {
    bytes: [MaybeUninit<u8>; SINK_BYTES],
}

const _: () = assert!(
    size_of::<Telemetry<HostLine>>() <= SINK_BYTES && align_of::<Telemetry<HostLine>>() <= 8,
    "`SINK_BYTES` zu klein"
);

/// Die Leitung in einer Senke, ein Griff je Aufruf.
///
/// # Safety
///
/// `sink` ist null oder eine Senke aus [`takt_sink_init`], die noch lebt;
/// solange der Griff lebt, greift kein anderer auf sie zu: Der Wirt sagt zu,
/// dass nie zwei Kontexte zugleich in dieselbe Senke schreiben.
unsafe fn telemetry<'s>(sink: *mut TraceSink) -> Option<&'s mut Telemetry<HostLine>> {
    // SAFETY: siehe oben.
    unsafe { sink.cast::<Telemetry<HostLine>>().as_mut() }
}

/// Bindet die Leitung `line_ops` des Wirts an die Senke `sink` (12.5): Ueber
/// sie gehen der Trace der Programme, deren Kern sie bekommt, je Tick die
/// Zeitzeile, die Marke und die Bilanz. `ring` haelt `len` Byte zwischen
/// Schritt und Leitung (12.2): Ist er voll und nimmt die Leitung nichts, wird
/// verworfen und gezaehlt.
///
/// # Safety
///
/// `sink` zeigt auf [`SINK_BYTES`] beschreibbare, auf acht ausgerichtete Byte,
/// `line_ops` auf eine gueltige Tabelle, `ring` auf `len` beschreibbare Byte;
/// alle leben so lange wie jeder Kern, der die Senke bekommt, und Senke und
/// Ring benutzt nur das Archiv. Kein Schritt laeuft.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_sink_init(
    sink: *mut TraceSink,
    line_ops: *const LineOps,
    ring: *mut u8,
    len: u32,
) -> i32 {
    if sink.is_null() || line_ops.is_null() || ring.is_null() {
        return status::NULL;
    }
    let Ok(len) = usize::try_from(len) else { return status::BOUND };
    if len == 0 {
        return status::BOUND;
    }
    // SAFETY: nicht null; der Wirt stellt eine gueltige Tabelle.
    let ops = unsafe { *line_ops };
    // SAFETY: `len` Byte an `ring`, die nur das Archiv benutzt und die so
    // lange leben wie jeder Kern der Senke.
    let ring = unsafe { core::slice::from_raw_parts_mut(ring, len) };
    // SAFETY: `sink` fasst die Telemetrie (Pruefung oben) und ist ausgerichtet.
    unsafe { sink.cast::<Telemetry<HostLine>>().write(Telemetry::new(HostLine(ops), ring)) };
    status::OK
}

/// Macht die Senke verlustfrei (13.8): Ist ihr Ring voll, wartet der
/// Schreibende, bis die Leitung ihn geleert hat, statt zu verwerfen —
/// hoechstens [`takt_rt_baremetal::DRAIN_ROUNDS`] Anlaeufe lang. Nur fuer
/// Laeufe in logischer Zeit, wie die Bring-ups sie fahren: Im Betrieb
/// verbietet 12.2 das Warten. [`status::LINE`] ohne Senke.
///
/// # Safety
///
/// `sink` ist null oder eine Senke aus [`takt_sink_init`]; kein Schritt laeuft.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_sink_lossless(sink: *mut TraceSink) -> i32 {
    if sink.is_null() {
        return status::LINE;
    }
    let at = sink.cast::<Telemetry<HostLine>>();
    // SAFETY: eine Senke aus `takt_sink_init`; sie wird sofort zurueckgeschrieben.
    let t = unsafe { at.read() };
    // SAFETY: dieselbe Stelle, die Telemetrie verlustfrei.
    unsafe { at.write(t.lossless()) };
    status::OK
}

/// Gibt an die Leitung, was der Ring der Senke haelt und sie ohne Warten
/// nimmt: fuer Zeilen des Wirts, waehrend kein Schritt laeuft, etwa eine
/// Meldung, dass der Kern nicht annahm. Waehrend des Laufs gibt der Kern den
/// Ring nach jedem Tick ab.
///
/// # Safety
///
/// Wie [`takt_sink_lossless`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_sink_flush(sink: *mut TraceSink) {
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink) } {
        t.flush();
    }
}

/// Schreibt die Marke `takt trace`, ab der die Bilanz zaehlt, was gesendet
/// wurde (FB-304); davor darf der Wirt einen Kopf schreiben
/// (`takt_trace_text`).
///
/// # Safety
///
/// Wie [`takt_sink_lossless`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_sink_mark(sink: *mut TraceSink) {
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink) } {
        t.mark();
    }
}

/// Vom Rahmen gerufen, auch vom Wirt fuer seinen Kopf: ein Text in der Senke
/// `sink` (12.5). Ein Haken haengt nichts an; die Trenner schreibt der
/// Rahmen (FB-690).
///
/// # Safety
///
/// `sink` wie bei [`takt_sink_lossless`]; `text` ist null oder
/// nullterminiert. Der Rahmen uebergibt nur Literale und Namen aus seinen
/// Tabellen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_trace_text(sink: *mut c_void, text: *const c_char) {
    // SAFETY: siehe oben.
    let Some(t) = (unsafe { telemetry(sink.cast()) }) else { return };
    if text.is_null() {
        return;
    }
    // SAFETY: nicht null und nullterminiert, siehe oben.
    let text = unsafe { core::ffi::CStr::from_ptr(text) };
    for &b in text.to_bytes() {
        t.write_byte(b);
    }
}

/// Vom Rahmen gerufen: eine Zahl in der Senke.
///
/// # Safety
///
/// Wie [`takt_trace_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_trace_i64(sink: *mut c_void, value: i64) {
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink.cast()) } {
        t.write_i64(value);
    }
}

/// Vom Rahmen gerufen: eine Zahl ohne Vorzeichen in der Senke.
///
/// # Safety
///
/// Wie [`takt_trace_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_trace_u64(sink: *mut c_void, value: u64) {
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink.cast()) } {
        t.write_u64(value);
    }
}

/// Vom Rahmen gerufen: eine Fliesskommazahl als kuerzeste eindeutige
/// Ziffernfolge (4.2), wie in den Bring-ups.
///
/// # Safety
///
/// Wie [`takt_trace_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_trace_f64(sink: *mut c_void, value: f64) {
    use core::fmt::Write as _;
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink.cast()) } {
        let _ = write!(t, "{value:?}");
    }
}

/// Vom Rahmen gerufen: ein Byte eines Ausgabestroms, wie der Interpreter es schreibt.
///
/// # Safety
///
/// Wie [`takt_trace_text`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_trace_hex8(sink: *mut c_void, value: u8) {
    // SAFETY: siehe oben.
    if let Some(t) = unsafe { telemetry(sink.cast()) } {
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
    sink: *mut c_void,
    initialized: bool,
    memory: PhantomData<M>,
}

impl<M> CProgram<M> {
    fn new(ops: *const Ops, arena: *mut c_void, user: *mut c_void, sink: *mut TraceSink) -> CProgram<M> {
        CProgram { ops, arena, user, sink: sink.cast(), initialized: false, memory: PhantomData }
    }

    fn ops(&self) -> &Ops {
        // SAFETY: Der Wirt sagt zu, dass die Tabelle so lange lebt wie der Kern.
        unsafe { &*self.ops }
    }

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    fn ensure_init(&mut self) {
        if !self.initialized {
            // SAFETY: die Arena des Programms, Treiberobjekt und Senke des Wirts.
            unsafe { (self.ops().init)(self.arena, self.user, self.sink) };
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

    fn raise_abort(&mut self) {
        self.ensure_init();
        // SAFETY: wie oben.
        unsafe { (self.ops().operator_abort)(self.arena) }
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
            unsafe { (self.ops().init_with)(self.arena, self.user, self.sink, bytes.as_ptr().cast(), len) }
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
        // SAFETY: liest die Arena und schreibt in die Senke des Kerns.
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

/// Die Zeit des Wirts: der Augenblick, den er `takt_core_service` nennt, und
/// sein Weckereignis aus [`takt_core_wake`], das im Speicher des Kerns liegt.
/// Den Zeiger auf das Flag setzt jeder Aufruf aus dem Zeiger des Wirts neu und
/// loescht ihn danach: Der Kern traegt keinen Zeiger auf sich selbst und darf
/// zwischen zwei Aufrufen woanders liegen.
struct HostClock {
    now: i64,
    wake: *const AtomicBool,
}

impl Clock for HostClock {
    fn now(&self) -> i64 {
        self.now
    }

    /// Der Kern ohne Warten wartet nie; gerufen, stellt sie die Uhr nur vor.
    fn wait_until(&mut self, deadline: i64) {
        self.now = self.now.max(deadline);
    }

    /// Ein Weckereignis des Wirts seit dem letzten Aufruf (9.9).
    fn woken(&mut self) -> bool {
        // SAFETY: null oder das Flag des Kerns, den der laufende Aufruf haelt.
        unsafe { self.wake.as_ref() }.is_some_and(|wake| wake.swap(false, Ordering::SeqCst))
    }
}

/// Die Tunables des Wirts (`struct takt_tunables`, 8.4): `next` nennt je
/// Aufruf eine Aenderung der Grenze `k` in Aufzeichnungsreihenfolge und
/// liefert 1, am Ende 0; `idle` sagt, ob die Quelle ruht, und darf fehlen.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TunablesOps {
    /// Der Zeiger des Wirts fuer seine Rueckrufe.
    pub ctx: *mut c_void,
    /// Die naechste Aenderung der Grenze `k`: Parameter und Wert; 0 am Ende.
    pub next:
        unsafe extern "C" fn(ctx: *mut c_void, k: u64, param: *mut u32, value: *mut *const u8, len: *mut u32) -> i32,
    /// Ruht die Quelle? Null heisst: Das weiss sie nicht.
    pub idle: Option<unsafe extern "C" fn(ctx: *mut c_void) -> u8>,
}

/// Die Tunables hinter ihrer Tabelle.
struct HostTunables(TunablesOps);

impl Tunables for HostTunables {
    fn poll(&mut self, k: u64, apply: &mut dyn FnMut(u32, &[u8])) {
        loop {
            let (mut param, mut value, mut len) = (0u32, core::ptr::null::<u8>(), 0u32);
            // SAFETY: Der Wirt sagt zu, dass der Rueckruf waehrend des Aufrufs gilt.
            let more = unsafe { (self.0.next)(self.0.ctx, k, &mut param, &mut value, &mut len) };
            let Ok(len) = usize::try_from(len) else { break };
            let bytes: &[u8] = match (more, len, value.is_null()) {
                (0, _, _) | (_, 1.., true) => break,
                (_, 0, _) => &[],
                // SAFETY: Der Wirt nennt `len` lesbare Byte an `value`, gueltig bis
                // zum naechsten Aufruf von `next`.
                _ => unsafe { core::slice::from_raw_parts(value, len) },
            };
            apply(param, bytes);
        }
    }

    fn idle(&mut self) -> bool {
        // SAFETY: wie oben.
        self.0.idle.is_some_and(|idle| unsafe { idle(self.0.ctx) } != 0)
    }
}

/// Der Watchdog des Wirts (`struct takt_watchdog`, 12.3, 12.11): Der Kern
/// meldet ihm nach jedem Schritt und an jeder Tickgrenze im Schlaf, dass er
/// lebt. Ein Wirt ohne Watchdog gibt ausdruecklich einen `kick`, der nichts
/// tut.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WatchdogOps {
    /// Der Zeiger des Wirts fuer seinen Rueckruf.
    pub ctx: *mut c_void,
    /// Meldet, dass der Kern lebt.
    pub kick: unsafe extern "C" fn(ctx: *mut c_void),
}

/// Der Watchdog hinter seiner Tabelle (12.11, Wem was gehoert).
struct HostWatchdog(WatchdogOps);

impl Watchdog for HostWatchdog {
    fn kick(&mut self) {
        // SAFETY: Der Wirt sagt zu, dass der Rueckruf so lange gilt wie der Kern.
        unsafe { (self.0.kick)(self.0.ctx) }
    }
}

/// Der Schutz des Wirts (`struct takt_protection`, 12.3, 12.11; FB-755):
/// `open` macht den Programmbereich der Arena beschreibbar, `close` nur
/// lesbar, und `take_violation` nennt die Verletzung seit der letzten Frage
/// mit ihrer Adresse: 0 keine, [`violation::STATE`] im Programmbereich,
/// [`violation::GUARD`] in einem Waechter, sonst anderswo.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProtectionOps {
    /// Der Zeiger des Wirts fuer seine Rueckrufe.
    pub ctx: *mut c_void,
    /// Der Programmbereich wird beschreibbar.
    pub open: unsafe extern "C" fn(ctx: *mut c_void),
    /// Er wird wieder nur lesbar.
    pub close: unsafe extern "C" fn(ctx: *mut c_void),
    /// Die Verletzung seit der letzten Frage und ihre Adresse nach `address`.
    pub take_violation: unsafe extern "C" fn(ctx: *mut c_void, address: *mut u32) -> i32,
}

/// Wo eine Verletzung lag (`TAKT_VIOLATION_*`), wie `take_violation` sie nennt.
pub mod violation {
    /// Keine Verletzung seit der letzten Frage.
    pub const NONE: i32 = 0;
    /// Im Programmbereich der Arena.
    pub const STATE: i32 = 1;
    /// In einem Waechter unter einem Stack.
    pub const GUARD: i32 = 2;
}

/// Der Schutz hinter seiner Tabelle; ohne sie, bis der Wirt sie gibt, tut er
/// nichts und meldet nie.
struct HostProtection(Option<ProtectionOps>);

impl Protection for HostProtection {
    /// Was der Kopf nennt, entscheidet der Wirt zur Laufzeit
    /// ([`takt_core_protected`]); dieser Fall gilt nur mit Haken.
    const CASE: Case = Case::Window;

    fn open(&self) {
        if let Some(p) = self.0 {
            // SAFETY: Der Wirt sagt zu, dass die Rueckrufe so lange gelten wie der Kern.
            unsafe { (p.open)(p.ctx) }
        }
    }

    fn close(&self) {
        if let Some(p) = self.0 {
            // SAFETY: wie oben.
            unsafe { (p.close)(p.ctx) }
        }
    }

    fn take_violation(&self) -> Option<Violation> {
        let p = self.0?;
        let mut address = 0u32;
        // SAFETY: wie oben; der Rueckruf schreibt nur `address`.
        let region = match unsafe { (p.take_violation)(p.ctx, &mut address) } {
            violation::NONE => return None,
            violation::STATE => "Programmzustand",
            violation::GUARD => "Waechter",
            _ => "anderswo",
        };
        Some(Violation { region, address })
    }

    fn reports(&self) -> bool {
        self.0.is_some()
    }
}

/// Das Programm des Kerns: hinter dem Schutz des Wirts.
type Guard<M> = Guarded<CProgram<M>, HostProtection>;

/// Der Griff des Kerns auf seine Senke, je Aufruf.
struct SinkLead(*mut TraceSink);

impl Lead<HostLine> for SinkLead {
    fn lead(&mut self) -> Option<&'static mut Telemetry<HostLine>> {
        // SAFETY: die Senke aus `takt_core_init`; der Wirt sagt zu, dass sie
        // so lange lebt wie der Kern und nie zwei Kontexte zugleich schreiben.
        unsafe { telemetry(self.0) }
    }
}

/// Der Trace des Wirts (12.5): der Anfangszustand ganz, danach je Tick die
/// Aenderungen (9.3) und die Zeitzeile, in die Senke des Kerns.
type HostSink = Trace<SinkLead, Telemetry<HostLine>>;

fn host_sink(tick_ns: i64, sink: *mut TraceSink) -> HostSink {
    // Die Grenze des Laufs setzt der Wirt, nicht die Senke.
    Trace::new(Cadence::of(0, 1), tick_ns, SinkLead(sink))
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
    Volatile(Runtime<Guard<Volatile>, HostClock, HostWatchdog, HostSink>),
    Persisting(Runtime<Guard<Persisting>, HostClock, HostWatchdog, HostSink, Persist<'static, HostNvm>>),
}

impl Engine {
    /// Rechnet bis `now`, mit dem Weckereignis `wake` im Speicher des Kerns;
    /// zuerst meldet der Kern, was der Wirt seit dem letzten Aufruf von aussen
    /// meldete (`outer`: Operator-Abort, Schutzverletzung).
    fn service(
        &mut self,
        now: i64,
        wake: *const AtomicBool,
        outer: Outer,
        tunables: Option<&mut dyn Tunables>,
    ) -> takt_rt_core::Next {
        match self {
            Engine::Volatile(rt) => {
                rt.clock = HostClock { now, wake };
                outer.raise(rt);
                let next = rt.service_with(tunables);
                rt.clock.wake = null();
                next
            }
            Engine::Persisting(rt) => {
                rt.clock = HostClock { now, wake };
                outer.raise(rt);
                let next = rt.service_with(tunables);
                rt.clock.wake = null();
                next
            }
        }
    }

    fn finish(&mut self) {
        match self {
            Engine::Volatile(rt) => rt.finish(),
            Engine::Persisting(rt) => rt.finish(),
        };
    }

    /// Der Schutz des Programms.
    fn protection(&mut self) -> &mut HostProtection {
        match self {
            Engine::Volatile(rt) => &mut rt.program.protection,
            Engine::Persisting(rt) => &mut rt.program.protection,
        }
    }

    /// Hat der Kern die Haken des Wirts?
    fn protected(&self) -> bool {
        match self {
            Engine::Volatile(rt) => rt.program.protection.reports(),
            Engine::Persisting(rt) => rt.program.protection.reports(),
        }
    }

    /// Wie viele Verletzungen der Schutz zustellte, und die letzte.
    fn violations(&self) -> (u32, Option<Violation>) {
        match self {
            Engine::Volatile(rt) => (rt.program.violations, rt.program.last),
            Engine::Persisting(rt) => (rt.program.violations, rt.program.last),
        }
    }

    /// Die Bilanz in die Senke `sink`, davor die Zeile der fremden Last.
    fn report(&mut self, sink: *mut TraceSink, stacks: &Stacks, foreign: Option<Foreign>) {
        // Erst der Abschluss, der noch Trace schreiben kann (12.7, 9.9), dann
        // ein Griff auf die Senke fuer die Bilanz.
        let (stats, overrun, journal) = match self {
            Engine::Volatile(rt) => (takt_rt_baremetal::stats(rt), *rt.overrun(), journal_stats(rt.journal())),
            Engine::Persisting(rt) => (takt_rt_baremetal::stats(rt), *rt.overrun(), journal_stats(rt.journal())),
        };
        let (violations, last) = self.violations();
        // SAFETY: die Senke des Kerns; kein Schritt laeuft.
        let Some(t) = (unsafe { telemetry(sink) }) else { return };
        if let Some(f) = foreign {
            t.write(port_test::LINE);
            t.write_u64(f.rounds);
            t.write(port_test::ERRORS);
            t.write_u64(f.errors);
            t.newline();
        }
        // 12.3: was der Schutz abgewiesen hat, wie die Bring-ups es nennen.
        if let Some(v) = last {
            t.write("takt schutz verletzungen ");
            t.write_u64(u64::from(violations));
            t.write(" region ");
            t.write(v.region);
            t.write(" adresse 0x");
            for shift in [28u32, 24, 20, 16, 12, 8, 4, 0] {
                let nibble = (v.address >> shift) & 0xF;
                t.write_byte(b"0123456789abcdef".get(usize::try_from(nibble).unwrap_or(0)).copied().unwrap_or(b'0'));
            }
            t.newline();
        }
        takt_rt_baremetal::report(t, &overrun, &stats, &journal, stacks);
    }
}

/// Was im Speicher des Wirts liegt. Tabelle, Arena und die Flags der
/// Ereignisse von aussen stehen vor dem Kern: Der Job-Kontext liest nur
/// Tabelle und Arena, [`takt_core_wake`], [`takt_core_abort`] und
/// [`takt_core_hardware_fault`] setzen nur ihr Flag, waehrend der
/// Schrittkontext den Kern haelt.
#[repr(C)]
struct Parts {
    ops: *const Ops,
    arena: *mut c_void,
    sink: *mut TraceSink,
    wake: AtomicBool,
    abort: AtomicBool,
    hardware: AtomicBool,
    /// Der Schrittkontext hat `takt_core_service` gerufen: Der Lauf hat
    /// begonnen, und der Schutz steht fest.
    started: bool,
    engine: Engine,
}

/// Was der Wirt seit dem letzten Aufruf von aussen meldete.
#[derive(Clone, Copy)]
struct Outer {
    /// Ein Operator-Abort (5.4, [`takt_core_abort`]).
    abort: bool,
    /// Eine Schutzverletzung (12.3, [`takt_core_hardware_fault`]).
    hardware: bool,
}

impl Outer {
    /// Meldet dem Kern, was kam; er gibt es vor dem Schritt der naechsten
    /// Grenze an das Programm und beendet einen Schlaf dort.
    fn raise<P, C, W, S, J>(self, rt: &mut Runtime<P, C, W, S, J>)
    where
        P: takt_rt_core::Program,
        C: Clock,
        W: Watchdog,
        S: takt_rt_core::Sink,
        J: Store,
    {
        if self.abort {
            rt.abort();
        }
        if self.hardware {
            rt.hardware_fault();
        }
    }
}

/// Die Uhr eines neuen Kerns; das Weckereignis reicht erst `takt_core_service`.
const fn host_clock() -> HostClock {
    HostClock { now: 0, wake: null() }
}

const _: () = assert!(size_of::<Parts>() <= CORE_BYTES && align_of::<Parts>() <= 8, "`CORE_BYTES` zu klein");

/// Der Speicher des Kerns im Wirt (`takt_core`): [`CORE_BYTES`] Byte, auf
/// acht ausgerichtet.
#[repr(C, align(8))]
pub struct Core {
    bytes: [MaybeUninit<u8>; CORE_BYTES],
}

/// Prueft, was beide Einstiege brauchen, und nennt Profil, Politik und den
/// Watchdog. Die Fassung zuerst: Ein Wirt einer frueheren Fassung ruft mit
/// anderen Argumenten, und `watchdog` liest erst, wer die Fassung bestand.
///
/// # Safety
///
/// `ops` ist null oder zeigt auf eine gueltige Tabelle; `watchdog` ist null
/// oder zeigt auf eine gueltige Tabelle.
unsafe fn prepare(
    core: *mut Core,
    ops: *const Ops,
    arena: *mut c_void,
    sink: *mut TraceSink,
    profile: i32,
    persisting: bool,
    watchdog: *const WatchdogOps,
) -> Result<(Profile, Policy, HostWatchdog), i32> {
    if core.is_null() || ops.is_null() || arena.is_null() {
        return Err(status::NULL);
    }
    // SAFETY: nicht null, und der Wirt sagt zu, dass sie gilt.
    let o = unsafe { &*ops };
    if o.abi != ABI {
        return Err(status::ABI);
    }
    // SAFETY: null oder gueltig, siehe oben.
    let Some(watchdog) = (unsafe { watchdog.as_ref() }).copied() else { return Err(status::NULL) };
    if sink.is_null() {
        return Err(status::LINE);
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
    Ok((profile, if o.overrun_alert != 0 { Policy::Alert } else { Policy::Fault }, HostWatchdog(watchdog)))
}

/// Baut den Kern eines Programms ohne `persist` in `core`: `ops` sind seine
/// Einstiege (`P_ops`), `arena` seine Arena, `user` das Treiberobjekt,
/// `sink` die Senke seines Trace ([`takt_sink_init`]), `profile` das
/// Laufzeitprofil (12.8: 0 `baremetal`, 1 `shared`, 2 `linux_rt`),
/// `watchdog` der Watchdog des Wirts. Das Programm steht danach in Tick 0
/// (9.4).
///
/// # Safety
///
/// `core`, `ops`, `arena`, `sink` und `watchdog` zeigen auf gueltigen
/// Speicher, der so lange lebt wie der Kern; `arena` ist die Arena des
/// Programms von `ops`, `sink` eine Senke aus [`takt_sink_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_init(
    core: *mut Core,
    ops: *const Ops,
    arena: *mut c_void,
    user: *mut c_void,
    sink: *mut TraceSink,
    profile: i32,
    watchdog: *const WatchdogOps,
) -> i32 {
    // SAFETY: siehe oben.
    let (profile, policy, watchdog) = match unsafe { prepare(core, ops, arena, sink, profile, false, watchdog) } {
        Ok(p) => p,
        Err(code) => return code,
    };
    // SAFETY: von `prepare` geprueft.
    let tick_ns = unsafe { (*ops).tick_ns };
    let mut program = CProgram::new(ops, arena, user, sink);
    program.ensure_init();
    let program = Guarded::new(program, HostProtection(None));
    let rt = Runtime::new(program, host_clock(), watchdog, host_sink(tick_ns, sink), profile, tick_ns, policy);
    let (wake, abort, hardware) = (AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false));
    let parts = Parts { ops, arena, sink, wake, abort, hardware, started: false, engine: Engine::Volatile(rt) };
    // SAFETY: `core` fasst `Parts` (Pruefung oben) und ist ausgerichtet.
    unsafe { core.cast::<Parts>().write(parts) };
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
    sink: *mut TraceSink,
    profile: i32,
    watchdog: *const WatchdogOps,
    nvm: *const NvmOps,
    current: *mut u8,
    stored: *mut u8,
    bound: u32,
) -> i32 {
    // SAFETY: siehe oben.
    let (profile, policy, watchdog) = match unsafe { prepare(core, ops, arena, sink, profile, true, watchdog) } {
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
    let mut program = CProgram::new(ops, arena, user, sink);
    let (persist, _, _) = Unloaded::new(journal, current, stored).load(&mut program);
    program.ensure_init();
    let program = Guarded::new(program, HostProtection(None));
    let trace = host_sink(o.tick_ns, sink);
    let rt = Runtime::journaled(program, persist, host_clock(), watchdog, trace, profile, o.tick_ns, policy);
    let (wake, abort, hardware) = (AtomicBool::new(false), AtomicBool::new(false), AtomicBool::new(false));
    let parts = Parts { ops, arena, sink, wake, abort, hardware, started: false, engine: Engine::Persisting(rt) };
    // SAFETY: `core` fasst `Parts` (Pruefung oben) und ist ausgerichtet.
    unsafe { core.cast::<Parts>().write(parts) };
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
    // SAFETY: wie `takt_core_service_with`.
    unsafe { takt_core_service_with(core, now_ns, core::ptr::null()) }
}

/// Wie [`takt_core_service`], mit den Tunables des Wirts (8.4): Ihre
/// Aenderungen gehen vor dem Schritt ihrer Grenze in das Programm, und im
/// Schlaf weckt eine Aenderung (9.9). `tunables` darf null sein.
///
/// # Safety
///
/// Wie [`takt_core_service`]; `tunables` ist null oder zeigt auf eine
/// Tabelle, deren Rueckrufe waehrend des Aufrufs gelten.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_service_with(
    core: *mut Core,
    now_ns: i64,
    tunables: *const TunablesOps,
) -> NextStep {
    // SAFETY: null oder gueltig, siehe oben.
    let mut source = unsafe { tunables.as_ref() }.copied().map(HostTunables);
    let parts = core.cast::<Parts>();
    // SAFETY: nur die Adresse des Flags, ohne Referenz; andere Kontexte setzen es.
    let wake = unsafe { &raw const (*parts).wake };
    // SAFETY: nur das Flag; `takt_core_abort` setzt es aus jedem Kontext.
    let abort = unsafe { &(*parts).abort }.swap(false, Ordering::SeqCst);
    // SAFETY: ebenso das Flag von `takt_core_hardware_fault`.
    let hardware = unsafe { &(*parts).hardware }.swap(false, Ordering::SeqCst);
    let outer = Outer { abort, hardware };
    // SAFETY: Nur der Schrittkontext ruft, nie zwei Aufrufe zugleich, und
    // `takt_core_protect` laeuft vor dem ersten.
    unsafe { (*parts).started = true };
    // SAFETY: nur der Kern, nicht Tabelle, Arena und Flags, die andere Kontexte lesen.
    let engine = unsafe { &mut (*parts).engine };
    let source: Option<&mut dyn Tunables> = match source.as_mut() {
        Some(s) => Some(s),
        None => None,
    };
    engine.service(now_ns, wake, outer, source).into()
}

/// Gibt dem Kern den Schutz des Wirts (12.3, 12.11; FB-755): Ab jetzt ist
/// der Programmbereich der Arena nur um die Aufrufe offen, die ihn
/// schreiben, und eine Verletzung, die `take_violation` meldet, bekommt das
/// Programm im naechsten Tick als `Runtime(Hardware)`. Der Kern schliesst
/// ihn sofort; bis hierher durfte `init` schreiben. Nur vor dem ersten
/// [`takt_core_service`]: danach `TAKT_ERR_STARTED`, und der Schutz bleibt,
/// wie er war. Ohne diesen Aufruf schuetzt der Kern nichts, und
/// [`takt_core_protected`] sagt es.
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`]; `protection` ist null oder
/// zeigt auf eine Tabelle, deren Rueckrufe so lange gelten wie der Kern.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_protect(core: *mut Core, protection: *const ProtectionOps) -> i32 {
    if core.is_null() {
        return status::NULL;
    }
    // SAFETY: null oder gueltig, siehe oben.
    let Some(ops) = (unsafe { protection.as_ref() }).copied() else { return status::NULL };
    let parts = core.cast::<Parts>();
    // SAFETY: Vor dem ersten `service` haelt niemand den Kern.
    if unsafe { (*parts).started } {
        return status::STARTED;
    }
    // SAFETY: wie oben.
    let guard = unsafe { (*parts).engine.protection() };
    *guard = HostProtection(Some(ops));
    guard.close();
    status::OK
}

/// 1, wenn der Kern den Schutz des Wirts hat ([`takt_core_protect`]), sonst
/// 0: Danach richtet sich die Kopfzeile `Schutz` (12.11), nicht nach dem, was
/// der Wirt vorhatte.
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_protected(core: *const Core) -> i32 {
    // SAFETY: liest nur, ob Haken gesetzt sind; die setzt nur `takt_core_protect`.
    let engine = unsafe { &(*core.cast::<Parts>()).engine };
    i32::from(engine.protected())
}

/// Ein Operator-Abort (5.4): Der Kern meldet ihn dem Programm vor dem
/// Schritt der naechsten Grenze, die `takt_core_service` erreicht, fuer alle
/// Maschinen; schlaeft er, endet der Schlaf dort (9.9). Darf aus jedem
/// Kontext kommen, auch aus einer ISR oder waehrend eines Schritts; nie
/// rechnet er waehrend `service`.
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_abort(core: *mut Core) {
    // SAFETY: nur das Flag; den Kern haelt der Schrittkontext, Tabelle und
    // Arena beruehrt dieser Aufruf nicht.
    let abort = unsafe { &(*core.cast::<Parts>()).abort };
    abort.store(true, Ordering::SeqCst);
}

/// Eine Schutzverletzung (12.3): Die Schutzeinheit hat einen Zugriff der TCB
/// auf den Programmzustand abgewiesen. Der Kern meldet dem Programm
/// `Runtime(Hardware)` vor dem Schritt der naechsten Grenze, die
/// `takt_core_service` erreicht, fuer alle Maschinen; schlaeft er, endet der
/// Schlaf dort (9.9). Darf aus jedem Kontext kommen, auch aus einem
/// Fault-Handler oder einer ISR und waehrend eines Schritts.
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_hardware_fault(core: *mut Core) {
    // SAFETY: nur das Flag; den Kern haelt der Schrittkontext, Tabelle und
    // Arena beruehrt dieser Aufruf nicht.
    let hardware = unsafe { &(*core.cast::<Parts>()).hardware };
    hardware.store(true, Ordering::SeqCst);
}

/// Ein Weckereignis des Wirts (9.9): Schlaeft der Kern, endet der Schlaf an
/// der naechsten Grenze, die `takt_core_service` erreicht. Darf aus jedem
/// Kontext kommen, auch aus einer ISR oder einem Signal, auch waehrend
/// eines Schritts.
///
/// # Safety
///
/// `core` ist ein Kern aus [`takt_core_init`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn takt_core_wake(core: *mut Core) {
    // SAFETY: nur das Flag; den Kern haelt der Schrittkontext, Tabelle und
    // Arena beruehrt dieser Aufruf nicht.
    let wake = unsafe { &(*core.cast::<Parts>()).wake };
    wake.store(true, Ordering::SeqCst);
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
/// letzte Zeile in die Senke des Kerns (13.8): Schlaf, Ueberlaeufe, Rueckstand,
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
    let parts = core.cast::<Parts>();
    // SAFETY: die Senke, gesetzt in `takt_core_init`.
    let sink = unsafe { (*parts).sink };
    // SAFETY: wie in `takt_core_service`.
    let engine = unsafe { &mut (*parts).engine };
    // SAFETY: null oder gueltig.
    let stacks = unsafe { stacks.as_ref() }.copied().unwrap_or_default();
    // SAFETY: ebenso.
    let foreign = unsafe { foreign.as_ref() }.copied();
    engine.report(sink, &Stacks::from(stacks), foreign);
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

    use super::{
        ABI, CORE_BYTES, Foreign, LineOps, NextStep, NvmOps, Ops, ProtectionOps, SINK_BYTES, StackDepths, TunablesOps,
        WatchdogOps, nvm, status, violation,
    };

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
            ("operator_abort", offset_of!(Ops, operator_abort)),
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
        let watchdog = in_order(vec![("ctx", offset_of!(WatchdogOps, ctx)), ("kick", offset_of!(WatchdogOps, kick))]);
        assert_eq!(fields("struct takt_watchdog {"), watchdog);
        let protection = in_order(vec![
            ("ctx", offset_of!(ProtectionOps, ctx)),
            ("open", offset_of!(ProtectionOps, open)),
            ("close", offset_of!(ProtectionOps, close)),
            ("take_violation", offset_of!(ProtectionOps, take_violation)),
        ]);
        assert_eq!(fields("struct takt_protection {"), protection);
        let tunables = in_order(vec![
            ("ctx", offset_of!(TunablesOps, ctx)),
            ("next", offset_of!(TunablesOps, next)),
            ("idle", offset_of!(TunablesOps, idle)),
        ]);
        assert_eq!(fields("struct takt_tunables {"), tunables);
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
        // Jeder Einstieg des Archivs steht im Kopf: Einen, den er nicht
        // deklariert, kann kein C-Wirt rufen. Die Haken `takt_trace_*`
        // deklariert der Kopf des Programms (`P.h`), einmal (MISRA 8.5).
        let entries: Vec<&str> = include_str!("lib.rs")
            .lines()
            .filter_map(|l| l.split("extern \"C\" fn ").nth(1)?.split('(').next())
            .filter(|name| ["takt_core_", "takt_sink_", "takt_foreign_"].iter().any(|p| name.starts_with(p)))
            .collect();
        assert!(entries.contains(&"takt_core_hardware_fault") && entries.contains(&"takt_sink_init"), "{entries:?}");
        assert!(!HEADER.contains("void takt_trace_text("), "die Haken deklariert `P.h`");
        for name in entries {
            assert!(HEADER.contains(&format!(" {name}(")), "`{name}` fehlt im Kopf");
        }
        assert_eq!(define("TAKT_CORE_BYTES"), CORE_BYTES as i64);
        assert_eq!(define("TAKT_SINK_BYTES"), SINK_BYTES as i64);
        for (name, value) in [
            ("TAKT_OK", status::OK),
            ("TAKT_ERR_NULL", status::NULL),
            ("TAKT_ERR_ABI", status::ABI),
            ("TAKT_ERR_MEMORY", status::MEMORY),
            ("TAKT_ERR_PROFILE", status::PROFILE),
            ("TAKT_ERR_BOUND", status::BOUND),
            ("TAKT_ERR_LINE", status::LINE),
            ("TAKT_ERR_STARTED", status::STARTED),
            ("TAKT_VIOLATION_NONE", violation::NONE),
            ("TAKT_VIOLATION_STATE", violation::STATE),
            ("TAKT_VIOLATION_GUARD", violation::GUARD),
            ("TAKT_NVM_IDLE", nvm::IDLE),
            ("TAKT_NVM_BUSY", nvm::BUSY),
            ("TAKT_NVM_DONE", nvm::DONE),
            ("TAKT_NVM_FAILED", nvm::FAILED),
        ] {
            assert_eq!(define(name), i64::from(value), "{name}");
        }
    }
}

/// Der Kern hinter der C-Schnittstelle, wie ein C-Wirt ihn faehrt (M11
/// Schritt 33c): Programm, Leitung und Geraet des Journals sind Rueckrufe
/// dieses Tests. Unter Miri prueft der Lauf jeden Zugriff des Archivs ueber
/// die Zeiger des Wirts: den Kern in `takt_core`, die Senke in `takt_sink`,
/// die Arena und die Puffer des Journals.
#[cfg(test)]
mod core_abi {
    use core::cell::Cell;
    use core::ffi::c_void;
    use core::mem::MaybeUninit;
    use core::ops::Range;
    use core::ptr::{NonNull, null, null_mut};
    use std::string::String;
    use std::vec::Vec;

    use super::{
        ABI, Core, LineOps, NvmOps, Ops, ProtectionOps, TraceSink, TunablesOps, WatchdogOps, nvm, status,
        takt_core_abort, takt_core_finish, takt_core_hardware_fault, takt_core_init, takt_core_init_journaled,
        takt_core_job_work, takt_core_protect, takt_core_protected, takt_core_report, takt_core_service,
        takt_core_service_with, takt_core_wake, takt_sink_flush, takt_sink_init, takt_sink_lossless, takt_trace_text,
        violation,
    };

    const TICK_NS: i64 = 1_000_000;
    const TICKS: i64 = 12;
    const SLOT: usize = 256;

    /// Die Arena des Programms: der letzte Tick, wie oft es begann und
    /// welchen Stand es aus dem Journal bekam.
    #[derive(Default)]
    struct Counter {
        tick: i64,
        starts: u32,
        restored: i64,
        /// Die letzte Aenderung eines Tunables: Tick des Rahmens, Parameter,
        /// erstes Byte.
        tuned: Option<(i64, u32, u8)>,
        /// Ein Operator-Abort ist gemeldet und wartet auf den Schritt.
        abort_pending: bool,
        /// Der Tick des Rahmens, vor dessen Schritt der Abort kam.
        aborted_in: Option<i64>,
        /// Eine Schutzverletzung ist gemeldet und wartet auf den Schritt.
        hardware_pending: bool,
        /// Der Tick des Rahmens, vor dessen Schritt sie kam.
        hardware_in: Option<i64>,
        /// Die Senke aus `init`, in die das Programm schreibt.
        sink: Option<NonNull<c_void>>,
    }

    /// Die Arena hinter dem Zeiger des Archivs.
    ///
    /// # Safety
    ///
    /// `arena` zeigt auf einen `Counter`, auf den waehrend des Aufrufs sonst
    /// nichts zugreift.
    unsafe fn counter<'a>(arena: *mut c_void) -> &'a mut Counter {
        // SAFETY: siehe oben.
        unsafe { &mut *arena.cast::<Counter>() }
    }

    unsafe extern "C" fn init(arena: *mut c_void, _user: *mut c_void, sink: *mut c_void) {
        // SAFETY: die Arena des Tests.
        let c = unsafe { counter(arena) };
        c.tick = 0;
        c.starts += 1;
        c.sink = NonNull::new(sink);
    }

    unsafe extern "C" fn restore(arena: *mut c_void, bytes: *const c_void, len: i32) -> i32 {
        if len != 8 {
            return 0;
        }
        let mut value = [0u8; 8];
        // SAFETY: Das Archiv reicht `len` lesbare Byte.
        unsafe { core::ptr::copy_nonoverlapping(bytes.cast::<u8>(), value.as_mut_ptr(), value.len()) };
        // SAFETY: die Arena des Tests.
        unsafe { counter(arena) }.restored = i64::from_le_bytes(value);
        8
    }

    unsafe extern "C" fn init_with(
        arena: *mut c_void,
        user: *mut c_void,
        sink: *mut c_void,
        bytes: *const c_void,
        len: i32,
    ) -> i32 {
        // SAFETY: wie `init` und `restore`.
        unsafe {
            init(arena, user, sink);
            restore(arena, bytes, len)
        }
    }

    unsafe extern "C" fn tick(arena: *mut c_void, k: i64) {
        // SAFETY: die Arena des Tests.
        let c = unsafe { counter(arena) };
        c.tick = k;
        if core::mem::take(&mut c.abort_pending) {
            c.aborted_in = Some(k);
        }
        if core::mem::take(&mut c.hardware_pending) {
            c.hardware_in = Some(k);
        }
    }

    unsafe extern "C" fn hardware_fault(arena: *mut c_void) {
        // SAFETY: die Arena des Tests.
        unsafe { counter(arena) }.hardware_pending = true;
    }

    unsafe extern "C" fn operator_abort(arena: *mut c_void) {
        // SAFETY: die Arena des Tests.
        unsafe { counter(arena) }.abort_pending = true;
    }

    unsafe extern "C" fn snapshot(arena: *mut c_void, out: *mut c_void, cap: i32) -> i32 {
        if cap < 8 {
            return 0;
        }
        // SAFETY: die Arena des Tests.
        let value = unsafe { counter(arena) }.tick.to_le_bytes();
        // SAFETY: Das Archiv reicht `cap` beschreibbare Byte.
        unsafe { core::ptr::copy_nonoverlapping(value.as_ptr(), out.cast::<u8>(), value.len()) };
        8
    }

    /// Schreibt `text` in die Senke des Programms, wie der Rahmen es tut.
    ///
    /// # Safety
    ///
    /// Wie [`counter`]; `text` ist nullterminiert.
    unsafe fn say(arena: *mut c_void, text: &core::ffi::CStr) {
        // SAFETY: die Arena des Tests.
        let sink = unsafe { counter(arena) }.sink.map_or(null_mut(), NonNull::as_ptr);
        // SAFETY: die Senke aus `init`, die der Test haelt.
        unsafe { takt_trace_text(sink, text.as_ptr()) };
    }

    unsafe extern "C" fn dump(arena: *mut c_void, _all: i32) {
        // SAFETY: die Arena des Tests.
        unsafe { say(arena, c"t= dump\n") };
    }

    /// Wie [`dump`], fuer ein zweites Programm.
    unsafe extern "C" fn dump_other(arena: *mut c_void, _all: i32) {
        // SAFETY: die Arena des Tests.
        unsafe { say(arena, c"t= dump other\n") };
    }

    unsafe extern "C" fn tolerance(ns: *mut i64, runs: *mut u32) {
        // SAFETY: Das Archiv reicht zwei beschreibbare Zahlen.
        unsafe {
            ns.write(0);
            runs.write(0);
        }
    }

    extern "C" fn nothing(_: *mut c_void) {}

    extern "C" fn no(_: *mut c_void) -> u8 {
        0
    }

    extern "C" fn no_deadline(_: *mut c_void) -> i64 {
        -1
    }

    extern "C" fn advance(_: *mut c_void, _: i64) {}

    unsafe extern "C" fn tune(arena: *mut c_void, k: i64, param: u32, value: *const c_void, len: i32) -> i32 {
        if len < 1 {
            return 0;
        }
        // SAFETY: Das Archiv reicht `len` lesbare Byte.
        let first = unsafe { value.cast::<u8>().read() };
        // SAFETY: die Arena des Tests.
        unsafe { counter(arena) }.tuned = Some((k, param, first));
        1
    }

    extern "C" fn yes(_: *mut c_void) -> u8 {
        1
    }

    /// Die naechste Frist eines Programms, das bis 100 ms nichts vorhat.
    extern "C" fn far(_: *mut c_void) -> i64 {
        100 * TICK_NS
    }

    extern "C" fn woken(_: *mut c_void, _: i64) -> u8 {
        0
    }

    extern "C" fn constant() -> u8 {
        0
    }

    extern "C" fn pc(_: *mut c_void, _: i32) {}

    extern "C" fn next_run(_: *mut c_void, _: *mut i64) -> i32 {
        0
    }

    extern "C" fn job_dispatch(_: *mut c_void) -> i32 {
        0
    }

    /// Die Tabelle des Programms, wie `P_ops` sie fuellt.
    fn ops(persists: bool) -> Ops {
        Ops {
            abi: ABI,
            persists: u8::from(persists),
            overrun_alert: 0,
            tick_ns: TICK_NS,
            logic_hash: 0x7a6b_7400,
            persist_min_interval_ns: 0,
            persist_bound: 8,
            init,
            init_with,
            tick,
            commit: nothing,
            idle: no,
            deadline: no_deadline,
            advance,
            tune,
            woken,
            wake_sources: constant,
            persist_snapshot: snapshot,
            persist_restore: restore,
            dump,
            pc,
            next_run,
            end: nothing,
            overrun: nothing,
            hardware: nothing,
            operator_abort: nothing,
            tolerance,
            output_timing: constant,
            job_dispatch,
            job_work: nothing,
        }
    }

    /// Ein Flash im Speicher: zwei Slots, jeder Vorgang sofort fertig.
    struct Flash {
        slots: [[u8; SLOT]; 2],
        pending: i32,
    }

    /// Das Flash hinter dem Zeiger des Geraets.
    ///
    /// # Safety
    ///
    /// Wie [`counter`], fuer ein `Flash`.
    unsafe fn flash<'a>(ctx: *mut c_void) -> &'a mut Flash {
        // SAFETY: siehe oben.
        unsafe { &mut *ctx.cast::<Flash>() }
    }

    /// `offset..offset + len`, wenn es in einem Slot liegt.
    fn span(offset: u32, len: u32) -> Option<Range<usize>> {
        let start = usize::try_from(offset).ok()?;
        let end = start.checked_add(usize::try_from(len).ok()?)?;
        (end <= SLOT).then_some(start..end)
    }

    unsafe extern "C" fn erase(ctx: *mut c_void, slot: u8) -> u8 {
        // SAFETY: das Flash des Tests.
        let f = unsafe { flash(ctx) };
        f.slots[usize::from(slot & 1)].fill(0xff);
        f.pending = nvm::DONE;
        1
    }

    unsafe extern "C" fn write(ctx: *mut c_void, slot: u8, offset: u32, bytes: *const u8, len: u32) -> u8 {
        // SAFETY: das Flash des Tests.
        let f = unsafe { flash(ctx) };
        f.pending = match span(offset, len) {
            Some(range) => {
                // SAFETY: Das Archiv reicht `len` lesbare Byte.
                let bytes = unsafe { core::slice::from_raw_parts(bytes, range.len()) };
                f.slots[usize::from(slot & 1)][range].copy_from_slice(bytes);
                nvm::DONE
            }
            None => nvm::FAILED,
        };
        1
    }

    unsafe extern "C" fn poll(ctx: *mut c_void) -> i32 {
        // SAFETY: das Flash des Tests.
        core::mem::replace(&mut unsafe { flash(ctx) }.pending, nvm::IDLE)
    }

    unsafe extern "C" fn read(ctx: *mut c_void, slot: u8, offset: u32, into: *mut u8, len: u32) -> u8 {
        // SAFETY: das Flash des Tests.
        let f = unsafe { flash(ctx) };
        let Some(range) = span(offset, len) else { return 0 };
        // SAFETY: Das Archiv reicht `len` beschreibbare Byte.
        let into = unsafe { core::slice::from_raw_parts_mut(into, range.len()) };
        into.copy_from_slice(&f.slots[usize::from(slot & 1)][range]);
        1
    }

    /// Die Leitung: jedes Byte in den Puffer des Tests.
    unsafe extern "C" fn put(ctx: *mut c_void, byte: u8) -> u8 {
        // SAFETY: der Puffer des Tests; nur die Leitung schreibt in ihn.
        unsafe { (*ctx.cast::<Vec<u8>>()).push(byte) };
        1
    }

    /// Eine langsame Leitung: Sie nimmt nur jedes vierte angebotene Byte.
    struct Slow {
        bytes: Vec<u8>,
        offered: u32,
    }

    unsafe extern "C" fn slow(ctx: *mut c_void, byte: u8) -> u8 {
        // SAFETY: die Leitung des Tests; nur sie schreibt in ihn.
        let s = unsafe { &mut *ctx.cast::<Slow>() };
        s.offered += 1;
        if s.offered % 4 != 0 {
            return 0;
        }
        s.bytes.push(byte);
        1
    }

    /// Bindet eine langsame Leitung mit einem Ring von vier Byte an eine
    /// Senke, auf Wunsch verlustfrei, schreibt 16 Byte und gibt den Ring ab,
    /// bis er leer ist ([`takt_sink_flush`]); liefert, was ankam.
    fn through_a_slow_line(lossless: bool) -> Vec<u8> {
        let mut line = Slow { bytes: Vec::new(), offered: 0 };
        let mut ring = [0u8; 4];
        let mut sink = MaybeUninit::<TraceSink>::uninit();
        let ops = LineOps { ctx: (&raw mut line).cast(), try_write: slow, flush: None };
        // SAFETY: Senke, Tabelle, Leitung und Ring leben bis zum Ende; kein Schritt laeuft.
        assert_eq!(unsafe { takt_sink_init(sink.as_mut_ptr(), &ops, ring.as_mut_ptr(), 4) }, status::OK);
        if lossless {
            // SAFETY: wie oben.
            assert_eq!(unsafe { takt_sink_lossless(sink.as_mut_ptr()) }, status::OK);
        }
        // SAFETY: wie oben; der Text ist nullterminiert.
        unsafe { takt_trace_text(sink.as_mut_ptr().cast(), c"0123456789abcdef".as_ptr()) };
        // Ein Abgeben nimmt hoechstens ein Byte: Die Leitung lehnt danach ab.
        for _ in 0..64 {
            // SAFETY: wie oben.
            unsafe { takt_sink_flush(sink.as_mut_ptr()) };
        }
        line.bytes
    }

    /// Faehrt `core` wie ein C-Wirt in logischer Zeit ueber [`TICKS`] Ticks:
    /// die Uhr zu jeder Frist, jeder Job zu Ende vor dem naechsten Schritt.
    ///
    /// # Safety
    ///
    /// `core` ist ein Kern aus `takt_core_init` oder `takt_core_init_journaled`.
    unsafe fn drive(core: *mut Core) {
        let mut now = 0;
        for _ in 0..4 * TICKS {
            if now >= TICKS * TICK_NS {
                break;
            }
            // SAFETY: siehe oben; nie zwei Aufrufe zugleich.
            let next = unsafe { takt_core_service(core, now) };
            assert_eq!(next.ended, 0, "das Programm endet nie");
            if next.jobs != 0 {
                // SAFETY: wie oben, als Job-Kontext.
                unsafe { takt_core_job_work(core) };
            }
            assert!(next.deadline_ns > now, "die Frist {} liegt nicht nach {now}", next.deadline_ns);
            now = next.deadline_ns;
        }
    }

    /// Eine Quelle von Tunables mit einer Aenderung an der Grenze `at`:
    /// Parameter 7, Wert 42.
    struct Source {
        at: u64,
        given: bool,
    }

    static VALUE: [u8; 1] = [42];

    unsafe extern "C" fn change(
        ctx: *mut c_void,
        k: u64,
        param: *mut u32,
        value: *mut *const u8,
        len: *mut u32,
    ) -> i32 {
        // SAFETY: die Quelle des Tests.
        let s = unsafe { &mut *ctx.cast::<Source>() };
        if k != s.at || s.given {
            return 0;
        }
        s.given = true;
        // SAFETY: Das Archiv reicht drei beschreibbare Stellen.
        unsafe {
            param.write(7);
            value.write(VALUE.as_ptr());
            len.write(1);
        }
        1
    }

    /// Wie [`drive`], mit den Tunables `tunables`.
    ///
    /// # Safety
    ///
    /// Wie [`drive`].
    unsafe fn drive_with(core: *mut Core, tunables: &TunablesOps) {
        let mut now = 0;
        while now < TICKS * TICK_NS {
            // SAFETY: siehe oben; nie zwei Aufrufe zugleich.
            let next = unsafe { takt_core_service_with(core, now, tunables) };
            now = next.deadline_ns;
        }
    }

    /// Der Watchdog des Tests: zaehlt, wie oft der Kern meldet, dass er lebt.
    unsafe extern "C" fn kick(ctx: *mut c_void) {
        // SAFETY: `ctx` ist der Zaehler des Tests, der ihn ueberlebt.
        let kicks = unsafe { &*ctx.cast::<Cell<u64>>() };
        kicks.set(kicks.get() + 1);
    }

    /// **Ein Kern laeuft durch die C-Schnittstelle** (12.11): ohne Journal,
    /// dann zweimal mit Journal auf demselben Flash — der zweite Lauf
    /// beginnt mit dem Stand des ersten —, die Bilanz geht in seine Senke,
    /// und der Kern meldet dem Watchdog des Wirts, dass er lebt. Ohne Senke
    /// und ohne Watchdog nimmt er nicht an: Beide stellt der Wirt
    /// ausdruecklich, einen stillen Vorgabewert gibt es nicht.
    #[test]
    fn a_core_runs_through_the_c_interface() {
        let kicks = Cell::new(0u64);
        let watchdog = WatchdogOps { ctx: core::ptr::from_ref(&kicks).cast_mut().cast(), kick };
        let plain = ops(false);
        let mut arena = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        // SAFETY: Kern, Tabelle, Arena und Watchdog leben bis zum Ende des Tests.
        let unbound = unsafe {
            takt_core_init(core.as_mut_ptr(), &plain, (&raw mut arena).cast(), null_mut(), null_mut(), 0, &watchdog)
        };
        assert_eq!(unbound, status::LINE, "ohne Senke");
        // SAFETY: Kein Schritt laeuft.
        assert_eq!(unsafe { takt_sink_lossless(null_mut()) }, status::LINE, "verlustfrei ohne Senke");

        // Eine Leitung, die langsamer nimmt, als geschrieben wird: Im Betrieb
        // verwirft der volle Ring (12.2), verlustfrei wartet der Schreibende
        // (13.8), und alles kommt in seiner Reihenfolge an.
        let dropping = through_a_slow_line(false);
        let lossless = through_a_slow_line(true);
        assert!(dropping.len() < 16, "ohne Warten verwirft ein voller Ring: {dropping:?}");
        assert_eq!(lossless, b"0123456789abcdef");

        // Senke, Leitung und Ring leben so lange wie jeder Kern, der sie nutzt.
        let mut text = Vec::<u8>::new();
        let mut ring = std::vec![0u8; 512];
        let mut sink = MaybeUninit::<TraceSink>::uninit();
        let sink = sink.as_mut_ptr();
        let line = LineOps { ctx: (&raw mut text).cast(), try_write: put, flush: None };
        let len = u32::try_from(ring.len()).expect("Ring");
        // SAFETY: Tabelle, Leitung und Ring leben bis zum Ende des Tests; kein Schritt laeuft.
        assert_eq!(unsafe { takt_sink_init(sink, &line, ring.as_mut_ptr(), len) }, status::OK);

        // SAFETY: wie oben, ohne Watchdog.
        let blind =
            unsafe { takt_core_init(core.as_mut_ptr(), &plain, (&raw mut arena).cast(), null_mut(), sink, 0, null()) };
        assert_eq!(blind, status::NULL, "ohne Watchdog");
        // SAFETY: wie oben.
        let rc = unsafe {
            takt_core_init(core.as_mut_ptr(), &plain, (&raw mut arena).cast(), null_mut(), sink, 0, &watchdog)
        };
        assert_eq!(rc, status::OK);
        // SAFETY: der Kern von eben; danach die Bilanz, waehrend kein Schritt laeuft.
        unsafe {
            drive(core.as_mut_ptr());
            takt_core_report(core.as_mut_ptr(), null(), null());
        }
        assert!(arena.tick > 1 && arena.starts == 1, "Tick {}, Anfaenge {}", arena.tick, arena.starts);
        assert!(kicks.get() > 0, "der Kern meldete dem Watchdog nie");

        // Ein Tunable kommt an seiner Grenze an (8.4): Grenze 3 ist der Tick 4
        // des Rahmens, der ab 1 zaehlt.
        let mut source = Source { at: 3, given: false };
        let tunables = TunablesOps { ctx: (&raw mut source).cast(), next: change, idle: None };
        let mut tuned = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        // SAFETY: wie oben.
        let rc = unsafe {
            takt_core_init(core.as_mut_ptr(), &plain, (&raw mut tuned).cast(), null_mut(), sink, 0, &watchdog)
        };
        assert_eq!(rc, status::OK);
        // SAFETY: der Kern von eben, mit der Quelle des Tests.
        unsafe { drive_with(core.as_mut_ptr(), &tunables) };
        assert_eq!(tuned.tuned, Some((4, 7, 42)), "das Tunable kam nicht an seiner Grenze an");

        // Ein Programm, das schlafen darf, schlaeft bis zu seiner Frist; ein
        // Weckereignis des Wirts beendet den Schlaf an der naechsten Grenze
        // (9.9). `Counter::tick` ist der Tick des Rahmens, der ab 1 zaehlt.
        let mut sleepy = ops(false);
        sleepy.idle = yes;
        sleepy.deadline = far;
        let mut dozing = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        // SAFETY: wie oben.
        let rc = unsafe {
            takt_core_init(core.as_mut_ptr(), &sleepy, (&raw mut dozing).cast(), null_mut(), sink, 0, &watchdog)
        };
        assert_eq!(rc, status::OK);
        // SAFETY: der Kern von eben; nie zwei Aufrufe zugleich.
        let asleep = unsafe { takt_core_service(core.as_mut_ptr(), 0) };
        assert_eq!((asleep.deadline_ns, dozing.tick), (99 * TICK_NS, 1), "schlaeft bis zur Frist");
        // SAFETY: wie oben.
        unsafe { takt_core_service(core.as_mut_ptr(), 4 * TICK_NS) };
        assert_eq!(dozing.tick, 1, "ohne Weckereignis rechnet der Schlaf keinen Tick");
        // SAFETY: wie oben.
        unsafe {
            takt_core_wake(core.as_mut_ptr());
            takt_core_service(core.as_mut_ptr(), 6 * TICK_NS);
        }
        assert_eq!(dozing.tick, 7, "das Weckereignis beendet den Schlaf an der Grenze bei 6 ms");

        // Ein Operator-Abort (5.4) weckt ebenso und kommt vor dem Schritt an
        // dieser Grenze beim Programm an, einmal.
        let mut halting = ops(false);
        halting.idle = yes;
        halting.deadline = far;
        halting.operator_abort = operator_abort;
        let mut halted = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        // SAFETY: wie oben.
        let rc = unsafe {
            takt_core_init(core.as_mut_ptr(), &halting, (&raw mut halted).cast(), null_mut(), sink, 0, &watchdog)
        };
        assert_eq!(rc, status::OK);
        // SAFETY: wie oben; der Abort kommt zwischen zwei Aufrufen.
        unsafe {
            takt_core_service(core.as_mut_ptr(), 0);
            takt_core_abort(core.as_mut_ptr());
            takt_core_service(core.as_mut_ptr(), 6 * TICK_NS);
            takt_core_service(core.as_mut_ptr(), 8 * TICK_NS);
        }
        assert_eq!(halted.aborted_in, Some(7), "der Abort wirkt im Tick an der Grenze bei 6 ms");
        assert!(!halted.abort_pending, "der Kern meldete den Abort zweimal");

        // Eine Schutzverletzung (12.3) ebenso: Sie weckt und kommt als
        // `Runtime(Hardware)` vor dem Schritt an dieser Grenze an, einmal.
        let mut guarding = ops(false);
        guarding.idle = yes;
        guarding.deadline = far;
        guarding.hardware = hardware_fault;
        let mut guarded = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        // SAFETY: wie oben.
        let rc = unsafe {
            takt_core_init(core.as_mut_ptr(), &guarding, (&raw mut guarded).cast(), null_mut(), sink, 0, &watchdog)
        };
        assert_eq!(rc, status::OK);
        // SAFETY: wie oben; die Meldung kommt zwischen zwei Aufrufen.
        unsafe {
            takt_core_service(core.as_mut_ptr(), 0);
            takt_core_hardware_fault(core.as_mut_ptr());
            takt_core_service(core.as_mut_ptr(), 6 * TICK_NS);
            takt_core_service(core.as_mut_ptr(), 8 * TICK_NS);
        }
        assert_eq!(guarded.hardware_in, Some(7), "die Schutzverletzung wirkt im Tick an der Grenze bei 6 ms");
        assert!(!guarded.hardware_pending, "der Kern meldete sie zweimal");

        let journaled = ops(true);
        let mut flash = Flash { slots: [[0xff; SLOT]; 2], pending: nvm::IDLE };
        let device = NvmOps {
            ctx: (&raw mut flash).cast(),
            slot_size: u32::try_from(SLOT).expect("Slot"),
            blocking_ns: -1,
            begin_erase: erase,
            begin_write: write,
            poll,
            read,
        };
        let (mut current, mut stored) = ([0u8; 8], [0u8; 8]);
        let mut runs = Vec::new();
        for _ in 0..2 {
            let mut arena = Counter::default();
            let mut core = MaybeUninit::<Core>::uninit();
            // SAFETY: Kern, Tabelle, Arena, Geraet und Puffer leben bis zum
            // Ende des Laufs; die Puffer benutzt nur dieser Kern.
            let rc = unsafe {
                takt_core_init_journaled(
                    core.as_mut_ptr(),
                    &journaled,
                    (&raw mut arena).cast(),
                    null_mut(),
                    sink,
                    0,
                    &watchdog,
                    &device,
                    current.as_mut_ptr(),
                    stored.as_mut_ptr(),
                    8,
                )
            };
            assert_eq!(rc, status::OK);
            // SAFETY: der Kern von eben.
            unsafe {
                drive(core.as_mut_ptr());
                takt_core_finish(core.as_mut_ptr());
            }
            runs.push((arena.tick, arena.restored));
        }
        assert_eq!(runs[0].1, 0, "das leere Flash hat keinen Stand");
        assert!(runs[1].1 > 0, "der zweite Lauf beginnt mit dem Stand des ersten: {runs:?}");

        // Kein Kern laeuft mehr; der Puffer gehoert wieder dem Test.
        let text = String::from_utf8_lossy(&text).into_owned();
        assert!(text.contains("t= dump") && text.contains("takt end"), "{text}");
    }

    /// Der Schutz des Tests: zaehlt, wie oft der Kern oeffnet und schliesst,
    /// und haelt eine Verletzung bereit.
    #[derive(Default)]
    struct Unit {
        open: Cell<bool>,
        opened: Cell<u32>,
        closed: Cell<u32>,
        pending: Cell<Option<u32>>,
    }

    /// # Safety
    ///
    /// `ctx` ist die Einheit des Tests, die den Kern ueberlebt.
    unsafe fn unit<'a>(ctx: *mut c_void) -> &'a Unit {
        // SAFETY: siehe oben.
        unsafe { &*ctx.cast::<Unit>() }
    }

    unsafe extern "C" fn open(ctx: *mut c_void) {
        // SAFETY: die Einheit des Tests.
        let u = unsafe { unit(ctx) };
        u.open.set(true);
        u.opened.set(u.opened.get() + 1);
    }

    unsafe extern "C" fn close(ctx: *mut c_void) {
        // SAFETY: die Einheit des Tests.
        let u = unsafe { unit(ctx) };
        u.open.set(false);
        u.closed.set(u.closed.get() + 1);
    }

    unsafe extern "C" fn take_violation(ctx: *mut c_void, address: *mut u32) -> i32 {
        // SAFETY: die Einheit des Tests; das Archiv reicht eine beschreibbare Stelle.
        match unsafe { unit(ctx) }.pending.take() {
            Some(a) => {
                // SAFETY: wie oben.
                unsafe { address.write(a) };
                violation::STATE
            }
            None => violation::NONE,
        }
    }

    /// **Der Schutz des Wirts oeffnet den Programmbereich nur, wo der Kern
    /// ihn schreibt** (12.3, 12.11; FB-755): Nach `takt_core_protect` ist er
    /// geschlossen, jeder Aufruf, der ihn oeffnet, schliesst ihn wieder, und
    /// eine gemeldete Verletzung weckt den Schlaf und wird im Tick an der
    /// ersten Grenze danach `Runtime(Hardware)`; die Bilanz nennt sie. Ohne
    /// Setter schuetzt der Kern nichts und sagt es, und nach dem ersten Tick
    /// nimmt er keinen Schutz mehr an.
    #[test]
    fn the_host_protection_holds_the_state_shut_outside_the_writes() {
        let kicks = Cell::new(0u64);
        let watchdog = WatchdogOps { ctx: core::ptr::from_ref(&kicks).cast_mut().cast(), kick };
        let mut text = Vec::<u8>::new();
        let mut ring = [0u8; 512];
        let mut sink = MaybeUninit::<TraceSink>::uninit();
        let sink = sink.as_mut_ptr();
        let line = LineOps { ctx: (&raw mut text).cast(), try_write: put, flush: None };
        // SAFETY: Tabelle, Leitung und Ring leben bis zum Ende des Tests; kein Schritt laeuft.
        assert_eq!(unsafe { takt_sink_init(sink, &line, ring.as_mut_ptr(), 512) }, status::OK);
        let u = Unit::default();
        let protection = ProtectionOps { ctx: core::ptr::from_ref(&u).cast_mut().cast(), open, close, take_violation };
        let mut guarding = ops(false);
        guarding.idle = yes;
        guarding.deadline = far;
        guarding.hardware = hardware_fault;
        let mut guarded = Counter::default();
        let mut core = MaybeUninit::<Core>::uninit();
        let c = core.as_mut_ptr();
        // SAFETY: Kern, Tabelle, Arena, Senke, Watchdog und Einheit leben bis zum Ende des Tests.
        unsafe {
            assert_eq!(takt_core_init(c, &guarding, (&raw mut guarded).cast(), null_mut(), sink, 0, &watchdog), 0);
            assert_eq!(takt_core_protected(c), 0, "ohne Setter schuetzt der Kern nichts");
            assert_eq!(takt_core_protect(c, null()), status::NULL);
            assert_eq!(takt_core_protect(c, &protection), status::OK);
            assert_eq!(takt_core_protected(c), 1);
        }
        assert!(!u.open.get() && u.closed.get() == 1, "der Setter schliesst sofort");
        // SAFETY: wie oben; nie zwei Aufrufe zugleich, die Verletzung kommt dazwischen.
        unsafe {
            takt_core_service(c, 0);
            u.pending.set(Some(0x2000_0010));
            takt_core_service(c, 6 * TICK_NS);
            takt_core_service(c, 8 * TICK_NS);
        }
        assert!(u.opened.get() > 0 && !u.open.get(), "offen nur waehrend der Aufrufe");
        assert_eq!(u.opened.get() + 1, u.closed.get(), "jedes Oeffnen schliesst wieder");
        // Der Schutz ist eine Quelle von Weckereignissen: Die Schleife fragt
        // ihn im Schlaf an jeder Grenze, und die erste nach der Meldung
        // (1 ms, Tick 2 des Rahmens) bekommt sie.
        assert_eq!(guarded.hardware_in, Some(2), "die Verletzung wirkt an der ersten Grenze danach");
        // SAFETY: wie oben; danach die Bilanz, waehrend kein Schritt laeuft.
        unsafe {
            assert_eq!(takt_core_protect(c, &protection), status::STARTED, "nur vor dem ersten Tick");
            takt_core_report(c, null(), null());
        }
        let text = String::from_utf8_lossy(&text);
        assert!(text.contains("takt schutz verletzungen 1 region Programmzustand adresse 0x20000010"), "{text}");
    }

    /// **Zwei Kerne, zwei Senken** (12.11, FB-691): Zwei Programme in einem
    /// Wirt, jedes mit eigener Senke und abwechselnd gerechnet wie zwei Kerne
    /// eines Wirts; jede Senke traegt nur den Trace und die Bilanz ihres
    /// Programms.
    #[test]
    fn two_cores_write_into_their_own_sinks() {
        let kicks = Cell::new(0u64);
        let watchdog = WatchdogOps { ctx: core::ptr::from_ref(&kicks).cast_mut().cast(), kick };
        let (first, mut second) = (ops(false), ops(false));
        second.dump = dump_other;
        let (mut text_a, mut text_b) = (Vec::<u8>::new(), Vec::<u8>::new());
        let (mut ring_a, mut ring_b) = ([0u8; 256], [0u8; 256]);
        let (mut sink_a, mut sink_b) = (MaybeUninit::<TraceSink>::uninit(), MaybeUninit::<TraceSink>::uninit());
        let (sink_a, sink_b) = (sink_a.as_mut_ptr(), sink_b.as_mut_ptr());
        let line_a = LineOps { ctx: (&raw mut text_a).cast(), try_write: put, flush: None };
        let line_b = LineOps { ctx: (&raw mut text_b).cast(), try_write: put, flush: None };
        // SAFETY: Senken, Tabellen, Leitungen und Ringe leben bis zum Ende des Tests.
        unsafe {
            assert_eq!(takt_sink_init(sink_a, &line_a, ring_a.as_mut_ptr(), 256), status::OK);
            assert_eq!(takt_sink_init(sink_b, &line_b, ring_b.as_mut_ptr(), 256), status::OK);
        }
        let (mut arena_a, mut arena_b) = (Counter::default(), Counter::default());
        let (mut core_a, mut core_b) = (MaybeUninit::<Core>::uninit(), MaybeUninit::<Core>::uninit());
        let (a, b) = (core_a.as_mut_ptr(), core_b.as_mut_ptr());
        // SAFETY: Kerne, Tabellen, Arenen, Senken und Watchdog leben bis zum Ende des Tests.
        unsafe {
            assert_eq!(takt_core_init(a, &first, (&raw mut arena_a).cast(), null_mut(), sink_a, 0, &watchdog), 0);
            assert_eq!(takt_core_init(b, &second, (&raw mut arena_b).cast(), null_mut(), sink_b, 0, &watchdog), 0);
        }
        let mut now = 0;
        while now < TICKS * TICK_NS {
            // SAFETY: die Kerne von eben; nie zwei Aufrufe desselben Kerns zugleich.
            let (next_a, next_b) = unsafe { (takt_core_service(a, now), takt_core_service(b, now)) };
            now = next_a.deadline_ns.min(next_b.deadline_ns);
        }
        // SAFETY: Kein Schritt laeuft.
        unsafe {
            takt_core_report(a, null(), null());
            takt_core_report(b, null(), null());
        }
        let (text_a, text_b) = (String::from_utf8_lossy(&text_a), String::from_utf8_lossy(&text_b));
        assert!(text_a.lines().any(|l| l == "t= dump") && !text_a.contains("other"), "{text_a}");
        assert!(text_b.lines().any(|l| l == "t= dump other") && !text_b.lines().any(|l| l == "t= dump"), "{text_b}");
        assert!(text_a.contains("takt end") && text_b.contains("takt end"), "je Senke die Bilanz ihres Kerns");
    }
}
