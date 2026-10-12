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
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use cortex_m::peripheral::SCB;
use takt_embed::{Context, Dispatch, Jobs};

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
    /// Legt den Faden des Jobs auf `stack` an; er beginnt in [`worker`],
    /// sobald die Hauptschleife ihn zum ersten Mal rechnen laesst.
    fn on(stack: &'static mut [u8], dispatch: D) -> JobContext<D> {
        paint(stack);
        // Die Hardware erwartet beim Verlassen von PendSV einen Rahmen aus
        // acht Worten auf dem PSP: r0–r3, r12, lr, pc, xPSR; der Stack ist
        // auf acht Byte ausgerichtet.
        let top = stack.as_mut_ptr_range().end.expose_provenance() & !7;
        let frame = core::ptr::with_exposed_provenance_mut::<u32>(top.saturating_sub(32));
        let entry = crate::word(worker_address());
        for (i, word) in [0, 0, 0, 0, 0, u32::MAX, entry & !1, THUMB].into_iter().enumerate() {
            // SAFETY: `frame` liegt am oberen Ende von `stack`, der diesem
            // Faden allein gehoert; noch laeuft er nicht.
            unsafe { write_volatile(frame.wrapping_add(i), word) };
        }
        // Vor dem ersten Wechsel liest niemand die Tabelle, und PendSV
        // bekommt erst danach die niedrigste Prioritaet: Was folgt, schreibt
        // allein.
        // SAFETY: bildet nur die Adresse eines Felds der Statik.
        let threads = unsafe { &raw mut TAKT_JOB_SWITCH.threads };
        let job = threads.cast::<[u32; 26]>().wrapping_add(crate::address(JOB));
        // SAFETY: der Platz des Job-Fadens in der Tabelle, siehe oben.
        unsafe { write_volatile(job, [0; 26]) };
        // SAFETY: Wort 0 des Fadens ist `sp`, siehe oben.
        unsafe { write_volatile(job.cast::<u32>(), crate::word(frame.expose_provenance())) };
        // SAFETY: Wort 9 ist `EXC_RETURN`, siehe oben.
        unsafe { write_volatile(job.cast::<u32>().wrapping_add(9), THREAD_PSP) };
        // SAFETY: bildet nur die Adresse eines Felds der Statik.
        let current = unsafe { &raw mut TAKT_JOB_SWITCH.current };
        // SAFETY: siehe oben.
        unsafe { write_volatile(current, MAIN) };
        // SAFETY: siehe oben.
        unsafe { write_volatile(want(), MAIN) };
        // SAFETY: die Register des SCB; nur dieser Faden schreibt die Prioritaet von PendSV.
        let scb = unsafe { &*SCB::PTR };
        // SHPR3, Byte fuer PendSV (Ausnahme 14): die niedrigste Prioritaet.
        // SAFETY: PendSV laeuft erst nach dem ersten Wunsch eines Wechsels.
        unsafe { scb.shpr[10].write(0xFF) };
        STARTED.store(true, Ordering::SeqCst);
        JobContext { bottom: crate::word(stack.as_ptr().expose_provenance()), dispatch }
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

    /// Das untere Ende des Job-Stacks; ein geschuetzter Rahmen legt dort
    /// den Waechter ab (12.3).
    pub fn bottom(&self) -> u32 {
        self.bottom
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
    if STARTED.load(Ordering::SeqCst) {
        // SAFETY: ein Wort, das PendSV bei gesperrten Interrupts liest.
        unsafe { write_volatile(want(), MAIN) };
        SCB::set_pendsv();
    }
}

/// Das Feld `want` der Tabelle, ohne es zu lesen; jeder Zugriff darauf ist
/// volatil, denn PendSV liest es.
fn want() -> *mut u32 {
    // SAFETY: bildet nur die Adresse eines Felds der Statik.
    unsafe { &raw mut TAKT_JOB_SWITCH.want }
}

fn switch_to(thread: u32) {
    // SAFETY: ein Wort, das PendSV bei gesperrten Interrupts liest.
    unsafe { write_volatile(want(), thread) };
    SCB::set_pendsv();
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
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
        switch_to(MAIN);
    }
}

/// Die Adresse von [`worker`]: Der Rahmen, den die Hardware beim ersten
/// Wechsel entstapelt, traegt sie als `pc`.
#[expect(
    clippy::fn_to_numeric_cast_any,
    clippy::as_conversions,
    reason = "MISRA C:2025 R.11.1: der Rahmen des Fadens nennt seinen Einsprung als Adresse; die einer Funktion liefert nur `as`"
)]
fn worker_address() -> usize {
    worker as extern "C" fn() -> ! as usize
}
