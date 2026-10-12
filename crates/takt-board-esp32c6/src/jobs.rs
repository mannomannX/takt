//! Der Job-Kontext (4.5, 12.3): ein zweiter Faden mit eigenem Stack, den
//! der Tick unterbricht.
//!
//! Umgeschaltet wird im Software-Interrupt `FROM_CPU_INTR0` mit der
//! niedrigsten Prioritaet: Er nimmt den Kern erst, wenn kein anderer
//! Interrupt mehr aktiv ist. `esp-hal` bindet ihn direkt an eine
//! CPU-Unterbrechung (`enable_direct`), ohne eigenen Verteiler: ein roher
//! Trap-Handler, der alle Register des laufenden Fadens samt `mepc` und
//! `mstatus` in eine statische Tabelle sichert, die Quelle loescht und den
//! anderen Faden laedt. Die Hauptschleife gibt den Kern in ihrer Wartezeit
//! ab ([`JobContext::resume`]), der Tick holt ihn zurueck ([`preempt`]), und
//! ein Job, der nichts mehr zu rechnen hat, gibt ihn selbst zurueck.
//!
//! **Interrupts laufen auf dem Stack des unterbrochenen Fadens.** Anders
//! als auf dem Cortex-M gibt es keinen eigenen Interrupt-Stack; der
//! Job-Stack traegt darum die Reserve der Interrupts mit
//! (`job_stack_reserve` der Hardware-Konfiguration, 8.10).
//!
//! **Das Programm bringt das Bring-up mit** (12.11): den Stack aus der
//! Lieferform (`job_stack`), den Griff, der verteilt, und den Griff, mit dem
//! der Faden rechnet (`takt_embed::{Dispatch, Jobs}`).

use core::ptr::write_volatile;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use esp_hal::interrupt::{DirectBindableCpuInterrupt, Priority};
use esp_hal::peripherals::{INTPRI, Interrupt};
use takt_embed::{Context, Dispatch, Jobs};

/// Die Register eines Fadens, wie der Trap-Handler sie ablegt: `ra`, `sp`,
/// `t0`–`t2`, `s0`, `s1`, `a0`–`a7`, `s2`–`s11`, `t3`–`t6`, `mepc`,
/// `mstatus`. `gp` und `tp` teilen sich beide Faeden.
type Thread = [u32; 31];

const SP: usize = 1;
const MEPC: usize = 29;
const MSTATUS: usize = 30;

/// Was der Trap-Handler liest und schreibt; die Versaetze stehen dort:
/// `cur` bei 0, `next` bei 4, die Adresse der Quelle bei 8.
#[repr(C)]
struct Switch {
    cur: *mut Thread,
    next: *mut Thread,
    source: *mut u32,
}

#[unsafe(no_mangle)]
static mut TAKT_JOB_SWITCH: Switch =
    Switch { cur: core::ptr::null_mut(), next: core::ptr::null_mut(), source: core::ptr::null_mut() };

static mut MAIN: Thread = [0; 31];
static mut JOB: Thread = [0; 31];

/// Ob es einen Job-Faden gibt; ohne ihn bleibt der Tick-Interrupt, wie er war.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Der Waechter am unteren Ende des Job-Stacks (12.3): die 32 Byte, die der
/// Rahmen dafuer bemisst (`takt_frame::mcu::job_stack_bytes`). Gemalt und
/// gelesen wird erst darueber.
const GUARD: usize = 32;

/// Unteres und oberes Ende des gemalten Job-Stacks, fuer [`high_water`].
static PAINTED: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Malt den Job-Stack ueber dem Waechter, bevor der Faden ihn benutzt, und
/// merkt sich seine Grenzen.
fn paint(stack: &mut [u8]) {
    let top = stack.as_ptr_range().end.expose_provenance();
    let from = GUARD.min(stack.len());
    let painted = stack.get_mut(from..).unwrap_or_default();
    let bottom = painted.as_ptr().expose_provenance();
    for word in painted.chunks_exact_mut(4) {
        word.copy_from_slice(&crate::stack::PATTERN.to_ne_bytes());
    }
    PAINTED[0].store(crate::word(bottom), Ordering::SeqCst);
    PAINTED[1].store(crate::word(top), Ordering::SeqCst);
}

