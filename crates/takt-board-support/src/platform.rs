//! Anfang und Ende eines Laufs ohne Register (12.7): wie jedes Board
//! `previous_run` bildet und wie lange es zwischen zwei Laeufen schlaeft.

/// `PreviousRun` aus dem Prelude als Diskriminante. Die Reihenfolge steht
/// in 12.7, und das Enum ist offen (2.5): Neue Varianten kommen hinten
/// dazu, die vorhandenen behalten ihre Nummer.
pub mod previous_run {
    /// Kein voriger Lauf bekannt: erster Start, Einschalten, ein Reset,
    /// den die Plattform keinem Lauf zuordnet.
    pub const NONE: u32 = 0;
    /// Der vorige Lauf endete geordnet ueber `next_run`.
    pub const ENDED: u32 = 1;
    /// Der Watchdog der Runtime brach den vorigen Lauf ab (12.3).
    pub const WATCHDOG: u32 = 2;
}

/// Was ein geordnet endender Lauf im Plattformblock hinterlaesst: Das Board
/// schreibt es, bevor es den Lauf beendet (12.7). Jeder Lauf ersetzt es zu
/// Beginn durch [`RUNNING`].
pub const ENDED_MARK: u32 = u32::from_le_bytes(*b"ENDE");

/// Was ein laufender Lauf im Plattformblock stehen hat.
pub const RUNNING: u32 = 0;

/// `sys/previous_run` (12.7) aus der Reset-Ursache und dem Wort, das der
/// vorige Lauf im Plattformblock hinterliess. Der Watchdog geht vor: Er
/// bricht einen Lauf ab, der vorher geordnet geendet haben kann.
pub fn previous_run(watchdog_reset: bool, stored: u32) -> u32 {
    if watchdog_reset {
        previous_run::WATCHDOG
    } else if stored == ENDED_MARK {
        previous_run::ENDED
    } else {
        previous_run::NONE
    }
}

/// Die kuerzeste Weckzeit, die ein Board schlaeft: Eine kuerzere Frist
/// faengt der Uebergang in den Tiefschlaf nicht, und 12.7 laesst sie dann
/// die kuerzeste schlafen.
pub const MIN_DEEP_SLEEP_NS: i64 = 1_000_000;

/// Ohne Zeitgeber verlangt mancher Chip trotzdem eine Weckquelle (der C6
/// unter `esp-hal`); sie steht dann auf dreissig Jahren. Der Vergleicher
/// des C6 fasst 48 Bit Takte des langsamen RTC-Takts, rund 65 Jahre.
pub const NO_WAKE_TIMER_US: u64 = 30 * 365 * 86_400 * 1_000_000;

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
    const NS: u64 = 1_000_000_000;
    const SPAN: u64 = 1 << 16;
    const TWO_SPANS: u64 = 1 << 17;
    const TWO_SPANS_NS: u64 = (1 << 17) * 1_000_000_000;
    // Der Registerwert ist die Zahl der Schritte minus eins.
    let wutr = |steps: u64| u16::try_from(steps.max(1).saturating_sub(1)).unwrap_or(u16::MAX);
    let d = duration_ns.max(MIN_DEEP_SLEEP_NS).unsigned_abs();
    let fine_hz = u64::from(rtc_hz / 16).max(1);
    let fine = u128::from(d).saturating_mul(u128::from(fine_hz)).div_ceil(u128::from(NS));
    if fine <= u128::from(SPAN) {
        return RtcWakeup { wucksel: 0b000, wutr: wutr(u64::try_from(fine).unwrap_or(u64::MAX)), rest_ns: 0 };
    }
    let secs = d.div_ceil(NS);
    if secs <= SPAN {
        return RtcWakeup { wucksel: 0b100, wutr: wutr(secs), rest_ns: 0 };
    }
    if secs <= TWO_SPANS {
        return RtcWakeup { wucksel: 0b110, wutr: wutr(secs.saturating_sub(SPAN)), rest_ns: 0 };
    }
    RtcWakeup {
        wucksel: 0b110,
        wutr: u16::MAX,
        rest_ns: i64::try_from(d.saturating_sub(TWO_SPANS_NS)).unwrap_or(i64::MAX),
    }
}

/// So viele Takte bei `core_hz` dauert `rest_ns`, aufgerundet: das wache
/// Warten eines Tiefschlafs, dessen RTC keinen Takt hat (FB-820). Wie der
/// Wakeup-Timer endet es nicht vor der Frist.
pub fn awake_cycles(rest_ns: u64, core_hz: u32) -> u64 {
    let cycles = u128::from(rest_ns).saturating_mul(u128::from(core_hz)).div_ceil(1_000_000_000);
    u64::try_from(cycles).unwrap_or(u64::MAX)
}

