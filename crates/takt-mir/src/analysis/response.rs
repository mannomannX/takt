//! Antwortzeiten mehrerer Programme eines Kerns (12.11, M11 Schritt 16).
//!
//! Die Rechnung mit festen Prioritaeten, wie 12.11 sie nennt:
//!
//! ```text
//! R_i = C_i + B + Σ_{j ∈ hp(i)} ⌈R_i / T_j⌉ · C_j + Σ_k ⌈R_i / p_k⌉ · c_k
//! ```
//!
//! von `R_i = C_i + B` an bis zum Fixpunkt, verlangt `R_i ≤ T_i`. `hp(i)`
//! sind die anderen Programme, deren Tick nicht laenger ist; bei gleichem
//! Tick zaehlt jedes als hoeher als das andere, damit das Ergebnis fuer jede
//! Rangfolge gilt, die der Wirt waehlt.
//!
//! **Jeder Summand traegt seine Herkunft** (11.5), und die Antwortzeit erbt
//! die schwaechste. Mit einem offenen Summanden gibt es keine Zahl: Eine
//! Antwortzeit, die eine fehlende Eingabe als null rechnete, waere zu
//! guenstig und saehe aus wie eine bewiesene.
//!
//! Gerechnet wird in Pikosekunden ohne Fliesskomma (4.2): Die Rechnung soll
//! auf jedem Wirt dieselbe Zahl ergeben.

/// Woher ein Summand stammt (11.5), von stark nach schwach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// Aus der MIR oder dem Abbild gerechnet.
    Exact,
    /// Gemessen: eine Kalibrierung oder `takt bench`.
    Measured,
    /// Vom Wirt angegeben, nicht bewiesen.
    Contract,
    /// Die Eingabe fehlt.
    Open,
}

impl Origin {
    /// Der Name im Bericht (11.5).
    pub fn name(self) -> &'static str {
        match self {
            Origin::Exact => "exakt",
            Origin::Measured => "gemessen",
            Origin::Contract => "Vertrag",
            Origin::Open => "offen",
        }
    }
}

/// Ein Programm des Kerns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    /// Sein Praefix.
    pub name: String,
    /// Der Tick `T_i` in Pikosekunden.
    pub period_ps: u64,
    /// Das Schrittbudget `C_i` in Pikosekunden; `None`, wenn es offen ist.
    pub budget_ps: Option<u64>,
    /// Die Herkunft des Budgets.
    pub origin: Origin,
}

/// Ein Stoerer ueber den Schrittkontexten, oder die laengste Sperre.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Load {
    /// Sein Name im Bericht.
    pub name: String,
    /// Was er je Auftreten kostet, in Pikosekunden.
    pub cost_ps: u64,
    /// Die Periode in Pikosekunden; fuer eine Sperre ohne Bedeutung.
    pub period_ps: u64,
    /// Die Herkunft beider Werte.
    pub origin: Origin,
}

/// Die Antwortzeit eines Programms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    /// Das Praefix des Programms.
    pub name: String,
    /// `R_i` in Pikosekunden: der Fixpunkt, oder der erste Wert ueber `T_i`;
    /// `None`, wenn ein Summand offen ist.
    pub ps: Option<u64>,
    /// Die schwaechste Herkunft der Summanden.
    pub origin: Origin,
    /// `R_i ≤ T_i`.
    pub fits: bool,
}

/// Die Antwortzeit jedes Programms, in der Reihenfolge von `programs`.
/// `blocking` ist die laengste Sperre `B` (keine: `None`), `isrs` sind die
/// Stoerer.
pub fn responses(programs: &[Program], blocking: Option<&Load>, isrs: &[Load]) -> Vec<Response> {
    programs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let higher: Vec<&Program> = programs
                .iter()
                .enumerate()
                .filter(|(j, q)| *j != i && q.period_ps <= p.period_ps)
                .map(|(_, q)| q)
                .collect();
            let origin = higher
                .iter()
                .map(|q| q.origin)
                .chain(blocking.map(|b| b.origin))
                .chain(isrs.iter().map(|k| k.origin))
                .fold(p.origin, Ord::max);
            let budgets: Option<Vec<(u64, u64)>> =
                higher.iter().map(|q| q.budget_ps.map(|c| (c, q.period_ps))).collect();
            let (Some(own), Some(budgets)) = (p.budget_ps, budgets) else {
                return Response { name: p.name.clone(), ps: None, origin: Origin::Open, fits: false };
            };
            let base = own.saturating_add(blocking.map_or(0, |b| b.cost_ps));
            let mut r = base;
            loop {
                let next = budgets
                    .iter()
                    .copied()
                    .chain(isrs.iter().map(|k| (k.cost_ps, k.period_ps)))
                    .fold(base, |sum, (c, t)| sum.saturating_add(r.div_ceil(t.max(1)).saturating_mul(c)));
                if next == r || next > p.period_ps {
                    let fits = next <= p.period_ps;
                    return Response { name: p.name.clone(), ps: Some(next), origin, fits };
                }
                r = next;
            }
        })
        .collect()
}

