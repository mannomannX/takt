//! Der Weg zurueck zum Host ohne Hand am Taster (FB-275, plan/f401.md 2).
//!
//! Die Black Pill V1.2 hat keinen KEY-Taster, und den DFU-Bootloader im ROM
//! (AN2606) erreicht die Hand nur ueber BOOT0 und NRST. Darum geht es
//! andersherum: Der Host schreibt `TAKT` in die Trace-Leitung, und die
//! laufende Anwendung springt selbst in den Systemspeicher. Danach ist das
//! Board ein DFU-Geraet; `dfu-util` schreibt das naechste Abbild und
//! startet es.
//!
//! **Erst ein Reset, dann der Sprung.** Den Watchdog haelt nur ein Reset
//! an (12.3), und der Bootloader bedient ihn nicht; er setzte den Chip
//! mitten im Schreiben zurueck. Darum merkt sich [`request`] den Wunsch im
//! Backup-Register 4 und setzt den Chip zurueck. Nach dem Reset laeuft
//! zuerst der HID-Bootloader von WeAct, der Takte und USB fuer sich
//! einrichtet; das Backup-Register laesst er stehen, RAM haette keinen
//! sicheren Platz. Der Start fragt den Wunsch als Erstes ab
//! ([`enter_if_requested`], aus [`crate::init`]) und springt. Der Sprung
//! raeumt auf, was der HID-Bootloader hinterliess, wie ST es fuer den
//! Sprung aus Anwendungscode beschreibt — Interrupts und SysTick aus, der
//! Takt zurueck auf HSI, PLL und HSE aus —, und setzt jede Peripherie ueber
//! ihr Reset-Bit zurueck. Der Bootloader findet den Chip so vor wie nach
//! einem Start mit BOOT0.

use cortex_m::peripheral::{NVIC, SCB, SYST};
use stm32f4::stm32f401::{FLASH, RCC, SYSCFG};

use crate::platform::{backup, backup_read};

/// Der Systemspeicher des F401 mit dem Bootloader von ST (AN2606).
const SYSTEM_MEMORY: u32 = 0x1FFF_0000;

/// Der Wunsch nach dem Bootloader, im Backup-Register [`WISH`].
const REQUESTED: u32 = u32::from_le_bytes(*b"DFU!");
const WISH: usize = 4;

/// Der Host hat das Board zurueckverlangt: merkt es sich und setzt den
/// Chip zurueck; kehrt nicht zurueck.
pub fn request() -> ! {
    backup(WISH, REQUESTED);
    SCB::sys_reset()
}

/// Gibt das Board an den Bootloader, wenn der vorige Lauf es verlangt
/// hat; sonst kehrt sie zurueck. Als Erstes beim Start zu rufen.
///
/// Die Reset-Ursache dieses Starts gilt dem Wunsch, nicht dem Programm,
/// das danach kommt: Sie wird geloescht, und das naechste Programm beginnt
/// wie nach dem Einschalten (12.7).
pub fn enter_if_requested() {
    if backup_read(WISH) == REQUESTED {
        backup(WISH, 0);
        crate::platform::boot_reason();
        enter();
    }
}

