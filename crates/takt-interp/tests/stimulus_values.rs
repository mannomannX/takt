//! Werte im Stimulus und im Trace (`grammar/trace.md` T2): was sich als Wert
//! eines Typs lesen laesst und was nicht, und wie ein Wert als Text steht.
//!
//! Ein Stimulus ist eine Eingabe von aussen. Was er nicht als Wert des
//! Kanaltyps ausdrueckt, darf der Interpreter nicht still umdeuten: eine
//! andere Einheit nicht umrechnen (3.2), eine zu grosse Zahl nicht kuerzen,
//! NaN nicht hereinlassen (4.1). Der Rahmen des erzeugten Codes liest
//! dieselbe Zeile ueber dieselbe Funktion, und ein Wert, den beide Seiten
//! verschieden deuten, waere ein Unterschied ohne Fehler im Programm.

use takt_diag::Span;
use takt_interp::Value;
use takt_interp::trace::{float_text, float32_text, parse_value};
use takt_mir::program::{Config, Program};
use takt_mir::types::{FloatWidth, IntWidth, Rational, Type, UnitDef};
use takt_mir::{TypeId, UnitId};

struct Types {
    p: Program,
    u8: TypeId,
    i8: TypeId,
    u64: TypeId,
    int_mv: TypeId,
    f64: TypeId,
    f32: TypeId,
    bar: TypeId,
    line4: TypeId,
    str8: TypeId,
}

fn types() -> Types {
    let mut p = Program::new(Config::new(1, 1_000_000));
    let unit = |name: &str, num: i64, den: u64| UnitDef {
        name: name.into(),
        dimension: [-1, 1, -2, 0, 0, 0, 0],
        factor: Rational { num, den },
        affine_offset: None,
        predefined: true,
        span: Span::default(),
    };
    p.units.push(unit("bar", 100_000, 1));
    p.units.push(unit("psi", 6_894_757_293_168, 1_000_000_000));
    p.units.push(UnitDef { dimension: [2, 1, -3, -1, 0, 0, 0], ..unit("mV", 1, 1000) });
    let mut t = |ty| p.types.intern(ty);
    let types = (
        t(Type::Int { width: IntWidth::U8, unit: None, range: None }),
        t(Type::Int { width: IntWidth::I8, unit: None, range: None }),
        t(Type::Int { width: IntWidth::U64, unit: None, range: None }),
        t(Type::Int { width: IntWidth::I64, unit: Some(UnitId(2)), range: None }),
        t(Type::Float { width: FloatWidth::F64, unit: None, range: None }),
        t(Type::Float { width: FloatWidth::F32, unit: None, range: None }),
        t(Type::Float { width: FloatWidth::F64, unit: Some(UnitId(0)), range: None }),
        t(Type::Line { cap: 4 }),
        t(Type::Str { cap: 8 }),
    );
    let (u8, i8, u64, int_mv, f64, f32, bar, line4, str8) = types;
    Types { p, u8, i8, u64, int_mv, f64, f32, bar, line4, str8 }
}

