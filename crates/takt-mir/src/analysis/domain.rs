//! Die abstrakte Domaene der Analyse (Referenz 3.4).
//!
//! `Domain` ist die Schnittstelle, `Intervals` die einzige Implementierung
//! der Stufe v1. Oktagone (3.4, v1.1) treten spaeter daneben, ohne dass eine
//! Aufrufstelle sich aendert: Die Schnittstelle spricht ueber *Fakten*
//! (verfeinern, vereinigen, weiten), nicht ueber Intervalle.

use crate::types::{Const, Range, RangeOrigin};

/// Ein Wertebereich der Analyse. `Top` heisst „nichts bekannt", `Bottom`
/// „unerreichbar" (etwa der Zweig hinter `if false:`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Interval {
    /// Unerreichbar.
    Bottom,
    /// Ganzzahlig, beide Grenzen einschliesslich; auch Dauern.
    Int {
        /// Untergrenze.
        lo: i128,
        /// Obergrenze.
        hi: i128,
    },
    /// Gleitkomma, beide Grenzen einschliesslich und endlich (4.2).
    Float {
        /// Untergrenze.
        lo: f64,
        /// Obergrenze.
        hi: f64,
    },
    /// Alles andere: Bool, Aggregate und jeder Wert ohne Schranke.
    Top,
}

impl Interval {
    /// Ein einzelner ganzzahliger Wert.
    pub fn point(v: i128) -> Interval {
        Interval::Int { lo: v, hi: v }
    }

    /// `lo..hi` in Gleitkomma; `Top`, wo eine Grenze nicht endlich ist —
    /// ein unbeschraenkter Wert beweist nichts.
    pub fn floats(lo: f64, hi: f64) -> Interval {
        if lo.is_finite() && hi.is_finite() && lo <= hi { Interval::Float { lo, hi } } else { Interval::Top }
    }

    /// Aus einer deklarierten Range (3.4: Startintervall).
    pub fn from_range(r: &Range) -> Interval {
        match (r.lo, r.hi) {
            (Const::Int(lo), Const::Int(hi)) => Interval::Int { lo: i128::from(lo), hi: i128::from(hi) },
            (Const::Float(lo), Const::Float(hi)) => Interval::floats(lo, hi),
            // Dauern sind ganzzahlige Nanosekunden und rechnen wie `int`.
            (Const::Duration(lo), Const::Duration(hi)) => Interval::Int { lo: i128::from(lo), hi: i128::from(hi) },
            _ => Interval::Top,
        }
    }

    /// Liegt dieses Intervall vollstaendig in `r`? Nur dann darf eine
    /// implizite Pruefung entfallen (3.4).
    pub fn fits(&self, r: &Range) -> bool {
        match (self, Interval::from_range(r)) {
            // Unerreichbarer Code verletzt nichts.
            (Interval::Bottom, _) => true,
            (Interval::Int { lo, hi }, Interval::Int { lo: rlo, hi: rhi }) => *lo >= rlo && *hi <= rhi,
            (Interval::Float { lo, hi }, Interval::Float { lo: rlo, hi: rhi }) => *lo >= rlo && *hi <= rhi,
            _ => false,
        }
    }

    /// Die Teile unter und ueber der Null; `Bottom`, wo keiner ist.
    /// Ohne den Randpunkt `c`; ein innerer Punkt aendert nichts.
    pub fn without(self, c: i128) -> Interval {
        match self {
            Interval::Int { lo, hi } if lo == c && hi == c => Interval::Bottom,
            Interval::Int { lo, hi } if lo == c => Interval::Int { lo: lo + 1, hi },
            Interval::Int { lo, hi } if hi == c => Interval::Int { lo, hi: hi - 1 },
            other => other,
        }
    }

    fn without_zero(self) -> (Interval, Interval) {
        match self {
            Interval::Int { lo, hi } => (
                if lo < 0 { Interval::Int { lo, hi: hi.min(-1) } } else { Interval::Bottom },
                if hi > 0 { Interval::Int { lo: lo.max(1), hi } } else { Interval::Bottom },
            ),
            other => (other, other),
        }
    }

