//! Der Job-Kontext (4.5, 12.3): ein zweiter Faden mit eigenem Stack, den
//! der Tick unterbricht.
//!
//! Die Hauptschleife laeuft auf dem MSP, der Job auf dem PSP, beide im
//! Thread-Modus. Umgeschaltet wird in PendSV, der Ausnahme mit der
//! niedrigsten Prioritaet: Sie laeuft erst, wenn kein anderer Interrupt mehr
//! aktiv ist. Die Hauptschleife gibt den Kern in ihrer Wartezeit ab
//! ([`JobContext::resume`]), der Tick holt ihn zurueck ([`preempt`]), und ein
//! Job, der nichts mehr zu rechnen hat, gibt ihn selbst zurueck.
//!
//! PendSV sichert je Faden `sp`, r4–r11, `EXC_RETURN` und — wenn der Rahmen
//! des Fadens die FPU traegt — s16–s31 in eine statische Tabelle; den Rest
//! stapelt die Hardware. Interrupts laufen auf dem MSP, also unterhalb des
//! gesicherten Rahmens der Hauptschleife und nie auf dem Job-Stack.
//!
//! **Das Programm bringt das Bring-up mit** (12.11): den Stack aus der
//! Lieferform (`job_stack`), den Griff, der verteilt, und den Griff, mit dem
//! der Faden rechnet (`takt_embed::{Dispatch, Jobs}`).

use core::ptr::write_volatile;
use core::sync::atomic::{AtomicBool, Ordering};

use cortex_m::peripheral::SCB;
use takt_embed::{Dispatch, Jobs};

const MAIN: u32 = 0;
const JOB: u32 = 1;

/// Was PendSV liest und schreibt. Die Versaetze stehen in der
/// Assemblerroutine: `current` bei 0, `want` bei 4, die Faeden ab 8, je
/// 104 Byte (`sp`, r4–r11, `EXC_RETURN`, s16–s31).
#[repr(C)]
struct Switch {
    current: u32,
    want: u32,
    threads: [[u32; 26]; 2],
}

#[unsafe(no_mangle)]
static mut TAKT_JOB_SWITCH: Switch = Switch { current: MAIN, want: MAIN, threads: [[0; 26]; 2] };

/// Ob es einen Job-Faden gibt; ohne ihn bleibt der Tick-Interrupt, wie er war.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Der Griff, mit dem der Faden des Jobs rechnet: vor `STARTED` gesetzt,
/// danach nur von diesem Faden benutzt.
static mut JOBS: Option<&'static mut dyn Jobs> = None;

/// `EXC_RETURN` fuer einen Faden im Thread-Modus auf dem PSP, ohne FPU-Rahmen.
const THREAD_PSP: u32 = 0xFFFF_FFFD;

/// `xPSR` mit gesetztem Thumb-Bit: sonst loest der erste Befehl einen Fault aus.
const THUMB: u32 = 0x0100_0000;

// Die Umschaltung. Interrupts sind dabei gesperrt: Kaeme der Tick mitten
// hinein, saehe `preempt` einen halben Wechsel. `.fpu` nennt die FPU
// ausdruecklich: Mit LTO uebersetzt der Assembler Modul-Assembler ohne die
// Merkmale des Ziels.
core::arch::global_asm!(
    ".syntax unified",
    ".thumb",
    ".fpu fpv4-sp-d16",
    ".section .text.PendSV,\"ax\",%progbits",
    ".global PendSV",
    ".type PendSV,%function",
    ".thumb_func",
    "PendSV:",
    "    cpsid i",
    "    ldr   r2, =TAKT_JOB_SWITCH",
    "    ldr   r3, [r2]",
    "    ldr   r12, [r2, #4]",
    "    cmp   r3, r12",
    "    beq   2f",
    "    movs  r1, #104",
    "    mul   r1, r3, r1",
    "    adds  r1, r1, r2",
    "    adds  r1, #8",
    "    tst   lr, #4",
    "    ite   eq",
    "    mrseq r0, msp",
    "    mrsne r0, psp",
    "    stmia r1!, {{r0, r4-r11, lr}}",
    "    tst   lr, #0x10",
    "    it    eq",
    "    vstmiaeq r1, {{s16-s31}}",
    "    str   r12, [r2]",
    "    movs  r1, #104",
    "    mul   r1, r12, r1",
    "    adds  r1, r1, r2",
    "    adds  r1, #8",
    "    ldmia r1!, {{r0, r4-r11, lr}}",
    "    tst   lr, #0x10",
    "    it    eq",
    "    vldmiaeq r1, {{s16-s31}}",
    "    tst   lr, #4",
    "    ite   eq",
    "    msreq msp, r0",
    "    msrne psp, r0",
    "2:",
    "    cpsie i",
    "    bx    lr",
    ".ltorg",
);

/// Der Job-Faden des Boards; `D` verteilt die Auftraege.
#[derive(Debug)]
pub struct JobContext<D> {
    bottom: u32,
    dispatch: D,
}

