//! Speicherschutz mit der MPU des Cortex-M4 (12.3).
//!
//! Drei Regionen. Der Programmzustand (`.takt_state`, am Anfang des RAM,
//! siehe `build.rs` des Bring-ups) ist nur waehrend des Programmschritts
//! beschreibbar; ausserhalb — und in jeder ISR, auch einer, die den Schritt
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
    (&raw const __takt_state_start as u32, &raw const __takt_state_end as u32)
}

/// Ein Waechter: 32 Byte an der ersten ausgerichteten Adresse ab `bottom`.
fn guard_above(bottom: u32) -> Region {
    Region { base: bottom.div_ceil(32) * 32, size: 32, eighths: 8 }
}

/// Der Waechter unter dem Hauptstack.
fn guard() -> Region {
    guard_above(&raw const _stack_end as u32)
}

/// Der Waechter unter dem Job-Stack, falls es einen gibt.
fn job_guard() -> Option<Region> {
    Some(JOB_STACK.load(Ordering::Relaxed)).filter(|&b| b != 0).map(guard_above)
}

/// Wo der Waechter unter dem Hauptstack liegt: die Adresse, auf die ein
/// Pruefzugriff zielt.
pub fn guard_address() -> u32 {
    guard().base
}

/// Wo der Waechter unter dem Job-Stack liegt, falls es einen gibt.
pub fn job_guard_address() -> Option<u32> {
    job_guard().map(|g| g.base)
}

/// Wo der Programmzustand beginnt: die Adresse, auf die ein Pruefzugriff zielt.
pub fn state_address() -> u32 {
    state_range().0
}

/// Loescht den Programmzustand. `.takt_state` ist `NOLOAD` und liegt vor
/// `.bss`, also nullt ihn der Startcode nicht; das geschieht hier, bevor
/// der Rahmen ihn zum ersten Mal beschreibt.
pub fn clear_state() {
    let (start, end) = state_range();
    // SAFETY: Der Bereich gehoert allein dem Programmzustand (Linker-
    // Skript), und noch liest oder schreibt ihn niemand.
    unsafe { core::ptr::write_bytes(start as *mut u8, 0, (end - start) as usize) };
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
    // SAFETY: wie oben.
    unsafe {
        mpu.rnr.write(STATE);
        mpu.rasr.write(before);
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    r
}

/// Die MPU mit ihren Regionen, der Programmzustand zunaechst offen.
pub struct Mpu {
    state: Region,
}

impl Mpu {
    /// Richtet die Regionen ein und schaltet MPU und `MemoryManagement` ein.
    /// `job_stack` ist das untere Ende des Job-Stacks; der Rahmen richtet
    /// ihn an 32 Byte aus und reserviert den Waechter darin.
    ///
    /// `None`, wenn das Linker-Skript den Programmzustand nicht an eine
    /// Grenze gelegt hat, die eine Region tragen kann.
    pub fn arm(mpu: &mut MPU, scb: &mut SCB, job_stack: Option<u32>) -> Option<Mpu> {
        let (start, end) = state_range();
        let state = Region::covering(start, end - start)?;
        JOB_STACK.store(job_stack.unwrap_or(0), Ordering::Relaxed);
        let guard = guard();
        // SAFETY: Konfiguration der eigenen MPU vor dem ersten Tick; kein
        // anderer Code benutzt sie.
        unsafe {
            mpu.ctrl.write(0);
            if let Some(g) = job_guard() {
                mpu.rnr.write(JOB_GUARD);
                mpu.rbar.write(g.rbar(JOB_GUARD));
                mpu.rasr.write(g.rasr(Access::None));
            }
            mpu.rnr.write(GUARD);
            mpu.rbar.write(guard.rbar(GUARD));
            mpu.rasr.write(guard.rasr(Access::None));
            // Zuletzt: `RNR` bleibt auf dem Programmzustand stehen (`isr`).
            mpu.rnr.write(STATE);
            mpu.rbar.write(state.rbar(STATE));
            mpu.rasr.write(state.rasr(Access::ReadWrite));
            // ENABLE und PRIVDEFENA: Ausserhalb der Regionen gilt die
            // Standardkarte.
            mpu.ctrl.write(0b101);
            scb.enable(cortex_m::peripheral::scb::Exception::MemoryManagement);
        }
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        Some(Mpu { state })
    }

    fn set(&self, access: Access) {
        // SAFETY: ein Registerpaar der MPU, von der Schleife zwischen den
        // Ticks geschrieben; eine ISR stellt es wieder her (`isr`).
        unsafe {
            let mpu = &*MPU::PTR;
            mpu.rnr.write(STATE);
            mpu.rasr.write(self.state.rasr(access));
        }
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
        let region = match REGION.swap(0, Ordering::Relaxed) {
            0 => return None,
            1 => "Programmzustand",
            2 => "Waechter",
            _ => "anderswo",
        };
        Some(Violation { region, address: ADDRESS.load(Ordering::Relaxed) })
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
    let hit = |g: Region| (g.base..g.base + g.size).contains(&address);
    let region = if (start..end).contains(&address) {
        1
    } else if hit(guard()) || job_guard().is_some_and(hit) {
        2
    } else {
        3
    };
    ADDRESS.store(address, Ordering::Relaxed);
    REGION.store(region, Ordering::Relaxed);
    // SAFETY: `frame` zeigt auf den gestapelten Rahmen (siehe oben); an
    // Index 6 steht der PC des verletzenden Befehls, und dort liegt Code.
    unsafe {
        let pc = frame.add(6).read_volatile();
        let halfword = (pc as *const u16).read_volatile();
        frame.add(6).write_volatile(pc + instruction_bytes(halfword));
        scb.cfsr.write(mmfsr);
    }
}
