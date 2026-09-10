//! Einheitenalgebra (Referenz 3.2): Normalform als sortiertes Produkt von
//! Atomen mit ganzzahligen Exponenten, nominale Identitaet je benanntem Atom,
//! Dimension und Faktor aus den Definitionen, Praefixe, Internierung der in
//! der MIR gebrauchten Kombinationen als `UnitDef`.

use std::collections::HashMap;

use takt_diag::Span;
use takt_mir::UnitId;
use takt_mir::program::Program;
use takt_mir::types::{BASE_DIMENSIONS, Rational, UnitDef};

/// Dimensionsvektor ueber m, kg, s, A, K, mol, cd.
pub type Dim = [i8; BASE_DIMENSIONS];

/// Atom einer Einheit: eine benannte Einheit oder eine Einheitenvariable
/// einer generischen Vorlage (3.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Atom {
    /// Benannte Einheit (nominal, 3.2).
    Named(UnitId),
    /// Einheitenvariable `U` (Index in der Umgebung).
    Var(u32),
}

/// Einheit in Normalform: Atome aufsteigend, Exponenten ungleich null.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Unit {
    /// Atome mit Exponenten.
    pub factors: Vec<(Atom, i8)>,
}

impl Unit {
    /// Dimensionslos.
    pub fn one() -> Unit {
        Unit::default()
    }

    /// Eine benannte Einheit.
    pub fn named(id: UnitId) -> Unit {
        Unit { factors: vec![(Atom::Named(id), 1)] }
    }

    /// Eine Einheitenvariable.
    pub fn var(i: u32) -> Unit {
        Unit { factors: vec![(Atom::Var(i), 1)] }
    }

    /// Dimensionslos?
    pub fn is_one(&self) -> bool {
        self.factors.is_empty()
    }

    /// Enthaelt Variablen?
    pub fn has_vars(&self) -> bool {
        self.factors.iter().any(|(a, _)| matches!(a, Atom::Var(_)))
    }

    /// Exponent eines Atoms.
    pub fn exponent(&self, atom: Atom) -> i8 {
        self.factors.iter().find(|(a, _)| *a == atom).map_or(0, |(_, e)| *e)
    }

    /// Variablen der Einheit.
    pub fn vars(&self) -> Vec<u32> {
        self.factors.iter().filter_map(|(a, _)| if let Atom::Var(v) = a { Some(*v) } else { None }).collect()
    }

    fn from_map(map: Vec<(Atom, i32)>) -> Unit {
        let mut factors: Vec<(Atom, i8)> =
            map.into_iter().filter(|(_, e)| *e != 0).map(|(a, e)| (a, e.clamp(-127, 127) as i8)).collect();
        factors.sort_by_key(|(a, _)| *a);
        Unit { factors }
    }

    /// Produkt.
    pub fn mul(&self, o: &Unit) -> Unit {
        let mut map: Vec<(Atom, i32)> = self.factors.iter().map(|(a, e)| (*a, i32::from(*e))).collect();
        for (a, e) in &o.factors {
            match map.iter_mut().find(|(b, _)| b == a) {
                Some(entry) => entry.1 += i32::from(*e),
                None => map.push((*a, i32::from(*e))),
            }
        }
        Unit::from_map(map)
    }

    /// Quotient.
    pub fn div(&self, o: &Unit) -> Unit {
        self.mul(&o.inv())
    }

    /// Kehrwert.
    pub fn inv(&self) -> Unit {
        self.pow(-1)
    }

    /// Potenz mit ganzem Exponenten.
    pub fn pow(&self, k: i32) -> Unit {
        Unit::from_map(self.factors.iter().map(|(a, e)| (*a, i32::from(*e) * k)).collect())
    }

    /// Ersetzt gebundene Variablen; ungebundene bleiben.
    pub fn substitute(&self, env: &[Option<Unit>]) -> Unit {
        let mut out = Unit::one();
        for (a, e) in &self.factors {
            let term = match a {
                Atom::Var(v) => match env.get(*v as usize) {
                    Some(Some(u)) => u.pow(i32::from(*e)),
                    _ => Unit { factors: vec![(*a, *e)] },
                },
                Atom::Named(_) => Unit { factors: vec![(*a, *e)] },
            };
            out = out.mul(&term);
        }
        out
    }
}