impl<D: Dispatch> JobContext<D> {
    /// Legt den Faden des Jobs auf `stack` an; er beginnt in [`worker`],
    /// sobald die Hauptschleife ihn zum ersten Mal rechnen laesst.
    fn on(stack: &'static mut [u8], dispatch: D) -> JobContext<D> {
        // Die Hardware erwartet beim Verlassen von PendSV einen Rahmen aus
        // acht Worten auf dem PSP: r0–r3, r12, lr, pc, xPSR; der Stack ist
        // auf acht Byte ausgerichtet.
        let top = (stack.as_mut_ptr() as usize + stack.len()) & !7;
        let frame = (top - 32) as *mut u32;
        let entry = worker as extern "C" fn() -> ! as usize as u32;
        // SAFETY: `frame` liegt am oberen Ende von `stack`, der diesem
        // Faden allein gehoert; noch laeuft er nicht.
        unsafe {
            for (i, word) in [0, 0, 0, 0, 0, u32::MAX, entry & !1, THUMB].into_iter().enumerate() {
                write_volatile(frame.add(i), word);
            }
        }
        let s = &raw mut TAKT_JOB_SWITCH;
        // SAFETY: Vor dem ersten Wechsel liest niemand die Tabelle, und
        // PendSV bekommt erst danach die niedrigste Prioritaet.
        unsafe {
            let job = &raw mut (*s).threads[JOB as usize];
            write_volatile(job, [0; 26]);
            write_volatile(&raw mut (*job)[0], frame as u32);
            write_volatile(&raw mut (*job)[9], THREAD_PSP);
            write_volatile(&raw mut (*s).current, MAIN);
            write_volatile(&raw mut (*s).want, MAIN);
            // SHPR3, Byte fuer PendSV (Ausnahme 14): die niedrigste Prioritaet.
            (*SCB::PTR).shpr[10].write(0xFF);
        }
        STARTED.store(true, Ordering::Release);
        JobContext { bottom: stack.as_ptr() as u32, dispatch }
    }

    /// Der Job-Faden des Programms auf `stack`, den der Rahmen fuer ihn
    /// bemisst; `None`, wenn das Programm keine Jobs startet und darum einer
    /// der drei fehlt.
    pub fn start(
        stack: Option<&'static mut [u8]>,
        dispatch: Option<D>,
        jobs: Option<&'static mut dyn Jobs>,
    ) -> Option<JobContext<D>> {
        let (stack, dispatch, jobs) = (stack?, dispatch?, jobs?);
        // SAFETY: einmal vor dem Faden, der es liest; `on` gibt ihn danach mit
        // `STARTED` frei.
        unsafe { JOBS = Some(jobs) };
        Some(JobContext::on(stack, dispatch))
    }

    /// Das untere Ende des Job-Stacks; ein geschuetzter Rahmen legt dort
    /// den Waechter ab (12.3).
    pub fn bottom(&self) -> u32 {
        self.bottom
    }

    /// Rechnet, was ansteht, bis der Job abgibt oder der naechste Tick ihn
    /// unterbricht. Nur aus der Hauptschleife, in ihrer Wartezeit.
    pub fn run(&mut self) {
        if self.dispatch.next() {
            self.resume();
        }
    }

    /// Rechnet jeden Job zu Ende — in logischer Zeit, wo die Hauptschleife
    /// warten darf (13.8).
    pub fn finish(&mut self) {
        while self.dispatch.next() {
            self.resume();
        }
    }

    /// Laesst den Job rechnen, bis er abgibt oder der naechste Tick ihn
    /// unterbricht.
    fn resume(&mut self) {
        switch_to(JOB);
    }
}

/// Aus dem Tick-Interrupt: Nach ihm gehoert der Kern der Hauptschleife.
///
/// Ohne Bedingung, damit kein Tick zwischen dem Wunsch der Hauptschleife
/// und dem Wechsel verloren geht: Rechnet der Job nicht, wechselt PendSV
/// nicht.
pub fn preempt() {
    if STARTED.load(Ordering::Acquire) {
        // SAFETY: ein Wort, das PendSV bei gesperrten Interrupts liest.
        unsafe { write_volatile(&raw mut TAKT_JOB_SWITCH.want, MAIN) };
        SCB::set_pendsv();
    }
}

fn switch_to(thread: u32) {
    // SAFETY: ein Wort, das PendSV bei gesperrten Interrupts liest.
    unsafe { write_volatile(&raw mut TAKT_JOB_SWITCH.want, thread) };
    SCB::set_pendsv();
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

/// Der Faden des Jobs: rechnet, was die Hauptschleife ihm gibt, und gibt
/// den Kern zurueck.
extern "C" fn worker() -> ! {
    loop {
        // SAFETY: `start` setzte es vor dem ersten Wechsel hierher; danach
        // benutzt es nur dieser Faden.
        if let Some(jobs) = unsafe { (&raw mut JOBS).as_mut() }.and_then(Option::as_deref_mut) {
            jobs.work();
        }
        switch_to(MAIN);
    }
}
