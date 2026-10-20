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
//! sicheren Platz. Danach springt er ins Sprungbrett in Sektor 1
//! (`takt-trampoline-stm32f401`), und das fragt den Wunsch als Erstes ab
//! (`enter_if_requested`) und springt. So gibt jede Anwendung das Board
//! auf demselben Weg zurueck, ob in Rust oder in C
//! (`examples/boards/stm32f401`): Sie schreibt den Wunsch und setzt den Chip
//! zurueck, den Rest tut das Sprungbrett. Der Sprung
//! raeumt auf, was der HID-Bootloader hinterliess, wie ST es fuer den
//! Sprung aus Anwendungscode beschreibt — Interrupts und SysTick aus, der
//! Takt zurueck auf HSI, PLL und HSE aus —, und setzt jede Peripherie ueber
//! ihr Reset-Bit zurueck. Der Bootloader findet den Chip so vor wie nach
//! einem Start mit BOOT0.
//!
//! **Den Sprung gibt es nur im Sprungbrett** (Merkmal `trampoline`, das nur
//! `takt-trampoline-stm32f401` einschaltet): Aus einer laufenden Anwendung
//! gerufen meldete sich der ROM-Bootloader nicht am USB an, und nur BOOT0
//! und NRST holten das Board zurueck (FB-758). Ein Bring-up, das `enter`
//! riefe, uebersetzt nicht.

use cortex_m::peripheral::SCB;
#[cfg(feature = "trampoline")]
use cortex_m::peripheral::{MPU, NVIC, SYST};
#[cfg(feature = "trampoline")]
use stm32f4::stm32f401::{FLASH, RCC, SYSCFG};
#[cfg(feature = "trampoline")]
use takt_board_support::mpu::{Access, Region};

use crate::platform::backup;
#[cfg(feature = "trampoline")]
use crate::platform::backup_read;

/// Der Systemspeicher des F401 mit dem Bootloader von ST (AN2606).
#[cfg(feature = "trampoline")]
const SYSTEM_MEMORY: u32 = 0x1FFF_0000;

/// Der Wunsch nach dem Bootloader, im Backup-Register [`WISH`].
const REQUESTED: u32 = u32::from_le_bytes(*b"DFU!");
const WISH: usize = 4;

/// Der Wunsch, dem naechsten Abbild eine Region der MPU zu hinterlassen
/// ([`bequeath_region`]), im Backup-Register [`LEFTOVER`].
const LEFT: u32 = u32::from_le_bytes(*b"MPU!");
const LEFTOVER: usize = 5;

/// Wo die hinterlassene Region liegt: 24 KiB unter dem Ende des RAM, wo
/// Zephyrs `z_main_stack` begann. Dort bemalt jedes Abbild seinen Stack, in
/// Rust wie in C.
#[cfg(feature = "trampoline")]
const LEFT_AT: u32 = 0x2000_A000;

/// Der Host hat das Board zurueckverlangt: merkt es sich und setzt den
/// Chip zurueck; kehrt nicht zurueck.
pub fn request() -> ! {
    backup(WISH, REQUESTED);
    SCB::sys_reset()
}

/// Bei der naechsten Rueckgabe hinterlaesst das Sprungbrett dem naechsten
/// Abbild eine Region der MPU (FB-752): die letzte, 32 Byte nur lesbar ueber
/// `LEFT_AT`, bei ausgeschalteter MPU, wie ein Vorgaenger sie stehen
/// laesst, der ohne Reset in den Bootloader springt. Das Programm `leftover`
/// des Bring-ups verlangt es fuer den Board-Test
/// `a_region_left_by_the_previous_image_changes_nothing`.
///
/// Zurueck geht das Board wie jedes Abbild auf `TAKT` — Wunsch, Reset,
/// HID-Bootloader, Sprungbrett, `enter` —, und erst das Sprungbrett
/// beschreibt die Region, nach dem Reset, der sie sonst loeschte. Ein
/// Sprung aus der laufenden Anwendung in den ROM-Bootloader brachte dessen
/// USB nicht hoch (FB-758).
pub fn bequeath_region() {
    backup(LEFTOVER, LEFT);
}

/// Gibt das Board an den Bootloader, wenn die Anwendung davor es verlangt
/// hat; sonst kehrt sie zurueck und laesst den Takt von PWR, wie sie ihn
/// fand. Das Sprungbrett ruft sie als Erstes.
///
/// Nur fuer das Sprungbrett (Merkmal `trampoline`).
///
/// Die Reset-Ursache dieses Starts gilt dem Wunsch, nicht dem Programm,
/// das danach kommt: Sie wird geloescht, und das naechste Programm beginnt
/// wie nach dem Einschalten (12.7).
#[cfg(feature = "trampoline")]
pub fn enter_if_requested() {
    // SAFETY: die Register des RCC; vor der Anwendung fasst sie niemand an.
    let rcc = unsafe { &*RCC::ptr() };
    let clocked = rcc.apb1enr().read().pwren().bit_is_set();
    // Der Wunsch nach einer Region gilt nur der naechsten Rueckgabe.
    let bequeathed = backup_read(LEFTOVER) == LEFT;
    if bequeathed {
        backup(LEFTOVER, 0);
    }
    if backup_read(WISH) == REQUESTED {
        backup(WISH, 0);
        crate::platform::previous_run();
        if bequeathed {
            leave_region();
        }
        enter();
    }
    if !clocked {
        rcc.apb1enr().modify(|_, w| w.pwren().clear_bit());
    }
}

