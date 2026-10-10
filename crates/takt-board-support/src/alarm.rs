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
        Compare::At((target & u64::from(u32::MAX)) as u32)
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
}