/// Gibt das Board an den DFU-Bootloader des ROM; kehrt nicht zurueck.
fn enter() -> ! {
    cortex_m::interrupt::disable();
    // SAFETY: Ab hier laeuft nichts mehr von der Anwendung: Interrupts sind
    // aus, und jedes Register, das die folgenden Zeilen anfassen, geht
    // danach an den Bootloader. Wer es vorher besass, kommt nicht mehr dran.
    unsafe {
        (*SYST::PTR).csr.write(0);
        let rcc = &*RCC::ptr();
        restore_reset_state(rcc, &*FLASH::ptr());
        let nvic = &*NVIC::PTR;
        for (icer, icpr) in nvic.icer.iter().zip(&nvic.icpr) {
            icer.write(u32::MAX);
            icpr.write(u32::MAX);
        }

        // Den Systemspeicher nach Adresse 0 abbilden, wie ein Start mit
        // BOOT0 es tut (RM0368, `SYSCFG_MEMRMP`), und die Vektortabelle
        // dorthin legen: Der Bootloader erwartet seine eigene.
        rcc.apb2enr().modify(|_, w| w.syscfgen().set_bit());
        let _ = rcc.apb2enr().read();
        (*SYSCFG::ptr()).memrmp().modify(|_, w| w.mem_mode().bits(0b01));
        (*SCB::PTR).vtor.write(SYSTEM_MEMORY);

        // CONTROL wie nach dem Reset: Hauptstack, keine FPU-Kontextmarke.
        cortex_m::register::control::write(cortex_m::register::control::Control::from_bits(0));
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        // Der Bootloader richtet seine Interrupts selbst ein und gibt sie
        // nicht selbst frei (ST, „Jump to the bootloader from application").
        cortex_m::interrupt::enable();
        cortex_m::asm::bootload(SYSTEM_MEMORY as *const u32)
    }
}

/// Takte und Peripherie wie nach dem Einschalten (RM0368, Resetwerte).
fn restore_reset_state(rcc: &stm32f4::stm32f401::rcc::RegisterBlock, flash: &stm32f4::stm32f401::flash::RegisterBlock) {
    // Scheitert der Wechsel auf HSI, bleibt die PLL der Systemtakt, und die
    // Hardware verweigert ihr Abschalten; der Bootloader richtet den Takt
    // dann auf dem ein, was er vorfindet. Einen Weg zurueck gibt es nicht.
    let _ = crate::run_on_hsi(rcc);
    rcc.cfgr().reset();
    rcc.cr().modify(|_, w| w.hseon().clear_bit().csson().clear_bit().plli2son().clear_bit());
    rcc.cr().modify(|_, w| w.hsebyp().clear_bit());
    rcc.pllcfgr().reset();
    rcc.cir().write(|w| {
        w.lsirdyc().set_bit();
        w.lserdyc().set_bit();
        w.hsirdyc().set_bit();
        w.hserdyc().set_bit();
        w.pllrdyc().set_bit();
        w.plli2srdyc().set_bit();
        w.cssc().set_bit()
    });
    // Erst nach dem Wechsel auf 16 MHz: null Wartezyklen tragen bis 30 MHz.
    flash.acr().reset();

    rcc.ahb1rstr().write(|w| {
        w.gpioarst().set_bit();
        w.gpiobrst().set_bit();
        w.gpiocrst().set_bit();
        w.gpiodrst().set_bit();
        w.gpioerst().set_bit();
        w.gpiohrst().set_bit();
        w.crcrst().set_bit();
        w.dma1rst().set_bit();
        w.dma2rst().set_bit()
    });
    rcc.ahb2rstr().write(|w| w.otgfsrst().set_bit());
    rcc.apb1rstr().write(|w| {
        w.tim2rst().set_bit();
        w.tim3rst().set_bit();
        w.tim4rst().set_bit();
        w.tim5rst().set_bit();
        w.wwdgrst().set_bit();
        w.spi2rst().set_bit();
        w.spi3rst().set_bit();
        w.usart2rst().set_bit();
        w.i2c1rst().set_bit();
        w.i2c2rst().set_bit();
        w.i2c3rst().set_bit();
        w.pwrrst().set_bit()
    });
    rcc.apb2rstr().write(|w| {
        w.tim1rst().set_bit();
        w.usart1rst().set_bit();
        w.usart6rst().set_bit();
        w.adcrst().set_bit();
        w.sdiorst().set_bit();
        w.spi1rst().set_bit();
        w.spi4rst().set_bit();
        w.syscfgrst().set_bit();
        w.tim9rst().set_bit();
        w.tim10rst().set_bit();
        w.tim11rst().set_bit()
    });
    rcc.ahb1rstr().reset();
    rcc.ahb2rstr().reset();
    rcc.apb1rstr().reset();
    rcc.apb2rstr().reset();

    rcc.ahb1enr().reset();
    rcc.ahb2enr().reset();
    rcc.apb1enr().reset();
    rcc.apb2enr().reset();
}