/// Zusatzwissen des Sema je `UnitDef`.
#[derive(Clone, Debug)]
pub struct UnitInfo {
    /// SI-Praefixe erlaubt (`mV`, `kHz`).
    pub prefixable: bool,
    /// Zerlegung einer internierten Kombination (`K/min`) in Atome.
    pub decomposition: Option<Unit>,
}

/// Einheitentabelle des Sema, parallel zu `Program::units`.
#[derive(Clone, Debug, Default)]
pub struct Units {
    /// Name → Einheit (auch Kombinationen unter ihrem kanonischen Namen).
    pub by_name: HashMap<String, UnitId>,
    /// Zusatzwissen je Einheit.
    pub infos: Vec<UnitInfo>,
    derived: HashMap<Unit, UnitId>,
}

/// SI-Praefixe mit Zehnerexponent; groessere als 10^18 sind nicht darstellbar (3.2).
const PREFIXES: &[(&str, i32)] = &[
    ("E", 18),
    ("P", 15),
    ("T", 12),
    ("G", 9),
    ("M", 6),
    ("k", 3),
    ("h", 2),
    ("da", 1),
    ("d", -1),
    ("c", -2),
    ("m", -3),
    ("u", -6),
    ("n", -9),
    ("p", -12),
    ("f", -15),
    ("a", -18),
];

/// Binaere Praefixe fuer `B`.
const BINARY: &[(&str, u32)] = &[("Ki", 10), ("Mi", 20), ("Gi", 30), ("Ti", 40)];

/// Basiseinheiten (3.2) mit Dimensionsvektor.
const BASE: &[(&str, Dim)] = &[
    ("m", [1, 0, 0, 0, 0, 0, 0]),
    ("kg", [0, 1, 0, 0, 0, 0, 0]),
    ("s", [0, 0, 1, 0, 0, 0, 0]),
    ("A", [0, 0, 0, 1, 0, 0, 0]),
    ("K", [0, 0, 0, 0, 1, 0, 0]),
    ("mol", [0, 0, 0, 0, 0, 1, 0]),
    ("cd", [0, 0, 0, 0, 0, 0, 1]),
];

impl Units {
    /// Tabelle mit den Basiseinheiten und `g` (Praefixe auf Gramm, `kg` ist Basis).
    pub fn new(program: &mut Program) -> Units {
        let mut units = Units::default();
        for (name, dim) in BASE {
            units
                .declare(program, name, *dim, Rational::int(1), None, true, true, Span::default())
                .expect("Basiseinheit");
        }
        units
            .declare(program, "g", BASE[1].1, Rational { num: 1, den: 1000 }, None, true, true, Span::default())
            .expect("Gramm");
        units
    }

    /// Kopie fuer die generische Pruefung (`in_scratch`).
    pub fn clone_shallow(&self) -> Units {
        self.clone()
    }

    /// Bekannte Einheit ohne Praefixauflösung.
    pub fn get(&self, name: &str) -> Option<UnitId> {
        self.by_name.get(name).copied()
    }

    /// Einheit zu einem Namen, mit Praefixauflösung (`mV`, `KiB`); ein
    /// praefixierter Name wird beim ersten Gebrauch als Einheit angelegt.
    pub fn lookup(&mut self, program: &mut Program, name: &str) -> Option<UnitId> {
        if let Some(id) = self.by_name.get(name) {
            return Some(*id);
        }
        for (prefix, exp) in BINARY {
            if let Some(rest) = name.strip_prefix(prefix) {
                if rest == "B" {
                    let base = self.by_name.get("B").copied()?;
                    let factor = rat_mul(program.units[base.index()].factor, Rational::int(1i64 << exp))?;
                    return self
                        .declare(
                            program,
                            name,
                            program.units[base.index()].dimension,
                            factor,
                            None,
                            false,
                            true,
                            Span::default(),
                        )
                        .ok();
                }
            }
        }
        for (prefix, exp) in PREFIXES {
            let Some(rest) = name.strip_prefix(prefix) else { continue };
            let Some(base) = self.by_name.get(rest).copied() else { continue };
            if !self.infos[base.index()].prefixable {
                continue;
            }
            let def = program.units[base.index()].clone();
            let scale = if *exp >= 0 {
                Rational::int(10i64.pow(*exp as u32))
            } else {
                Rational { num: 1, den: 10u64.pow(exp.unsigned_abs()) }
            };
            let factor = rat_mul(def.factor, scale)?;
            return self.declare(program, name, def.dimension, factor, None, false, true, Span::default()).ok();
        }
        None
    }

