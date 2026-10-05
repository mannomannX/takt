//! Qualitaet eines Inputs (3.5): Range, `max_slew`, `debounce`.
//!
//! 3.5 sagt den Ablauf woertlich: Ein Wert ausserhalb der Range wird `Bad`
//! mit Grund `OutOfRange`, ein Wert jenseits `max_slew` `Bad` mit Grund
//! `Implausible` — und mit `debounce = n` fuer bis zu `n` aufeinander
//! folgende Lieferungen zunaechst `Suspect`, wobei **der letzte gute Wert
//! gehalten wird**.

/// Qualitaet einer Abtastung (3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Quality {
    Good,
    Suspect,
    Stale,
    Bad,
}

/// Grund einer Degradierung (`x.reason`, 3.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Reason {
    Stale,
    OutOfRange,
    Implausible,
    Driver,
}

/// Was der Rand von einem Wert wissen muss, um ihn zu pruefen.
///
/// Die Pruefungen 3 und 4 aus 12.6 brauchen Ordnung (liegt der Wert in der
/// Range?) und Differenz (wie schnell hat er sich geaendert?) — sonst
/// nichts. Ein Wert, der keine Zahl ist (ein Record, ein Enum), traegt
/// keine Range und keine Steigung; fuer ihn liefern beide Methoden `None`,
/// und die Pruefungen lassen ihn durch.
pub trait Scalar {
    /// Der Wert als `f64`, wenn er eine Zahl ist.
    ///
    /// `f64` traegt jeden `i32` und jeden `f32` exakt. Ein `i64` jenseits
    /// von 2^53 verliert Stellen — deshalb `in_range` unten den
    /// Ganzzahlvergleich vorzieht, wo er moeglich ist.
    fn as_f64(&self) -> Option<f64>;

    /// Der Wert als `i64`, wenn er eine Ganzzahl oder eine Dauer ist.
    fn as_i64(&self) -> Option<i64>;
}

/// Liegt der Wert zwischen den Grenzen (3.4)?
///
/// Ganzzahlen werden als Ganzzahlen verglichen, damit ein `i64` jenseits
/// von 2^53 nicht an der Umrechnung nach `f64` scheitert; nur wenn die
/// Grenzen selbst ganzzahlig sind, ist dieser Vergleich der genauere.
pub fn within<S: Scalar>(v: &S, lo: f64, hi: f64) -> bool {
    if let (Some(x), true) = (v.as_i64(), integral(lo) && integral(hi)) {
        return lo as i64 <= x && x <= hi as i64;
    }
    match v.as_f64() {
        Some(x) => x >= lo && x <= hi,
        None => true,
    }
}

/// Ist `x` eine ganze Zahl im Bereich von `i64`? Wie `x.fract() == 0.0`,
/// das `core` nicht hat; NaN, die Unendlichen und Grenzen jenseits von
/// `i64` sind es nicht.
fn integral(x: f64) -> bool {
    x == (x as i64) as f64
}

shared_with_c! {
    /// Zustand der Qualitaetsmaschine eines Inputs zwischen den Lieferungen.
    ///
    /// Der letzte gute Wert traegt den Wert, den `debounce` haelt, und zugleich den
    /// Bezugspunkt fuer `max_slew` — 3.5 nennt beide denselben: „gegenueber dem
    /// letzten guten Wert", und „der erste Wert nach Start oder nach `Bad` gilt
    /// als gut".
    ///
    /// `repr(C)` und ohne `Option`, weil der erzeugte C-Rahmen den Zustand je
    /// Kanal in seinem eigenen Speicher haelt; lauter Nullen ist der Anfang.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Gate as "takt_gate" {
        /// Letzter guter Wert.
        good: f64,
        /// Sein Zeitpunkt in Nanosekunden.
        good_t: i64,
        /// Gibt es einen letzten guten Wert?
        has_good: bool,
        /// Wie viele Lieferungen in Folge bereits verletzt haben.
        strikes: u32,
    }
}

/// Ergebnis einer Pruefung: Qualitaet und Grund, plus ob der letzte gute
/// Wert gehalten wird.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Verdict {
    /// Qualitaet der Abtastung.
    pub quality: Quality,
    /// Grund, wenn nicht `Good`.
    pub reason: Option<Reason>,
    /// Bei `Suspect`: Der letzte gute Wert wird gehalten (3.5).
    ///
    /// Nicht der Wert selbst — der Rand kennt den Typ des Kanals nicht,
    /// und ein `int`-Kanal bekaeme aus der Entprellung sonst einen `f64`.
    /// Wer geliefert hat, weiss, was er zuletzt geliefert hat.
    pub held: bool,
}

impl Verdict {
    /// Eine unauffaellige Lieferung.
    pub fn good() -> Self {
        Verdict { quality: Quality::Good, reason: None, held: false }
    }
}

