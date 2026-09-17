//! Schlaf und Neustart (9.9, 12.3).

use takt_rt_baremetal::Sleep;

use crate::tick;

/// Schlaeft mit `wfi`, bis ein Interrupt kommt; der Alarm weckt spaetestens
/// zum naechsten Tick (Satz 9.9.1).
#[derive(Clone, Copy, Debug, Default)]
pub struct WfiSleep;

impl Sleep for WfiSleep {
    #[esp_hal::ram]
    fn sleep_until_event(&mut self) -> u64 {
        let before = tick::count();
        crate::wfi();
        tick::count().saturating_sub(before)
    }
}

/// Startet den Chip neu.
pub fn reboot() -> ! {
    esp_hal::system::software_reset()
}
