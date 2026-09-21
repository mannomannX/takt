//! Die kanonische Byteform (5.9; plan/m6.md 2.2): entstanden fuer
//! `persist`, die Form aller Grenzen zwischen Interpreter und nativem Code.
//!
//! Zwei Eigenschaften traegt jeder Test: `decode(encode(v)) == v`, und
//! dieselbe Eingabe ergibt dieselben Bytes. Ohne die zweite waere der
//! CRC32 des Journals nicht reproduzierbar.

use takt_diag::Policy;
use takt_interp::Value;
use takt_interp::bytes::{decode, encode};
use takt_mir::bytes::{Error, max_size};
use takt_mir::types::{FloatWidth, IntWidth};
use takt_mir::{Program, TypeId};
use takt_sema::{Build, Options};

const HEAD: &str = "system:\n    language = 1\n    tick = 1 ms\n\n";

fn compile(body: &str) -> Program {
    let src = format!("{HEAD}{body}");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let out = takt_sema::compile(&src, &options);
    let errors: Vec<String> = out.diagnostics.iter().filter(|d| d.is_error()).map(|d| format!("{d}")).collect();
    assert!(errors.is_empty(), "unerwartete Fehler:\n{}", errors.join("\n"));
    out.program.expect("Programm")
}

/// Typ der einzigen `persist`-Variablen.
fn only_type(p: &Program) -> TypeId {
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("Maschine mit persist");
    m.vars[m.persist[0].var.index()].ty
}

/// Ein Programm mit einer `persist`-Variablen des gegebenen Typs.
fn with_var(decls: &str, var: &str) -> Program {
    compile(&format!(
        "{decls}
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    {var}
    initial RUN
    state RUN:
        loop:
            n = 1
"
    ))
}

/// Kodiert, dekodiert und prueft beide Eigenschaften.
fn roundtrip(p: &Program, ty: TypeId, v: &Value) -> Vec<u8> {
    let bytes = encode(p, v, ty).expect("kodierbar");
    let again = encode(p, v, ty).expect("kodierbar");
    assert_eq!(bytes, again, "dieselbe Eingabe, andere Bytes");
    let back = decode(p, &bytes, ty).expect("dekodierbar");
    assert_eq!(&back, v, "Roundtrip aendert den Wert");
    let limit = max_size(p, ty).expect("POD");
    assert!(bytes.len() <= limit as usize, "{} Byte ueber der Schranke {limit}", bytes.len());
    bytes
}

#[test]
fn a_bool_is_one_byte() {
    let p = with_var("", "persist var k : bool = false");
    let ty = only_type(&p);
    assert_eq!(roundtrip(&p, ty, &Value::Bool(false)), vec![0]);
    assert_eq!(roundtrip(&p, ty, &Value::Bool(true)), vec![1]);
}