/// SEM2-056, SEM2-057, INT-003: Zeilen, die kein Wert ihres Typs sind.
#[test]
fn a_value_outside_its_type_is_refused() {
    let t = types();
    let refused = [
        ("5 psi", t.bar, "andere Einheit: nominal, keine stille Umrechnung (3.2)"),
        ("1500 mbar", t.bar, "unbekannte Einheit"),
        ("5 bar", t.f64, "Einheit an einem Wert ohne Einheit"),
        ("-1000 V", t.int_mv, "andere Einheit an einer Ganzzahl"),
        ("300", t.u8, "jenseits der Breite"),
        ("-1", t.u8, "negativ in einer vorzeichenlosen Breite"),
        ("128", t.i8, "jenseits von i8"),
        ("-129", t.i8, "jenseits von i8"),
        ("18446744073709551616", t.u64, "jenseits von u64"),
        ("NaN", t.f64, "NaN gibt es nicht (4.1)"),
        ("nan", t.f64, "auch klein geschrieben"),
        ("inf", t.f64, "unendlich gibt es nicht (4.1)"),
        ("-infinity", t.f64, "auch ausgeschrieben"),
        ("1e400", t.f64, "laeuft beim Lesen ueber"),
        ("1e39", t.f32, "laeuft in f32 ueber"),
        ("NaN bar", t.bar, "NaN mit Einheit"),
        ("\"abcdefghi\"", t.str8, "laenger als str<8>"),
        ("\"a\\q\"", t.str8, "unbekanntes Escape"),
        ("\"ab\" c", t.str8, "Rest nach dem Text"),
        ("\"ab", t.str8, "Text ohne Ende"),
    ];
    let mut accepted = Vec::new();
    for (text, ty, why) in refused {
        if let Ok(v) = parse_value(text, ty, &t.p) {
            accepted.push(format!("  `{text}` ({why}) wurde {v:?}"));
        }
    }
    assert!(accepted.is_empty(), "angenommen statt abgelehnt:\n{}", accepted.join("\n"));
}

#[test]
fn a_value_of_its_type_reads_exactly() {
    let t = types();
    let read = |text: &str, ty| parse_value(text, ty, &t.p).unwrap_or_else(|e| panic!("`{text}`: {e}"));
    assert_eq!(read("255", t.u8), Value::UInt(255));
    assert_eq!(read("0", t.u8), Value::UInt(0));
    assert_eq!(read("-128", t.i8), Value::Int(-128));
    assert_eq!(read("18446744073709551615", t.u64), Value::UInt(u64::MAX));
    assert_eq!(read("-1000 mV", t.int_mv), Value::Int(-1000));
    assert_eq!(read("45 bar", t.bar), Value::F64(45.0));
    assert_eq!(read("45", t.bar), Value::F64(45.0), "ohne Einheit gilt die des Typs");
    let Value::F64(z) = read("-0.0", t.f64) else { panic!("f64") };
    assert_eq!(z.to_bits(), (-0.0f64).to_bits(), "-0.0 bleibt -0.0");
    // In der Breite gelesen, nicht ueber f64: 16777217 liegt zwischen zwei
    // f32 und rundet direkt nach 16777216; 0.1 ist das f32 naechst 0.1.
    assert_eq!(read("16777217", t.f32), Value::F32(16_777_216.0));
    assert_eq!(read("0.1", t.f32), Value::F32(0.1));
    // Ueber f64 gerundet waere es 1.0: der Text liegt knapp ueber der
    // Mitte zwischen 1 und dem naechsten f32, als f64 aber genau auf ihr,
    // und die Mitte rundet zur geraden Seite.
    assert_eq!(read("1.000000059604644775390625000000001", t.f32), Value::F32(1.000_000_1));
    assert_eq!(read("\"say \\\"hi\\\"\"", t.str8), Value::Str("say \"hi\"".into()));
    assert_eq!(read("\"a\\\\b\\n\"", t.str8), Value::Str("a\\b\n".into()));
    assert_eq!(read("\"\\u{e4}\"", t.str8), Value::Str("\u{e4}".into()));
}

/// 3.9: Eine `line<N>` kuerzt der Rand auf N Bytes und sagt es mit
/// `.truncated`; sonst merkte das Programm den Verlust nicht.
#[test]
fn a_long_line_is_cut_and_marked() {
    let t = types();
    assert_eq!(parse_value("\"abcdef\"", t.line4, &t.p), Ok(Value::Line { text: "abcd".into(), truncated: true }));
    assert_eq!(parse_value("\"abcd\"", t.line4, &t.p), Ok(Value::Line { text: "abcd".into(), truncated: false }));
    // Ein Zeichen, das ueber die Grenze reicht, faellt ganz weg.
    assert_eq!(parse_value("\"abc\u{e4}\"", t.line4, &t.p), Ok(Value::Line { text: "abc".into(), truncated: true }));
}