    /// Legt eine benannte Einheit an.
    #[allow(clippy::too_many_arguments)]
    pub fn declare(
        &mut self,
        program: &mut Program,
        name: &str,
        dimension: Dim,
        factor: Rational,
        affine_offset: Option<Rational>,
        prefixable: bool,
        predefined: bool,
        span: Span,
    ) -> Result<UnitId, String> {
        if self.by_name.contains_key(name) {
            return Err(format!("Einheit `{name}` ist schon definiert"));
        }
        let id = UnitId(program.units.len() as u32);
        program.units.push(UnitDef { name: name.to_string(), dimension, factor, affine_offset, predefined, span });
        self.infos.push(UnitInfo { prefixable, decomposition: None });
        self.by_name.insert(name.to_string(), id);
        Ok(id)
    }

    /// Einheit einer `UnitId` als Normalform.
    pub fn unit_of(&self, id: UnitId) -> Unit {
        match self.infos.get(id.index()).and_then(|i| i.decomposition.clone()) {
            Some(u) => u,
            None => Unit::named(id),
        }
    }

    /// `UnitId` fuer eine Normalform ohne Variablen: atomar direkt, sonst eine
    /// internierte Kombination mit kanonischem Namen.
    pub fn intern(&mut self, program: &mut Program, unit: &Unit) -> Result<UnitId, String> {
        if unit.has_vars() {
            return Err("Einheit mit offener Variable".into());
        }
        if let [(Atom::Named(id), 1)] = unit.factors.as_slice() {
            return Ok(*id);
        }
        if let Some(id) = self.derived.get(unit) {
            return Ok(*id);
        }
        let name = self.display(program, unit);
        let dimension = self.dimension(program, unit);
        let factor = self.factor(program, unit).ok_or_else(|| format!("Faktor von `{name}` nicht darstellbar"))?;
        if unit
            .factors
            .iter()
            .any(|(a, _)| matches!(a, Atom::Named(id) if program.units[id.index()].affine_offset.is_some()))
        {
            return Err(format!("affine Einheit in `{name}` nur allein erlaubt (3.2)"));
        }
        let id = UnitId(program.units.len() as u32);
        program.units.push(UnitDef {
            name: name.clone(),
            dimension,
            factor,
            affine_offset: None,
            predefined: false,
            span: Span::default(),
        });
        self.infos.push(UnitInfo { prefixable: false, decomposition: Some(unit.clone()) });
        self.by_name.insert(name, id);
        self.derived.insert(unit.clone(), id);
        Ok(id)
    }

    /// Dimensionsvektor (Variablen zaehlen nicht).
    pub fn dimension(&self, program: &Program, unit: &Unit) -> Dim {
        let mut dim = [0i8; BASE_DIMENSIONS];
        for (a, e) in &unit.factors {
            if let Atom::Named(id) = a {
                for (d, x) in dim.iter_mut().zip(program.units[id.index()].dimension) {
                    *d = d.saturating_add(x.saturating_mul(*e));
                }
            }
        }
        dim
    }

    /// Faktor zur Basiseinheit; `None`, wenn nicht als Rational darstellbar.
    pub fn factor(&self, program: &Program, unit: &Unit) -> Option<Rational> {
        let mut f = Rational::int(1);
        for (a, e) in &unit.factors {
            if let Atom::Named(id) = a {
                f = rat_mul(f, rat_pow(program.units[id.index()].factor, i32::from(*e))?)?;
            }
        }
        Some(f)
    }