    /// Enthaelt das Intervall die Null? Entscheidet ueber die
    /// Divisionspruefung (3.4: „Division mit Nullausschluss").
    pub fn contains_zero(&self) -> bool {
        match self {
            Interval::Bottom => false,
            Interval::Int { lo, hi } => *lo <= 0 && 0 <= *hi,
            Interval::Float { lo, hi } => *lo <= 0.0 && 0.0 <= *hi,
            Interval::Top => true,
        }
    }

    /// Passt das Intervall in `i32`? Grundlage der Darstellungsverengung
    /// (3.4, Lemma 3.4).
    pub fn fits_i32(&self) -> bool {
        match self {
            Interval::Bottom => true,
            Interval::Int { lo, hi } => *lo >= i128::from(i32::MIN) && *hi <= i128::from(i32::MAX),
            Interval::Float { .. } | Interval::Top => false,
        }
    }

    /// Als bewiesene Range fuer die MIR-Annotation; `None` bei `Top` und
    /// `Bottom`, weil dort nichts zu annotieren ist, und bei Gleitkomma:
    /// Die Annotation dient der Darstellungsverengung, und die kennt nur
    /// Ganzzahlen.
    pub fn to_range(self) -> Option<Range> {
        match self {
            Interval::Int { lo, hi } => {
                let (lo, hi) = (i64::try_from(lo).ok()?, i64::try_from(hi).ok()?);
                Some(Range { lo: Const::Int(lo), hi: Const::Int(hi), origin: RangeOrigin::Proven })
            }
            _ => None,
        }
    }
}

/// Was eine Domaene ueber Werte weiss und wie sie es fortschreibt.
///
/// Die Methoden sind genau die Operationen, die der Durchlauf braucht: die
/// Verbandsoperationen (`join`, `widen`) und die Verfeinerung an einer
/// Bedingung. Eine Domaene, die Relationen kennt (Oktagone), setzt dieselbe
/// Schnittstelle mit mehr Praezision um.
pub trait Domain: Clone {
    /// Der Wert, den die Domaene je Groesse fuehrt.
    type Value: Clone + PartialEq;

    /// „Nichts bekannt".
    fn top() -> Self::Value;

    /// Vereinigung zweier Zweige.
    fn join(a: &Self::Value, b: &Self::Value) -> Self::Value;

    /// Weitung an einer Schleife, deren Koerper nicht abgerollt wird
    /// (3.4): auf die deklarierte Range, sonst auf `top`.
    fn widen(current: &Self::Value, declared: Option<&Range>) -> Self::Value;
}

/// Die Intervalldomaene der Stufe v1 (3.4: „absichtlich einfach —
/// Intervalle, keine Relationen zwischen Variablen").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Intervals;

impl Domain for Intervals {
    type Value = Interval;

    fn top() -> Interval {
        Interval::Top
    }

    fn join(a: &Interval, b: &Interval) -> Interval {
        match (a, b) {
            (Interval::Bottom, x) | (x, Interval::Bottom) => *x,
            (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh }) => {
                Interval::Int { lo: (*al).min(*bl), hi: (*ah).max(*bh) }
            }
            (Interval::Float { lo: al, hi: ah }, Interval::Float { lo: bl, hi: bh }) => {
                Interval::Float { lo: al.min(*bl), hi: ah.max(*bh) }
            }
            _ => Interval::Top,
        }
    }

    fn widen(current: &Interval, declared: Option<&Range>) -> Interval {
        match declared {
            // 3.4: „sonst mit Widening auf die deklarierten Ranges der
            // beschriebenen Variablen".
            Some(r) => Interval::from_range(r),
            None => match current {
                Interval::Bottom => Interval::Bottom,
                _ => Interval::Top,
            },
        }
    }
}

// --------------------------------------------------------------- Arithmetik

/// Intervallarithmetik (3.4). Jede Operation ist total: was nicht bewiesen
/// werden kann, ist `Top`.
impl Interval {
    /// Betrag.
    pub fn abs(self) -> Interval {
        match self {
            Interval::Int { lo, hi } => {
                let (a, b) = (lo.abs(), hi.abs());
                let low = if lo <= 0 && 0 <= hi { 0 } else { a.min(b) };
                Interval::Int { lo: low, hi: a.max(b) }
            }
            Interval::Float { lo, hi } => {
                let low = if lo <= 0.0 && 0.0 <= hi { 0.0 } else { lo.abs().min(hi.abs()) };
                Interval::Float { lo: low, hi: lo.abs().max(hi.abs()) }
            }
            other => other,
        }
    }

