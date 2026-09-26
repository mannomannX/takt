//! Die Plattformschnittstelle ohne Register (12.7): was jedes Board zur
//! Reset-Ursache und zum Tiefschlaf rechnet.

/// `BootReason` aus dem Prelude als Diskriminante. Die Reihenfolge steht
/// in 12.7, und das Enum ist offen (2.5): Neue Varianten kommen hinten
/// dazu, die vorhandenen behalten ihre Nummer.
pub mod boot_reason {
    /// Einschalten, Brown-out, externer Reset.
    pub const POWER_ON: i32 = 0;
    /// Ein Watchdog hat zurueckgesetzt.
    pub const WATCHDOG: i32 = 1;
    /// Das Programm hat neu gestartet (`reboot = RESTART`).
    pub const SOFTWARE: i32 = 2;
    /// Der Tiefschlaf ist zu Ende (`DEEP_SLEEP`, `DEEP_SLEEP_FOR`).
    pub const DEEP_SLEEP_WAKE: i32 = 3;
}

/// Die kuerzeste Weckzeit, die ein Board schlaeft: Eine kuerzere Frist
/// faengt der Uebergang in den Tiefschlaf nicht, und 12.7 laesst sie dann
/// die kuerzeste schlafen.
pub const MIN_DEEP_SLEEP_NS: i64 = 1_000_000;

/// Die Weckzeit in Mikrosekunden, aufgerundet: Ein Board weckt nicht vor
/// der Frist, die das Programm genannt hat.
pub fn deep_sleep_us(duration_ns: i64) -> u64 {
    u64::try_from(duration_ns.max(MIN_DEEP_SLEEP_NS)).unwrap_or(0).div_ceil(1_000)
}

/// Ein Abschnitt des Tiefschlafs am Wakeup-Timer einer STM32-RTC
/// (RM0368 22.3.6): Taktwahl `WUCKSEL`, Nachladewert `WUTR` und was danach
/// noch zu schlafen bleibt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtcWakeup {
    /// `0b000`: RTC/16; `0b100`: `ck_spre` (1 Hz); `0b110`: `ck_spre` plus 2^16.
    pub wucksel: u8,
    /// Der Timer zaehlt `wutr + 1` Schritte.
    pub wutr: u16,
    /// Was der naechste Abschnitt schlaeft; null, wenn dieser der letzte ist.
    pub rest_ns: i64,
}

/// Wie der Wakeup-Timer `duration_ns` schlaeft, bei einem RTC-Takt von
/// `rtc_hz` und `ck_spre` = 1 Hz.
///
/// Bis 2^16 Schritte von RTC/16 zaehlt er fein, bis 2^16 s in Sekunden,
/// bis 2^17 s mit dem Zuschlag von 2^16 s; was darueber liegt, schlaeft
/// der naechste Abschnitt. Aufgerundet, damit er nicht vor der Frist weckt.
pub fn rtc_wakeup(duration_ns: i64, rtc_hz: u32) -> RtcWakeup {
    const NS: i64 = 1_000_000_000;
    const SPAN: i64 = 1 << 16;
    let d = duration_ns.max(MIN_DEEP_SLEEP_NS);
    let fine_hz = i64::from(rtc_hz / 16).max(1);
    let fine = (i128::from(d) * i128::from(fine_hz) + i128::from(NS) - 1) / i128::from(NS);
    if fine <= i128::from(SPAN) {
        return RtcWakeup { wucksel: 0b000, wutr: (fine.max(1) - 1) as u16, rest_ns: 0 };
    }
    let secs = (d + NS - 1) / NS;
    if secs <= SPAN {
        return RtcWakeup { wucksel: 0b100, wutr: (secs - 1) as u16, rest_ns: 0 };
    }
    if secs <= 2 * SPAN {
        return RtcWakeup { wucksel: 0b110, wutr: (secs - SPAN - 1) as u16, rest_ns: 0 };
    }
    RtcWakeup { wucksel: 0b110, wutr: u16::MAX, rest_ns: d - 2 * SPAN * NS }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSE: u32 = 32_768;

    #[test]
    fn a_short_sleep_counts_finely_and_never_early() {
        // 2048 Hz: 2 s sind 4096 Schritte, 1 ms ist aufgerundet 3 Schritte.
        assert_eq!(rtc_wakeup(2_000_000_000, LSE), RtcWakeup { wucksel: 0, wutr: 4095, rest_ns: 0 });
        assert_eq!(rtc_wakeup(1_000_000, LSE).wutr, 2);
        assert_eq!(rtc_wakeup(32_000_000_000, LSE).wutr, u16::MAX);
    }

    #[test]
    fn longer_sleeps_count_seconds_and_split_beyond_the_timer() {
        assert_eq!(rtc_wakeup(33_000_000_000, LSE), RtcWakeup { wucksel: 0b100, wutr: 32, rest_ns: 0 });
        let day = 86_400_000_000_000;
        assert_eq!(rtc_wakeup(day, LSE), RtcWakeup { wucksel: 0b110, wutr: 20_863, rest_ns: 0 });
        let week = rtc_wakeup(7 * day, LSE);
        assert_eq!((week.wucksel, week.wutr), (0b110, u16::MAX));
        assert_eq!(week.rest_ns, 7 * day - 131_072 * 1_000_000_000);
    }

    #[test]
    fn nothing_sleeps_shorter_than_the_platform_can() {
        assert_eq!(rtc_wakeup(-5, LSE), rtc_wakeup(MIN_DEEP_SLEEP_NS, LSE));
        assert_eq!(deep_sleep_us(0), 1_000);
        assert_eq!(deep_sleep_us(2_000_000_001), 2_000_001);
    }
}
