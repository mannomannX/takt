//! Das erzeugte Programm hinter seiner C-ABI (12.1, 12.3), einmal fuer
//! alle Boards.
//!
//! Der Rahmen aus `takt_frame::mcu` liefert `app_init_with`, `app_tick`
//! und die Schwestern — `app` ist das Praefix der eigenen Bring-ups
//! (`takt_llvm::symbols::Prefix::default`, 12.11); hier werden sie zum
//! [`takt_rt_core::Program`], das der Kern kennt. Ein Board bringt nur
//! noch Uhr, Leitung und Treiber mit.
//!
//! **Laden vor dem Eintritt.** s0 enthaelt die geladenen `persist`-Werte
//! (5.9), also muss das Journal *vor* dem ersten `enter` gelesen sein. Das
//! Programm beginnt darum uninitialisiert; die erste `persist_restore`
//! initialisiert es mit den Bytes, und [`Generated::ensure_init`] holt den
//! Start ohne Journal nach, wenn keines da war.
//!
//! **Die Natives** ruft der erzeugte Code als `takt_native_*`; sie kommen
//! aus `takt-native-abi`, derselben Rechnung wie im Interpreter (FB-293).
//!
//! **Der Wirtszeiger** ist das Treiberobjekt des Bring-ups: Der Rahmen
//! reicht ihn an jeden Treiber, und der erzeugte Kleber ruft darauf die
//! Methode des Traits `Drivers` (12.6).
//!
//! **Die Gegenrichtung fehlt mit Absicht.** Der Rahmen ruft
//! `takt_board_trace*`, um Traces auszugeben; die stellt das Programm, das
//! ihn bindet — es weiss, wohin die Telemetrie geht.

#![no_std]
#![allow(unsafe_code, reason = "C-ABI des erzeugten Rahmens; 9.5 fuehrt ihn in der TCB")]

use core::ffi::c_void;

use takt_native_abi as _;
use takt_rt_core::{NextRun, Outputs, Program, Tolerance};

// TODO(M11 Schritt 10): Das erzeugte Rust-Modul der Lieferform ersetzt diese
// Liste und das feste Praefix.
unsafe extern "C" {
    fn app_init(arena: *mut c_void, user: *mut c_void);
    fn app_init_with(arena: *mut c_void, user: *mut c_void, persist: *const c_void, len: i32) -> i32;
    fn app_tick(arena: *mut c_void, k: i64);
    fn app_overrun(arena: *mut c_void);
    fn app_hardware(arena: *mut c_void);
    fn app_tolerance(ns: *mut i64, runs: *mut u32);
    fn app_output_timing() -> u8;
    fn app_dump(arena: *mut c_void, all: i32);
    fn app_pc(arena: *mut c_void);
    fn app_output(arena: *mut c_void, index: i32) -> i64;
    fn app_commit(arena: *mut c_void);
    fn app_idle(arena: *mut c_void) -> u8;
    fn app_deadline(arena: *mut c_void) -> i64;
    fn app_advance(arena: *mut c_void, n: i64);
    fn app_tune(arena: *mut c_void, param: u32, value: *const c_void, len: i32) -> i32;
    fn app_persist_snapshot(arena: *mut c_void, out: *mut c_void, cap: i32) -> i32;
    fn app_persist_restore(arena: *mut c_void, bytes: *const c_void, len: i32) -> i32;
    fn app_next_run(arena: *mut c_void, delay: *mut i64) -> i32;
    fn app_end(arena: *mut c_void);
    fn app_job_dispatch(arena: *mut c_void) -> i32;
    fn app_job_work(arena: *mut c_void);
    fn app_job_stack(size: *mut u32) -> *mut u8;
}

