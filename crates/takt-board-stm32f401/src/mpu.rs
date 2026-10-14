//! Speicherschutz mit der MPU des Cortex-M4 (12.3).
//!
//! Drei Regionen; jede andere schaltet [`Mpu::arm`] aus. Der
//! Programmzustand — der Programmbereich der Arena in `.takt_state` am
//! Anfang des RAM, siehe `build.rs` des Bring-ups (12.11) — ist nur
//! waehrend des Programmschritts beschreibbar; ausserhalb — und in jeder ISR, auch einer, die den Schritt
//! unterbricht ([`isr`]) — darf ihn die TCB nur lesen. Unter dem Hauptstack
//! und unter dem Stack des Job-Kontexts liegt je ein Waechter ohne Zugriff.
//! Ein Zugriff, der eine Regel verletzt, loest `MemoryManagement` aus: Der
//! Handler merkt sich Region und Adresse, uebergeht den Befehl — der
//! Zugriff findet nicht statt — und kehrt zurueck; die Runtime stellt die
//! Verletzung im naechsten Tick als `Runtime(Hardware)` zu
//! (`takt_rt_baremetal::Guarded`).
//!
//! Was sich nicht uebergehen laesst — ein Fehler beim Stapeln der
//! Ausnahme, wenn ein Stack in seinen Waechter gewachsen ist —, setzt den
//! Chip zurueck: Weiterzulaufen hiesse, auf einem zerstoerten Stack zu
//! rechnen.

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::peripheral::{MPU, SCB};
use takt_board_support::mpu::{Access, Region, instruction_bytes};
use takt_rt_baremetal::{Protection, Violation};

unsafe extern "C" {
    static __takt_state_start: u8;
    static __takt_state_end: u8;
    static _stack_end: u32;
}

/// Die Regionen der MPU, nach Nummer: Die hoehere gewinnt bei Ueberlappung,
/// und es gibt keine.
const STATE: u32 = 0;
const GUARD: u32 = 1;
const JOB_GUARD: u32 = 2;

/// Die Prioritaet der Geraete-Interrupts: unter der von `MemoryManagement`
/// (0). Eine Verletzung in einer ISR gleicher Prioritaet eskalierte zum
/// HardFault, statt gemeldet zu werden.
pub const ISR_PRIORITY: u8 = 0x10;

/// Region der letzten Verletzung (0 keine, 1 Programmzustand, 2 Waechter,
/// 3 anderswo) und ihre Adresse; der Handler schreibt, die Schleife liest.
static REGION: AtomicU32 = AtomicU32::new(0);
static ADDRESS: AtomicU32 = AtomicU32::new(0);

/// Das untere Ende des Job-Stacks, 0 ohne Job-Kontext.
static JOB_STACK: AtomicU32 = AtomicU32::new(0);

/// Der Programmzustand als Adressbereich.
fn state_range() -> (u32, u32) {
    let (start, end) = (&raw const __takt_state_start, &raw const __takt_state_end);
    (crate::word(start.expose_provenance()), crate::word(end.expose_provenance()))
}

/// Ein Waechter: 32 Byte an der ersten ausgerichteten Adresse ab `bottom`.
fn guard_above(bottom: u32) -> Region {
    Region { base: bottom.div_ceil(32).saturating_mul(32), size: 32, eighths: 8 }
}

/// Der Waechter unter dem Hauptstack.
fn guard() -> Region {
    guard_above(crate::word((&raw const _stack_end).expose_provenance()))
}

/// Wie viele Regionen die MPU hat (`MPU_TYPE.DREGION`; der Cortex-M4 hat 8).
pub fn regions(mpu: &MPU) -> u32 {
    (mpu._type.read() >> 8) & 0xFF
}

/// Der Waechter unter dem Job-Stack, falls es einen gibt.
fn job_guard() -> Option<Region> {
    Some(JOB_STACK.load(Ordering::SeqCst)).filter(|&b| b != 0).map(guard_above)
}

/// Wo der Waechter unter dem Hauptstack liegt: die Adresse, auf die ein
/// Pruefzugriff zielt.
pub fn guard_address() -> u32 {
    guard().base
}

/// Das untere Ende des nutzbaren Hauptstacks: ueber dem Waechter. Darunter
/// darf auch das Stack-Painting nicht schreiben und nicht lesen (FB-353).
pub fn stack_floor() -> u32 {
    let g = guard();
    g.base.saturating_add(g.size)
}

/// Wo der Waechter unter dem Job-Stack liegt, falls es einen gibt.
pub fn job_guard_address() -> Option<u32> {
    job_guard().map(|g| g.base)
}

/// Wo der Programmzustand beginnt: die Adresse, auf die ein Pruefzugriff zielt.
pub fn state_address() -> u32 {
    state_range().0
}