    /// Quadratwurzel (4.2), korrekt gerundet und monoton wie die Ecken in
    /// `corners`; was unter null liegt, faultet (`Domain`) und liefert
    /// keinen Wert.
    pub fn sqrt(self) -> Interval {
        match self {
            Interval::Float { hi, .. } if hi < 0.0 => Interval::Bottom,
            Interval::Float { lo, hi } => Interval::floats(lo.max(0.0).sqrt(), hi.sqrt()),
            Interval::Bottom => Interval::Bottom,
            _ => Interval::Top,
        }
    }

    /// Nach aussen auf das `f32`-Raster gerundet (4.2): Ein `f32`-Ergebnis
    /// liegt dort, und die Laufzeit prueft eine `f32`-Range gegen die
    /// gerundeten Grenzen. Was ueber `f32` hinaus reicht, ist nicht endlich
    /// und beweist nichts.
    pub fn on_f32_grid(self) -> Interval {
        match self {
            Interval::Float { lo, hi } => {
                let (l, h) = (lo as f32, hi as f32);
                let l = if f64::from(l) > lo { l.next_down() } else { l };
                let h = if f64::from(h) < hi { h.next_up() } else { h };
                Interval::floats(f64::from(l), f64::from(h))
            }
            other => other,
        }
    }

    /// Beide Seiten ganzzahlig? Dann `f`, sonst `Top`; `Bottom` steckt an.
    fn lift(a: Interval, b: Interval, f: impl Fn(i128, i128, i128, i128) -> Option<(i128, i128)>) -> Interval {
        match (a, b) {
            (Interval::Bottom, _) | (_, Interval::Bottom) => Interval::Bottom,
            (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh }) => match f(al, ah, bl, bh) {
                Some((lo, hi)) if lo <= hi => Interval::Int { lo, hi },
                _ => Interval::Top,
            },
            _ => Interval::Top,
        }
    }

    /// Schnitt zweier Intervalle: die Verfeinerung an einer Bedingung
    /// (3.4: „Vergleiche verfeinern Intervalle in Zweigen").
    pub fn meet(self, o: Interval) -> Interval {
        match (self, o) {
            (Interval::Bottom, _) | (_, Interval::Bottom) => Interval::Bottom,
            (Interval::Top, x) | (x, Interval::Top) => x,
            (Interval::Int { lo: al, hi: ah }, Interval::Int { lo: bl, hi: bh }) => {
                let (lo, hi) = (al.max(bl), ah.min(bh));
                if lo > hi { Interval::Bottom } else { Interval::Int { lo, hi } }
            }
            (Interval::Float { lo: al, hi: ah }, Interval::Float { lo: bl, hi: bh }) => {
                let (lo, hi) = (al.max(bl), ah.min(bh));
                if lo > hi { Interval::Bottom } else { Interval::Float { lo, hi } }
            }
            // Ganzzahl und Gleitkomma beschreiben nichts Gemeinsames.
            _ => Interval::Top,
        }
    }

    /// Alles echt unterhalb von `hi`.
    pub fn below(hi: i128) -> Interval {
        match hi.checked_sub(1) {
            Some(h) => Interval::Int { lo: i128::from(i64::MIN), hi: h },
            None => Interval::Bottom,
        }
    }

    /// Alles echt oberhalb von `lo`.
    pub fn above(lo: i128) -> Interval {
        match lo.checked_add(1) {
            Some(l) => Interval::Int { lo: l, hi: i128::from(i64::MAX) },
            None => Interval::Bottom,
        }
    }

    /// Alles bis `hi` einschliesslich.
    pub fn at_most(hi: i128) -> Interval {
        Interval::Int { lo: i128::from(i64::MIN), hi }
    }

    /// Alles ab `lo` einschliesslich.
    pub fn at_least(lo: i128) -> Interval {
        Interval::Int { lo, hi: i128::from(i64::MAX) }
    }
}

