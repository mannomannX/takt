//! Was ein Board liefern muss (12.3).
//!
//! **Die Grenze verlaeuft an der Frage „braucht es ein Register?".** Was
//! ein Register anfasst, gehoert ins Board-Crate; was daraus folgt, steht
//! hier. Der Tick etwa: *Dass* ein Timer-Interrupt ein Flag setzt und die
//! Hauptschleife es abholt, ist die Regel aus 12.3 und steht hier; *welcher*
//! Timer und wie man ihn scharf schaltet, steht im Board.
//!
//! Der Schnitt folgt `takt-hal`, wo derselbe Gedanke die Treiber traegt:
//! Der Simulationstreiber ist eine Trait-Implementierung, kein zweiter
//! Programmpfad (8.3). Hier ist es dieselbe Konstruktion, eine Ebene
//! tiefer — und sie hat denselben Zweck: Eine Tickschleife, nicht zwei.

/// Der Hardware-Timer, der den Basis-Tick erzeugt (7.1, 12.3).
///
/// 12.3 gibt das Muster vor: „die ISR setzt ein Flag und sampelt ggf.
/// zeitkritische Inputs; die Hauptschleife fuehrt den Tick aus. Ist das
/// Flag beim naechsten Interrupt noch gesetzt → `Runtime(Overrun)`."
///
/// **Der Zaehler gehoert in die ISR, nicht in die Schleife.** Wer in der
/// Hauptschleife zaehlt, verliert jeden Tick, den ein zu langer Schritt
/// ueberdeckt — und damit genau die Information, die den Overrun belegt.
pub trait TickSource {
    /// Wie viele Tick-Ereignisse seit dem Start aufgetreten sind.
    ///
    /// Monoton steigend, von der ISR erhoeht. Ein Sprung um mehr als eins
    /// zwischen zwei Abfragen heisst: Die Schleife hat Ticks verpasst.
    fn ticks(&self) -> u64;

    /// Die zuletzt gemessene Periode in Nanosekunden.
    ///
    /// Sie ist der *gemessene* Wert; `tick` in `system:` bleibt der
    /// nominale (7.1). Aus der Abweichung entsteht die Pruefung gegen
    /// `tick_tolerance` — siehe [`crate::tolerance`].
    fn last_period_ns(&self) -> i64;

    /// Die Zeit des Timers in Nanosekunden, feiner als ein Tick: fuer
    /// `took` und `drift` (7.3). Monoton, ab einem beliebigen Nullpunkt.
    fn now_ns(&self) -> i64;

    /// Wartet, bis das naechste Tick-Ereignis vorliegt.
    ///
    /// Auf einer MCU ist das ein `WFI` mit anschliessender Pruefung des
    /// Flags, nicht eine Warteschleife: Ein Kern, der zwischen den Ticks
    /// rechnet, verbraucht Strom fuer nichts und heizt die Messung auf.
    fn wait_for_tick(&mut self);
}

/// Der Hardware-Watchdog (12.3, 12.4).
///
/// Anders als [`takt_rt_core::Watchdog`] kennt er die Reset-Ursache: 12.3
/// verlangt, dass Outputs nach einem Watchdog-Reset auf `safe` stehen,
/// *bevor* das Programm neu startet. Das ist keine Aufgabe der Schleife,
/// sondern des Starts — die Schleife sieht nur noch das Ergebnis.
pub trait HardwareWatchdog {
    /// Bestaetigt den durchgelaufenen Tick.
    fn kick(&mut self);

    /// Kam der letzte Reset vom Watchdog?
    ///
    /// Wahr heisst: Der vorige Lauf ist nicht sauber beendet worden. Der
    /// Start setzt dann alle Outputs auf `safe` (12.3) und vermerkt es im
    /// Lauf-Header (11.3) — ein Neustart nach Watchdog ist ein Befund,
    /// kein Normalfall.
    fn reset_was_watchdog(&self) -> bool;
}

/// Der Schutzbereich unter dem Stack (12.3).
///
/// Zwei Bauformen, je nach Target dieselbe Aufgabe: eine MPU-Region ohne
/// Zugriff, wo es eine MPU gibt, sonst ein Kanarienwort, das am Tickende
/// geprueft wird. Beide beweisen im Fehlerfall dasselbe — einen Fehler in
/// der TCB, nicht im Programm, denn „das Programm kann per Konstruktion
/// nicht ausserhalb seiner Objekte schreiben" (12.3).
pub trait StackGuard {
    /// Ist der Schutzbereich unversehrt?
    ///
    /// `false` ist ein `Runtime(Hardware)`-Fault. Die MPU-Variante meldet
    /// ihn schon beim Zugriff; die Kanarienwort-Variante erst hier, und
    /// darum wird am Ende jedes Ticks gefragt.
    fn intact(&self) -> bool;
}

/// Der Schlafmodus (9.9, 12.3).
///
/// `idle`-Zustaende werden auf der MCU zu WFI/STOP mit Wake-Quellen als
/// Interrupts. Satz 9.9.1 verlangt, dass der Trace mit und ohne Schlaf
/// derselbe ist — der Schlaf darf die Zeit also nicht anhalten, sondern
/// nur die Rechnung aussetzen.
pub trait Sleep {
    /// Schlaeft bis zum naechsten Tick-Ereignis oder einer Wake-Quelle.
    ///
    /// Gibt zurueck, wie viele Tick-Ereignisse waehrend des Schlafs
    /// vergangen sind. Die Schleife rechnet sie als virtuelle Ticks nach
    /// (9.9); der Schlaf selbst erzeugt keine.
    fn sleep_until_event(&mut self) -> u64;
}