/// Fuehrt den Rumpf einer ISR bei schreibgeschuetztem Programmzustand aus;
/// danach gilt wieder, was vorher galt. Jede ISR des Boards laeuft hierin.
///
/// Nach [`Mpu::arm`] waehlt jeder nur noch die Region des Programmzustands;
/// eine ISR, die eine Umschaltung der Schleife unterbricht, hinterlaesst
/// Auswahl und Rechte, wie sie sie vorfand.
pub fn isr<R>(f: impl FnOnce() -> R) -> R {
    // SAFETY: ein Registerpaar der MPU, das die ISR vor ihrer Rueckkehr
    // wiederherstellt.
    let mpu = unsafe { &*MPU::PTR };
    // SAFETY: wie oben.
    unsafe { mpu.rnr.write(STATE) };
    let before = mpu.rasr.read();
    // SAFETY: wie oben; nur das Feld `AP` aendert sich.
    unsafe { mpu.rasr.write(before & !(0b111 << 24) | Access::ReadOnly.ap() << 24) };
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    let r = f();
    // SAFETY: wie oben; die ISR hat `RNR` vielleicht umgestellt.
    unsafe { mpu.rnr.write(STATE) };
    // SAFETY: wie oben; die Rechte von vorher.
    unsafe { mpu.rasr.write(before) };
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    r
}

/// Die MPU mit ihren Regionen, der Programmzustand zunaechst offen.
pub struct Mpu {
    state: Region,
}

impl Mpu {
    /// Richtet die Regionen ein und schaltet MPU und `MemoryManagement` ein;
    /// jede andere Region schaltet sie aus.
    /// `job_stack` ist das untere Ende des Job-Stacks; der Rahmen richtet
    /// ihn an 32 Byte aus und reserviert den Waechter darin.
    ///
    /// `None`, wenn das Linker-Skript den Programmzustand nicht an eine
    /// Grenze gelegt hat, die eine Region tragen kann.
    pub fn arm(mpu: &mut MPU, scb: &mut SCB, job_stack: Option<u32>) -> Option<Mpu> {
        let (start, end) = state_range();
        let state = Region::covering(start, end.checked_sub(start)?)?;
        JOB_STACK.store(job_stack.unwrap_or(0), Ordering::SeqCst);
        let guard = guard();
        // Konfiguration der eigenen MPU vor dem ersten Tick; kein anderer
        // Code benutzt sie, und bis `CTRL` sie wieder einschaltet, gilt keine
        // Region.
        // SAFETY: schaltet die MPU aus, siehe oben.
        unsafe { mpu.ctrl.write(0) };
        // Jede Region aus, bevor die eigenen kommen: Wer ohne Reset in den
        // Bootloader sprang, liess seine stehen, und das Einschalten unten
        // machte sie wieder wirksam (FB-752). So beginnt das Abbild wie nach
        // einem Reset, gleich, was vorher lief.
        for region in 0..regions(mpu) {
            // SAFETY: waehlt eine Region, bei ausgeschalteter MPU.
            unsafe { mpu.rnr.write(region) };
            // SAFETY: Null in `RASR` schaltet die Region aus.
            unsafe { mpu.rasr.write(0) };
        }
        if let Some(g) = job_guard() {
            // SAFETY: waehlt die Region des Waechters unter dem Job-Stack.
            unsafe { mpu.rnr.write(JOB_GUARD) };
            // SAFETY: seine Basis, an 32 Byte ausgerichtet (`guard_above`).
            unsafe { mpu.rbar.write(g.rbar(JOB_GUARD)) };
            // SAFETY: 32 Byte ohne Zugriff.
            unsafe { mpu.rasr.write(g.rasr(Access::None)) };
        }
        // SAFETY: waehlt die Region des Waechters unter dem Hauptstack.
        unsafe { mpu.rnr.write(GUARD) };
        // SAFETY: seine Basis, an 32 Byte ausgerichtet (`guard_above`).
        unsafe { mpu.rbar.write(guard.rbar(GUARD)) };
        // SAFETY: 32 Byte ohne Zugriff.
        unsafe { mpu.rasr.write(guard.rasr(Access::None)) };
        // Zuletzt: `RNR` bleibt auf dem Programmzustand stehen (`isr`).
        // SAFETY: waehlt die Region des Programmzustands.
        unsafe { mpu.rnr.write(STATE) };
        // SAFETY: seine Basis aus `Region::covering`, passend ausgerichtet.
        unsafe { mpu.rbar.write(state.rbar(STATE)) };
        // SAFETY: zunaechst offen; die Schleife schliesst ihn (`Protection`).
        unsafe { mpu.rasr.write(state.rasr(Access::ReadWrite)) };
        // ENABLE und PRIVDEFENA: Ausserhalb der Regionen gilt die
        // Standardkarte.
        // SAFETY: Code, Stacks und Peripherie liegen ausserhalb der Waechter
        // und bleiben zugaenglich.
        unsafe { mpu.ctrl.write(0b101) };
        scb.enable(cortex_m::peripheral::scb::Exception::MemoryManagement);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        Some(Mpu { state })
    }