/// Die Jobs des Rahmens (4.5) fuer den Job-Kontext eines Boards: Die
/// Hauptschleife gibt Auftraege, der Kontext rechnet sie.
pub mod jobs {
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicBool, Ordering};

    /// Die Jobs eines Programms: der Griff auf seine Arena, in der Slots und
    /// Auftrag liegen. Kopierbar, damit der Job-Kontext ihn mitnimmt.
    #[derive(Clone, Copy, Debug)]
    pub struct Jobs {
        arena: *mut c_void,
    }

    // SAFETY: Hauptschleife und Job-Kontext teilen die Arena nach dem Protokoll
    // des Rahmens: Den Auftrag schreibt die Hauptschleife nur, solange der
    // Kontext ruht, und liest ihn erst, wenn er fertig gemeldet ist.
    unsafe impl Send for Jobs {}

    impl Jobs {
        /// Die Jobs des Programms auf `arena`, fuer einen Job-Kontext, der vor
        /// dem Programm entsteht.
        ///
        /// # Safety
        ///
        /// Wie [`super::Generated::new`]: `arena` ist die Arena des Programms
        /// und lebt so lange wie es.
        pub unsafe fn new(arena: *mut c_void) -> Jobs {
            Jobs { arena }
        }

        /// Gibt dem ruhenden Job-Kontext den aeltesten wartenden Job; wahr,
        /// wenn er etwas zu rechnen hat. Nur aus der Hauptschleife.
        pub fn dispatch(self) -> bool {
            // SAFETY: siehe `Send`; die Arena lebt so lange wie das Programm.
            unsafe { super::app_job_dispatch(self.arena) != 0 }
        }

        /// Rechnet den Auftrag. Nur im Job-Kontext.
        pub fn work(self) {
            // SAFETY: wie oben.
            unsafe { super::app_job_work(self.arena) }
        }
    }

    /// Der Stack des Job-Kontexts: so gross wie der groesste `stack`-Vertrag
    /// der Jobs plus Reserve. `None` ohne Jobs und bei jedem weiteren Aufruf.
    pub fn stack() -> Option<&'static mut [u8]> {
        static TAKEN: AtomicBool = AtomicBool::new(false);
        if TAKEN.swap(true, Ordering::Relaxed) {
            return None;
        }
        let mut size = 0u32;
        // SAFETY: Der Rahmen liefert einen statischen Puffer dieser
        // Groesse, und `TAKEN` gibt ihn hoechstens einmal heraus.
        let at = unsafe { super::app_job_stack(&mut size) };
        (size > 0 && !at.is_null()).then(|| unsafe { core::slice::from_raw_parts_mut(at, size as usize) })
    }
}

/// Das gebundene Programm.
#[derive(Clone, Copy, Debug)]
pub struct Generated {
    initialized: bool,
    /// Die Arena des Programms (12.11), die das Bring-up anlegt.
    arena: *mut c_void,
    /// Das Treiberobjekt, das der Rahmen an jeden Treiber reicht.
    drivers: *mut c_void,
}

impl Generated {
    /// Ein Programm vor Tick 0: das Journal darf noch laden.
    ///
    /// # Safety
    ///
    /// `arena` zeigt auf die Arena des Programms, so gross und ausgerichtet,
    /// wie der Rahmen sie verlangt, und wird nach `init` nicht bewegt;
    /// `drivers` zeigt auf das Treiberobjekt, fuer das der Kleber des
    /// Programms erzeugt ist. Beide leben so lange wie das Programm, und
    /// niemand sonst greift auf die Arena zu.
    pub unsafe fn new(arena: *mut c_void, drivers: *mut c_void) -> Generated {
        Generated { initialized: false, arena, drivers }
    }

    /// Initialisiert das Programm ohne Journal (Tick 0, 9.4).
    ///
    /// # Safety
    ///
    /// Wie [`Generated::new`].
    pub unsafe fn init(arena: *mut c_void, drivers: *mut c_void) -> Generated {
        // SAFETY: siehe oben.
        let mut p = unsafe { Generated::new(arena, drivers) };
        p.ensure_init();
        p
    }

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    pub fn ensure_init(&mut self) {
        if !self.initialized {
            // SAFETY: einmal vor dem ersten Tick, auf der Arena aus `new`.
            unsafe { app_init(self.arena, self.drivers) };
            self.initialized = true;
        }
    }

    /// Der Wert eines Outputs, als Bitmuster.
    pub fn output(&self, index: i32) -> i64 {
        // SAFETY: liest einen Latch der Arena; ein fremder Index liefert 0.
        unsafe { app_output(self.arena, index) }
    }

