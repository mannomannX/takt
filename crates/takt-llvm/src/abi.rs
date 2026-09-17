//! Was der erzeugte Code von der Runtime ruft (9.3, 5.4).
//!
//! Der Tickschritt rechnet, aber er *meldet* auch: `alert`, `log`,
//! `measure` und `verify` sind Beobachtungen (9.3) — sie aendern den
//! Zustand nicht, sie tragen etwas nach draussen. Und `abort` beendet den
//! Lauf aller Maschinen (5.4).
//!
//! Beides kann der erzeugte Code nicht selbst: Ein Trace-Eintrag braucht
//! einen Puffer, ein `abort` die anderen Maschinen. Er ruft darum die
//! Runtime, und diese Aufrufe sind eine ABI wie das Prozessabbild.
//!
//! **Die Signaturen tragen keine Zeichenketten, sondern Nummern.** Ein
//! `alert` traegt seinen Text im Programm, nicht im Aufruf: Die Meldung
//! steht in einer Tabelle, die der Compiler erzeugt, und der Aufruf nennt
//! ihren Index. Das spart auf einer MCU den Text im RAM (12.3), macht die
//! Signatur unabhaengig von der Zeichenkettendarstellung — und der
//! Trace bleibt kanonisch, weil die Runtime den Text aus derselben
//! Tabelle nimmt wie der Report.

use crate::emit::Module;
use crate::ty::LlvmType;

/// Die Funktionen, die die Runtime bereitstellt.
///
/// Sie werden deklariert, nicht definiert: Der erzeugte Code ruft sie,
/// `takt-rt-core` liefert sie.
pub struct Abi;

impl Abi {
    /// `alert cond, "text"` (9.3): Eine Flanke wird gemeldet.
    ///
    /// `machine` und `site` identifizieren die Stelle, `active` ist der
    /// Wert der Bedingung in diesem Tick. Die Runtime bildet daraus die
    /// Flanke — sie kennt den vorigen Wert, der erzeugte Code muesste ihn
    /// sonst im Zustand fuehren.
    pub const ALERT: &'static str = "takt_alert";

    /// `log "text"` (9.3).
    pub const LOG: &'static str = "takt_log";

    /// `measure name = e` (13.2): ein Messwert fuer den Report.
    pub const MEASURE: &'static str = "takt_measure";

    /// `verify cond, "text"` (13.2): eine Pruefung mit Verdikt.
    pub const VERIFY: &'static str = "takt_verify";

    /// `abort "text"` (5.4): Fault fuer *alle* Maschinen im selben Tick.
    pub const ABORT: &'static str = "takt_abort";

    /// Ein Fault-Uebergang (5.3): Maschine und der verlassene Zustand. Die
    /// Runtime schreibt ihn in den Trace, damit ein nativer Lauf sagt, *wo*
    /// er vom Interpreter abwich — nicht nur, dass die Outputs anders sind.
    pub const FAULT: &'static str = "takt_fault";

    /// `now` (3.3): die Dauer seit dem Start des Laufs.
    ///
    /// Sie steht nicht im Zustand einer Maschine, sondern gehoert der
    /// Runtime: Alle Maschinen lesen dieselbe Uhr, und der Tickzaehler
    /// liegt in der Tickschleife (12.1). Eine Kopie je Maschine waere
    /// eine zweite Quelle fuer dieselbe Zahl.
    pub const NOW: &'static str = "takt_now";

    /// `job v = f(args)` (4.5): `(machine, slot, native, args, len)` — die
    /// Argumente als Folge kanonischer Bloecke (je `u32` Laenge, dann die
    /// Bytes). Die Runtime fuehrt den Job und schreibt `done`/`result` in
    /// den Slot des Abbilds (`image::job_offset`).
    pub const JOB_BEGIN: &'static str = "takt_job_begin";

    /// Ein Fault-Uebergang bricht die Jobs der Maschine ab (5.3):
    /// `(machine, slot)`; der Slot wird `done` mit `Err(CANCELLED)`.
    pub const JOB_CANCEL: &'static str = "takt_job_cancel";

    /// `verdict pass | fail` (13.2): das Urteil eines Tests.
    ///
    /// Wie `verify` eine reine Beobachtung — sie kann nie einen Fault
    /// ausloesen (Leitentscheidung 14). Der Report sammelt sie; die
    /// Runtime reicht sie weiter.
    pub const VERDICT: &'static str = "takt_verdict";

