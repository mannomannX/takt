//! Der Kostenbericht (9.4.3, 7.2; FB-135).
//!
//! **Was der Compiler rechnet, soll man auch sehen koennen.** `B_m` und
//! `F_m` entstehen seit M3 in [`super::cost`] und standen bis hierher in
//! keinem Bericht: `takt size` schluesselt Bytes auf, `takt latency`
//! Ticks, fuer Kosten gab es nichts. Wer nach der Kalibrierung (M5) ein
//! Budget verkleinern muss, bekam „passt nicht" ohne das *woran*.
//!
//! **Die Einheit ist die Operation, nicht die Sekunde.** 9.4.3 zaehlt je
//! Klasse; was das in Zeit heisst, sagt erst `c_target` aus der Messung
//! (13.8). Der Bericht nennt darum Operationen und sagt dazu, was ihm
//! fehlt — wie `takt latency` es mit seiner Zeitspalte vormacht. Eine
//! Zeitspalte zu erfinden, indem man Klassen gleich gewichtet, waere die
//! schlechteste Antwort: Auf einem Kern ohne FPU liegt `c_target[f64]`
//! zwei Groessenordnungen ueber `c_target[i32]` (7.2).
//!
//! **Warum je Zustand.** Eine Maschine ist eine Summe aus dem, was immer
//! laeuft (`loop:`, Handler), und dem teuersten Zustand — und „teuerster
//! Zustand" ist je Klasse ein anderer, weil das Maximum komponentenweise
//! ist. Wer kuerzen will, muss wissen, welcher Zustand welche Klasse
//! treibt; die Summe allein sagt es nicht.

use crate::Program;
use crate::fns::{CostClass, CostVec};
use crate::machine::{Machine, MachineKind};

/// Der Kostenbericht eines Programms.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Je Maschine ihr Posten, in Deklarationsreihenfolge.
    pub machines: Vec<MachineCost>,
}

/// Die Kosten einer Maschine (9.4.3).
#[derive(Clone, Debug)]
pub struct MachineCost {
    /// Name der Maschine.
    pub name: String,
    /// Periode `n_m` in Basis-Ticks (7.2).
    ///
    /// Sie steht dabei, weil `B_m` *je Aktivierung* zaehlt: Eine Maschine
    /// mit `n_m = 10` belastet den Tick nur jeden zehnten. Wer die
    /// Spitzenlast sucht (7.2: `Peak = Σ_m B_m`), braucht beide Zahlen.
    pub period: u32,
    /// `B_m`: Operationen je Aktivierung.
    pub activation: CostVec,
    /// `F_m`: Operationen des Fault-Pfades (Abort-Phase, 7.2).
    pub fault_path: CostVec,
    /// Was unabhaengig vom Zustand anfaellt.
    pub base: CostVec,
    /// Je Zustand sein Anteil, mit Namen.
    pub states: Vec<StateCost>,
}

/// Die Kosten eines Zustands.
#[derive(Clone, Debug)]
pub struct StateCost {
    /// Name des Zustands.
    pub name: String,
    /// Seine Operationen.
    pub cost: CostVec,
    /// Klassen, deren Spitze dieser Zustand bestimmt.
    ///
    /// Leer heisst: Der Zustand treibt keine Klasse — er kostet in jeder
    /// weniger als ein anderer und faellt aus `B_m` heraus.
    pub drives: Vec<CostClass>,
}

/// Rechnet den Bericht (9.4.3).
///
/// Die Kosten selbst stehen bereits in `Machine::budget` (aus M3); die
/// Aufschluesselung je Zustand entsteht hier neu, weil sie nirgends
/// gespeichert ist — und mit Absicht nicht: Sie in die MIR zu schreiben
/// hiesse, jede Datei um Zahlen zu vergroessern, die ein Werkzeug
/// gelegentlich anzeigt (siehe [`super::cost::Activation`]).
pub fn report(p: &Program) -> Report {
    let types = p.types.list.clone();
    let natives: Vec<CostVec> = p.natives.iter().map(|n| n.cost).collect();
    let machines = p
        .machines
        .iter()
        .filter(|m| !matches!(m.kind, MachineKind::Template))
        .map(|m| machine_cost(m, &types, &natives))
        .collect();
    Report { machines }
}

fn machine_cost(m: &Machine, types: &[crate::types::Type], natives: &[CostVec]) -> MachineCost {
    let a = super::cost::activation(m, types, natives);
    let fault_path = m.budget.as_ref().map_or_else(CostVec::default, |b| b.fault_path);

    let states = m
        .states
        .iter()
        .enumerate()
        .map(|(i, s)| StateCost {
            name: s.name.clone(),
            cost: a.states[i],
            drives: CostClass::ALL
                .iter()
                .copied()
                .filter(|c| a.states[i].of(*c) > 0 && a.driver(*c) == Some(i))
                .collect(),
        })
        .collect();

    MachineCost { name: m.name.clone(), period: m.period, activation: a.total, fault_path, base: a.base, states }
}

impl Report {
    /// Der Bericht als Zeilen, wie ihn `takt cost` ausgibt.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("  {:<24}{}", "", header()));
        for m in &self.machines {
            out.extend(m.lines());
        }
        out.push(String::new());
        out.push("  Operationen je Aktivierung (9.4.3). Was sie in Zeit bedeuten, sagt".into());
        out.push("  die kalibrierte Tabelle `c_target` je Ziel (13.8); ohne sie ist die".into());
        out.push("  Schedulability (7.2) nicht pruefbar.".into());
        out
    }
}

impl MachineCost {
    /// Die Zeilen einer Maschine: ihr Budget, dann die Zustaende.
    pub fn lines(&self) -> Vec<String> {
        let period = match self.period {
            1 => "jeder Tick".to_string(),
            n => format!("jeder {n}. Tick"),
        };
        let mut out = vec![
            format!("  {} ({})", self.name, period),
            format!("    {:<22}{}", "B_m je Aktivierung", row(self.activation)),
        ];
        if !self.fault_path.is_zero() {
            out.push(format!("    {:<22}{}", "F_m Fault-Pfad", row(self.fault_path)));
        }
        if !self.base.is_zero() {
            out.push(format!("    {:<22}{}", "davon zustandsfrei", row(self.base)));
        }
        for s in &self.states {
            let drives = if s.drives.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = s.drives.iter().map(|c| c.name()).collect();
                format!("  <- {}", names.join(" "))
            };
            out.push(format!("    {:<22}{}{}", format!("  {}", s.name), row(s.cost), drives));
        }
        out
    }
}

/// Die Kopfzeile: die sieben Klassen in der Reihenfolge der Referenz.
fn header() -> String {
    CostClass::ALL.iter().map(|c| format!("{:>8}", c.name())).collect()
}

/// Eine Zeile aus sieben Zahlen; eine Null bleibt leer, damit die Spalte
/// mit einem Wert ins Auge faellt.
fn row(c: CostVec) -> String {
    CostClass::ALL
        .iter()
        .map(|k| match c.of(*k) {
            0 => format!("{:>8}", "."),
            v => format!("{v:>8}"),
        })
        .collect()
}
