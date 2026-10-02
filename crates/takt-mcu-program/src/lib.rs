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
    fn app_init(user: *mut c_void);
    fn app_init_with(user: *mut c_void, persist: *const c_void, len: i32) -> i32;
    fn app_tick(k: i64);
    fn app_overrun();
    fn app_hardware();
    fn app_tolerance(ns: *mut i64, runs: *mut u32);
    fn app_dump(all: i32);
    fn app_pc();
    fn app_output(index: i32) -> i64;
    fn app_commit();
    fn app_idle() -> u8;
    fn app_deadline() -> i64;
    fn app_advance(n: i64);
    fn app_persist_snapshot(out: *mut c_void, cap: i32) -> i32;
    fn app_persist_restore(bytes: *const c_void, len: i32) -> i32;
    fn app_next_run(delay: *mut i64) -> i32;
    fn app_end();
    fn app_job_dispatch() -> i32;
    fn app_job_work();
    fn app_job_stack(size: *mut u32) -> *mut u8;
}

/// Die Jobs des Rahmens (4.5) fuer den Job-Kontext eines Boards: Die
/// Hauptschleife gibt Auftraege, der Kontext rechnet sie.
pub mod jobs {
    use core::sync::atomic::{AtomicBool, Ordering};

    /// Gibt dem ruhenden Job-Kontext den aeltesten wartenden Job; wahr,
    /// wenn er etwas zu rechnen hat. Nur aus der Hauptschleife.
    pub fn dispatch() -> bool {
        // SAFETY: Slots und Auftrag liegen statisch im Rahmen; die
        // Hauptschleife schreibt den Auftrag nur, solange der Kontext ruht.
        unsafe { super::app_job_dispatch() != 0 }
    }

    /// Rechnet den Auftrag. Nur im Job-Kontext.
    pub fn work() {
        // SAFETY: Den Auftrag fasst die Hauptschleife nicht an, bis der
        // Kontext ihn als fertig meldet.
        unsafe { super::app_job_work() }
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
    /// Das Treiberobjekt, das der Rahmen an jeden Treiber reicht.
    drivers: *mut c_void,
}

impl Generated {
    /// Ein Programm vor Tick 0: das Journal darf noch laden.
    ///
    /// # Safety
    ///
    /// `drivers` zeigt auf das Treiberobjekt, fuer das der Kleber des
    /// Programms erzeugt ist, und lebt so lange wie das Programm.
    pub unsafe fn new(drivers: *mut c_void) -> Generated {
        Generated { initialized: false, drivers }
    }

    /// Initialisiert das Programm ohne Journal (Tick 0, 9.4).
    ///
    /// # Safety
    ///
    /// Wie [`Generated::new`].
    pub unsafe fn init(drivers: *mut c_void) -> Generated {
        // SAFETY: siehe oben.
        let mut p = unsafe { Generated::new(drivers) };
        p.ensure_init();
        p
    }

    /// Tick 0 ohne geladene Werte, wenn das Journal keine hatte.
    pub fn ensure_init(&mut self) {
        if !self.initialized {
            // SAFETY: einmal vor dem ersten Tick; der Rahmen haelt seinen Zustand statisch.
            unsafe { app_init(self.drivers) };
            self.initialized = true;
        }
    }

    /// Der Wert eines Outputs, als Bitmuster.
    pub fn output(&self, index: i32) -> i64 {
        // SAFETY: liest einen Latch des Rahmens; ein fremder Index liefert 0.
        unsafe { app_output(index) }
    }
}

impl Program for Generated {
    fn raise_overrun(&mut self) {
        // SAFETY: setzt ein Flag des Rahmens; der naechste Tick stellt den
        // Fault zu (7.3).
        unsafe { app_overrun() };
    }

    fn raise_hardware(&mut self) {
        // SAFETY: setzt ein Flag des Rahmens; der naechste Tick stellt den
        // Fault zu (12.3).
        unsafe { app_hardware() };
    }

    fn tick_tolerance(&self) -> Option<Tolerance> {
        let (mut ns, mut runs) = (0i64, 0u32);
        // SAFETY: der Rahmen schreibt zwei Zahlen an die uebergebenen Stellen.
        unsafe { app_tolerance(&mut ns, &mut runs) };
        Some(Tolerance { ns, runs })
    }

    fn sleep_allowed(&self) -> bool {
        // SAFETY: liest nur den statischen Zustand des Rahmens.
        unsafe { app_idle() != 0 }
    }

    fn next_deadline(&self) -> Option<i64> {
        // SAFETY: liest nur den statischen Zustand des Rahmens.
        let ns = unsafe { app_deadline() };
        (ns >= 0).then_some(ns)
    }

    fn advance(&mut self, ticks: u64) {
        // SAFETY: der Rahmen rueckt seine Uhr vor; die Schleife ruft es nur im Schlaf.
        unsafe { app_advance(ticks as i64) };
    }

    fn persist_snapshot(&mut self, out: &mut [u8]) -> usize {
        let cap = i32::try_from(out.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen schreibt hoechstens `cap` Byte an `out`.
        let n = unsafe { app_persist_snapshot(out.as_mut_ptr().cast(), cap) };
        usize::try_from(n).unwrap_or(0)
    }

    fn persist_restore(&mut self, bytes: &[u8]) -> usize {
        let len = i32::try_from(bytes.len()).unwrap_or(i32::MAX);
        // SAFETY: der Rahmen liest `len` Byte ab `bytes`; vor dem Start
        // initialisiert er sich damit, danach ersetzt er nur die Werte.
        let n = unsafe {
            if self.initialized {
                app_persist_restore(bytes.as_ptr().cast(), len)
            } else {
                self.initialized = true;
                app_init_with(self.drivers, bytes.as_ptr().cast(), len)
            }
        };
        usize::try_from(n).unwrap_or(0)
    }

    fn next_run(&self) -> Option<NextRun> {
        let mut delay = 0i64;
        // SAFETY: liest nur den statischen Zustand des Rahmens und schreibt `delay`.
        match unsafe { app_next_run(&mut delay) } {
            1 => Some(NextRun::Now),
            2 => Some(NextRun::After(delay)),
            3 => Some(NextRun::OnWake),
            4 => Some(NextRun::OnStart),
            _ => None,
        }
    }

    fn tick(&mut self, k: u64, _now: i64) {
        // Die Schleife zaehlt ihre Schritte ab 0; Rahmen und Interpreter
        // nennen den ersten Tick nach dem Start `t=1` (Tick 0 ist der Start).
        // SAFETY: ein Schritt des Rahmens, einmal je Tick aus der Schleife.
        unsafe { app_tick(k as i64 + 1) };
    }

    fn commit(&mut self) {
        // SAFETY: gibt die Latches des Rahmens an die Treiber; der Kern ruft es einmal je Tick.
        unsafe { app_commit() };
    }

    fn trace(&mut self, outputs: Outputs) {
        let all = match outputs {
            Outputs::None => return,
            Outputs::Changed => 0,
            Outputs::All => 1,
        };
        // SAFETY: liest den statischen Zustand des Rahmens und schreibt den Trace.
        unsafe {
            app_dump(all);
            app_pc();
        }
    }

    fn end(&mut self) {
        // SAFETY: schreibt die Zeile `end` und die `safe`-Werte in den Latch des Rahmens.
        unsafe { app_end() };
    }

    fn dispatch_job(&mut self) -> bool {
        jobs::dispatch()
    }
}