#[test]
fn integers_use_their_width_little_endian() {
    let p = with_var("", "persist var k : u16 = 0");
    let ty = only_type(&p);
    assert_eq!(roundtrip(&p, ty, &Value::UInt(0x1234)), vec![0x34, 0x12]);

    let p = with_var("", "persist var k : i32 = 0");
    let ty = only_type(&p);
    assert_eq!(roundtrip(&p, ty, &Value::Int(-2)), vec![0xFE, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn a_negative_narrow_integer_survives_sign_extension() {
    let p = with_var("", "persist var k : i8 = 0");
    let ty = only_type(&p);
    for n in [-128i64, -1, 0, 127] {
        roundtrip(&p, ty, &Value::Int(n));
    }
}

#[test]
fn a_float_keeps_its_bit_pattern() {
    // 4.2 verlangt bitgleiche Ergebnisse; ein Roundtrip, der rundet,
    // waere eine zweite Rundungsquelle.
    let p = with_var("", "persist var k : float = 0.0");
    let ty = only_type(&p);
    for f in [0.0f64, -0.0, 1.5, f64::MIN, f64::MAX, f64::INFINITY] {
        roundtrip(&p, ty, &Value::F64(f));
    }
    let bytes = encode(&p, &Value::F64(f64::NAN), ty).expect("kodierbar");
    let back = decode(&p, &bytes, ty).expect("dekodierbar");
    assert!(matches!(back, Value::F64(f) if f.is_nan()), "NaN geht verloren");
}

#[test]
fn a_duration_is_eight_bytes_of_nanoseconds() {
    let p = with_var("", "persist var k : Duration = 0 s");
    let ty = only_type(&p);
    assert_eq!(roundtrip(&p, ty, &Value::Duration(1_000_000_000)).len(), 8);
}

#[test]
fn an_enum_carries_its_discriminant_not_its_index() {
    // Die Diskriminante ist erklaerter Teil des Typs (3.7); der Index
    // haengt an der Deklarationsreihenfolge.
    let p = with_var("enum Kind: A = 7, B = 9\n", "persist var k : Kind = A");
    let ty = only_type(&p);
    let bytes = roundtrip(&p, ty, &Value::Enum { variant: 0, fields: vec![] });
    assert_eq!(bytes[0], 7, "Diskriminante erwartet, nicht Index");
    let bytes = roundtrip(&p, ty, &Value::Enum { variant: 1, fields: vec![] });
    assert_eq!(bytes[0], 9);
}

#[test]
fn the_discriminant_is_eight_bytes_even_with_a_narrow_layout() {
    // `layout u8` gehoert zum Drahtformat; die Journal-Form daran zu
    // koppeln hiesse, dass eine Protokollaenderung Eintraege entwertet.
    let p = with_var("enum Kind layout u8: A = 0, B = 1\n", "persist var k : Kind = A");
    let ty = only_type(&p);
    assert_eq!(roundtrip(&p, ty, &Value::Enum { variant: 1, fields: vec![] }).len(), 8);
}

#[test]
fn an_enum_with_fields_encodes_them_after_the_discriminant() {
    let p = with_var("enum Cmd: NONE, ERASE(sector: u32)\n", "persist var k : Cmd = NONE");
    let ty = only_type(&p);
    roundtrip(&p, ty, &Value::Enum { variant: 0, fields: vec![] });
    let bytes = roundtrip(&p, ty, &Value::Enum { variant: 1, fields: vec![Value::UInt(0x11223344)] });
    assert_eq!(bytes.len(), 12, "acht Byte Diskriminante plus vier Byte Feld");
}

#[test]
fn a_record_has_no_padding() {
    let p = with_var(
        "record Reading:\n    flag : bool\n    code : u32\n",
        "persist var k : Reading = Reading(flag = false, code = 0)",
    );
    let ty = only_type(&p);
    let bytes = roundtrip(&p, ty, &Value::Record(vec![Value::Bool(true), Value::UInt(1)]));
    assert_eq!(bytes.len(), 5, "1 + 4 ohne Ausrichtung");
}

#[test]
fn an_array_encodes_its_elements_in_order() {
    let p = with_var("", "persist var k : [3] u8 = [0, 0, 0]");
    let ty = only_type(&p);
    let bytes = roundtrip(&p, ty, &Value::Array(vec![Value::UInt(1), Value::UInt(2), Value::UInt(3)]));
    assert_eq!(bytes, vec![1, 2, 3]);
}

#[test]
fn a_bytes_value_is_length_prefixed() {
    let p = with_var("", "persist var k : bytes<4> = default");
    let ty = only_type(&p);
    let bytes = roundtrip(&p, ty, &Value::Bytes(vec![0xAA, 0xBB]));
    assert_eq!(bytes, vec![2, 0, 0, 0, 0xAA, 0xBB]);
    roundtrip(&p, ty, &Value::Bytes(vec![]));
}

#[test]
fn a_vec_is_length_prefixed() {
    let p = with_var("", "persist var k : vec<u16, 3> = default");
    let ty = only_type(&p);
    roundtrip(&p, ty, &Value::Vec(vec![]));
    let bytes = roundtrip(&p, ty, &Value::Vec(vec![Value::UInt(1), Value::UInt(2)]));
    assert_eq!(bytes.len(), 8, "vier Byte Laenge plus zwei mal zwei");
}

#[test]
fn a_nested_record_in_an_array_survives() {
    let p = with_var(
        "record Pair:\n    a : bool\n    b : i16\n",
        "persist var k : [2] Pair = [Pair(a = false, b = 0), Pair(a = false, b = 0)]",
    );
    let ty = only_type(&p);
    let one = Value::Record(vec![Value::Bool(true), Value::Int(-300)]);
    let two = Value::Record(vec![Value::Bool(false), Value::Int(300)]);
    let bytes = roundtrip(&p, ty, &Value::Array(vec![one, two]));
    assert_eq!(bytes.len(), 6);
}

#[test]
fn the_selftest_record_of_example_14_7_roundtrips() {
    // Der Fall, der `wire::encode` ausschliesst: ein Record ohne `layout`
    // mit einem Float-Feld (definition.md 14.7).
    let p = with_var(
        "record SelftestResult:\n    passed : bool\n    code   : int in 0..255\n    r_int  : float[mohm] in \
         0..1000 mohm\n",
        "persist var k : SelftestResult = SelftestResult(passed = false, code = 0, r_int = 0 mohm)",
    );
    let ty = only_type(&p);
    let v = Value::Record(vec![Value::Bool(true), Value::Int(42), Value::F64(123.5)]);
    assert_eq!(roundtrip(&p, ty, &v).len(), 17, "1 + 8 + 8");
}

#[test]
fn trailing_bytes_are_rejected() {
    // Ein Eintrag, der laenger ist als sein Typ, passt nicht zu ihm.
    let p = with_var("", "persist var k : u8 = 0");
    let ty = only_type(&p);
    assert_eq!(decode(&p, &[1, 2], ty), Err(Error::Malformed));
}

#[test]
fn missing_bytes_are_rejected() {
    let p = with_var("", "persist var k : u32 = 0");
    let ty = only_type(&p);
    assert_eq!(decode(&p, &[1, 2], ty), Err(Error::Malformed));
}

#[test]
fn a_bool_byte_other_than_zero_or_one_is_rejected() {
    // Sonst entstuende ein `Value::Bool`, den kein Lauf erzeugen kann.
    let p = with_var("", "persist var k : bool = false");
    let ty = only_type(&p);
    assert_eq!(decode(&p, &[2], ty), Err(Error::Malformed));
}

#[test]
fn an_unknown_discriminant_is_rejected() {
    let p = with_var("enum Kind: A = 0, B = 1\n", "persist var k : Kind = A");
    let ty = only_type(&p);
    let mut bytes = encode(&p, &Value::Enum { variant: 0, fields: vec![] }, ty).expect("kodierbar");
    bytes[0] = 99;
    assert_eq!(decode(&p, &bytes, ty), Err(Error::Malformed));
}

#[test]
fn a_length_above_the_capacity_is_rejected() {
    let p = with_var("", "persist var k : bytes<2> = default");
    let ty = only_type(&p);
    assert_eq!(decode(&p, &[9, 0, 0, 0, 1, 2], ty), Err(Error::Malformed));
}

#[test]
fn a_non_pod_type_has_no_byte_form() {
    let p = compile(
        "
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    var k : int? = none
    initial RUN
    state RUN:
        loop:
            n = 1
",
    );
    let m = &p.machines[0];
    let ty = m.vars.iter().find(|v| v.name == "k").expect("Variable").ty;
    assert_eq!(max_size(&p, ty), Err(Error::NotPod));
    assert_eq!(encode(&p, &Value::Optional(None), ty), Err(Error::NotPod));
}

#[test]
fn the_size_bound_holds_for_the_corpus() {
    // Jede `persist`-Variable des Korpus muss eine Schranke haben, sonst
    // kann `takt size` den Journal-Posten nicht rechnen (11.5).
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus-try/35_persist.takt");
    let src = std::fs::read_to_string(path).expect("lesbar");
    let options = Options { policy: Policy::default(), build: Build::Sim, profile: None };
    let p = takt_sema::compile(&src, &options).program.expect("Programm");
    let mut seen = 0;
    for m in &p.machines {
        for pv in &m.persist {
            let ty = m.vars[pv.var.index()].ty;
            assert!(max_size(&p, ty).is_ok(), "`{}` ohne Schranke", m.vars[pv.var.index()].name);
            seen += 1;
        }
    }
    assert_eq!(seen, 3, "drei persist-Variablen erwartet");
}

/// Der Weg, den ein Wert wirklich nimmt: Programmzustand → kanonische
/// Bytes → Journal-Slot → zurueck (5.9).
///
/// Die Einzelteile sind je fuer sich geprueft; hier laufen sie zusammen,
/// weil ein Fehler an ihrer Naht sonst erst auf Hardware auffiele.
#[test]
fn a_value_survives_the_whole_journal_path() {
    use std::collections::HashMap;
    use takt_interp::nvm::Nvm;
    use takt_rt_core::journal::{FakeNvm, Journal, Loaded};

    const SLOT: usize = 256;

    let p = with_var(
        "record SelftestResult:\n    passed : bool\n    code   : int in 0..255\n",
        "persist var last : SelftestResult = SelftestResult(passed = false, code = 0)",
    );
    let m = p.machines.iter().find(|m| !m.persist.is_empty()).expect("persist");
    let hash = m.persist[0].type_hash;
    let ty = only_type(&p);
    let value = Value::Record(vec![Value::Bool(true), Value::Int(200)]);

    let payload = Nvm::payload(&p, &[(hash, &value, ty)]).expect("kodierbar");
    let logic = u64::from_le_bytes(takt_mir::hash::program_hash(&p).0[..8].try_into().expect("acht Byte"));

    let mut j = Journal::new(FakeNvm::<SLOT>::new(), logic, 0);
    j.load(&mut [0u8; SLOT]);
    let (mut stored, mut len) = ([0u8; SLOT], 0);
    for _ in 0..64 {
        j.poll(0, &payload, &mut stored, &mut len);
    }
    assert_eq!(len, payload.len(), "Journal hat nicht geschrieben");

    // Neuer Lauf: Slot lesen, Werte zurueckgewinnen.
    let mut next = Journal::new(j.into_inner(), logic, 0);
    let mut buf = [0u8; SLOT];
    let Loaded::Found { length, .. } = next.load(&mut buf) else {
        panic!("Slot nicht gefunden");
    };
    let mut nvm = Nvm::new();
    nvm.from_payload(&p, &buf[..length as usize], &HashMap::from([(hash, ty)]));
    assert_eq!(nvm.get(hash), Some(&value), "der Wert ueberlebte den Weg nicht");
}

#[test]
fn an_entry_of_an_unknown_type_hash_is_skipped() {
    use std::collections::HashMap;
    use takt_interp::nvm::Nvm;

    let p = with_var("", "persist var k : u16 = 0");
    let ty = only_type(&p);
    let payload = Nvm::payload(&p, &[(0xDEAD, &Value::UInt(7), ty)]).expect("kodierbar");

    // Das Programm kennt diesen Hash nicht mehr — etwa nach einer
    // Typaenderung.
    let mut nvm = Nvm::new();
    nvm.from_payload(&p, &payload, &HashMap::new());
    assert!(nvm.is_empty(), "fremder Eintrag wurde uebernommen");
}

#[test]
fn the_journal_appears_in_the_size_report() {
    // 11.5 fuehrt den Posten „Flash (Code, Konstanten, `persist`-Journal)";
    // ohne NVM-Geometrie ist er `offen`, nicht geschaetzt.
    use takt_mir::analysis::size;
    use takt_mir::hardware::{NvmGeometry, Target};

    let p = with_var("", "persist var k : u32 = 0");
    let report = size::size(&p);
    let ram = report.items.iter().find(|i| i.name == "persist-Journal (RAM)").expect("RAM-Posten");
    assert_eq!(ram.origin, size::Origin::Exact);
    assert_eq!(ram.bytes, 32, "zweimal 12 Byte Kopf plus vier Byte Wert");

    let flash = report.items.iter().find(|i| i.name == "persist-Journal (Flash)").expect("Flash-Posten");
    assert_eq!(flash.origin, size::Origin::Open, "ohne Geometrie offen");

    let target = Target {
        nvm: Some(NvmGeometry { sector_bytes: 2048, sectors: 2, ..NvmGeometry::default() }),
        ..Target::default()
    };
    let filled = size::size(&p).with_hardware(&target);
    let flash = filled.items.iter().find(|i| i.name == "persist-Journal (Flash)").expect("Flash-Posten");
    assert_eq!(flash.bytes, 4096);
    assert_eq!(flash.origin, size::Origin::Exact);
}

#[test]
fn a_program_without_persist_has_no_journal_items() {
    use takt_mir::analysis::size;
    let p = compile(
        "
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    initial RUN
    state RUN:
        loop:
            n = 1
",
    );
    let report = size::size(&p);
    assert!(!report.items.iter().any(|i| i.name.starts_with("persist-Journal")));
}

#[test]
fn the_write_interval_is_the_tightest_declared_one() {
    // Ein Slot traegt alle Variablen; die engste Zusage bindet (5.9).
    let p = compile(
        "
output n : int in 0..9 @ hw(\"o/n\") with safe = 0

machine m:
    persist var a : u32 = 0 with min_interval = 30 s
    persist var b : u32 = 0 with min_interval = 10 s
    initial RUN
    state RUN:
        loop:
            n = 1
",
    );
    assert_eq!(takt_mir::persist::min_interval_ns(&p), Some(10_000_000_000));

    let p = with_var("", "persist var k : u32 = 0");
    assert_eq!(takt_mir::persist::min_interval_ns(&p), None, "ohne Deklaration entscheidet das Ziel");
}

#[test]
fn the_tcb_encoder_writes_the_same_bytes() {
    // plan/m6.md 2.2: `takt_native::bytes` ist das `no_std`-Gegenstueck —
    // dieselbe Folge, dieselben Bytes.
    let mut host = takt_mir::bytes::Encoder::new();
    host.bool(true);
    host.int(-2, IntWidth::I16);
    host.int(7, IntWidth::U32);
    host.float(0x3f80_0000, FloatWidth::F32);
    host.float(0x3ff0_0000_0000_0000, FloatWidth::F64);
    host.duration(1);
    host.len(3);
    host.discriminant(-1);
    host.raw(&[0xaa, 0xbb]);
    let mut buf = [0u8; 64];
    let mut tcb = takt_native::bytes::Encoder::new(&mut buf);
    tcb.bool(true).expect("Platz");
    tcb.int(-2, 2).expect("Platz");
    tcb.int(7, 4).expect("Platz");
    tcb.f32(0x3f80_0000).expect("Platz");
    tcb.f64(0x3ff0_0000_0000_0000).expect("Platz");
    tcb.duration(1).expect("Platz");
    tcb.len(3).expect("Platz");
    tcb.discriminant(-1).expect("Platz");
    tcb.raw(&[0xaa, 0xbb]).expect("Platz");
    assert_eq!(host.bytes, tcb.written());
}

/// 8.9: Ein Capture-Element geht durch die Byteform und zurueck.
///
/// `capture` ist kein `persist`-Typ (nur Stromelement), der Typ kommt
/// darum aus dem Kanal.
#[test]
fn a_capture_element_round_trips_through_bytes() {
    let p = compile(
        "input  wave : stream<capture<float[V], 4>> @ hw(\"daq/c\") with max_rate = 10 Hz, capacity = 2
output dip  : float[V]                     @ hw(\"o/dip\") with safe = 0 V

machine m:
    initial RUN
    state RUN:
        on wave as w:
            dip = w.data.samples.min()
",
    );
    let ch = p.channels.iter().find(|c| c.name == "wave").expect("Kanal");
    let takt_mir::types::Type::Stream(elem) = p.types.list[ch.ty.index()] else { panic!("kein Strom") };
    let v = Value::Record(vec![
        Value::Duration(20_000_000),
        Value::Int(2),
        Value::Int(2),
        Value::F64(1000.0),
        Value::Array(vec![Value::F64(1.0), Value::F64(2.0), Value::F64(0.5), Value::F64(3.0)]),
    ]);
    let bytes = roundtrip(&p, elem, &v);
    // Kopf (24) plus vier `f64`.
    assert_eq!(bytes.len(), 24 + 4 * 8);
    assert_eq!(max_size(&p, elem), Ok(24 + 4 * 8));
}
