//! Der Zyklenzaehler des Kerns (12.3, 13.8).
//!
//! **Das ist das Stueck, wegen dem M5 Hardware braucht.** 12.3 verlangt
//! „Verifikation per Zyklenzaehler (DWT) in HIL-Laeufen", und aus diesen
//! Messungen entsteht `c_target` (13.8) — die Tabelle, die aus „N
//! Operationen" (9.4.3) echte Zeit macht. Ohne sie bleibt die
//! Schedulability (7.2) unbeweisbar und `wcet` im Budget abgelehnt.
//!
//! **Der DWT gehoert dem Kern, nicht dem Chip.** Er steht in `cortex-m`
//! und nicht in der PAC, weil ARM ihn definiert. Derselbe Code misst
//! spaeter auf jedem anderen Cortex-M — der nRF52 ist ebenfalls ein M4.
//!
//! **Messen ohne Debugger.** Ein SWD-Anschluss kann den DWT von aussen
//! lesen, aber noetig ist er nicht: Das Programm liest sein eigenes
//! Register. Was dabei fehlt, ist die Gegenprobe — ein Messfehler im
//! Programm faellt nicht auf. Fuer `takt bench` mit festen Referenzkernen
//! (13.8) genuegt es; fuer die Fehlersuche bleibt der Debugger wertvoll.
//!
//! Die Umrechnung von Zyklen in Zeit steht in `takt-board-support`, wo
//! sie getestet ist.

use cortex_m::peripheral::{DCB, DWT};
use takt_board_support::Measurement;

/// Schaltet den Zyklenzaehler ein.
///
/// Er braucht zwei Schritte: Der Debug-Block muss die Zaehleinheit
/// freigeben (`DCB::enable_trace`), erst dann laeuft der DWT. Ohne den
/// ersten bleibt der Zaehler auf null stehen — stumm, nicht laut, und
/// darum eine gute Falle. [`running`] fragt danach.
pub fn enable(dcb: &mut DCB, dwt: &mut DWT) {
    dcb.enable_trace();
    dwt.enable_cycle_counter();
}

/// Laeuft der Zaehler?
///
/// Wer misst, ohne das zu pruefen, bekommt lauter Nullen und haelt sie
/// fuer schnellen Code.
pub fn running() -> bool {
    DWT::cycle_counter_enabled()
}

/// Der aktuelle Zaehlerstand.
///
/// 32 Bit: Bei 84 MHz laeuft er alle 51 Sekunden ueber. Fuer eine Messung
/// zwischen zwei Punkten ist das unerheblich (`Measurement::between`
/// rechnet ueber den Ueberlauf hinweg); fuer eine absolute Zeitangabe
/// taugt er nicht — dafuer gibt es die Tickzaehlung.
pub fn now() -> u32 {
    DWT::cycle_count()
}

/// Misst einen Abschnitt in Zyklen.
///
/// Der Aufruf selbst kostet ein paar Zyklen; wer sehr kurze Abschnitte
/// misst, misst sie mit. `takt bench` (13.8) umgeht das, indem seine
/// Referenzkerne lange genug laufen, dass der Aufwand verschwindet — eine
/// Kalibrierung, die den Messaufwand herausrechnet, waere eine zweite
/// Fehlerquelle.
pub fn measure<F: FnOnce()>(core_hz: u32, f: F) -> Measurement {
    let start = now();
    f();
    Measurement::between(start, now(), core_hz)
}
