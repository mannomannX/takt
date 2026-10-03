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
//! Job-Stack traegt darum die Reserve der Interrupts mit (Build-Skript des
//! Bring-ups, `TAKT_JOB_STACK_RESERVE`).

use core::ptr::write_volatile;
use core::sync::atomic::{AtomicBool, Ordering};

use esp_hal::interrupt::{DirectBindableCpuInterrupt, Priority};
use esp_hal::peripherals::{INTPRI, Interrupt};
use takt_mcu_program::jobs::Jobs;

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

/// Die Jobs des Programms fuer den Faden des Jobs: vor `STARTED` gesetzt,
/// danach nur gelesen.
static mut JOBS: Option<Jobs> = None;

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

/// Der Job-Faden des Boards.
#[derive(Debug)]
pub struct JobContext {
    bottom: u32,
    jobs: Jobs,
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

impl JobContext {
    /// Legt den Faden des Jobs auf `stack` an und bindet die Umschaltung;
    /// der Faden beginnt in [`worker`], sobald die Hauptschleife ihn zum
    /// ersten Mal rechnen laesst.
    fn on(stack: &'static mut [u8], jobs: Jobs) -> JobContext {
        let bottom = stack.as_ptr() as u32;
        watch(bottom);
        let top = (stack.as_mut_ptr() as usize + stack.len()) & !15;
        let mut mstatus: u32;
        // SAFETY: liest nur das Statusregister des Kerns.
        unsafe { core::arch::asm!("csrr {0}, mstatus", out(reg) mstatus) };
        mstatus = (mstatus & !MIE) | MPIE | MPP_MACHINE;
        let mut job: Thread = [0; 31];
        job[SP] = top as u32;
        job[MEPC] = worker as extern "C" fn() -> ! as usize as u32;
        job[MSTATUS] = mstatus;
        let s = &raw mut TAKT_JOB_SWITCH;
        // SAFETY: Vor dem ersten Wechsel liest niemand die Tabelle; die
        // Unterbrechung wird erst danach gebunden.
        unsafe {
            write_volatile(&raw mut JOB, job);
            write_volatile(&raw mut (*s).cur, &raw mut MAIN);
            write_volatile(&raw mut (*s).next, &raw mut MAIN);
            write_volatile(&raw mut (*s).source, INTPRI::regs().cpu_intr_from_cpu(0).as_ptr());
        }
        esp_hal::interrupt::enable_direct(
            Interrupt::FROM_CPU_INTR0,
            Priority::Priority1,
            DirectBindableCpuInterrupt::Interrupt0,
            takt_job_switch,
        );
        STARTED.store(true, Ordering::Release);
        JobContext { bottom, jobs }
    }

    /// Der Job-Faden des Programms, auf dem Stack, den der Rahmen fuer ihn
    /// bemisst; `None`, wenn das Programm keine Jobs startet.
    pub fn start(jobs: Jobs) -> Option<JobContext> {
        let stack = takt_mcu_program::jobs::stack()?;
        // SAFETY: einmal vor dem Faden, der es liest; `on` gibt ihn danach mit
        // `STARTED` frei.
        unsafe { JOBS = Some(jobs) };
        Some(JobContext::on(stack, jobs))
    }

    /// Das untere Ende des Job-Stacks, wo der Waechter liegt (12.3).
    pub fn bottom(&self) -> u32 {
        self.bottom
    }

    /// Rechnet, was ansteht, bis der Job abgibt oder der naechste Tick ihn
    /// unterbricht. Nur aus der Hauptschleife, in ihrer Wartezeit.
    pub fn run(&mut self) {
        if self.jobs.dispatch() {
            self.resume();
        }
    }

    /// Rechnet jeden Job zu Ende — in logischer Zeit, wo die Hauptschleife
    /// warten darf (13.8).
    pub fn finish(&mut self) {
        while self.jobs.dispatch() {
            self.resume();
        }
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
    if STARTED.load(Ordering::Acquire) {
        // SAFETY: ein Wort, das der Handler bei gesperrten Interrupts liest.
        unsafe { write_volatile(&raw mut TAKT_JOB_SWITCH.next, &raw mut MAIN) };
        raise();
    }
}

/// Wechselt zu `thread` und kehrt zurueck, wenn der Kern wieder hier ist.
fn switch_to(thread: *mut Thread) {
    // SAFETY: ein Wort, das der Handler bei gesperrten Interrupts liest.
    unsafe { write_volatile(&raw mut TAKT_JOB_SWITCH.next, thread) };
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
    // SAFETY: `start` setzte es vor dem ersten Wechsel hierher.
    let jobs = unsafe { JOBS };
    loop {
        if let Some(jobs) = jobs {
            jobs.work();
        }
        switch_to(&raw mut MAIN);
    }
}
