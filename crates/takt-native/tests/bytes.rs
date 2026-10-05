//! Die kanonische Byteform ohne Allokation: feste Breiten je Bestandteil
//! (little-endian, ohne Padding) und der volle Puffer.

use takt_native::bytes::{Encoder, Overflow};

#[test]
fn every_part_has_its_fixed_width() {
    let mut buf = [0u8; 64];
    let mut e = Encoder::new(&mut buf);
    e.bool(true).expect("Platz");
    e.int(-2, 2).expect("Platz");
    e.int(0x0102_0304, 4).expect("Platz");
    e.f32(0x3f80_0000).expect("Platz");
    e.f64(0x3ff0_0000_0000_0000).expect("Platz");
    e.duration(1).expect("Platz");
    e.len(3).expect("Platz");
    e.discriminant(-1).expect("Platz");
    e.raw(&[0xaa, 0xbb]).expect("Platz");
    let want: Vec<u8> = [
        vec![1],
        vec![0xfe, 0xff],
        vec![4, 3, 2, 1],
        vec![0, 0, 0x80, 0x3f],
        vec![0, 0, 0, 0, 0, 0, 0xf0, 0x3f],
        vec![1, 0, 0, 0, 0, 0, 0, 0],
        vec![3, 0, 0, 0],
        vec![0xff; 8],
        vec![0xaa, 0xbb],
    ]
    .concat();
    assert_eq!(e.written(), &want[..]);
}

#[test]
fn a_full_buffer_refuses_the_next_byte() {
    let mut buf = [0u8; 4];
    let mut e = Encoder::new(&mut buf);
    assert_eq!(e.len(7), Ok(()));
    assert_eq!(e.bool(true), Err(Overflow));
    assert_eq!(e.written(), &[7, 0, 0, 0]);
}

/// INT-025 (5.9, 3.7, 8.6): Ein NaN- oder Inf-Bitmuster in einem
/// Gleitkommafeld ist keine kanonische Form — allein, im Record, im Array
/// und als Rate im Kopf eines `capture`. Endliche Werte, auch -0.0 und die
/// kleinste Subnormale, sind es.
#[test]
fn a_non_finite_bit_pattern_is_no_canonical_form() {
    use takt_native::bytes::{decodes, shape};
    let f64s = |v: f64| v.to_bits().to_le_bytes().to_vec();
    let f32s = |v: f32| v.to_bits().to_le_bytes().to_vec();
    for v in [0.0, -0.0, 1.5, f64::MAX, f64::MIN_POSITIVE, 5e-324] {
        assert!(decodes(&[shape::F64], &f64s(v), true), "{v:e}");
    }
    for v in [f64::NAN, -f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(!decodes(&[shape::F64], &f64s(v), true), "{v}");
    }
    assert!(decodes(&[shape::F32], &f32s(-0.0), true));
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(!decodes(&[shape::F32], &f32s(v), true), "f32 {v}");
    }
    let record = [shape::RECORD, 2, 0, shape::F32, shape::F64];
    assert!(decodes(&record, &[f32s(1.0), f64s(2.0)].concat(), true));
    assert!(!decodes(&record, &[f32s(1.0), f64s(f64::NAN)].concat(), true), "im Record");
    let array = [shape::ARRAY, 3, 0, 0, 0, shape::F64];
    assert!(!decodes(&array, &[f64s(1.0), f64s(f64::INFINITY), f64s(2.0)].concat(), true), "im Array");
    let capture = [shape::CAPTURE, 1, 0, 0, 0, shape::F64];
    let head = |rate: f64| [vec![0; 8], vec![0; 4], vec![0; 4], f64s(rate)].concat();
    assert!(decodes(&capture, &[head(1000.0), f64s(1.0)].concat(), true));
    assert!(!decodes(&capture, &[head(f64::NAN), f64s(1.0)].concat(), true), "Rate des Kopfs");
    assert!(!decodes(&capture, &[head(1000.0), f64s(f64::NAN)].concat(), true), "Abtastwert");
}

/// INT-025: Ein Bestandteil, der nicht mehr ganz passt, schreibt nichts —
/// auch kein Anfangsstueck (f64 bei drei freien Bytes).
#[test]
fn a_part_that_does_not_fit_writes_nothing() {
    let mut buf = [0xEEu8; 7];
    let mut e = Encoder::new(&mut buf);
    e.len(1).expect("Platz");
    assert_eq!(e.f64(0x3ff0_0000_0000_0000), Err(Overflow));
    assert_eq!(e.duration(1), Err(Overflow));
    assert_eq!(e.written(), &[1, 0, 0, 0]);
    e.int(-1, 2).expect("Platz");
    assert_eq!(e.written(), &[1, 0, 0, 0, 0xff, 0xff]);
    assert_eq!(buf[6], 0xEE, "das Byte hinter dem Geschriebenen bleibt");
}

/// INT-025: Die Gestalten mit Laenge und Kapazitaet an ihren Raendern —
/// `vec` ueber seiner Kapazitaet, ein Array mit einem Element zu wenig,
/// ein `capture` ohne vollen Kopf, eine `map` mit unbekannter Slotmarke —
/// und die Schachtelung bis zur Tiefe 32.
#[test]
fn every_counted_shape_stops_at_its_bounds() {
    use takt_native::bytes::{decodes, shape};
    let vec2 = [shape::VEC, 2, 0, 0, 0, shape::INT, 1];
    assert!(decodes(&vec2, &[2, 0, 0, 0, 7, 8], true));
    assert!(decodes(&vec2, &[0, 0, 0, 0], true), "leer");
    assert!(!decodes(&vec2, &[3, 0, 0, 0, 7, 8, 9], true), "ueber der Kapazitaet");
    assert!(!decodes(&vec2, &[2, 0, 0, 0, 7], true), "ein Element fehlt");
    let array3 = [shape::ARRAY, 3, 0, 0, 0, shape::BOOL];
    assert!(decodes(&array3, &[0, 1, 0], true));
    assert!(!decodes(&array3, &[0, 1], true), "zu kurz");
    assert!(!decodes(&array3, &[0, 2, 0], true), "ein Element ist kein bool");
    let capture = [shape::CAPTURE, 2, 0, 0, 0, shape::INT, 1];
    let head = [vec![0; 16], 1000f64.to_bits().to_le_bytes().to_vec()].concat();
    assert!(decodes(&capture, &[head.clone(), vec![1, 2]].concat(), true));
    assert!(!decodes(&capture, &head[..20], true), "Kopf ohne Rate");
    assert!(!decodes(&capture, &[head, vec![1]].concat(), true), "ein Abtastwert fehlt");
    // `map<u8, bool, 2>`: je Slot eine Marke, Schluessel und Wert in ihrer Breite.
    let map = [shape::MAP, 2, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, shape::INT, 1, shape::BOOL];
    assert!(decodes(&map, &[1, 5, 1, 0, 0, 0], true), "ein belegter, ein freier Slot");
    assert!(!decodes(&map, &[2, 5, 1, 0, 0, 0], true), "Marke 2");
    assert!(!decodes(&map, &[1, 5, 2, 0, 0, 0], true), "Wert kein bool");
    assert!(!decodes(&map, &[1, 5, 1, 0, 0], true), "ein Slot zu kurz");
    let nested = |levels: usize| {
        let mut s: Vec<u8> = (0..levels).flat_map(|_| [shape::ARRAY, 1, 0, 0, 0]).collect();
        s.extend([shape::INT, 1]);
        s
    };
    assert!(decodes(&nested(32), &[9], true), "Tiefe 32");
    assert!(!decodes(&nested(33), &[9], true), "Tiefe 33");
}