    /// Kanonische Schreibweise (`K/min`, `kg*m/s^2`, `1/s`).
    pub fn display(&self, program: &Program, unit: &Unit) -> String {
        let name = |a: &Atom| match a {
            Atom::Named(id) => program.units[id.index()].name.clone(),
            Atom::Var(v) => format!("?{v}"),
        };
        let term = |a: &Atom, e: i8| if e.abs() == 1 { name(a) } else { format!("{}^{}", name(a), e.abs()) };
        let num: Vec<String> = unit.factors.iter().filter(|(_, e)| *e > 0).map(|(a, e)| term(a, *e)).collect();
        let den: Vec<String> = unit.factors.iter().filter(|(_, e)| *e < 0).map(|(a, e)| term(a, *e)).collect();
        let mut out = if num.is_empty() { "1".to_string() } else { num.join("*") };
        if !den.is_empty() {
            out.push('/');
            out.push_str(&den.join("/"));
        }
        out
    }

    /// Affine Einheit (`degC`)?
    pub fn is_affine(&self, program: &Program, unit: &Unit) -> bool {
        matches!(unit.factors.as_slice(), [(Atom::Named(id), 1)] if program.units[id.index()].affine_offset.is_some())
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Kuerzt und prueft die Darstellbarkeit.
pub fn rat_reduce(num: i128, den: u128) -> Option<Rational> {
    if den == 0 {
        return None;
    }
    let g = gcd(num.unsigned_abs(), den).max(1);
    let (n, d) = (num / g as i128, den / g);
    Some(Rational { num: i64::try_from(n).ok()?, den: u64::try_from(d).ok()? })
}

/// Produkt zweier Rationalen.
pub fn rat_mul(a: Rational, b: Rational) -> Option<Rational> {
    rat_reduce(i128::from(a.num) * i128::from(b.num), u128::from(a.den) * u128::from(b.den))
}

/// Potenz einer Rationalen mit ganzem Exponenten.
pub fn rat_pow(a: Rational, k: i32) -> Option<Rational> {
    let mut r = Rational::int(1);
    let base = if k < 0 {
        if a.num == 0 {
            return None;
        }
        let (n, d) = (i128::from(a.den) * a.num.signum() as i128, a.num.unsigned_abs() as u128);
        rat_reduce(n, d)?
    } else {
        a
    };
    for _ in 0..k.unsigned_abs() {
        r = rat_mul(r, base)?;
    }
    Some(r)
}

/// Dezimaltext (`6894.757293168`, `0.01`, `1e-3`, `0x10`) exakt als Rational.
pub fn rational_from_text(text: &str) -> Option<Rational> {
    let t = text.replace('_', "");
    if let Some(hex) = t.strip_prefix("0x") {
        return Some(Rational::int(i64::from_str_radix(hex, 16).ok()?));
    }
    if let Some(bin) = t.strip_prefix("0b") {
        return Some(Rational::int(i64::from_str_radix(bin, 2).ok()?));
    }
    if let Some(oct) = t.strip_prefix("0o") {
        return Some(Rational::int(i64::from_str_radix(oct, 8).ok()?));
    }
    let (mantissa, exp) = match t.find(['e', 'E']) {
        Some(i) => (&t[..i], t[i + 1..].parse::<i32>().ok()?),
        None => (t.as_str(), 0),
    };
    let (neg, mantissa) = match mantissa.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, mantissa),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{int_part}{frac_part}");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut num: i128 = digits.parse().ok()?;
    if neg {
        num = -num;
    }
    let scale = exp - frac_part.len() as i32;
    if scale >= 0 {
        rat_reduce(num.checked_mul(10i128.checked_pow(scale as u32)?)?, 1)
    } else {
        rat_reduce(num, 10u128.checked_pow(scale.unsigned_abs())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use takt_mir::program::Config;

    fn setup() -> (Program, Units) {
        let mut p = Program::new(Config::new(1, 1_000_000));
        let u = Units::new(&mut p);
        (p, u)
    }

    #[test]
    fn normal_form_is_commutative_and_cancels() {
        let (mut p, mut u) = setup();
        let m = Unit::named(u.get("m").unwrap());
        let s = Unit::named(u.get("s").unwrap());
        assert_eq!(m.mul(&s), s.mul(&m));
        assert_eq!(m.mul(&s).div(&s), m);
        assert_eq!(m.div(&m), Unit::one());
        assert_eq!(u.display(&p, &m.div(&s.pow(2))), "m/s^2");
        assert_eq!(u.display(&p, &Unit::one().div(&s)), "1/s");
        let kg = Unit::named(u.get("kg").unwrap());
        let newton = kg.mul(&m).div(&s.pow(2));
        assert_eq!(u.dimension(&p, &newton), [1, 1, -2, 0, 0, 0, 0]);
        let id = u.intern(&mut p, &newton).unwrap();
        assert_eq!(p.units[id.index()].name, "m*kg/s^2", "kanonisch nach UnitId, nicht nach Text");
        assert_eq!(u.intern(&mut p, &newton).unwrap(), id, "einmal interniert");
        assert_eq!(u.unit_of(id), newton);
    }

    #[test]
    fn prefixes_scale_factors() {
        let (mut p, mut u) = setup();
        let mv = u.lookup(&mut p, "mm").unwrap();
        assert_eq!(p.units[mv.index()].factor, Rational { num: 1, den: 1000 });
        assert_eq!(p.units[mv.index()].dimension, [1, 0, 0, 0, 0, 0, 0]);
        let km = u.lookup(&mut p, "km").unwrap();
        assert_eq!(p.units[km.index()].factor, Rational::int(1000));
        assert_eq!(u.lookup(&mut p, "mm"), Some(mv), "praefixierte Einheit einmal");
        assert!(u.lookup(&mut p, "mmm").is_none(), "kein doppelter Praefix");
        assert_eq!(
            u.lookup(&mut p, "cd").map(|id| p.units[id.index()].dimension),
            Some([0, 0, 0, 0, 0, 0, 1]),
            "exakt vor Praefix"
        );
        let ug = u.lookup(&mut p, "ug").unwrap();
        assert_eq!(p.units[ug.index()].factor, Rational { num: 1, den: 1_000_000_000 });
        u.declare(&mut p, "B", [0; 7], Rational::int(1), None, false, true, Span::default()).unwrap();
        let kib = u.lookup(&mut p, "KiB").unwrap();
        assert_eq!(p.units[kib.index()].factor, Rational::int(1024));
        assert!(u.lookup(&mut p, "kB").is_none(), "B nur binaer");
    }

    #[test]
    fn rationals_from_text() {
        // gekuerzt: 6894757293168/1000000000 = 430922330823/62500000
        assert_eq!(rational_from_text("6894.757293168"), Some(Rational { num: 430_922_330_823, den: 62_500_000 }));
        assert_eq!(rational_from_text("0.01"), Some(Rational { num: 1, den: 100 }));
        assert_eq!(rational_from_text("1e-3"), Some(Rational { num: 1, den: 1000 }));
        assert_eq!(rational_from_text("2.5e3"), Some(Rational::int(2500)));
        assert_eq!(rational_from_text("100_000"), Some(Rational::int(100_000)));
        assert_eq!(rational_from_text("0x10"), Some(Rational::int(16)));
        assert_eq!(rational_from_text("-0.5"), Some(Rational { num: -1, den: 2 }));
        assert_eq!(rat_pow(Rational { num: 1, den: 1000 }, -1), Some(Rational::int(1000)));
        assert_eq!(rat_pow(Rational::int(10), 2), Some(Rational::int(100)));
    }

    #[test]
    fn substitution_binds_variables() {
        let (p, u) = setup();
        let m = Unit::named(u.get("m").unwrap());
        let s = Unit::named(u.get("s").unwrap());
        let pattern = Unit::var(0).div(&Unit::var(1)).mul(&s);
        assert!(pattern.has_vars());
        assert_eq!(pattern.vars(), vec![0, 1]);
        let bound = pattern.substitute(&[Some(m.clone()), Some(s.clone())]);
        assert_eq!(bound, m);
        assert_eq!(u.display(&p, &pattern.substitute(&[Some(m), None])), "m*s/?1");
    }
}
