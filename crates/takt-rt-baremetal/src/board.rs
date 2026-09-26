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

    /// Schlaeft bis zum naechsten Interrupt, ausser der Zaehler steht schon
    /// bei `target` — spaetestens also bis zum Tick. Ein Board, das
    /// zwischen den Ticks etwas zu tun hat (den Ring leeren, wenn der Host
    /// ein Paket abgeholt hat), kehrt frueher zurueck; die Uhr prueft danach
    /// die Frist erneut.
    ///
    /// **Pruefung und Schlaf sind eins** (FB-296): Kaeme der Tick zwischen
    /// „noch nicht erreicht" und `wfi`, schliefe der Kern eine Periode zu
    /// lang. Darum prueft das Board bei gesperrten Interrupts — `wfi` weckt
    /// auch dann, und die ISR laeuft gleich danach.
    fn wait_event(&mut self, target: u64);

    /// Wartet, bis das naechste Tick-Ereignis vorliegt.
    ///
    /// Ein `wfi` je Ereignis, keine Warteschleife: Ein Kern, der zwischen
    /// den Ticks rechnet, verbraucht Strom fuer nichts und heizt die
    /// Messung auf.
    fn wait_for_tick(&mut self) {
        let target = self.ticks().saturating_add(1);
        while self.ticks() < target {
            self.wait_event(target);
        }
    }
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