    /// Die Jobs des Programms, fuer den Job-Kontext des Boards (4.5).
    pub fn jobs(&self) -> jobs::Jobs {
        // SAFETY: die Arena aus `new`, mit derselben Lebensdauer.
        unsafe { jobs::Jobs::new(self.arena) }
    }
}

impl Program for Generated {
    fn raise_overrun(&mut self) {
        // SAFETY: setzt ein Flag des Rahmens; der naechste Tick stellt den
        // Fault zu (7.3).
        unsafe { app_overrun(self.arena) };
    }

    fn raise_hardware(&mut self) {
        // SAFETY: setzt ein Flag des Rahmens; der naechste Tick stellt den
        // Fault zu (12.3).
        unsafe { app_hardware(self.arena) };
    }

    fn tick_tolerance(&self) -> Option<Tolerance> {
        let (mut ns, mut runs) = (0i64, 0u32);
        // SAFETY: der Rahmen schreibt zwei Zahlen an die uebergebenen Stellen.
        unsafe { app_tolerance(&mut ns, &mut runs) };
        Some(Tolerance { ns, runs })
    }

    fn commit_at_boundary(&self) -> bool {
        // SAFETY: liest eine Konstante des Rahmens.
        unsafe { app_output_timing() == 1 }
    }

    fn sleep_allowed(&self) -> bool {
        // SAFETY: liest nur die Arena.
        unsafe { app_idle(self.arena) != 0 }
    }

    fn next_deadline(&self) -> Option<i64> {
        // SAFETY: liest nur die Arena.
        let ns = unsafe { app_deadline(self.arena) };
        (ns >= 0).then_some(ns)
    }

    fn advance(&mut self, ticks: u64) {
        // SAFETY: der Rahmen rueckt seine Uhr vor; die Schleife ruft es nur im Schlaf.
        unsafe { app_advance(self.arena, ticks as i64) };
    }

    fn tune(&mut self, param: u32, value: &[u8]) {
        let len = i32::try_from(value.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen liest `len` Byte ab `value` (8.4).
        let _ = unsafe { app_tune(self.arena, param, value.as_ptr().cast(), len) };
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        let cap = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen schreibt hoechstens `cap` Byte an `out`.
        let n = unsafe { app_persist_snapshot(self.arena, out.as_mut_ptr().cast(), cap) };
        usize::try_from(n).unwrap_or(0)
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen liest `len` Byte ab `bytes`; vor dem Start
        // initialisiert er sich damit, danach ersetzt er nur die Werte.
        let n = unsafe {
            if self.initialized {
                app_persist_restore(self.arena, bytes.as_ptr().cast(), len)
            } else {
                self.initialized = true;
                app_init_with(self.arena, self.drivers, bytes.as_ptr().cast(), len)
            }
        };
        usize::try_from(n).unwrap_or(0)
    }

    fn next_run(&self) -> Option<NextRun> {
        let mut delay = 0i64;
        // SAFETY: liest nur die Arena und schreibt `delay`.
        let code = unsafe { app_next_run(self.arena, &mut delay) };
        NextRun::from_code(code, delay)
    }

    fn tick(&mut self, k: u64, _now: i64) {
        // Die Schleife zaehlt ihre Schritte ab 0; Rahmen und Interpreter
        // nennen den ersten Tick nach dem Start `t=1` (Tick 0 ist der Start).
        // SAFETY: ein Schritt des Rahmens, einmal je Tick aus der Schleife.
        unsafe { app_tick(self.arena, k as i64 + 1) };
    }

    fn commit(&mut self) {
        // SAFETY: gibt die Latches des Rahmens an die Treiber; der Kern ruft es einmal je Tick.
        unsafe { app_commit(self.arena) };
    }

    fn trace(&mut self, outputs: Outputs) {
        let all = match outputs {
            Outputs::None => return,
            Outputs::Changed => 0,
            Outputs::All => 1,
        };
        // SAFETY: liest die Arena und schreibt den Trace.
        unsafe {
            app_dump(self.arena, all);
            app_pc(self.arena);
        }
    }

    fn end(&mut self) {
        // SAFETY: schreibt die Zeile `end` und die `safe`-Werte in den Latch des Rahmens.
        unsafe { app_end(self.arena) };
    }

    fn dispatch_job(&mut self) -> bool {
        self.jobs().dispatch()
    }
}
