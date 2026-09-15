//! Die Prüfungen, die eine Kalibrierung brauchen (12, 32; 7.2, 9.4.3).
//!
//! **Warum sie nicht in `checks.rs` stehen.** Jede andere Prüfung
//! entscheidet aus dem Programm allein. Diese beiden brauchen eine zweite
//! Eingabe — die gemessene Kostentabelle aus 13.8 —, und die hat nicht
//! jeder Aufrufer: Ein `takt check` ohne Hardware-Konfiguration ist der
//! Normalfall, kein Mangel. Sie als Pflichtfeld in [`crate::Options`] zu
//! führen hieße, dreiundfünfzig Aufrufstellen eine Angabe abzuverlangen,
//! die sie nicht machen können.
//!
//! Darum laufen sie hinterher und über ein fertiges Programm. Wer
//! kalibriert hat, ruft sie; wer nicht, bekommt weiterhin den Hinweis aus
//! `checks.rs`, der sagt, was fehlt.
//!
//! **Was sie melden, wenn etwas fehlt.** Nicht nichts. Eine Prüfung, die
//! bei fehlender Eingabe schweigt, ist von einer bestandenen Prüfung
//! nicht zu unterscheiden — genau der Zustand, den FB-136 festhielt. Eine
//! unvollständige Tabelle nennt darum die Klassen, die ihr fehlen, und
//! ein fehlendes `tick_source` sagt, dass Prüfung 32 es verlangt.

use takt_diag::{Diagnostic, Severity, Span};
use takt_mir::analysis::schedulability::{self, Load};
use takt_mir::hardware::Target;
use takt_mir::machine::MachineKind;
use takt_mir::program::Program;

use crate::checks::{SC12, SC32};

/// Prüft Kostenbudget und Schedulability gegen eine Kalibrierung.
///
/// `span` ist die Stelle, an der ein Fehler angezeigt wird — sinnvoll ist
/// der `system:`-Block, weil `tick` und `tick_source` dort stehen.
pub fn check(p: &Program, target: &Target, span: Span) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let load = schedulability::load(p);

    // Prüfung 32 verlangt zweierlei: die Ungleichung *und* dass
    // `tick_source` in der Hardware-Konfiguration steht. Das Zweite ist
    // unabhängig von der Kalibrierung und wird darum zuerst geprüft.
    if p.config.tick_source.is_none() {
        out.push(
            Diagnostic::new(
                Severity::Warning,
                SC32,
                span,
                "`tick_source` fehlt; Prüfung 32 verlangt sie in der Hardware-Konfiguration (7.1)".to_string(),
            )
            .with_suggestion("`system: tick_source = hw(\"…\")` nennt die Quelle, aus der der Tick kommt".to_string()),
        );
    }

    let Some(verdict) = load.judge(&target.c_target, target.t_io_ps) else {
        let fehlend: Vec<&str> = target.c_target.missing().iter().map(|c| c.name()).collect();
        out.push(
            Diagnostic::new(
                Severity::Warning,
                SC32,
                span,
                format!(
                    "die Kalibrierung für `{}` ist unvollständig: {} ohne Messwert",
                    target.name,
                    fehlend.join(", ")
                ),
            )
            .with_suggestion(
                "`takt bench` misst die fehlenden Klassen; eine Null wäre kein Messwert, sondern eine Lücke, \
                 und die Schedulability rechnete dann zu günstig"
                    .to_string(),
            ),
        );
        return out;
    };

    if !verdict.fits() {
        out.push(
            Diagnostic::error(
                SC32,
                span,
                format!(
                    "die Rechenlast eines Ticks passt nicht: {} ns nötig, {} ns verfügbar ({} %)",
                    ns(verdict.needed_ps),
                    ns(verdict.available_ps),
                    verdict.utilisation_percent()
                ),
            )
            .with_suggestion(
                "Periode erhöhen, Phase verschieben oder Fault-Pfade verkleinern (7.2); `takt cost` zeigt, \
                 welcher Zustand welche Klasse treibt"
                    .to_string(),
            ),
        );
    }

    out.extend(declared_budgets(p, target, &load));
    out
}

/// Prüfung 12: `with budget = {wcet = …}` je Maschine.
///
/// **Je Aktivierung, nicht je Tick.** 9.4.3 nennt `B_m` „eine obere
/// Schranke der pro Aktivierung von m ausgeführten abstrakten
/// Operationen"; eine Maschine mit `n_m = 10` belastet den Tick nur jeden
/// zehnten, aber ihr deklariertes `wcet` gilt für den Durchlauf. `T_IO`
/// geht hier nicht ein — es ist eine Eigenschaft des Ticks, nicht der
/// Maschine.
fn declared_budgets(p: &Program, target: &Target, _load: &Load) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for m in p.machines.iter().filter(|m| m.kind != MachineKind::Template) {
        let (Some(declared), Some(budget)) = (m.declared_budget, m.budget) else { continue };
        let Some(want) = declared.wcet_ns else { continue };
        let have_ps = target.c_target.duration_ps(budget.activation + budget.fault_path);
        let want_ps = (want.max(0) as u64).saturating_mul(1000);
        if have_ps > want_ps {
            out.push(
                Diagnostic::error(
                    SC12,
                    declared.span,
                    format!("`{}` braucht {} ns je Aktivierung, deklariert sind {want} ns", m.name, ns(have_ps)),
                )
                .with_suggestion(
                    "Budget anheben, Schleifen verkürzen oder Fault-Pfade verkleinern (9.4.3); der Fault-Pfad \
                     zählt mit, weil er im selben Tick läuft (5.4)"
                        .to_string(),
                ),
            );
        }
    }
    out
}

/// Pikosekunden als Nanosekunden, kaufmännisch gerundet.
fn ns(ps: u64) -> u64 {
    ps.saturating_add(500) / 1000
}
