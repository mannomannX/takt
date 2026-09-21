//! Passt die Rechenlast in einen Tick? (7.2, Prüfung 32)
//!
//! Die Ungleichung steht wörtlich in 7.2:
//!
//! ```text
//! Σ_c (Peak_c + Σ_m F_m,c) · c_target[c] ≤ T₀ − T_IO
//! ```
//!
//! **Was daran rechenbar ist und was gemessen sein muss.** Die linke
//! Seite entsteht vollstaendig aus der MIR: `Peak_c` ist die Summe der
//! Aktivierungsbudgets (7.2: „`Peak = Σ_m B_m`, denn bei Phase 0 sind in
//! Tick 0 alle Maschinen aktiv"), `F_m,c` das Fault-Pfad-Budget. Beide
//! rechnet `analysis::cost` seit M3. Was fehlt, ist der Faktor: Ohne
//! `c_target` sind es Operationszahlen, keine Zeiten.
//!
//! Darum liefert dieses Modul **zwei Antworten statt einer**: [`Load`]
//! ohne Kalibrierung sagt, was gerechnet wurde und was fehlt; mit
//! Kalibrierung urteilt es. Das ist dieselbe Zwischenform, die `wcet` im
//! Budget seit je traegt — sie nennt den Grund, statt zu schweigen, und
//! sie behauptet nichts, was sie nicht wissen kann.
//!
//! **`F_m` wird einmal gezaehlt, nicht zweimal.** 9.4.3 schreibt
//! `B_m = max_C[…] + F_m`, 7.2 addiert `Σ_m F_m,c` separat zu `Peak_c`.
//! Beide Lesarten zusammen zaehlten den Fault-Pfad doppelt. Der Code
//! folgt 7.2: `Activation::total` ist ohne `F_m`, und die Abort-Phase
//! kommt hier dazu. Die Referenz ist an dieser Stelle inkonsistent; die
//! 7.2-Lesart ist die, aus der die Pruefung stammt.

use crate::fns::CostVec;
use crate::hardware::CTarget;
use crate::machine::{Machine, MachineKind};
use crate::program::Program;

/// Die Rechenlast eines Ticks (7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Load {
    /// `Peak_c = Σ_m B_m,c` — alle Maschinen in Tick 0 aktiv.
    pub peak: CostVec,
    /// `Σ_m F_m,c` — die Abort-Phase (5.4).
    pub abort: CostVec,
    /// Der Basis-Tick `T₀` in Pikosekunden.
    pub tick_ps: u64,
}

/// Das Urteil, wenn eine Kalibrierung vorliegt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// Was die linke Seite kostet, in Pikosekunden.
    pub needed_ps: u64,
    /// Was zur Verfuegung steht: `T₀ − T_IO`, in Pikosekunden.
    pub available_ps: u64,
}

impl Verdict {
    /// Passt die Last in den Tick?
    pub fn fits(&self) -> bool {
        self.needed_ps <= self.available_ps
    }

    /// Der genutzte Anteil in Prozent, aufgerundet.
    ///
    /// Fuer den Bericht: „91 %" sagt mehr als zwei Zahlen, und wer eine
    /// Periode sucht, die noch passt, rechnet damit.
    pub fn utilisation_percent(&self) -> u64 {
        if self.available_ps == 0 {
            return u64::MAX;
        }
        self.needed_ps.saturating_mul(100).div_ceil(self.available_ps)
    }
}

impl Load {
    /// Die Operationen, die ein Tick im schlimmsten Fall kostet.
    pub fn total(&self) -> CostVec {
        self.peak + self.abort
    }

    /// Das Urteil mit einer Kalibrierung.
    ///
    /// `None`, wenn die Tabelle unvollstaendig ist: Eine fehlende Klasse
    /// machte das Skalarprodukt zu klein, und ein „passt" auf zu kleiner
    /// Grundlage ist schlimmer als kein Urteil.
    pub fn judge(&self, c: &CTarget, t_io_ps: u64) -> Option<Verdict> {
        if !c.is_complete() {
            return None;
        }
        Some(Verdict { needed_ps: c.duration_ps(self.total()), available_ps: self.tick_ps.saturating_sub(t_io_ps) })
    }
}

/// Rechnet die Last eines Ticks (7.2).
///
/// **Ohne `phase`-Versatz.** 7.2 erlaubt, die Spitze ueber die
/// Hyperperiode exakt zu pruefen, wenn `H ≤ 10⁶`; solange `phase` nicht
/// umgesetzt ist, ist `Σ_m B_m` die richtige und zugleich sichere
/// Antwort — sie ueberschaetzt nie.
pub fn load(p: &Program) -> Load {
    // 5.11: Eine gescopte Instanz laeuft nur, solange ihr Scope steht;
    // zwei in exklusiven Zustaenden nie zugleich. Ihre Last gehoert darum
    // in die Spitze ihres Besitzers, nicht in die flache Summe (9.4.3).
    let scoped: Vec<crate::MachineId> =
        crate::machine::scoped_instances(p).into_iter().map(|(_, si)| si.machine).collect();
    let mut peak = CostVec::default();
    let mut abort = CostVec::default();
    for (i, m) in p.machines.iter().enumerate() {
        if m.kind == MachineKind::Template || scoped.contains(&crate::MachineId(i as u32)) {
            continue;
        }
        let Some(b) = m.budget else { continue };
        peak = peak + b.activation + peak_of_instances(p, m);
        abort = abort + b.fault_path + abort_of_instances(p, m);
    }
    // `tick` steht in Nanosekunden (3.3); gerechnet wird in Pikosekunden.
    Load { peak, abort, tick_ps: (p.config.tick.max(0) as u64).saturating_mul(1000) }
}