/// Wie tief der Job-Stack bisher reichte, in Byte vom oberen Ende (12.3,
/// 13.8); `None` ohne Job-Faden. Nur, wenn der Faden nicht rechnet: nach dem
/// Lauf oder aus dem Kontext des Schritts.
pub fn high_water() -> Option<u32> {
    if !STARTED.load(Ordering::SeqCst) {
        return None;
    }
    let (bottom, top) = (PAINTED[0].load(Ordering::SeqCst), PAINTED[1].load(Ordering::SeqCst));
    Some(crate::stack::depth(crate::address(bottom), crate::address(top)))
}

/// Der Griff, mit dem der Faden des Jobs rechnet: vor `STARTED` gesetzt,
/// danach nur von diesem Faden benutzt.
static mut JOBS: Option<&'static mut dyn Jobs> = None;

/// `mstatus`: `MPIE` (nach `mret` Interrupts an) und `MPP` = Maschinenmodus.
const MPIE: u32 = 1 << 7;
const MPP_MACHINE: u32 = 3 << 11;
const MIE: u32 = 1 << 3;

// Der Trap-Handler, im RAM wie alles, was waehrend eines
// Flash-Schreibvorgangs laufen kann (12.3).
core::arch::global_asm!(
    ".section .rwtext.takt_job_switch,\"ax\"",
    ".global takt_job_switch",
    ".balign 4",
    "takt_job_switch:",
    "    csrw  mscratch, t0",
    "    la    t0, TAKT_JOB_SWITCH",
    "    lw    t0, 0(t0)",
    "    sw    ra, 0(t0)",
    "    sw    sp, 4(t0)",
    "    sw    t1, 12(t0)",
    "    sw    t2, 16(t0)",
    "    sw    s0, 20(t0)",
    "    sw    s1, 24(t0)",
    "    sw    a0, 28(t0)",
    "    sw    a1, 32(t0)",
    "    sw    a2, 36(t0)",
    "    sw    a3, 40(t0)",
    "    sw    a4, 44(t0)",
    "    sw    a5, 48(t0)",
    "    sw    a6, 52(t0)",
    "    sw    a7, 56(t0)",
    "    sw    s2, 60(t0)",
    "    sw    s3, 64(t0)",
    "    sw    s4, 68(t0)",
    "    sw    s5, 72(t0)",
    "    sw    s6, 76(t0)",
    "    sw    s7, 80(t0)",
    "    sw    s8, 84(t0)",
    "    sw    s9, 88(t0)",
    "    sw    s10, 92(t0)",
    "    sw    s11, 96(t0)",
    "    sw    t3, 100(t0)",
    "    sw    t4, 104(t0)",
    "    sw    t5, 108(t0)",
    "    sw    t6, 112(t0)",
    "    csrr  t1, mscratch",
    "    sw    t1, 8(t0)",
    "    csrr  t1, mepc",
    "    sw    t1, 116(t0)",
    "    csrr  t1, mstatus",
    "    sw    t1, 120(t0)",
    "    la    t1, TAKT_JOB_SWITCH",
    "    lw    t2, 8(t1)",
    "    sw    zero, 0(t2)",
    "    lw    t0, 4(t1)",
    "    sw    t0, 0(t1)",
    "    lw    t1, 116(t0)",
    "    csrw  mepc, t1",
    "    lw    t1, 120(t0)",
    "    csrw  mstatus, t1",
    "    lw    ra, 0(t0)",
    "    lw    sp, 4(t0)",
    "    lw    t1, 12(t0)",
    "    lw    t2, 16(t0)",
    "    lw    s0, 20(t0)",
    "    lw    s1, 24(t0)",
    "    lw    a0, 28(t0)",
    "    lw    a1, 32(t0)",
    "    lw    a2, 36(t0)",
    "    lw    a3, 40(t0)",
    "    lw    a4, 44(t0)",
    "    lw    a5, 48(t0)",
    "    lw    a6, 52(t0)",
    "    lw    a7, 56(t0)",
    "    lw    s2, 60(t0)",
    "    lw    s3, 64(t0)",
    "    lw    s4, 68(t0)",
    "    lw    s5, 72(t0)",
    "    lw    s6, 76(t0)",
    "    lw    s7, 80(t0)",
    "    lw    s8, 84(t0)",
    "    lw    s9, 88(t0)",
    "    lw    s10, 92(t0)",
    "    lw    s11, 96(t0)",
    "    lw    t3, 100(t0)",
    "    lw    t4, 104(t0)",
    "    lw    t5, 108(t0)",
    "    lw    t6, 112(t0)",
    "    lw    t0, 8(t0)",
    "    mret",
);

