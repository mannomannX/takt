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

    /// Ein Ausgang aus dem Latch, nach Stellung in der Speicherform.
    fn takt_mcu_output(index: i32) -> i64;

    /// Gibt den Latch an die Treiber (12.1, Schritt 10).
    ///
    /// Ruft je gebundenem Ausgang `takt_out_<adresse>` — die Funktionen,
    /// die das Board stellt.
    fn takt_mcu_commit();

    /// Sind alle Maschinen in `idle` und ohne vorgemerkten Fault (9.9)?
    fn takt_mcu_idle() -> bool;

    /// Ticks bis zur fruehesten `after`-Frist; `-1` heisst keine.
    fn takt_mcu_deadline() -> i64;
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

impl Generated {
    /// Gibt den Latch als Trace-Zeilen aus (`grammar/trace.md`).
    ///
    /// **Getrennt von [`Program::tick`], weil die Leitung langsamer ist
    /// als der Tick.** Eine Zeile ueber UART dauert bei 115200 Baud rund
    /// 1,7 ms; bei 1 ms Tickperiode kaeme eine Schleife, die je Tick
    /// ausgibt, nie zum Rechnen. Wer vergleichen will, ruft das hier in
    /// dem Takt, den die Leitung traegt — 12.8 nennt `states` als
    /// Instrumentierungs-Default fuer `baremetal`, nicht `statements`.
    pub fn dump(&self) {
        unsafe { takt_mcu_dump() };
    }

    /// Gibt den Latch an die Treiber (12.1, Schritt 10).
    ///
    /// **Die Stelle, an der ein Takt-Programm die Welt erreicht.** Bis
    /// hierher ist alles Rechnung; erst hier wird aus dem Latch eine
    /// Wirkung. Der erzeugte Rahmen ruft je gebundenem Ausgang eine
    /// Funktion, deren Name aus dem Pfad in `@ hw(...)` entsteht — aus
    /// `hw("ui/led")` wird `takt_out_ui_led`. Wer die Peripherie besitzt,
    /// stellt sie bereit; fehlt eine, meldet es der Linker (9.5).
    pub fn commit(&self) {
        unsafe { takt_mcu_commit() };
    }

    /// Liest einen Ausgang aus dem Latch, nach Stellung.
    ///
    /// **Fuer Diagnose, nicht fuers Stellen** — das macht [`commit`]. Hier
    /// kommt man an einen Wert heran, ohne ihn auszugeben: beim Bring-up,
    /// bevor ein Treiber existiert, oder wenn ein Test den Latch prueft.
    ///
    /// `index` ist die Stellung in der Speicherform; der erzeugte Rahmen
    /// schreibt die Namen dazu in seinen Kopf. Ein unbekannter Index gibt
    /// null zurueck, statt den Lauf anzuhalten (4.1).
    ///
    /// [`commit`]: Generated::commit
    pub fn output(&self, index: i32) -> i64 {
        unsafe { takt_mcu_output(index) }
    }
}

impl Program for Generated {
    /// 9.9: Vier der sechs Konjunkte beantwortet der erzeugte Code. Die
    /// geplanten Ausgaben und Jobs kennt der MCU-Rahmen nicht, und die
    /// Wake-Fenster haetten nur Stroeme — beides gibt es dort noch nicht.
    fn sleep_allowed(&self) -> bool {
        unsafe { takt_mcu_idle() }
    }

    fn next_deadline(&self) -> Option<i64> {
        let ticks = unsafe { takt_mcu_deadline() };
        (ticks >= 0).then_some(ticks)
    }

    fn tick(&mut self, k: u64, _now: i64) {
        // `now` kommt vom Rahmen, nicht von der Schleife: Er rechnet es
        // aus `k` und der nominalen Periode, die im erzeugten Code steht.
        // Zwei Quellen fuer dieselbe Zeit waeren eine zu viel.
        unsafe { takt_mcu_tick(k as i64) };
        if self.trace {
            self.dump();
        }
    }
}
