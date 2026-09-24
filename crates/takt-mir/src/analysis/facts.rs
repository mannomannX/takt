//! Was der Durchlauf an einer Programmstelle weiss (Referenz 3.4, 3.8, 6.1).
//!
//! Drei Gitter, ein Durchlauf (plan/m3.md 1.6): das Intervall einer
//! Variablen, ob sie sicher zugewiesen ist (Pruefungen 6 und 25) und ob ein
//! Channel-Wert dominiert ist (Pruefung 5). Sie teilen sich die Kanten des
//! Kontrollflusses; sie zweimal zu durchlaufen waere doppelte Arbeit und
//! eine doppelte Fehlerquelle.

use std::collections::{BTreeMap, BTreeSet};

use crate::VarId;
use crate::analysis::domain::{Domain, Interval};
use crate::analysis::term::Term;

/// Der Zustand der Analyse an einer Stelle.
#[derive(Clone, Debug, PartialEq)]
pub struct Facts {
    /// Intervall je Variable; fehlt eine, gilt `Top`.
    vars: BTreeMap<u32, Interval>,
    /// Sicher zugewiesene Variablen (Pruefung 6, 25).
    assigned: BTreeMap<u32, bool>,
    /// Dominierte Wrapper: der Schluessel ist die Stelle, an der die
    /// Dominanz entstand (`x.valid`, `r.ok`), 3.8.
    dominated: BTreeMap<String, bool>,
    /// Ist die Stelle erreichbar? Nach `abort` oder in einem toten Zweig
    /// nicht.
    reachable: bool,
    /// Variablenpaare, die ein dominierender Vergleich in Beziehung gesetzt
    /// hat — nur die Kennzahl der Oktagon-Kandidaten liest sie
    /// (plan/m6.md 2.12); die Intervalle wissen nichts davon.
    relations: BTreeSet<(u32, u32)>,
    /// Differenzschranken `a - b <= c` zwischen Termen (3.4, der
    /// Differenzanteil der Oktagone): ein dominierender Vergleich traegt
    /// sie ein, eine Zuweisung an eine ihrer Variablen loescht sie.
    bounds: BTreeMap<(Term, Term), i128>,
}

impl Default for Facts {
    fn default() -> Self {
        Facts {
            vars: BTreeMap::new(),
            assigned: BTreeMap::new(),
            dominated: BTreeMap::new(),
            reachable: true,
            relations: BTreeSet::new(),
            bounds: BTreeMap::new(),
        }
    }
}

impl Facts {
    /// Der Anfangszustand eines Einstiegspunkts.
    pub fn entry() -> Facts {
        Facts::default()
    }

    /// Unerreichbar: der Zustand hinter `abort` oder in einem toten Zweig.
    pub fn unreachable() -> Facts {
        Facts { reachable: false, ..Facts::default() }
    }

    /// Ist die Stelle erreichbar?
    pub fn is_reachable(&self) -> bool {
        self.reachable
    }

    /// Markiert alles Folgende als unerreichbar.
    pub fn cut(&mut self) {
        self.reachable = false;
    }

    // ------------------------------------------------------------ Relationen

    /// Ein Vergleich zweier Variablen dominiert ab hier.
    pub fn relate(&mut self, a: VarId, b: VarId) {
        self.relations.insert((a.0.min(b.0), a.0.max(b.0)));
    }

    /// Steht die Variable in einer dominierenden Relation?
    pub fn related(&self, v: VarId) -> bool {
        self.relations.iter().any(|(a, b)| *a == v.0 || *b == v.0)
    }

    /// Ab hier gilt `a - b <= c`.
    pub fn bound(&mut self, a: Term, b: Term, c: i128) {
        let e = self.bounds.entry((a, b)).or_insert(c);
        *e = (*e).min(c);
    }

    /// Die bekannte Schranke von `a - b`.
    pub fn difference(&self, a: &Term, b: &Term) -> Option<i128> {
        self.bounds.get(&(a.clone(), b.clone())).copied()
    }

    /// Eine Zuweisung an `v` loescht jede Schranke, die `v` nennt.
    pub fn forget(&mut self, v: VarId) {
        self.bounds.retain(|(a, b), _| !a.mentions(v) && !b.mentions(v));
    }

    // ------------------------------------------------------------ Intervalle