impl Gate {
    /// Prueft eine Lieferung gegen Range und `max_slew` (12.6, Zeilen 3
    /// und 4).
    ///
    /// `now` ist der Zeitstempel der Lieferung in Nanosekunden; er geht nur
    /// in `max_slew` ein, das eine Rate ist. Zwei Lieferungen mit
    /// demselben Zeitstempel koennen keine Steigung verletzen — ohne
    /// Zeitdifferenz ist die Rate nicht definiert, und 3.5 laesst den Wert
    /// dann durch, statt ihn auf Verdacht zu verwerfen.
    pub fn check<S: Scalar>(&mut self, v: &S, now: i64, limits: &Limits) -> Verdict {
        let violated = self.violation(v, now, limits);
        let Some(reason) = violated else {
            self.strikes = 0;
            if let Some(x) = v.as_f64() {
                (self.good, self.good_t, self.has_good) = (x, now, true);
            }
            return Verdict::good();
        };
        self.strikes = self.strikes.saturating_add(1);
        if self.strikes <= limits.debounce {
            // 3.5: bis zu `debounce` Lieferungen als `Suspect`, der letzte
            // gute Wert wird gehalten, `.valid` bleibt wahr.
            return Verdict { quality: Quality::Suspect, reason: Some(reason), held: self.has_good };
        }
        // Danach `Bad`. Der Bezugspunkt faellt weg: 3.5 sagt, der erste
        // Wert nach `Bad` gilt wieder als gut — sonst bliebe ein Kanal nach
        // einem Sprung dauerhaft implausibel, weil er sich am alten Wert
        // maesse.
        self.has_good = false;
        Verdict { quality: Quality::Bad, reason: Some(reason), held: false }
    }

    /// Meldet den Kanal als vom Treiber degradiert (12.6, Zeile 2).
    ///
    /// Der Bezugspunkt faellt weg wie bei `Bad`: Was nach einer Stoerung
    /// des Treibers kommt, ist gegen den Wert davor nicht sinnvoll zu
    /// vergleichen.
    pub fn driver_bad(&mut self) -> Verdict {
        self.has_good = false;
        self.strikes = 0;
        Verdict { quality: Quality::Bad, reason: Some(Reason::Driver), held: false }
    }

    /// Welche der beiden Pruefungen schlaegt an — Range vor `max_slew`.
    ///
    /// Die Reihenfolge ist nicht beliebig: Ein Wert weit ausserhalb der
    /// Range verletzt fast immer auch die Steigung, und `OutOfRange` ist
    /// die praezisere Auskunft.
    ///
    /// Ein nicht endlicher Wert verletzt immer die Range, auch ohne
    /// deklarierte: `float` kennt nur endliche Werte (4.1), und was es in der
    /// Sprache nicht gibt, kommt nicht als `Good` hinein (Anhang A, INT-025).
    fn violation<S: Scalar>(&self, v: &S, now: i64, limits: &Limits) -> Option<Reason> {
        if v.as_f64().is_some_and(|x| !x.is_finite()) {
            return Some(Reason::OutOfRange);
        }
        if let Some((lo, hi)) = limits.range
            && !within(v, lo, hi)
        {
            return Some(Reason::OutOfRange);
        }
        let slew = limits.max_slew?;
        let x = v.as_f64()?;
        if !self.has_good {
            return None;
        }
        let (prev, t) = (self.good, self.good_t);
        let dt = now.checked_sub(t)?;
        if dt <= 0 {
            return None;
        }
        let step = if x < prev { prev - x } else { x - prev };
        let per_second = step / (dt as f64 / 1e9);
        (per_second > slew).then_some(Reason::Implausible)
    }
}

/// Die ausgerechneten Grenzen eines Channels.
///
/// Sie stehen als Ausdruck in der MIR (`max_slew` ist ein `const_expr`) und
/// werden einmal beim Start ausgewertet, nicht in jedem Tick: Der Rand
/// laeuft vor jedem Abbild, und 12.1 verlangt im Tick-Thread keine Arbeit,
/// die sich vorziehen laesst.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    /// Untere und obere Grenze der deklarierten Range (3.4).
    pub range: Option<(f64, f64)>,
    /// `max_slew` in Einheiten je Sekunde (3.5).
    pub max_slew: Option<f64>,
    /// `debounce`: so viele Verletzungen in Folge bleiben `Suspect` (3.5).
    pub debounce: u32,
}

shared_with_c! {
    /// [`Limits`] in der Form des C-Rahmens: ohne `Option`, weil der Rahmen
    /// sie als Konstante je Kanal haelt.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Bounds as "takt_bounds" {
        /// Hat der Kanal eine Range?
        pub has_range: bool,
        /// Ihre untere Grenze.
        pub lo: f64,
        /// Ihre obere Grenze.
        pub hi: f64,
        /// Hat er `max_slew`?
        pub has_slew: bool,
        /// `max_slew` in Einheiten je Sekunde.
        pub slew: f64,
        /// `debounce`.
        pub debounce: u32,
    }
}

impl Bounds {
    /// Die Grenzen, wie der Kern sie prueft.
    pub fn limits(&self) -> Limits {
        Limits {
            range: self.has_range.then_some((self.lo, self.hi)),
            max_slew: self.has_slew.then_some(self.slew),
            debounce: self.debounce,
        }
    }
}
