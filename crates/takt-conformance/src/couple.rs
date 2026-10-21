//! Die Kopplung zweier Programme eines Kerns auf der Seite der Tests (12.11):
//! welchen Stempel ein gekoppelter Eingang in einem Tick erwartet, und der
//! Stimulus eines Programms aus dem Trace des anderen.
//!
//! Vor dem Ursprung rechnet jedes Programm in der Rangfolge seinen Tick 0
//! bis zum Commit; ab dem Ursprung beginnt Tick k an der Grenze (k − 1)·T,
//! und sein `now` ist k·T. Der Stempel eines gekoppelten Werts ist das `now`
//! des Ticks, dessen Wert er traegt. Der Rahmen des Lesers prueft denselben
//! Stempel (`takt_frame::mcu`, `coupling_check`).

use std::collections::BTreeMap;

/// Der Schreiber eines gekoppelten Eingangs, wie die Hardware-Konfiguration
/// ihn nennt (8.10: `from_tick_ns`, `from_timing`, `from_first`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Writer {
    /// Der Tick des Schreibers in Nanosekunden.
    pub tick_ns: i64,
    /// `output_timing = boundary`.
    pub boundary: bool,
    /// Der Schreiber rechnet an einer gemeinsamen Grenze vor dem Leser.
    pub higher: bool,
}

/// Der Stempel, den der Eingang in Tick `k` eines Lesers mit Tick
/// `reader_ns` erwartet; `None`, solange der Schreiber noch nichts
/// freigegeben haben kann (12.11).
///
/// In Tick 0 sieht der Leser nur den Commit von Tick 0 eines hoeheren
/// Schreibers unter `asap`, Stempel 0; einen Latch gibt noch niemand frei.
/// Danach sieht er an seiner Grenze b vom hoeheren Schreiber den Tick, den
/// dieser an seiner letzten Grenze ≤ b begann (`asap`: dessen Commit,
/// Stempel die Grenze plus `T_W`; `boundary`: die Freigabe an dieser Grenze,
/// Stempel die Grenze), vom niedrigeren die Freigabe an dessen letzter
/// Grenze vor b.
pub fn due(reader_ns: i64, k: u64, w: Writer) -> Option<i64> {
    if k == 0 {
        return (w.higher && !w.boundary).then_some(0);
    }
    let b = i64::try_from(k - 1).ok()?.checked_mul(reader_ns)?;
    let tw = w.tick_ns;
    match (w.higher, w.boundary) {
        (true, false) => (b / tw).checked_mul(tw)?.checked_add(tw),
        (true, true) => (b / tw).checked_mul(tw),
        (false, _) => (b > 0).then(|| (b - 1) / tw * tw),
    }
}

/// Der Wert des Ausgangs `output` je Tick, an dem er sich aenderte, aus
/// einem Trace (`t=<k> out <output> <wert>`).
pub fn changes(trace: &str, output: &str) -> BTreeMap<u64, String> {
    let marker = format!(" out {output} ");
    trace
        .lines()
        .filter_map(|l| {
            let (tick, value) = l.trim_end().strip_prefix("t=")?.split_once(&marker)?;
            Some((tick.parse().ok()?, value.to_string()))
        })
        .collect()
}

/// Der Wert eines Ausgangs zu Tick `k`: der seiner letzten Aenderung bis `k`.
pub fn at(values: &BTreeMap<u64, String>, k: u64) -> Option<&str> {
    values.range(..=k).next_back().map(|(_, v)| v.as_str())
}

/// Der Stimulus des Eingangs `input` eines Lesers mit Tick `reader_ns` ueber
/// die Ticks 0 bis `ticks`, aus dem Trace `writer_trace` des Schreibers und
/// seinem Ausgang `output` (12.11): je Tick mit faelligem Stempel der Wert
/// des Ticks, dessen `now` der Stempel ist.
pub fn stimulus(writer_trace: &str, output: &str, input: &str, reader_ns: i64, w: Writer, ticks: u64) -> String {
    let values = changes(writer_trace, output);
    (0..=ticks)
        .filter_map(|k| {
            let stamp = due(reader_ns, k, w)?;
            let n = u64::try_from(stamp / w.tick_ns).ok()?;
            Some(format!("t={k} in {input} {} t={stamp}\n", at(&values, n)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    /// **Die Faelle aus 12.11** am Paar der Board-Tests: `slow` (10 ms) liest
    /// `fast` (1 ms, `asap`), `fast` liest `slow` unter `boundary`.
    #[test]
    fn the_due_stamps_follow_the_time_axis() {
        let fast = Writer { tick_ns: MS, boundary: false, higher: true };
        // Tick 0 von `slow` sieht Tick 0 von `fast`; Tick 1 am Ursprung den
        // Commit von Tick 1 von `fast`, der dort vorher rechnete; Tick 2 an
        // 10 ms den von Tick 11.
        assert_eq!(
            (due(10 * MS, 0, fast), due(10 * MS, 1, fast), due(10 * MS, 2, fast)),
            (Some(0), Some(MS), Some(11 * MS))
        );
        let slow = Writer { tick_ns: 10 * MS, boundary: true, higher: false };
        // Am Ursprung noch nichts; an 1 ms bis 10 ms die Freigabe von 0, an
        // 10 ms tastet `fast` vor `slow` ab; ab 11 ms die von 10 ms.
        let seen: Vec<Option<i64>> = [0, 1, 2, 11, 12, 21, 22].iter().map(|&k| due(MS, k, slow)).collect();
        assert_eq!(seen, [None, None, Some(0), Some(0), Some(10 * MS), Some(10 * MS), Some(20 * MS)]);
        let latched = Writer { tick_ns: MS, boundary: true, higher: true };
        assert_eq!(
            (due(10 * MS, 0, latched), due(10 * MS, 1, latched), due(10 * MS, 2, latched)),
            (None, Some(0), Some(10 * MS))
        );
    }

    /// Der Stimulus nimmt den Wert des Ticks, dessen `now` der Stempel ist.
    #[test]
    fn the_stimulus_carries_the_value_of_the_stamped_tick() {
        let trace = "t=0 out count 0\nt=11 out count 1\nt=21 out count 2\n";
        let fast = Writer { tick_ns: MS, boundary: false, higher: true };
        let s = stimulus(trace, "count", "seen", 10 * MS, fast, 3);
        assert_eq!(
            s,
            "t=0 in seen 0 t=0\nt=1 in seen 0 t=1000000\nt=2 in seen 1 t=11000000\nt=3 in seen 2 t=21000000\n"
        );
    }
}
