//! Die Schrittordnung eines Ticks (Referenz 7.2, 9.4).
//!
//! Ohne `follows` ist die Reihenfolge semantisch irrelevant (Satz 9.4.1)
//! und die Prioritaetsordnung gilt: kuerzere Periode zuerst, dann
//! Deklaration. Mit `follows` ist die Ordnung eine topologische ueber den
//! Kanten, und unter den zulaessigen Ordnungen entscheidet derselbe
//! Tie-Break. Interpreter, Codegen-Rahmen und der Test zu Satz 9.4.1
//! lesen die Ordnung von hier — eine Quelle, dreimal gelesen.

use crate::machine::{Machine, MachineKind};
use crate::{MachineId, Program};

/// Laufende Maschinen in Deklarationsreihenfolge: Vorlagen und Szenarien
/// laufen nicht mit (5.8, 13.6).
pub fn runnable(p: &Program) -> Vec<MachineId> {
    p.machines
        .iter()
        .enumerate()
        .filter(|(_, m)| !matches!(m.kind, MachineKind::Template | MachineKind::Scenario) && !m.states.is_empty())
        .map(|(i, _)| MachineId(i as u32))
        .collect()
}

/// Die Schrittordnung; `Err` nennt die Maschinen eines `follows`-Zyklus.
pub fn order(p: &Program) -> Result<Vec<MachineId>, Vec<MachineId>> {
    topological(p, |ready| {
        ready.iter().copied().min_by_key(|id| (p.machines[id.index()].period.max(1), id.0)).expect("nicht leer")
    })
}

/// Eine zufaellige lineare Erweiterung der `follows`-Kanten (Test zu
/// Satz 9.4.1): deterministisch aus dem Startwert.
pub fn linear_extension(p: &Program, seed: u64) -> Vec<MachineId> {
    let mut state = seed | 1;
    topological(p, |ready| {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ready[(state >> 33) as usize % ready.len()]
    })
    .unwrap_or_else(|_| runnable(p))
}

/// Koennen zwei Maschinen im selben Tick aktiv sein (7.2)?
pub fn same_tick(a: &Machine, b: &Machine) -> bool {
    let g = gcd(a.period.max(1), b.period.max(1));
    (i64::from(a.phase) - i64::from(b.phase)).rem_euclid(i64::from(g)) == 0
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Kahns Algorithmus; `pick` waehlt unter den freien Maschinen.
fn topological(p: &Program, mut pick: impl FnMut(&[MachineId]) -> MachineId) -> Result<Vec<MachineId>, Vec<MachineId>> {
    let running = runnable(p);
    let edges = |id: MachineId, to: Option<MachineId>| {
        p.machines[id.index()].follows.iter().filter(|f| running.contains(f) && to.is_none_or(|t| t == **f)).count()
    };
    let mut indegree: Vec<usize> = (0..p.machines.len()).map(|i| edges(MachineId(i as u32), None)).collect();
    let mut ready: Vec<MachineId> = running.iter().copied().filter(|id| indegree[id.index()] == 0).collect();
    let mut out = Vec::with_capacity(running.len());
    while !ready.is_empty() {
        let next = pick(&ready);
        ready.retain(|id| *id != next);
        out.push(next);
        for id in &running {
            let n = edges(*id, Some(next));
            if n > 0 {
                indegree[id.index()] -= n;
                if indegree[id.index()] == 0 {
                    ready.push(*id);
                }
            }
        }
    }
    if out.len() == running.len() { Ok(out) } else { Err(running.into_iter().filter(|id| !out.contains(id)).collect()) }
}