    /// `at T: o = v` (9.8): ein geplanter Schreibvorgang.
    ///
    /// **Die Warteschlange gehoert der Runtime, nicht dem Maschinen-
    /// zustand.** 11.2 sagt es woertlich: „`sched`-Warteschlangen als
    /// feste Arrays im Runtime-Anteil des Outputs". Das folgt der
    /// Zustaendigkeit — `apply_scheduled(k)` laeuft zu Tick-Beginn ueber
    /// *alle* Outputs, bevor irgendeine Maschine schreitet (9.8), und
    /// der Tickschritt kann das nicht tun.
    ///
    /// Das Ergebnis sagt, ob geplant werden konnte: `false` heisst
    /// `TimingFault` (der Zeitpunkt liegt nicht in der Zukunft) oder
    /// `ScheduleOverflow` (K_o erreicht). Beide sind Faults der
    /// Maschine, also nimmt der Aufrufer seinen Fault-Pfad.
    ///
    /// Der Wert geht als `i64`: Der Latch traegt je Output einen Wert
    /// fester Groesse, und `double` passt bitgleich hinein
    /// (`bitcast`). Eine zweite Signatur je Breite waere eine zweite
    /// Gelegenheit, sie verschieden zu waehlen.
    pub const SCHEDULE: &'static str = "takt_schedule";

    /// `cancel o` (9.8): verwirft die geplanten Schreibvorgaenge eines
    /// Outputs.
    pub const CANCEL: &'static str = "takt_cancel";

    /// Ein Laufzeitmonitor meldet eine Verletzung (13.3): `(index, position)`.
    ///
    /// Der Index zaehlt die Eigenschaften des Programms; die Position ist
    /// der Tick, an dem die Formel falsch ist — die Runtime traegt beides
    /// in den Trace, wie der Interpreter.
    pub const PROPERTY: &'static str = "takt_property";

    /// Das Fault-Flag einer reinen Funktion (4.1).
    ///
    /// Eine Funktion hat keinen eigenen Fault-Pfad — sie faultet den
    /// Aufrufer. Sie setzt darum dieses Flag, und der Aufrufer prueft es
    /// nach dem Aufruf; trifft er es gesetzt, nimmt er seinen eigenen
    /// Fault-Pfad.
    ///
    /// Eine Stelle genuegt: 9.4 kennt keinen nebenlaeufigen Zugriff auf
    /// den Zustand einer Maschine (Satz 9.4.1, die Schritte kommutieren),
    /// und innerhalb eines Schritts laeuft immer nur ein Aufruf.
    pub const FAULT_FLAG: &'static str = "takt_fn_fault";

    /// Schreibt die Deklarationen in den Modulkopf.
    ///
    /// Alle nehmen `(machine: i32, site: i32, ...)`: Die Stelle ist das,
    /// was der Trace braucht, und sie ist beim Uebersetzen bekannt.
    pub fn declare(m: &mut Module) {
        m.declare("\n; Runtime-Schnittstelle (9.3, 5.4); `takt-rt-core` liefert sie");
        m.declare(&format!("declare void @{}(i32, i32, i1)", Abi::ALERT));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::LOG));
        m.declare(&format!("declare void @{}(i32, i32, double)", Abi::MEASURE));
        m.declare(&format!("declare void @{}(i32, i32, i1)", Abi::VERIFY));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::ABORT));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::FAULT));
        m.declare(&format!("declare i64 @{}()", Abi::NOW));
        m.declare(&format!("declare void @{}(i32, i32, i1)", Abi::VERDICT));
        m.declare(&format!("declare void @{}(i32, i64)", Abi::PROPERTY));
        // 9.8: `(channel, T, wert) -> konnte geplant werden`.
        m.declare(&format!("declare i1 @{}(i32, i64, i64)", Abi::SCHEDULE));
        m.declare(&format!("declare void @{}(i32)", Abi::CANCEL));
        m.declare(&format!("declare void @{}(i32, i32, i32, ptr, i32)", Abi::JOB_BEGIN));
        m.declare(&format!("declare void @{}(i32, i32)", Abi::JOB_CANCEL));
        // `append` kopiert eine ganze Folge in einem Zug (3.9); LLVM
        // kennt das als Intrinsic, und eine Schleife braeuchte eine
        // Schranke, die 4.1 ohnehin verlangt.
        m.declare(&format!("@{} = external global i8", Abi::FAULT_FLAG));
        m.declare("declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)");
    }
}

/// Der Typ eines Meldungsindex.
pub const SITE: LlvmType = LlvmType::Int(32);
