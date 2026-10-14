//! Zahlen als Text (12.3, 12.5, `grammar/trace.md` T2): fuer die Leitung
//! der Boards, die Haken des Trace und den Interpreter.
//!
//! **Ganzzahlen ohne `core::fmt`.** Unter `xip_flash` liegt alles, was der
//! Tick ruft, im RAM (`takt check-image`); `core::fmt` ist gross und laege
//! mit allem, was es mitbringt, dort. Eine Zahl je Tick braucht nur ihre
//! Ziffern.
//!
//! **Fliesskomma mit `core::fmt`, in einer Form fuer alle.** Die kuerzeste
//! Ziffernfolge, die beim Zuruecklesen denselben Wert ergibt, rechnet
//! `core::fmt` (4.2: keine eigene Dezimalkonversion). Wann sie in
//! Exponentform steht und dass sie immer einen Dezimalpunkt traegt, regelt
//! T2; [`float64`] und [`float32`] sind diese Regel einmal, fuer den
//! Interpreter wie fuer den Rahmen (FB-750).

use core::fmt::Write as _;

/// Die Dezimalziffern von `n`, hinten in `buf`; zurueck kommt der belegte
/// Teil. Die Ziffern einer `u64` passen in 20 Byte.
pub fn decimal(n: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut used = 0usize;
    let mut slots = buf.iter_mut().rev();
    let mut put = |d: u8| {
        if let Some(slot) = slots.next() {
            *slot = d;
            used = used.saturating_add(1);
        }
    };
    // In u64 nur, bis der Rest in u32 passt: Auf RV32 ist die Division in
    // u64 ein Aufruf, die in u32 ein Befehl.
    let mut wide = n;
    while wide > u64::from(u32::MAX) {
        put(decimal_digit(wide % 10));
        wide /= 10;
    }
    let mut narrow = u32::try_from(wide).unwrap_or(u32::MAX);
    loop {
        put(decimal_digit(u64::from(narrow % 10)));
        narrow /= 10;
        if narrow == 0 {
            break;
        }
    }
    let from = buf.len().saturating_sub(used);
    buf.get(from..).unwrap_or(&[])
}

/// Die Hexziffer der unteren vier Bit von `n`, klein geschrieben.
pub fn hex(n: u8) -> u8 {
    b"0123456789abcdef".get(usize::from(n & 15)).copied().unwrap_or(b'?')
}

/// Ab diesem Betrag steht ein `f64` in Exponentform (3.9, T2): Die Stellen
/// vor dem Punkt waeren nicht mehr signifikant.
pub const F64_BIG: f64 = 1e16;

/// Dasselbe fuer `f32`.
pub const F32_BIG: f64 = 1e7;

/// Steht `x` in Exponentform (3.9)? Ab `big` und unter `1e-5`, nie die Null.
pub fn exponent_form(x: f64, big: f64) -> bool {
    let a = x.abs();
    a.is_finite() && (a >= big || (a > 0.0 && a < 1e-5))
}

/// Ein `f64` als Text (3.9, T2): die kuerzeste Ziffernfolge, die beim
/// Zuruecklesen denselben Wert ergibt, immer mit Dezimalpunkt; ab dem Betrag
/// `1e16` und unter `1e-5` in Exponentform (`1.0e30`, `1.5e-7`), hoechstens
/// 24 Zeichen.
pub fn float64<W: core::fmt::Write>(x: f64, out: &mut W) -> core::fmt::Result {
    let mut buf = Buf::default();
    if exponent_form(x, F64_BIG) {
        write!(buf, "{x:e}")?;
    } else {
        write!(buf, "{x}")?;
    }
    with_point(buf.text(), out)
}

/// Ein `f32` als Text nach derselben Regel mit der Grenze `1e7`, hoechstens
/// 16 Zeichen. Geschrieben wird der `f32` selbst, nicht sein Wert in `f64`.
pub fn float32<W: core::fmt::Write>(x: f32, out: &mut W) -> core::fmt::Result {
    let mut buf = Buf::default();
    if exponent_form(f64::from(x), F32_BIG) {
        write!(buf, "{x:e}")?;
    } else {
        write!(buf, "{x}")?;
    }
    with_point(buf.text(), out)
}