/// Der Uhrenquarz einer STM32-RTC.
pub const LSE_HZ: u32 = 32_768;

/// Der interne RC einer STM32-RTC, nominal; der F401 nennt 17 bis 47 kHz.
pub const LSI_HZ: u32 = 32_000;

/// Woran die RTC eines STM32 laeuft (RM0368 6.3.20, `RCC_BDCR`; 12.7,
/// FB-820). Den Wakeup-Timer zaehlt nur ein laufender Takt: Ohne ihn weckte
/// der Standby nie, und das Board kaeme nur ueber NRST zurueck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtcClock {
    /// Die RTC ist aus: Seit die Backup-Domaene Strom hat, schlief das Board
    /// nicht.
    Off,
    /// Der Uhrenquarz.
    Lse,
    /// Der interne RC.
    Lsi,
    /// Ein Takt ist gewaehlt, aber er laeuft nicht.
    Stopped,
}

impl RtcClock {
    /// Aus `RTCEN`, `RTCSEL` und den Bereit-Bits `LSERDY` und `LSIRDY`.
    pub fn of(enabled: bool, select: u8, lse_ready: bool, lsi_ready: bool) -> RtcClock {
        match (enabled, select) {
            (false, _) => RtcClock::Off,
            (true, 0b01) if lse_ready => RtcClock::Lse,
            (true, 0b10) if lsi_ready => RtcClock::Lsi,
            _ => RtcClock::Stopped,
        }
    }

    /// Der Takt, an dem der Wakeup-Timer zaehlt; `None`, wenn keiner laeuft.
    pub fn hz(self) -> Option<u32> {
        match self {
            RtcClock::Lse => Some(LSE_HZ),
            RtcClock::Lsi => Some(LSI_HZ),
            RtcClock::Off | RtcClock::Stopped => None,
        }
    }

    /// Das Wort der Kopfzeile `RTC`.
    pub fn name(self) -> &'static str {
        match self {
            RtcClock::Off => "aus",
            RtcClock::Lse => "LSE",
            RtcClock::Lsi => "LSI",
            RtcClock::Stopped => "STEHT",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSE: u32 = LSE_HZ;

    /// **Nur ein laufender Takt weckt** (FB-820): Gewaehlt ist nicht genug;
    /// ein LSI nach einem Reset oder ein Quarz, der nicht schwingt, laesst den
    /// Wakeup-Timer stehen, und der Standby endete nie.
    #[test]
    fn only_a_running_clock_wakes_the_board() {
        assert_eq!(RtcClock::of(true, 0b01, true, false).hz(), Some(LSE_HZ));
        assert_eq!(RtcClock::of(true, 0b10, false, true).hz(), Some(LSI_HZ));
        for (select, lse, lsi) in [(0b01, false, true), (0b10, true, false), (0b11, true, true), (0b00, true, true)] {
            let clock = RtcClock::of(true, select, lse, lsi);
            assert_eq!((clock, clock.hz(), clock.name()), (RtcClock::Stopped, None, "STEHT"), "{select:#04b}");
        }
        assert_eq!(RtcClock::of(false, 0b01, true, true), RtcClock::Off);
    }

    /// Das wache Warten rundet auf und laeuft auch bei Jahren nicht ueber.
    #[test]
    fn waiting_awake_counts_cycles_and_never_ends_early() {
        assert_eq!(awake_cycles(2_000_000_000, 84_000_000), 168_000_000);
        assert_eq!(awake_cycles(1, 84_000_000), 1);
        assert_eq!(awake_cycles(0, 84_000_000), 0);
        assert_eq!(awake_cycles(u64::MAX, u32::MAX), u64::MAX);
    }

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
    fn the_previous_run_is_ended_only_after_an_orderly_end() {
        assert_eq!(previous_run(false, ENDED_MARK), previous_run::ENDED);
        assert_eq!(previous_run(false, RUNNING), previous_run::NONE, "ein Reset mitten im Lauf");
        assert_eq!(previous_run(false, 0xDEAD_BEEF), previous_run::NONE, "nach dem Einschalten steht irgendetwas");
        assert_eq!(previous_run(true, RUNNING), previous_run::WATCHDOG);
        assert_eq!(previous_run(true, ENDED_MARK), previous_run::WATCHDOG, "der Watchdog geht vor");
    }

    #[test]
    fn nothing_sleeps_shorter_than_the_platform_can() {
        assert_eq!(rtc_wakeup(-5, LSE), rtc_wakeup(MIN_DEEP_SLEEP_NS, LSE));
        assert_eq!(deep_sleep_us(0), 1_000);
        assert_eq!(deep_sleep_us(2_000_000_001), 2_000_001);
    }
}
