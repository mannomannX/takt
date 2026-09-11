//! Kostenvertrag je Funktion (Referenz 9.4.3).
//!
//! Bis M4 rechnete das Kostenmodell jede Intrinsic gleich: ein `sin`
//! kostete so viel wie ein `abs` (`analysis/cost.rs`). Solange die
//! Mathematik aus der Plattform-libm kam, war das nicht besser zu machen
//! — niemand kannte die Kosten. Mit eigenen Implementierungen sind sie
//! bekannt, und sie gehoeren in das Budget, das die Schedulability (7.2)
//! und die Safe-State-Latenz (9.4.5) tragen.
//!
//! Die Zahlen sind **Operationen, keine Zeit**. Dieselbe Trennung wie
//! ueberall sonst: Die Operationszahl ist exakt und gilt immer, die
//! Umrechnung in Zeit braucht die kalibrierte Tabelle `c_target[c]`
//! (13.8, M5). Ein Kostenvertrag wie bei nativen Funktionen (4.5) — mit
//! dem Unterschied, dass diese Bibliothek nicht zur TCB gehoert, weil
//! ihre Ergebnisse gegen Vektoren geprueft sind.

use crate::Fun;

/// Operationen einer Funktion, nach den Klassen aus 9.4.3.
///
/// Nur die Klassen, die hier vorkommen: `f32`/`f64` fuer die Rechnung,
/// `call` fuer den Aufruf selbst. Speicher (`mem`) faellt nicht an —
/// keine Funktion dieses Crates liest oder schreibt Speicher.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// Operationen in der Klasse der gerechneten Breite.
    pub ops: u64,
    /// Aufrufe.
    pub call: u64,
}

/// Der Kostenvertrag einer Funktion.
///
/// Die Werte sind obere Schranken fuer die Operationszahl, nicht
/// gemessene Zeiten. Ein Hardwarebefehl kostet eine Operation; eine
/// Funktion mit Argumentreduktion und Polynom kostet, was ihr laengster
/// Pfad braucht — darum stehen bei den noch nicht kuratierten Funktionen
/// die Schaetzungen aus der CORE-MATH-Literatur, sichtbar als solche.
pub fn cost_of(f: Fun) -> Cost {
    match f {
        // Ein Hardwarebefehl (VSQRT, FSQRT, VFMA, FMADD, FMA3; 4.2).
        Fun::Sqrt | Fun::Fma => Cost { ops: 1, call: 1 },
        // Rundung und Vorzeichen: ein Befehl, oft ohne Aufruf.
        Fun::Round | Fun::Floor | Fun::Ceil | Fun::Trunc | Fun::Abs | Fun::CopySign => Cost { ops: 1, call: 0 },
        // Noch nicht kuratiert (13.8). Die Schranken stammen aus der
        // CORE-MATH-Literatur und werden mit der Implementierung durch
        // gemessene ersetzt; bis dahin sind sie bewusst grosszuegig —
        // ein zu kleines Budget waere schlimmer als ein zu grosses.
        Fun::Sin | Fun::Cos => Cost { ops: 30, call: 1 },
        Fun::Tan | Fun::Atan | Fun::Asin | Fun::Acos => Cost { ops: 50, call: 1 },
        Fun::Atan2 => Cost { ops: 60, call: 1 },
        Fun::Exp | Fun::Log => Cost { ops: 40, call: 1 },
        Fun::Pow => Cost { ops: 120, call: 1 },
    }
}
