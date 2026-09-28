//! Die Messschleife an PA0 und PA1 (13.8): eine Drahtbruecke zwischen zwei
//! Nachbarpins der Stiftleiste, ohne Loeten. Was gemessen wird, steht in
//! `takt_board_support::wire`.

use stm32f4::stm32f401::{GPIOA, RCC};
use takt_board_support::wire::WireStats;

/// Der Ausgang der Schleife, PA0.
const OUT_PIN: u32 = 0;

/// Der Eingang der Schleife, PA1.
const IN_PIN: u32 = 1;

/// So oft fragt ein Schreibvorgang den Eingang hoechstens ab, bis sein
/// Pegel dort ansteht; bei 84 MHz weit ueber einer Mikrosekunde.
const ARRIVAL_POLLS: u32 = 1_000;

/// Port A und was ueber die Schleife gemessen wurde.
pub struct Wire {
    gpioa: GPIOA,
    /// Die Messwerte des Laufs.
    pub stats: WireStats,
}

impl Wire {
    /// PA0 als Ausgang auf `Low`, PA1 als Eingang mit Pull-down: Eine
    /// offene Bruecke liest dann `Low` statt eines schwebenden Pegels.
    pub fn new(gpioa: GPIOA, rcc: &RCC) -> Wire {
        rcc.ahb1enr().modify(|_, w| w.gpioaen().set_bit());
        // Errata: zwei APB-Takte Verzoegerung, siehe lib.rs.
        let _ = rcc.ahb1enr().read();
        gpioa.bsrr().write(|w| unsafe { w.bits(1 << (OUT_PIN + 16)) });
        let (out, inp) = (OUT_PIN * 2, IN_PIN * 2);
        gpioa.moder().modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << out | 0b11 << inp)) | 0b01 << out) });
        gpioa.pupdr().modify(|r, w| unsafe { w.bits((r.bits() & !(0b11 << inp)) | 0b10 << inp) });
        Wire { gpioa, stats: WireStats::default() }
    }

    fn input(&self) -> bool {
        self.gpioa.idr().read().bits() & (1 << IN_PIN) != 0
    }

    /// Setzt den Ausgang und wartet, bis der Eingang den Pegel zeigt.
    pub fn write(&mut self, level: bool) {
        let start = crate::cycles::now();
        let shift = if level { OUT_PIN } else { OUT_PIN + 16 };
        self.gpioa.bsrr().write(|w| unsafe { w.bits(1 << shift) });
        let arrived = (0..ARRIVAL_POLLS).find_map(|_| (self.input() == level).then(crate::cycles::now));
        self.stats.wrote(start, arrived, level);
    }

    /// Liest den Eingang.
    pub fn read(&mut self) -> bool {
        let start = crate::cycles::now();
        let level = self.input();
        self.stats.read(start, crate::cycles::now(), level);
        level
    }
}