/// Die Spitzenlast der in `m` gescopten Instanzen (9.4.3, 5.11).
fn peak_of_instances(p: &Program, m: &Machine) -> CostVec {
    instances_over(p, m, &m.roots, &|b| b.activation)
}

/// Dasselbe fuer die Abort-Phase (7.2).
fn abort_of_instances(p: &Program, m: &Machine) -> CostVec {
    instances_over(p, m, &m.roots, &|b| b.fault_path)
}

/// Maximum ueber Geschwisterzustaende, Summe innerhalb eines Zustands —
/// dieselbe Rekursion wie beim Speicher-Overlay (11.5). Eine Instanz kann
/// selbst Instanzen scopen, darum rekursiv ueber ihren Zustandsbaum.
fn instances_over(
    p: &Program,
    m: &Machine,
    siblings: &[crate::StateId],
    pick: &dyn Fn(&crate::machine::Budget) -> CostVec,
) -> CostVec {
    siblings
        .iter()
        .map(|id| {
            let s = &m.states[id.index()];
            let mut own = CostVec::default();
            for si in &s.instances {
                let inst = &p.machines[si.machine.index()];
                if let Some(b) = inst.budget {
                    own = own + pick(&b);
                }
                own = own + instances_over(p, inst, &inst.roots, pick);
            }
            own + instances_over(p, m, &s.children, pick)
        })
        .fold(CostVec::default(), CostVec::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fns::CostClass;

    fn tabelle(ps: u64) -> CTarget {
        let mut c = CTarget::default();
        for class in CostClass::ALL {
            c.set(class, ps);
        }
        c
    }

    /// Eine Last unter der Grenze passt.
    #[test]
    fn a_load_below_the_bound_fits() {
        let load =
            Load { peak: CostVec { i32: 100, ..CostVec::default() }, abort: CostVec::default(), tick_ps: 10_000_000 };
        let v = load.judge(&tabelle(1000), 0).expect("vollstaendig");
        assert!(v.fits());
        assert_eq!(v.needed_ps, 100_000);
    }

    /// Eine Last darueber passt nicht.
    #[test]
    fn a_load_above_the_bound_does_not_fit() {
        let load = Load {
            peak: CostVec { i32: 100_000, ..CostVec::default() },
            abort: CostVec::default(),
            tick_ps: 10_000_000,
        };
        let v = load.judge(&tabelle(1000), 0).expect("vollstaendig");
        assert!(!v.fits());
    }

    /// **`T_IO` verkleinert das Budget.**
    ///
    /// 7.2 vergleicht gegen `T₀ − T_IO`, nicht gegen `T₀`: Was der Tick
    /// fuer Abtastung und Commit braucht, steht dem Programm nicht zur
    /// Verfuegung.
    #[test]
    fn the_io_share_shrinks_the_budget() {
        let load = Load { peak: CostVec { i32: 9, ..CostVec::default() }, abort: CostVec::default(), tick_ps: 10_000 };
        assert!(load.judge(&tabelle(1000), 0).expect("v").fits(), "ohne T_IO passt es");
        assert!(!load.judge(&tabelle(1000), 2_000).expect("v").fits(), "mit T_IO nicht mehr");
    }

    /// Die Abort-Phase zaehlt mit (5.4, 7.2).
    #[test]
    fn the_abort_phase_counts_towards_the_tick() {
        let load = Load {
            peak: CostVec { i32: 5, ..CostVec::default() },
            abort: CostVec { i32: 5, ..CostVec::default() },
            tick_ps: 10_000,
        };
        assert_eq!(load.total().of(CostClass::I32), 10);
        assert_eq!(load.judge(&tabelle(1000), 0).expect("v").needed_ps, 10_000);
    }

    /// **Eine unvollstaendige Tabelle urteilt nicht.**
    ///
    /// Sie ergaebe ein zu kleines Skalarprodukt und damit ein „passt", das
    /// auf einer Luecke beruht.
    #[test]
    fn an_incomplete_table_refuses_to_judge() {
        let mut c = tabelle(1000);
        c.set(CostClass::Mem, 0);
        let load = Load { peak: CostVec::default(), abort: CostVec::default(), tick_ps: 1000 };
        assert!(load.judge(&c, 0).is_none());
    }

    /// Die Auslastung rundet auf: 91 % heisst „passt knapp", nicht „passt".
    #[test]
    fn utilisation_rounds_up() {
        let v = Verdict { needed_ps: 901, available_ps: 1000 };
        assert_eq!(v.utilisation_percent(), 91);
        let genau = Verdict { needed_ps: 1000, available_ps: 1000 };
        assert_eq!(genau.utilisation_percent(), 100);
        assert!(genau.fits(), "genau aufgebraucht passt noch");
    }
}