    fn set(&self, access: Access) {
        // Ein Registerpaar der MPU, von der Schleife zwischen den Ticks
        // geschrieben; eine ISR stellt es wieder her (`isr`).
        // SAFETY: die Register der MPU, siehe oben.
        let mpu = unsafe { &*MPU::PTR };
        // SAFETY: waehlt die Region des Programmzustands, siehe oben.
        unsafe { mpu.rnr.write(STATE) };
        // SAFETY: aendert nur ihre Rechte, siehe oben.
        unsafe { mpu.rasr.write(self.state.rasr(access)) };
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
}

impl Protection for Mpu {
    fn open(&self) {
        self.set(Access::ReadWrite);
    }

    fn close(&self) {
        self.set(Access::ReadOnly);
    }

    fn take_violation(&self) -> Option<Violation> {
        let region = match REGION.swap(0, Ordering::SeqCst) {
            0 => return None,
            1 => "Programmzustand",
            2 => "Waechter",
            _ => "anderswo",
        };
        Some(Violation { region, address: ADDRESS.load(Ordering::SeqCst) })
    }
}

// Der Einstieg holt den Zeiger auf den gestapelten Rahmen — vom Stack, der
// beim Eintritt aktiv war — und springt in den Handler; dessen Rueckkehr
// ist die Rueckkehr aus der Ausnahme, denn `lr` traegt noch EXC_RETURN.
core::arch::global_asm!(
    ".section .text.MemoryManagement,\"ax\",%progbits",
    ".global MemoryManagement",
    ".type MemoryManagement,%function",
    ".thumb_func",
    "MemoryManagement:",
    "    tst lr, #4",
    "    ite eq",
    "    mrseq r0, msp",
    "    mrsne r0, psp",
    "    b takt_mem_manage",
);

/// `MemoryManagement`: merkt sich Region und Adresse und uebergeht den
/// Befehl, der sie verletzt hat.
///
/// # Safety
///
/// Nur der Einstieg oben ruft sie, mit dem Zeiger auf den gestapelten
/// Rahmen (r0 bis r3, r12, lr, pc, xpsr).
#[unsafe(no_mangle)]
unsafe extern "C" fn takt_mem_manage(frame: *mut u32) {
    // SAFETY: SCB-Register im Handler; niemand sonst schreibt sie.
    let scb = unsafe { &*SCB::PTR };
    let mmfsr = scb.cfsr.read() & 0xFF;
    // Genau DACCVIOL mit gueltiger Adresse: ein Datenzugriff, den man
    // uebergehen kann. Ein Fehler beim Stapeln, Entstapeln oder Befehlsabruf
    // — auch zusammen mit DACCVIOL — laesst keinen verlaesslichen Rahmen
    // zurueck und setzt zurueck.
    if mmfsr != 0b1000_0010 {
        SCB::sys_reset();
    }
    let address = scb.mmfar.read();
    let (start, end) = state_range();
    let hit = |g: Region| (g.base..g.base.saturating_add(g.size)).contains(&address);
    let region = if (start..end).contains(&address) {
        1
    } else if hit(guard()) || job_guard().is_some_and(hit) {
        2
    } else {
        3
    };
    ADDRESS.store(address, Ordering::SeqCst);
    REGION.store(region, Ordering::SeqCst);
    let slot = frame.wrapping_add(6);
    // SAFETY: `frame` zeigt auf den gestapelten Rahmen (siehe oben); an
    // Index 6 steht der PC des verletzenden Befehls.
    let pc = unsafe { slot.read_volatile() };
    let instruction = core::ptr::with_exposed_provenance::<u16>(crate::address(pc));
    // SAFETY: Der PC zeigt auf den Befehl, der die Verletzung ausloeste, und
    // dort liegt lesbarer Code.
    let halfword = unsafe { instruction.read_volatile() };
    // SAFETY: wie beim Lesen; der Ruecksprung entstapelt den Rahmen und
    // faehrt hinter dem Befehl fort.
    unsafe { slot.write_volatile(pc.wrapping_add(instruction_bytes(halfword))) };
    // SAFETY: `CFSR` loescht, was mit Einsen beschrieben wird: genau die
    // gemeldete Verletzung.
    unsafe { scb.cfsr.write(mmfsr) };
}