/// `core::fmt` schreibt einen ganzzahligen Wert und eine ganzzahlige
/// Mantisse ohne Punkt (`1e30`, `100`); die Textform traegt immer einen
/// (`1.0e30`). `NaN` und `inf` bleiben, wie sie sind.
fn with_point<W: core::fmt::Write>(text: &str, out: &mut W) -> core::fmt::Result {
    let (mantissa, exponent) = match text.split_once('e') {
        Some((m, e)) => (m, Some(e)),
        None => (text, None),
    };
    out.write_str(mantissa)?;
    if !mantissa.contains('.') && mantissa.trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) {
        out.write_str(".0")?;
    }
    if let Some(e) = exponent {
        out.write_char('e')?;
        out.write_str(e)?;
    }
    Ok(())
}

/// Ein Puffer fester Groesse fuer `core::fmt`: Eine Zahl in der Textform hat
/// hoechstens 24 Zeichen (3.9). Was nicht hineinpasst, ist ein Fehler.
struct Buf {
    bytes: [u8; 32],
    len: usize,
}

impl Default for Buf {
    fn default() -> Buf {
        Buf { bytes: [0; 32], len: 0 }
    }
}

impl Buf {
    fn text(&self) -> &str {
        core::str::from_utf8(self.bytes.get(..self.len).unwrap_or(&[])).unwrap_or("")
    }
}

impl core::fmt::Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(core::fmt::Error)?;
        self.bytes.get_mut(self.len..end).ok_or(core::fmt::Error)?.copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Die Dezimalziffer von `d`; der Aufrufer gibt `d < 10`.
fn decimal_digit(d: u64) -> u8 {
    usize::try_from(d).ok().and_then(|i| b"0123456789".get(i)).copied().unwrap_or(b'?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digits_of_the_edges_of_u64() {
        let mut buf = [0u8; 20];
        assert_eq!(decimal(0, &mut buf), b"0");
        assert_eq!(decimal(u64::from(u32::MAX), &mut buf), b"4294967295");
        assert_eq!(decimal(u64::from(u32::MAX) + 1, &mut buf), b"4294967296");
        assert_eq!(decimal(u64::MAX, &mut buf), b"18446744073709551615");
        assert_eq!((hex(0x0a), hex(0xff)), (b'a', b'f'));
    }

    extern crate std;

    fn f64_text(x: f64) -> std::string::String {
        let mut s = std::string::String::new();
        float64(x, &mut s).expect("passt");
        s
    }

    fn f32_text(x: f32) -> std::string::String {
        let mut s = std::string::String::new();
        float32(x, &mut s).expect("passt");
        s
    }

    /// **Die Textform aus T2**: immer ein Dezimalpunkt, Exponentform ab
    /// `1e16` (`f32`: `1e7`) und unter `1e-5`, die Null nie; `NaN` und die
    /// Unendlichen bleiben.
    #[test]
    fn floats_are_written_as_t2_says() {
        for (x, want) in [
            (3.0, "3.0"),
            (-0.0, "-0.0"),
            (2.5, "2.5"),
            (1e16, "1.0e16"),
            (-1e16, "-1.0e16"),
            (9999999999999998.0, "9999999999999998.0"),
            (1e-5, "0.00001"),
            (1.5e-7, "1.5e-7"),
            (1e30, "1.0e30"),
            (f64::NAN, "NaN"),
            (f64::NEG_INFINITY, "-inf"),
        ] {
            assert_eq!(f64_text(x), want, "{x:?}");
        }
        assert_eq!((f32_text(0.1), f32_text(1e7), f32_text(3.0)), ("0.1".into(), "1.0e7".into(), "3.0".into()));
        assert!(f64_text(-0.000012345678901234567).len() <= 24 && f64_text(-1.2345678901234567e-300).len() <= 24);
    }
}