unsafe extern "C" {
    fn takt_job_switch();
}

/// Der Job-Faden des Boards; `D` verteilt die Auftraege.
#[derive(Debug)]
pub struct JobContext<D> {
    bottom: u32,
    dispatch: D,
}

/// Der Waechter unter dem Job-Stack (12.3): die 32 Byte, die der Rahmen am
/// unteren Ende reserviert, als Daten-Watchpoint fuer Schreibzugriffe auf
/// Trigger 2 — `esp-hal` belegt 0 (Waechter des Hauptstacks) und 1. Ein
/// Treffer haelt den Kern mit `Breakpoint exception` und der Adresse in
/// `mtval` an; die Felder setzt er wie `esp_hal::debugger`.
fn watch(bottom: u32) {
    // NAPOT: Die unteren Bits der Adresse tragen die Laenge, 0b01111 fuer 32 Byte.
    let tdata2 = bottom | 0b0_1111;
    // `tdata1`: NAPOT (Bit 7), Maschinenmodus (Bit 6), Schreiben (Bit 1).
    let tdata1: u32 = 1 << 7 | 1 << 6 | 1 << 1;
    // `tcontrol`: Trigger im Maschinenmodus an (`mte`, Bit 3).
    let tcontrol: u32 = 1 << 3;
    // SAFETY: Trigger 2 benutzt sonst niemand; `tselect` waehlt jeder Leser
    // selbst, bevor er `tdata1` liest.
    unsafe {
        core::arch::asm!(
            "csrw 0x7a0, {id}",
            "csrw 0x7a5, {tcontrol}",
            "csrw 0x7a1, {tdata1}",
            "csrw 0x7a2, {tdata2}",
            id = in(reg) 2u32,
            tcontrol = in(reg) tcontrol,
            tdata1 = in(reg) tdata1,
            tdata2 = in(reg) tdata2,
        );
    }
}

/// Nur aus der Hauptschleife, in ihrer Wartezeit; `finish` dort, wo sie
/// warten darf (13.8).
impl<D: Dispatch> Context for JobContext<D> {
    fn run(&mut self) {
        if self.dispatch.next() {
            self.resume();
        }
    }

    fn finish(&mut self) {
        while self.dispatch.next() {
            self.resume();
        }
    }
}

impl<D: Dispatch> JobContext<D> {
    /// Legt den Faden des Jobs auf `stack` an und bindet die Umschaltung;
    /// der Faden beginnt in [`worker`], sobald die Hauptschleife ihn zum
    /// ersten Mal rechnen laesst.
    fn on(stack: &'static mut [u8], dispatch: D) -> JobContext<D> {
        paint(stack);
        let bottom = crate::word(stack.as_ptr().expose_provenance());
        watch(bottom);
        let top = stack.as_mut_ptr_range().end.expose_provenance() & !15;
        let mut mstatus: u32;
        // SAFETY: liest nur das Statusregister des Kerns.
        unsafe { core::arch::asm!("csrr {0}, mstatus", out(reg) mstatus) };
        mstatus = (mstatus & !MIE) | MPIE | MPP_MACHINE;
        let mut job: Thread = [0; 31];
        job[SP] = crate::word(top);
        job[MEPC] = crate::word(worker_address());
        job[MSTATUS] = mstatus;
        // Vor dem ersten Wechsel liest niemand die Tabelle; die Unterbrechung
        // wird erst danach gebunden. Was folgt, schreibt allein.
        let (main, slot) = (&raw mut MAIN, &raw mut JOB);
        // SAFETY: die Register des Job-Fadens, siehe oben.
        unsafe { write_volatile(slot, job) };
        // SAFETY: bildet nur die Adresse eines Felds der Statik.
        let cur = unsafe { &raw mut TAKT_JOB_SWITCH.cur };
        // SAFETY: Es laeuft die Hauptschleife, siehe oben.
        unsafe { write_volatile(cur, main) };
        // SAFETY: Ohne Wunsch bleibt sie es, siehe oben.
        unsafe { write_volatile(next(), main) };
        // SAFETY: bildet nur die Adresse eines Felds der Statik.
        let source = unsafe { &raw mut TAKT_JOB_SWITCH.source };
        // SAFETY: die Quelle, die der Handler loescht, siehe oben.
        unsafe { write_volatile(source, INTPRI::regs().cpu_intr_from_cpu(0).as_ptr()) };
        esp_hal::interrupt::enable_direct(
            Interrupt::FROM_CPU_INTR0,
            Priority::Priority1,
            DirectBindableCpuInterrupt::Interrupt0,
            takt_job_switch,
        );
        STARTED.store(true, Ordering::SeqCst);
        JobContext { bottom, dispatch }
    }

