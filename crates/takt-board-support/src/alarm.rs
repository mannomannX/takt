//! Der Alarm der Interruptform (12.11): ein freilaufender Zaehler als
//! Zeitachse des Laufs, und wie ein Vergleichsregister auf eine Frist
//! gestellt wird.
//!
//! In der Interruptform rechnet der Schritt in der ISR eines Zeitgebers,
//! der nicht periodisch feuert, sondern einmal zu der Frist, die `service`
//! nennt. Die Zeitachse liefert ein freilaufender Zaehler: auf dem F401
//! TIM2 mit 32 Bit und seinen Ueberlaeufen, auf dem C6 der SYSTIMER mit
//! 52 Bit. Hier steht die Rechnung dazwischen: Zaehlerstand und
//! Ueberlaeufe zu einem 64-Bit-Stand und ob ein Vergleich in dieser Epoche
//! faellt, schon faellig ist oder erst nach einem Ueberlauf kommt. Zwischen
//! Nanosekunden und Zaehlerstaenden rechnet [`crate::clock::Scale`].

/// Der 64-Bit-Stand aus `epoch` Ueberlaeufen und dem Zaehlerstand `low`.
///
/// Ein Ueberlauf, dessen Interrupt noch ansteht (`wrapped`), zaehlt schon
/// mit, wenn `low` nach ihm gelesen wurde — erkennbar an einem kleinen
/// Wert. Ohne das liefe die Zeit beim Lesen zwischen Ueberlauf und
/// Interrupt um 2^32 Schritte zurueck.
pub fn counts(epoch: u32, low: u32, wrapped: bool) -> u64 {
    let epoch = if wrapped && low < 1 << 31 { u64::from(epoch) + 1 } else { u64::from(epoch) };
    (epoch << 32) | u64::from(low)
}

/// Wie ein Vergleich auf einen Zielstand steht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    /// Das Ziel ist erreicht: Die ISR soll sofort laufen.
    Due,
    /// Das Ziel liegt in dieser Epoche: das Vergleichsregister auf diesen
    /// Wert. Liest der Zaehler nach dem Schreiben schon das Ziel oder mehr,
    /// war es zu knapp, und es gilt [`Compare::Due`] ([`passed`]).
    At(u32),
    /// Das Ziel liegt hinter dem naechsten Ueberlauf: Dessen Interrupt
    /// fragt erneut.
    Later,
}

/// Wie ein Vergleich auf `target` steht, wenn der Zaehler bei `now` steht.
pub fn compare(target: u64, now: u64) -> Compare {
    if target <= now {
        Compare::Due
    } else if target >> 32 == now >> 32 {
        // Die unteren 32 Bit des Ziels, ohne die Epoche.
        Compare::At(crate::halves(target).1)
    } else {
        Compare::Later
    }
}

/// Hat der Zaehler das Ziel erreicht, waehrend das Vergleichsregister
/// gestellt wurde? Dann loest der Vergleich nicht mehr aus, und die ISR
/// muss von Hand anstehen.
pub fn passed(target: u64, now: u64) -> bool {
    now >= target
}

/// **Die naechste Grenze eines periodischen Weckers** (12.3, FB-742): die
/// erste `last + n·period` mit `n >= 1` nach `now`.
///
/// Der eigene Kern weckt sich an jeder Tickgrenze ueber denselben Vergleich
/// wie die Interruptform. Stand der Kern laenger als eine Periode — eine
/// Sektorloeschung haelt ihn auf dem F401 197 ms an —, liegen die naechsten
/// Grenzen schon zurueck; sie fallen fuer das Wecken weg, denn gezaehlt
/// werden die Ticks an der Zeitachse ([`ticks_since`]), nicht am Wecken.
pub fn next_boundary(last: u64, period: u32, now: u64) -> u64 {
    let period = u64::from(period.max(1));
    let behind = now.saturating_sub(last) / period;
    last.saturating_add(period.saturating_mul(behind.saturating_add(1)))
}

/// Die Ticks seit `origin` am Stand `now` der Zeitachse, je `counts`
/// Schritte einer (FB-742): Der Zaehler ist die Zeit, der Interrupt nur das
/// Wecken, und was der Kern verschlief, zaehlt mit.
pub fn ticks_since(origin: u64, counts: u32, now: u64) -> u64 {
    now.saturating_sub(origin).checked_div(u64::from(counts)).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein Ueberlauf, dessen Interrupt noch ansteht, zaehlt nur fuer einen
    /// Stand, der nach ihm gelesen wurde.
    #[test]
    fn a_pending_wrap_counts_only_after_it() {
        assert_eq!(counts(3, 5, false), (3 << 32) | 5);
        assert_eq!(counts(3, 5, true), (4 << 32) | 5);
        assert_eq!(counts(3, u32::MAX - 2, true), (3 << 32) | u64::from(u32::MAX - 2));
    }

    /// Faellig, in dieser Epoche oder hinter dem naechsten Ueberlauf.
    #[test]
    fn a_target_is_due_in_this_epoch_or_later() {
        let now = (2 << 32) | 100;
        assert_eq!(compare(now, now), Compare::Due);
        assert_eq!(compare(now - 1, now), Compare::Due);
        assert_eq!(compare(now + 5, now), Compare::At(105));
        assert_eq!(compare((3 << 32) | 1, now), Compare::Later);
        assert!(passed(now, now) && !passed(now + 1, now));
    }

    /// Ein periodischer Wecker auf der Zeitachse: Er weckt an jeder Grenze,
    /// und nach dem Wecken steht die naechste immer vor ihm.
    #[test]
    fn the_next_boundary_lies_ahead() {
        assert_eq!(next_boundary(1_000, 1_000, 1_000), 2_000);
        assert_eq!(next_boundary(1_000, 1_000, 1_999), 2_000);
        assert_eq!(next_boundary(1_000, 1_000, 2_000), 3_000, "eine erreichte Grenze ist vorbei");
        assert_eq!(next_boundary(1_000, 1_050, 1_200), 2_050, "gestreckt vom Pruefgeraet");
        assert_eq!(next_boundary(5, 0, 5), 6, "eine leere Periode ist ein Schritt");
    }

    /// **Ein Stillstand verliert keinen Tick** (FB-742). Der Wecker steht auf
    /// Tick 6, der Kern steht von 5,5 bis 202,7 Perioden — eine
    /// Sektorloeschung —, und die ISR kommt einmal. Die Zahl der Ticks folgt
    /// der Zeitachse und nicht den Aufrufen der ISR: 202 statt 6. Der
    /// naechste Wecker steht auf Tick 203.
    #[test]
    fn a_stall_loses_no_tick() {
        let (origin, period) = (40_000, 10_000);
        let wake = |k: u64| origin + k * u64::from(period);
        let mut armed = wake(6);
        assert_eq!(ticks_since(origin, period, wake(5) + 5_000), 5);
        let after_stall = wake(202) + 7_000;
        assert!(passed(armed, after_stall), "der Vergleich fiel in den Stillstand");
        armed = next_boundary(armed, period, after_stall);
        assert_eq!(ticks_since(origin, period, after_stall), 202);
        assert_eq!(armed, wake(203));
        assert_eq!(ticks_since(origin, 0, after_stall), 0, "ohne Periode kein Tick");
    }
}
