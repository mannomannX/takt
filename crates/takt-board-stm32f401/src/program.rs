//! Die Bruecke zwischen erzeugtem Code und Tickschleife (12.1).
//!
//! **Warum sie hier steht und nicht im Kern.** `takt-rt-core` kennt die
//! Semantik nicht — es ruft [`takt_rt_core::Program::tick`] und weiss
//! nicht, was dahinter liegt. Der erzeugte Code spricht die C-ABI, und
//! die anzusprechen braucht `extern "C"`; das verbietet der Workspace
//! (13.4), erlaubt aber dieses Crate, weil es ohnehin zur Trusted
//! Computing Base gehoert (9.5).
//!
//! Was hier steht, ist darum so wenig wie moeglich: drei Deklarationen
//! und ein Trait mit drei Zeilen. Die Arbeit macht der Rahmen aus
//! `takt-conformance::mcu`, der zusammen mit dem Programm uebersetzt und
//! gebunden wird.
//!
//! **Die Gegenrichtung fehlt bewusst.** Der Rahmen ruft
//! `takt_board_trace`, um Traces auszugeben; diese Funktionen stellt das
//! *Programm*, nicht dieses Crate — es weiss nicht, ob die Telemetrie
//! ueber USART1, einen Ringpuffer oder gar nicht geht. Wer den Rahmen
//! bindet, liefert sie (siehe `takt-bringup-stm32f401`).

use takt_rt_core::Program;

unsafe extern "C" {
    /// Einmal vor dem ersten Tick (12.1, Schritt 1).
    ///
    /// Setzt Prozessabbild, Latch und Maschinenzustaende auf null, traegt
    /// die Parameter ein und ruft `<maschine>_init` fuer jede Maschine.
    fn takt_mcu_init();

    /// Ein Tick (12.1, Schritte 2 bis 10).
    ///
    /// `k` ist die Tickzahl; der Rahmen rechnet daraus `now` mit der
    /// nominalen Periode aus `system: tick` — nicht aus der gemessenen,
    /// weil die logische Zeit die nominale ist (7.1).
    fn takt_mcu_tick(k: i64);

    /// Die Ausgaenge als Trace-Zeilen (`grammar/trace.md`).
    fn takt_mcu_dump();
}

/// Das erzeugte Programm als [`Program`] der Tickschleife.
///
/// Ein leerer Typ: Der ganze Zustand steht im erzeugten Code, statisch in
/// `.bss` (12.3). Diese Struktur ist nur der Griff daran.
#[derive(Clone, Copy, Debug, Default)]
pub struct Generated {
    /// Soll nach jedem Tick der Latch ausgegeben werden?
    ///
    /// Fuer den Vergleich mit dem Interpreter ja; im Betrieb nein, weil
    /// die Ausgabe ueber UART laenger dauert als der Tick. 12.8 nennt
    /// darum `states` als Instrumentierungs-Default fuer `baremetal` —
    /// nicht `statements` wie auf Linux.
    pub trace: bool,
}

impl Generated {
    /// Bereitet das Programm vor.
    ///
    /// **Genau einmal, vor dem ersten Tick.** Ein zweiter Aufruf setzte
    /// die Maschinenzustaende zurueck, und der Lauf begaenne mitten im
    /// Trace von vorn.
    pub fn init(trace: bool) -> Generated {
        // Sicher, weil der Rahmen nichts tut, was ihn braeuchte: Er
        // schreibt nur in seine eigenen statischen Puffer, die kein
        // anderer Code kennt.
        unsafe { takt_mcu_init() };
        Generated { trace }
    }
}

impl Program for Generated {
    fn tick(&mut self, k: u64, _now: i64) {
        // `now` kommt vom Rahmen, nicht von der Schleife: Er rechnet es
        // aus `k` und der nominalen Periode, die im erzeugten Code steht.
        // Zwei Quellen fuer dieselbe Zeit waeren eine zu viel.
        unsafe { takt_mcu_tick(k as i64) };
        if self.trace {
            unsafe { takt_mcu_dump() };
        }
    }
}