    /// Der Job-Faden des Programms auf `stack`, den der Rahmen fuer ihn
    /// bemisst. Nur ein Programm mit Jobs bekommt ihn (`takt_embed::Work`,
    /// FB-385).
    pub fn start(stack: &'static mut [u8], dispatch: D, jobs: &'static mut dyn Jobs) -> JobContext<D> {
        // SAFETY: einmal vor dem Faden, der es liest; `on` gibt ihn danach mit
        // `STARTED` frei.
        unsafe { JOBS = Some(jobs) };
        JobContext::on(stack, dispatch)
    }

    /// Das untere Ende des Job-Stacks, wo der Waechter liegt (12.3).
    pub fn bottom(&self) -> u32 {
        self.bottom
    }

    /// Laesst den Job rechnen, bis er abgibt oder der naechste Tick ihn
    /// unterbricht.
    fn resume(&mut self) {
        switch_to(&raw mut JOB);
    }
}

/// Aus dem Tick-Interrupt: Nach ihm gehoert der Kern der Hauptschleife.
///
/// Ohne Bedingung, damit kein Tick zwischen dem Wunsch der Hauptschleife
/// und dem Wechsel verloren geht: Rechnet der Job nicht, laedt der Handler
/// die Hauptschleife, die schon lief.
#[esp_hal::ram]
pub fn preempt() {
    if STARTED.load(Ordering::SeqCst) {
        let main = &raw mut MAIN;
        // SAFETY: ein Wort, das der Handler bei gesperrten Interrupts liest.
        unsafe { write_volatile(next(), main) };
        raise();
    }
}

/// Das Feld `next` der Tabelle, ohne es zu lesen; jeder Zugriff darauf ist
/// volatil, denn der Handler liest es.
#[esp_hal::ram]
fn next() -> *mut *mut Thread {
    // SAFETY: bildet nur die Adresse eines Felds der Statik.
    unsafe { &raw mut TAKT_JOB_SWITCH.next }
}

/// Wechselt zu `thread` und kehrt zurueck, wenn der Kern wieder hier ist.
fn switch_to(thread: *mut Thread) {
    // SAFETY: ein Wort, das der Handler bei gesperrten Interrupts liest.
    unsafe { write_volatile(next(), thread) };
    raise();
    // Der Handler loescht die Quelle beim Wechsel; steht sie nicht mehr,
    // ist dieser Faden schon wieder dran.
    while INTPRI::regs().cpu_intr_from_cpu(0).read().cpu_intr().bit_is_set() {}
}

#[esp_hal::ram]
fn raise() {
    INTPRI::regs().cpu_intr_from_cpu(0).write(|w| w.cpu_intr().set_bit());
}

/// Der Faden des Jobs: rechnet, was die Hauptschleife ihm gibt, und gibt
/// den Kern zurueck.
extern "C" fn worker() -> ! {
    loop {
        let slot = &raw mut JOBS;
        // SAFETY: `start` setzte es vor dem ersten Wechsel hierher; danach
        // benutzt es nur dieser Faden.
        if let Some(jobs) = unsafe { slot.as_mut() }.and_then(Option::as_deref_mut) {
            jobs.work();
        }
        switch_to(&raw mut MAIN);
    }
}

/// Die Adresse von [`worker`]: Der Handler laedt sie beim ersten Wechsel in
/// den Job-Faden als `mepc`.
#[expect(
    clippy::fn_to_numeric_cast_any,
    clippy::as_conversions,
    reason = "MISRA C:2025 R.11.1: die Register des Fadens nennen seinen Einsprung als Adresse; die einer Funktion liefert nur `as`"
)]
fn worker_address() -> usize {
    worker as extern "C" fn() -> ! as usize
}