/// Die Auslastung `U = Σ C_i/T_i + Σ c_k/p_k` in Millionsteln, aufgerundet;
/// `None`, wenn ein Budget offen ist.
pub fn utilisation_ppm(programs: &[Program], isrs: &[Load]) -> Option<u64> {
    let mut sum: u128 = 0;
    let budgets = programs.iter().map(|p| p.budget_ps.map(|c| (c, p.period_ps)));
    for entry in budgets.chain(isrs.iter().map(|k| Some((k.cost_ps, k.period_ps)))) {
        let (c, t) = entry?;
        sum = sum.saturating_add((u128::from(c) * 1_000_000).div_ceil(u128::from(t.max(1))));
    }
    Some(u64::try_from(sum).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(name: &str, period: u64, budget: u64) -> Program {
        Program { name: name.into(), period_ps: period, budget_ps: Some(budget), origin: Origin::Measured }
    }

    fn isr(name: &str, cost: u64, period: u64, origin: Origin) -> Load {
        Load { name: name.into(), cost_ps: cost, period_ps: period, origin }
    }

    /// **Das Lehrbuchbeispiel** (Burns und Wellings, Tabelle 13.6): drei
    /// Aufgaben mit (T, C) = (7, 3), (12, 3), (20, 5) haben die Antwortzeiten
    /// 3, 6 und 20; alle passen.
    #[test]
    fn the_textbook_set_has_its_response_times() {
        let set = [program("a", 7, 3), program("b", 12, 3), program("c", 20, 5)];
        let r: Vec<Option<u64>> = responses(&set, None, &[]).into_iter().map(|r| r.ps).collect();
        assert_eq!(r, [Some(3), Some(6), Some(20)]);
        assert!(responses(&set, None, &[]).iter().all(|r| r.fits));
        // 3/7 + 3/12 + 5/20 = 0,9286
        assert_eq!(utilisation_ppm(&set, &[]), Some(428_572 + 250_000 + 250_000));
    }

    /// Ein Programm, dessen Antwortzeit ueber seinen Tick reicht, passt
    /// nicht; die Rechnung haelt am ersten Wert darueber.
    #[test]
    fn a_response_over_the_tick_does_not_fit() {
        let set = [program("fast", 10, 6), program("slow", 20, 9)];
        let slow = &responses(&set, None, &[])[1];
        assert!(!slow.fits);
        assert!(slow.ps.is_some_and(|r| r > 20), "{slow:?}");
    }

    /// Stoerer und Sperre zaehlen fuer jedes Programm; bei gleichem Tick
    /// stoert jedes Programm das andere.
    #[test]
    fn interferers_blocking_and_equal_ticks_count() {
        let set = [program("a", 100, 10), program("b", 100, 20)];
        let block = isr("sperre", 5, 0, Origin::Contract);
        let uart = isr("uart", 1, 10, Origin::Measured);
        let r = responses(&set, Some(&block), &[uart]);
        // a: 15 → 15 + 20 + 2 = 37 → 15 + 20 + 4 = 39, der Fixpunkt.
        assert_eq!(r[0].ps, Some(39));
        // b: 25 → 25 + 10 + 3 = 38 → 25 + 10 + 4 = 39.
        assert_eq!(r[1].ps, Some(39));
        assert!(r.iter().all(|x| x.fits && x.origin == Origin::Contract));
    }

    /// Die Antwortzeit erbt die schwaechste Herkunft; ein offenes Budget,
    /// auch das eines hoeheren Programms, laesst sie offen.
    #[test]
    fn the_weakest_origin_wins_and_open_has_no_number() {
        let measured = [program("a", 10, 2), program("b", 20, 3)];
        assert!(responses(&measured, None, &[]).iter().all(|r| r.origin == Origin::Measured));
        let open = [Program { budget_ps: None, origin: Origin::Open, ..program("a", 10, 2) }, program("b", 20, 3)];
        let r = responses(&open, None, &[]);
        assert_eq!((r[0].ps, r[0].origin), (None, Origin::Open));
        assert_eq!((r[1].ps, r[1].origin, r[1].fits), (None, Origin::Open, false));
        assert_eq!(utilisation_ppm(&open, &[]), None);
    }
}