/// Belegt die letzte Region der MPU nur lesbar ueber [`LEFT_AT`] und laesst
/// die MPU aus ([`bequeath_region`]).
#[cfg(feature = "trampoline")]
fn leave_region() {
    // SAFETY: die Register der MPU; das Sprungbrett laeuft allein, und nach
    // ihm kommt der Bootloader, der die MPU nicht einschaltet.
    let mpu = unsafe { &*MPU::PTR };
    let region = crate::mpu::regions(mpu).saturating_sub(1);
    let left = Region { base: LEFT_AT, size: 32, eighths: 8 };
    // SAFETY: die MPU aus; keine Region gilt, bis ein Abbild sie einschaltet.
    unsafe { mpu.ctrl.write(0) };
    // SAFETY: waehlt die letzte Region.
    unsafe { mpu.rnr.write(region) };
    // SAFETY: ihre Basis, an 32 Byte ausgerichtet.
    unsafe { mpu.rbar.write(left.rbar(region)) };
    // SAFETY: 32 Byte nur lesbar, die Region eingeschaltet.
    unsafe { mpu.rasr.write(left.rasr(Access::ReadOnly)) };
}

/// Gibt das Board an den DFU-Bootloader des ROM; kehrt nicht zurueck. Das
/// Sprungbrett ruft es, wenn keine Anwendung da ist (FB-385), und nach dem
/// Wunsch einer Anwendung, gleich nach Reset und HID-Bootloader. Nur dieser
/// Weg ist erprobt: Aus einer laufenden Anwendung gerufen meldete sich der
/// ROM-Bootloader nicht am USB an, und nur BOOT0 und NRST holten das Board
/// zurueck (FB-758). Eine Anwendung verlangt den Bootloader darum mit
/// [`request`]; uebersetzt wird `enter` nur mit dem Merkmal `trampoline`.
#[cfg(feature = "trampoline")]
pub fn enter() -> ! {
    cortex_m::interrupt::disable();
    // Ab hier laeuft nichts mehr von der Anwendung: Interrupts sind aus, und
    // jedes Register, das die folgenden Zeilen anfassen, geht danach an den
    // Bootloader. Wer es vorher besass, kommt nicht mehr dran; darum nimmt
    // dieser Weg die Peripherie ueber ihre Adressen.
    // SAFETY: siehe oben; `SYST::PTR` zeigt auf die Register des SysTick.
    let syst = unsafe { &*SYST::PTR };
    // SAFETY: Null in `CSR` haelt den SysTick an und sperrt seine Ausnahme.
    unsafe { syst.csr.write(0) };
    // SAFETY: siehe oben; die Register des RCC.
    let rcc = unsafe { &*RCC::ptr() };
    // SAFETY: siehe oben; die Register des Flash.
    let flash = unsafe { &*FLASH::ptr() };
    restore_reset_state(rcc, flash);
    // SAFETY: siehe oben; die Register des NVIC.
    let nvic = unsafe { &*NVIC::PTR };
    for (icer, icpr) in nvic.icer.iter().zip(&nvic.icpr) {
        // SAFETY: Einsen in `ICER` sperren jeden Interrupt dieses Worts.
        unsafe { icer.write(u32::MAX) };
        // SAFETY: Einsen in `ICPR` nehmen jeden anstehenden zurueck.
        unsafe { icpr.write(u32::MAX) };
    }

    // Den Systemspeicher nach Adresse 0 abbilden, wie ein Start mit
    // BOOT0 es tut (RM0368, `SYSCFG_MEMRMP`), und die Vektortabelle
    // dorthin legen: Der Bootloader erwartet seine eigene.
    rcc.apb2enr().modify(|_, w| w.syscfgen().set_bit());
    let _ = rcc.apb2enr().read();
    // SAFETY: siehe oben; die Register von SYSCFG, dessen Takt eben anlief.
    let syscfg = unsafe { &*SYSCFG::ptr() };
    // SAFETY: `0b01` ist in `MEM_MODE` der Systemspeicher.
    syscfg.memrmp().modify(|_, w| unsafe { w.mem_mode().bits(0b01) });
    // SAFETY: siehe oben; die Register des SCB.
    let scb = unsafe { &*SCB::PTR };
    // SAFETY: Am Anfang des Systemspeichers steht die Vektortabelle des
    // Bootloaders, an 512 Byte ausgerichtet, wie `VTOR` es verlangt.
    unsafe { scb.vtor.write(SYSTEM_MEMORY) };

    // CONTROL wie nach dem Reset: Hauptstack, keine FPU-Kontextmarke.
    // SAFETY: Der Kern laeuft privilegiert auf dem Hauptstack; der Wert
    // aendert davon nichts und loescht nur die Marke der FPU.
    unsafe { cortex_m::register::control::write(cortex_m::register::control::Control::from_bits(0)) };
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    // Der Bootloader richtet seine Interrupts selbst ein und gibt sie
    // nicht selbst frei (ST, „Jump to the bootloader from application").
    // SAFETY: Jeder Interrupt ist im NVIC gesperrt und keiner steht an.
    unsafe { cortex_m::interrupt::enable() };
    let table = core::ptr::with_exposed_provenance(crate::address(SYSTEM_MEMORY));
    // SAFETY: Die Tabelle beginnt mit Stackzeiger und Resetvektor des
    // Bootloaders; von dieser Anwendung lebt danach nichts weiter.
    unsafe { cortex_m::asm::bootload(table) }
}

/// Takte und Peripherie wie nach dem Einschalten (RM0368, Resetwerte).
#[cfg(feature = "trampoline")]
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
