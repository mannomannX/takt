//! Die korrekt gerundete Mathematik (4.2) hinter der C-ABI des erzeugten
//! Codes: `takt_m_<name>_<breite>`.
//!
//! Die Rechnung steht in `libtaktm` und braucht weder `std` noch eine FPU;
//! Interpreter, Wirtsrahmen und Boards rufen dieselbe (FB-293). Den
//! Definitionsbereich prueft der erzeugte Code vor dem Aufruf (4.1), ein
//! nicht endliches Ergebnis danach — hier steht nur die Grenze.

macro_rules! unary {
    ($($entry:ident = $f:ident : $t:ty;)*) => {$(
        #[doc = concat!("`libtaktm::", stringify!($f), "` fuer den erzeugten Code.")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $entry(x: $t) -> $t {
            libtaktm::$f(x)
        }
    )*};
}

macro_rules! binary {
    ($($entry:ident = $f:ident : $t:ty;)*) => {$(
        #[doc = concat!("`libtaktm::", stringify!($f), "` fuer den erzeugten Code.")]
        #[unsafe(no_mangle)]
        pub extern "C" fn $entry(x: $t, y: $t) -> $t {
            libtaktm::$f(x, y)
        }
    )*};
}

unary! {
    takt_m_exp_f64 = exp_f64: f64;
    takt_m_log_f64 = log_f64: f64;
    takt_m_sin_f64 = sin_f64: f64;
    takt_m_cos_f64 = cos_f64: f64;
    takt_m_tan_f64 = tan_f64: f64;
    takt_m_asin_f64 = asin_f64: f64;
    takt_m_acos_f64 = acos_f64: f64;
    takt_m_atan_f64 = atan_f64: f64;
    takt_m_exp_f32 = exp_f32: f32;
    takt_m_log_f32 = log_f32: f32;
    takt_m_sin_f32 = sin_f32: f32;
    takt_m_cos_f32 = cos_f32: f32;
    takt_m_tan_f32 = tan_f32: f32;
    takt_m_asin_f32 = asin_f32: f32;
    takt_m_acos_f32 = acos_f32: f32;
    takt_m_atan_f32 = atan_f32: f32;
}

binary! {
    takt_m_atan2_f64 = atan2_f64: f64;
    takt_m_pow_f64 = pow_f64: f64;
    takt_m_atan2_f32 = atan2_f32: f32;
    takt_m_pow_f32 = pow_f32: f32;
}

/// `libtaktm::scale_f64` fuer den erzeugten Code: `x · num / den`, korrekt
/// gerundet (3.2, `x.to(U)`; INT-008). Die Faktoren sind `u128` und kommen
/// in zwei Haelften, weil die C-ABI `u128` nicht auf jedem Ziel gleich
/// uebergibt.
#[unsafe(no_mangle)]
pub extern "C" fn takt_m_scale_f64(x: f64, num_lo: u64, num_hi: u64, den_lo: u64, den_hi: u64) -> f64 {
    libtaktm::scale_f64(x, wide(num_lo, num_hi), wide(den_lo, den_hi))
}

/// `libtaktm::scale_f32` fuer den erzeugten Code, wie [`takt_m_scale_f64`].
#[unsafe(no_mangle)]
pub extern "C" fn takt_m_scale_f32(x: f32, num_lo: u64, num_hi: u64, den_lo: u64, den_hi: u64) -> f32 {
    libtaktm::scale_f32(x, wide(num_lo, num_hi), wide(den_lo, den_hi))
}

/// Ein `u128` aus seinen zwei Haelften.
fn wide(lo: u64, hi: u64) -> u128 {
    (u128::from(hi) << 64) | u128::from(lo)
}

/// Ein Einstieg, wie ihn das Messprogramm der Boards ruft: ueber den Zeiger,
/// also genau die Funktion, die auch der erzeugte Code ruft.
#[derive(Clone, Copy)]
pub enum Entry {
    /// Einstellig in `f64`.
    F64(extern "C" fn(f64) -> f64),
    /// Zweistellig in `f64`.
    F64Pair(extern "C" fn(f64, f64) -> f64),
    /// Einstellig in `f32`.
    F32(extern "C" fn(f32) -> f32),
    /// Zweistellig in `f32`.
    F32Pair(extern "C" fn(f32, f32) -> f32),
}

impl Entry {
    /// Ruft den Einstieg mit Bitmustern; das Ergebnis als Bitmuster.
    pub fn call(self, x: u64, y: u64) -> u64 {
        let narrow = |b: u64| f32::from_bits(b as u32);
        match self {
            Entry::F64(f) => f(f64::from_bits(x)).to_bits(),
            Entry::F64Pair(f) => f(f64::from_bits(x), f64::from_bits(y)).to_bits(),
            Entry::F32(f) => u64::from(f(narrow(x)).to_bits()),
            Entry::F32Pair(f) => u64::from(f(narrow(x), narrow(y)).to_bits()),
        }
    }
}

/// Der Einstieg einer Funktion in einer Breite.
pub fn by_name(name: &str, wide: bool) -> Option<Entry> {
    Some(match (name, wide) {
        ("exp", true) => Entry::F64(takt_m_exp_f64),
        ("log", true) => Entry::F64(takt_m_log_f64),
        ("sin", true) => Entry::F64(takt_m_sin_f64),
        ("cos", true) => Entry::F64(takt_m_cos_f64),
        ("tan", true) => Entry::F64(takt_m_tan_f64),
        ("asin", true) => Entry::F64(takt_m_asin_f64),
        ("acos", true) => Entry::F64(takt_m_acos_f64),
        ("atan", true) => Entry::F64(takt_m_atan_f64),
        ("atan2", true) => Entry::F64Pair(takt_m_atan2_f64),
        ("pow", true) => Entry::F64Pair(takt_m_pow_f64),
        ("exp", false) => Entry::F32(takt_m_exp_f32),
        ("log", false) => Entry::F32(takt_m_log_f32),
        ("sin", false) => Entry::F32(takt_m_sin_f32),
        ("cos", false) => Entry::F32(takt_m_cos_f32),
        ("tan", false) => Entry::F32(takt_m_tan_f32),
        ("asin", false) => Entry::F32(takt_m_asin_f32),
        ("acos", false) => Entry::F32(takt_m_acos_f32),
        ("atan", false) => Entry::F32(takt_m_atan_f32),
        ("atan2", false) => Entry::F32Pair(takt_m_atan2_f32),
        ("pow", false) => Entry::F32Pair(takt_m_pow_f32),
        _ => return None,
    })
}
