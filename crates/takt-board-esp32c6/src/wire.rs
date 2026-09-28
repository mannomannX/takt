//! Die Drahtbruecke GPIO7 an GPIO17 als Messschleife (13.8).
//!
//! Dieselbe Bruecke traegt sonst UART0 (`pins.rs`). Ein Programm, das
//! `gpio/loop_out` oder `gpio/loop_in` bindet, bekommt die beiden Pins als
//! GPIO, jedes andere die UART-Route; `main` entscheidet es aus den
//! Adressen des Programms. Was gemessen wird, steht in
//! `takt_board_support::wire`.

use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::peripherals::{GPIO7, GPIO17};
use takt_board_support::wire::WireStats;

/// So oft fragt ein Schreibvorgang den Eingang hoechstens ab, bis sein
/// Pegel dort ansteht; bei 160 MHz weit ueber einer Mikrosekunde.
const ARRIVAL_POLLS: u32 = 1_000;

/// Ausgang, Eingang und was ueber sie gemessen wurde.
pub struct Wire {
    out: Output<'static>,
    input: Input<'static>,
    /// Die Messwerte des Laufs.
    pub stats: WireStats,
}

impl Wire {
    /// Nimmt die Pins als GPIO; der Ausgang beginnt auf `Low`, der Eingang
    /// hat einen Pull-down: Eine offene Bruecke liest dann `Low` statt
    /// eines schwebenden Pegels.
    pub fn new(out: GPIO7<'static>, input: GPIO17<'static>) -> Wire {
        crate::cycles::enable();
        Wire {
            out: Output::new(out, Level::Low, OutputConfig::default()),
            input: Input::new(input, InputConfig::default().with_pull(Pull::Down)),
            stats: WireStats::default(),
        }
    }

    /// Setzt den Ausgang und wartet, bis der Eingang den Pegel zeigt.
    pub fn write(&mut self, level: bool) {
        let start = crate::cycles::now();
        self.out.set_level(if level { Level::High } else { Level::Low });
        let arrived = (0..ARRIVAL_POLLS).find_map(|_| (self.input.is_high() == level).then(crate::cycles::now));
        self.stats.wrote(start, arrived, level);
    }

    /// Liest den Eingang.
    pub fn read(&mut self) -> bool {
        let start = crate::cycles::now();
        let level = self.input.is_high();
        self.stats.read(start, crate::cycles::now(), level);
        level
    }
}
