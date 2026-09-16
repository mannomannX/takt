//! Die Tickquelle des Profils `linux_rt` (12.2, 7.1).
//!
//! **Absolute Deadlines, nicht relative Pausen.** 12.2 nennt
//! `clock_nanosleep` mit `TIMER_ABSTIME`, und der Grund steht in
//! `takt-rt-core`: Ein zu spaeter Tick darf die folgenden nicht
//! verschieben. Eine relative Pause („schlafe 10 ms") addiert ihren
//! eigenen Verzug bei jedem Durchlauf; eine absolute Frist („wache um
//! T auf") tut es nicht.
//!
//! **Die monotone Uhr, nicht die Systemzeit.** `CLOCK_MONOTONIC` springt
//! nicht: Weder eine Zeitumstellung noch NTP verschieben sie. 7.4 haelt
//! die Systemzeit ausdruecklich aus der Semantik heraus („nur als
//! Input-Channel"), und dieselbe Ueberlegung gilt fuer die Tickquelle.

use std::time::Instant;

use takt_rt_core::Clock;

/// Die Uhr des Profils `linux_rt`.
///
/// Sie misst in Nanosekunden seit ihrem Start, nicht seit der Epoche: Ein
/// Lauf braucht die Dauer seit seinem Anfang, und ein absoluter
/// Zeitpunkt waere eine Zahl, deren Groesse nichts bedeutet.
#[derive(Debug)]
pub struct RealtimeClock {
    /// Der Nullpunkt dieses Laufs.
    start: Instant,
    /// Wie oft `wait_until` zu spaet kam (7.3: der Tick ist ueberfaellig).
    ///
    /// Der Zaehler gehoert hierher und nicht in die Schleife: Er misst
    /// die *Uhr*, nicht das Programm. Ein Wert ueber null heisst, dass
    /// das Betriebssystem nicht geliefert hat, was 12.2 verlangt.
    pub late: u64,
}

impl Default for RealtimeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeClock {
    /// Eine Uhr, deren Nullpunkt jetzt ist.
    pub fn new() -> RealtimeClock {
        RealtimeClock { start: Instant::now(), late: 0 }
    }
}

impl Clock for RealtimeClock {
    fn now(&self) -> i64 {
        i64::try_from(self.start.elapsed().as_nanos()).unwrap_or(i64::MAX)
    }

    #[cfg(target_os = "linux")]
    fn wait_until(&mut self, deadline: i64) {
        let verbleibend = deadline - self.now();
        if verbleibend <= 0 {
            // Schon vorbei: Der Tick ist ueberfaellig (7.3). Nicht
            // schlafen und nicht aufholen — der Tick wird nie
            // uebersprungen, die Verzoegerung wird protokolliert.
            self.late = self.late.saturating_add(1);
            return;
        }
        // Die Frist gilt auf *unserer* Uhr (Nanosekunden seit dem Start
        // des Laufs), `clock_nanosleep` will sie auf `CLOCK_MONOTONIC`.
        // Umgerechnet wird ueber die verbleibende Dauer, nicht ueber
        // einen gemeinsamen Nullpunkt — den gibt es nicht.
        let jetzt = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let ziel_ns = i128::from(jetzt.tv_sec) * 1_000_000_000 + i128::from(jetzt.tv_nsec) + i128::from(verbleibend);
        let spec = rustix::thread::Timespec {
            tv_sec: (ziel_ns / 1_000_000_000) as _,
            tv_nsec: (ziel_ns % 1_000_000_000) as _,
        };
        // `TIMER_ABSTIME`: Ein zu spaeter Tick verschiebt die folgenden
        // nicht (12.2).
        let _ = rustix::thread::clock_nanosleep_absolute(rustix::thread::ClockId::Monotonic, &spec);
    }

    /// Ausserhalb von Linux gibt es `clock_nanosleep` nicht.
    ///
    /// Der Rueckfall schlaeft grob und wartet den Rest ab. Er traegt die
    /// Zusage aus 12.2 **nicht** — dafuer steht [`crate::Guarantee`], und
    /// auf einem Nicht-Linux meldet sie `Unknown`. Er ist da, damit sich
    /// die Runtime auf jedem Rechner bauen und testen laesst, nicht damit
    /// sie dort Echtzeit behauptet.
    #[cfg(not(target_os = "linux"))]
    fn wait_until(&mut self, deadline: i64) {
        let jetzt = self.now();
        if jetzt >= deadline {
            self.late = self.late.saturating_add(1);
            return;
        }
        let rest = (deadline - jetzt) as u64;
        // Grob schlafen, fein warten: Der Schlaf trifft die Frist nicht
        // genau, das Warten schon. Die Schwelle ist grosszuegig, weil ein
        // zu frueher Schlaf besser ist als ein zu langer.
        const GROB: u64 = 2_000_000;
        if rest > GROB {
            std::thread::sleep(std::time::Duration::from_nanos(rest - GROB));
        }
        while self.now() < deadline {
            std::hint::spin_loop();
        }
    }
}

/// Die Wanduhr (7.4) fuer den Treiberrand von `sys/clock`: Nanosekunden
/// seit der Unix-Epoche, ausserhalb der Semantik — ein Input wie jeder
/// andere, den die Simulation aus dem Stimulus nimmt.
#[cfg(target_os = "linux")]
pub fn wall_clock_ns() -> i64 {
    let t = rustix::time::clock_gettime(rustix::time::ClockId::Realtime);
    t.tv_sec.saturating_mul(1_000_000_000).saturating_add(t.tv_nsec)
}

/// Ausserhalb von Linux aus `SystemTime`; dieselbe Auskunft, ohne die
/// Zusage aus 12.2.
#[cfg(not(target_os = "linux"))]
pub fn wall_clock_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