/// T2, 3.9 (Fuenfte Runde): Ein Fliesskommawert steht in der kuerzesten
/// Ziffernfolge, die beim Zuruecklesen denselben Wert ergibt, immer mit
/// Dezimalpunkt; ab dem Betrag `1e16` (`f64`) bzw. `1e7` (`f32`) und unter
/// `1e-5` in Exponentform. Jeder Text liest sich als derselbe Wert zurueck,
/// und kein `f64` braucht mehr als 24, kein `f32` mehr als 16 Zeichen.
#[test]
fn a_float_is_written_in_its_shortest_form() {
    let t = types();
    let table: [(f64, &str); 18] = [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1.5, "1.5"),
        (0.1, "0.1"),
        (1e14, "100000000000000.0"),
        (1e15, "1000000000000000.0"),
        (9999999999999998.0, "9999999999999998.0"),
        (1e16, "1.0e16"),
        (-1e16, "-1.0e16"),
        (1e30, "1.0e30"),
        (2f64.powi(63), "9.223372036854776e18"),
        (1e-5, "0.00001"),
        (1e-6, "1.0e-6"),
        (1.5e-7, "1.5e-7"),
        (f64::MAX, "1.7976931348623157e308"),
        (f64::MIN_POSITIVE, "2.2250738585072014e-308"),
        (f64::from_bits(1), "5.0e-324"),
        (-f64::from_bits(0x000F_FFFF_FFFF_FFFF), "-2.225073858507201e-308"),
    ];
    for (x, want) in table {
        let text = float_text(x);
        assert_eq!(text, want, "{x:e}");
        assert!(text.len() <= 24, "{text}: mehr als 24 Zeichen");
        assert_eq!(parse_value(&text, t.f64, &t.p).map(|v| v.as_f64().map(f64::to_bits)), Ok(Some(x.to_bits())));
    }
    // Die laengste Form unter 1e16 und die laengste unter 1e-5.
    assert!(float_text(-0.000012345678901234567).len() <= 24);
    assert!(float_text(-1.2345678901234567e-300).len() <= 24);
    let table32: [(f32, &str); 9] = [
        (0.1, "0.1"),
        (-0.0, "-0.0"),
        (3.0, "3.0"),
        (9999999.0, "9999999.0"),
        (1e7, "1.0e7"),
        (1e15, "1.0e15"),
        (9007200328482816.0, "9.0072e15"),
        (1e-6, "1.0e-6"),
        (f32::MAX, "3.4028235e38"),
    ];
    for (x, want) in table32 {
        let text = float32_text(x);
        assert_eq!(text, want, "{x:e}");
        assert!(text.len() <= 16, "{text}: mehr als 16 Zeichen");
        assert_eq!(parse_value(&text, t.f32, &t.p).map(|v| v.as_f64()), Ok(Some(f64::from(x))));
    }
    assert!(float32_text(-1.2345678e-30).len() <= 16 && float32_text(-0.000012345678).len() <= 16);
}

/// 3.9: `{x:.N}` schreibt N Nachkommastellen in derselben Form — fest bis
/// unter `1e16` bzw. `1e7`, darueber und unter `1e-5` als Exponent; dann
/// braucht ein `f64` hoechstens 18 + N Zeichen, ein `f32` 9 + N.
#[test]
fn a_fixed_precision_keeps_the_same_form() {
    use takt_interp::format::fixed;
    assert_eq!(fixed(2.5, 2, false), "2.50");
    assert_eq!(fixed(-1234.5678, 1, false), "-1234.6");
    assert_eq!(fixed(1e30, 3, false), "1.000e30");
    assert_eq!(fixed(1.5e-7, 2, false), "1.50e-7");
    assert_eq!(fixed(0.0, 3, false), "0.000");
    assert_eq!(fixed(-9999999999999998.0, 4, false).len(), 18 + 4);
    assert_eq!(fixed(1e7, 2, true), "1.00e7");
    assert_eq!(fixed(-9999999.0, 3, true).len(), 9 + 3);
}