    /// Intervall einer Variablen.
    pub fn interval(&self, v: VarId) -> Interval {
        self.vars.get(&v.0).copied().unwrap_or(Interval::Top)
    }

    /// Setzt das Intervall einer Variablen und markiert sie als zugewiesen.
    pub fn assign(&mut self, v: VarId, i: Interval) {
        self.vars.insert(v.0, i);
        self.assigned.insert(v.0, true);
        self.forget(v);
    }

    /// Setzt nur das Intervall (Verfeinerung in einem Zweig), ohne die
    /// Zuweisung zu behaupten.
    pub fn refine(&mut self, v: VarId, i: Interval) {
        let now = self.interval(v).meet(i);
        self.vars.insert(v.0, now);
        if now == Interval::Bottom {
            self.reachable = false;
        }
    }

    // --------------------------------------------------- Definite Assignment

    /// Ist die Variable sicher zugewiesen (Pruefung 6)?
    pub fn is_assigned(&self, v: VarId) -> bool {
        self.assigned.get(&v.0).copied().unwrap_or(false)
    }

    /// Erklaert eine Variable als zugewiesen, ohne ihr Intervall zu kennen
    /// (Parameter, Captures, zustandslokale Variablen bei Eintritt).
    pub fn declare(&mut self, v: VarId, i: Interval) {
        self.vars.insert(v.0, i);
        self.assigned.insert(v.0, true);
        self.forget(v);
    }

    // ------------------------------------------------------------- Dominanz

    /// Gilt an dieser Stelle `x.valid` beziehungsweise `r.ok` (3.8)?
    pub fn dominates(&self, key: &str) -> bool {
        self.dominated.get(key).copied().unwrap_or(false)
    }

    /// Traegt die Dominanz eines Wrappers ein.
    pub fn dominate(&mut self, key: String) {
        self.dominated.insert(key, true);
    }

    // ------------------------------------------------------- Verbandsoperationen

    /// Vereinigung zweier Zweige: was beide wissen.
    pub fn join<D: Domain<Value = Interval>>(a: &Facts, b: &Facts) -> Facts {
        if !a.reachable {
            return b.clone();
        }
        if !b.reachable {
            return a.clone();
        }
        let mut vars = BTreeMap::new();
        for (k, x) in &a.vars {
            let y = b.vars.get(k).copied().unwrap_or(Interval::Top);
            vars.insert(*k, D::join(x, &y));
        }
        // Was nur ein Zweig kennt, ist danach unbekannt.
        for k in b.vars.keys() {
            vars.entry(*k).or_insert(Interval::Top);
        }
        // Zugewiesen und dominiert ist nur, was in *beiden* Zweigen gilt.
        let assigned =
            a.assigned.iter().filter(|(k, v)| **v && b.is_assigned_raw(**k)).map(|(k, _)| (*k, true)).collect();
        let dominated =
            a.dominated.iter().filter(|(k, v)| **v && b.dominates(k)).map(|(k, _)| (k.clone(), true)).collect();
        let relations = a.relations.intersection(&b.relations).copied().collect();
        let bounds = a.bounds.iter().filter_map(|(k, x)| b.bounds.get(k).map(|y| (k.clone(), (*x).max(*y)))).collect();
        Facts { vars, assigned, dominated, reachable: true, relations, bounds }
    }

    /// Weitung an einer nicht abgerollten Schleife (3.4): Variablen, die der
    /// Koerper schreibt, verlieren ihr Intervall bis auf die deklarierte
    /// Range. Zuweisung und Dominanz bleiben, weil eine Schleife nichts
    /// zurueknimmt, was vor ihr galt.
    pub fn widen<D: Domain<Value = Interval>>(&mut self, written: &[VarId], declared: impl Fn(VarId) -> Interval) {
        for v in written {
            let now = self.interval(*v);
            let d = declared(*v);
            let wide = match d {
                Interval::Top => D::widen(&now, None),
                other => other,
            };
            self.vars.insert(v.0, wide);
        }
        self.relations.retain(|(a, b)| !written.iter().any(|w| w.0 == *a || w.0 == *b));
        self.bounds.retain(|(a, b), _| !written.iter().any(|w| a.mentions(*w) || b.mentions(*w)));
    }

    fn is_assigned_raw(&self, k: u32) -> bool {
        self.assigned.get(&k).copied().unwrap_or(false)
    }
}