/// Eine Gleitkommaoperation an den Ecken zweier Intervalle (4.2).
///
/// Das Programm rundet jede Operation korrekt zum Naechsten, und Runden ist
/// monoton: Das `f64`-Ergebnis liegt zwischen den gerundeten Ecken, die
/// hier auf dem Wirt genauso entstehen. Ein `f32`-Ergebnis rundet `Walk`
/// danach nach aussen auf das `f32`-Raster; weil zwischen zwei
/// `f32`-Zahlen viele `f64`-Schritte liegen, ueberspringt die Rundung der
/// Ecke in `f64` keine `f32`-Zahl. `None`, wenn nicht beide Seiten
/// Gleitkomma sind.
fn corners(a: Interval, b: Interval, f: impl Fn(f64, f64) -> f64) -> Option<Interval> {
    let (Interval::Float { lo: al, hi: ah }, Interval::Float { lo: bl, hi: bh }) = (a, b) else { return None };
    let e = [f(al, bl), f(al, bh), f(ah, bl), f(ah, bh)];
    let lo = e.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = e.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(Interval::floats(lo, hi))
}

/// Summe.
impl std::ops::Add for Interval {
    type Output = Interval;

    fn add(self, o: Interval) -> Interval {
        corners(self, o, |x, y| x + y)
            .unwrap_or_else(|| Self::lift(self, o, |a, b, c, d| Some((a.checked_add(c)?, b.checked_add(d)?))))
    }
}

/// Differenz.
impl std::ops::Sub for Interval {
    type Output = Interval;

    fn sub(self, o: Interval) -> Interval {
        corners(self, o, |x, y| x - y)
            .unwrap_or_else(|| Self::lift(self, o, |a, b, c, d| Some((a.checked_sub(d)?, b.checked_sub(c)?))))
    }
}

/// Produkt: alle vier Eckprodukte, weil Vorzeichen wechseln koennen.
impl std::ops::Mul for Interval {
    type Output = Interval;

    fn mul(self, o: Interval) -> Interval {
        corners(self, o, |x, y| x * y).unwrap_or_else(|| {
            Self::lift(self, o, |a, b, c, d| {
                let e = [a.checked_mul(c)?, a.checked_mul(d)?, b.checked_mul(c)?, b.checked_mul(d)?];
                Some((*e.iter().min()?, *e.iter().max()?))
            })
        })
    }
}

/// Quotient. Die Null im Divisor faultet (4.1) und liefert keinen Wert;
/// das Ergebnis deckt die beiden Seiten daneben. In Gleitkomma liefert ein
/// Divisor nahe null beliebig grosse Werte: Enthaelt er die Null, ist
/// nichts bekannt.
impl std::ops::Div for Interval {
    type Output = Interval;

    fn div(self, o: Interval) -> Interval {
        if let Interval::Float { lo, hi } = o {
            return match self {
                Interval::Bottom => Interval::Bottom,
                _ if lo <= 0.0 && 0.0 <= hi => Interval::Top,
                _ => corners(self, o, |x, y| x / y).unwrap_or(Interval::Top),
            };
        }
        let (neg, pos) = o.without_zero();
        let part = |o: Interval| {
            Self::lift(self, o, |a, b, c, d| {
                let e = [a.checked_div(c)?, a.checked_div(d)?, b.checked_div(c)?, b.checked_div(d)?];
                Some((*e.iter().min()?, *e.iter().max()?))
            })
        };
        Intervals::join(&part(neg), &part(pos))
    }
}

/// Rest. Der Betrag ist kleiner als der des Divisors; das Vorzeichen
/// folgt dem Dividenden (4.1, trunkierend).
impl std::ops::Rem for Interval {
    type Output = Interval;

    fn rem(self, o: Interval) -> Interval {
        let Interval::Int { lo: c, hi: d } = o else {
            return if o == Interval::Bottom { o } else { Interval::Top };
        };
        if c == 0 && d == 0 {
            return Interval::Bottom;
        }
        let m = c.abs().max(d.abs()) - 1;
        match self {
            Interval::Bottom => Interval::Bottom,
            Interval::Int { lo, .. } if lo >= 0 => Interval::Int { lo: 0, hi: m },
            _ => Interval::Int { lo: -m, hi: m },
        }
    }
}

/// Vorzeichenwechsel.
impl std::ops::Neg for Interval {
    type Output = Interval;

    fn neg(self) -> Interval {
        match self {
            Interval::Int { lo, hi } => match (lo.checked_neg(), hi.checked_neg()) {
                (Some(a), Some(b)) => Interval::Int { lo: b, hi: a },
                _ => Interval::Top,
            },
            Interval::Float { lo, hi } => Interval::Float { lo: -hi, hi: -lo },
            other => other,
        }
    }
}
