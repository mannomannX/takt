//! Formatstrings im erzeugten Code (3.9, 8.8).
//!
//! **Wofuer.** `send dut_tx, "UPDATE {size} {crc:hex}\n"` schreibt einen
//! Text in einen Puffer fester Hoechstlaenge (8.8). Die Laenge steht in
//! `Format::len_max` und ist statisch geprueft; hier entsteht der
//! Inhalt.
//!
//! **Warum als IR.** Dieselbe Naht wie beim Mustervergleich
//! (`captures.rs`): Ein Aufruf in die Runtime muesste die Bausteine und
//! ihre Werte ueber Rohzeiger reichen, und `unsafe_code = "forbid"`
//! gilt workspace-weit (13.4). Die Bausteine stehen ohnehin zur
//! Uebersetzungszeit fest — ein Literal ist eine Konstante, ein `{x}`
//! ein Ausdruck, den der Codegen schon senken kann.
//!
//! **Was hier nicht steht: `float`.** Die Ausgabe einer Gleitkommazahl
//! als kuerzeste wiedereinlesbare Dezimalzahl ist ein eigener
//! Algorithmus (Grisu, Ryū), und der Interpreter nimmt dafuer den aus
//! `std`. Ihn in IR nachzubauen hiesse, eine zweite Rundungsquelle neben
//! `libtaktm` zu schaffen — 4.2 verlangt bitgleiche Ergebnisse ueber
//! alle Targets, und zwei Implementierungen derselben Konversion sind
//! genau das, was die Zusage bricht. Der Codegen meldet den Fall, statt
//! still eine andere Zahl zu schreiben; dieselbe Entscheidung wie bei
//! `{x:float}` in `takt-match` und `captures.rs`.

use takt_mir::TypeId;
use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::program::Program;
use takt_mir::types::Type;

use crate::emit::{Module, Reg};
use crate::expr::NotYet;
use crate::ty::LlvmType;

/// Schreibt einen Formatstring in einen Puffer `{ i32 len, [N x i8] }`.
///
/// `ziel` zeigt auf den Puffer, `cap` ist seine Kapazitaet. Der Text
/// wird bei `cap` abgeschnitten — 8.8 prueft statisch, dass das nicht
/// noetig ist, aber ein Puffer, der ueberlaeuft, waere schlimmer als
/// einer, der kuerzt (4.1).
pub fn render(
    f: &Format,
    target: Reg,
    ty: &LlvmType,
    cap: u32,
    p: &Program,
    m: &mut Module,
    vars: &dyn crate::expr::Vars,
) -> Result<(), NotYet> {
    let buffer = m.inst(&format!("getelementptr inbounds {ty}, ptr {target}, i32 0, i32 1"));
    // `at` ist die Schreibstelle; sie waechst mit jedem Baustein.
    let at_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {at_ptr}"));
    for piece in &f.pieces {
        match piece {
            FormatPiece::Text(t) => text(t.as_bytes(), buffer, at_ptr, cap, m),
            FormatPiece::Expr { expr, spec } => {
                let value_of = crate::expr::lower(expr, p, m, vars)?;
                let out = Out { buffer, at_ptr, cap };
                match p.types.list.get(expr.ty.index()) {
                    Some(Type::Duration { .. }) => duration(&value_of.value, &out, m),
                    Some(Type::Enum(e)) => variant_name(&value_of, *e, p, &out, m)?,
                    _ => number(&value_of, spec.as_deref(), expr.ty, p, buffer, at_ptr, cap, m)?,
                }
            }
        }
    }
    let len = m.inst(&format!("load i32, ptr {at_ptr}"));
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {target}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {len}, ptr {len_ptr}"));
    Ok(())
}

/// Literaler Text: Die Bytes stehen fest, also wird abgerollt.
fn text(bytes: &[u8], buffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    for b in bytes {
        let at = m.inst(&format!("load i32, ptr {at_ptr}"));
        // Am Rand wird nicht geschrieben; die Stelle wird geklemmt und
        // die Laenge waechst nicht weiter.
        let fits = m.inst(&format!("icmp slt i32 {at}, {cap}"));
        let safe = m.inst(&format!("select i1 {fits}, i32 {at}, i32 0"));
        let p = m.inst(&format!("getelementptr inbounds i8, ptr {buffer}, i32 {safe}"));
        let old = m.inst(&format!("load i8, ptr {p}"));
        let new_val = m.inst(&format!("select i1 {fits}, i8 {b}, i8 {old}"));
        m.void_inst(&format!("store i8 {new_val}, ptr {p}"));
        let go_on = m.inst(&format!("add i32 {at}, 1"));
        let target = m.inst(&format!("select i1 {fits}, i32 {go_on}, i32 {at}"));
        m.void_inst(&format!("store i32 {target}, ptr {at_ptr}"));
    }
}

/// Wohin ein Baustein schreibt: Puffer, Schreibstelle, Kapazitaet.
struct Out {
    buffer: Reg,
    at_ptr: Reg,
    cap: u32,
}

/// Eine Dauer wie der Interpreter sie schreibt (`takt_mir::dump::duration`):
/// in der groessten Einheit, in der sie ganzzahlig ist, null als `0 ns`.
fn duration(ns: &str, out: &Out, m: &mut Module) {
    const UNITS: [(&str, i64); 6] = [
        ("d", 86_400_000_000_000),
        ("h", 3_600_000_000_000),
        ("min", 60_000_000_000),
        ("s", 1_000_000_000),
        ("ms", 1_000_000),
        ("us", 1_000),
    ];
    let k = m.next_label();
    let done = format!("dauer{k}_fertig");
    let nonzero = m.inst(&format!("icmp ne i64 {ns}, 0"));
    for (i, (name, factor)) in UNITS.iter().enumerate() {
        let (hit, next) = (format!("dauer{k}_{i}"), format!("dauer{k}_{i}_sonst"));
        let rest = m.inst(&format!("srem i64 {ns}, {factor}"));
        let whole = m.inst(&format!("icmp eq i64 {rest}, 0"));
        let take = m.inst(&format!("and i1 {nonzero}, {whole}"));
        m.void_inst(&format!("br i1 {take}, label %{hit}, label %{next}"));
        m.label(&hit);
        let q = m.inst(&format!("sdiv i64 {ns}, {factor}"));
        digits(&q.to_string(), 10, true, true, out.buffer, out.at_ptr, out.cap, m);
        text(format!(" {name}").as_bytes(), out.buffer, out.at_ptr, out.cap, m);
        m.void_inst(&format!("br label %{done}"));
        m.label(&next);
    }
    digits(ns, 10, true, true, out.buffer, out.at_ptr, out.cap, m);
    text(b" ns", out.buffer, out.at_ptr, out.cap, m);
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
}

/// Ein Enum ohne Felder als Name seiner Variante, wie im Interpreter
/// (3.7); die Diskriminante waere eine Zahl, die das Programm nie schrieb.
fn variant_name(
    value_of: &crate::expr::Lowered,
    e: takt_mir::EnumId,
    p: &Program,
    out: &Out,
    m: &mut Module,
) -> Result<(), NotYet> {
    let def = p.enums.get(e.index()).ok_or(NotYet { what: "Enum" })?;
    if def.variants.iter().any(|v| !v.fields.is_empty()) {
        return Err(NotYet { what: "Enum mit Feldern in einem Formatstring" });
    }
    let LlvmType::Int(bits) = value_of.ty else { return Err(NotYet { what: "Diskriminante" }) };
    let k = m.next_label();
    let done = format!("variante{k}_fertig");
    let Some((last, others)) = def.variants.split_last() else { return Err(NotYet { what: "leeres Enum" }) };
    for (i, v) in others.iter().enumerate() {
        let (hit, next) = (format!("variante{k}_{i}"), format!("variante{k}_{i}_sonst"));
        let is = m.inst(&format!("icmp eq i{bits} {}, {}", value_of.value, v.discriminant));
        m.void_inst(&format!("br i1 {is}, label %{hit}, label %{next}"));
        m.label(&hit);
        text(v.name.as_bytes(), out.buffer, out.at_ptr, out.cap, m);
        m.void_inst(&format!("br label %{done}"));
        m.label(&next);
    }
    // Eine andere Diskriminante gibt es nicht: Das Sema laesst nur die
    // Varianten zu.
    text(last.name.as_bytes(), out.buffer, out.at_ptr, out.cap, m);
    m.void_inst(&format!("br label %{done}"));
    m.label(&done);
    Ok(())
}

/// Ein Ausdruck als Text (3.9). Die Vorzeichenart kommt aus dem Typ:
/// Ein `u8` 200 ist `200` und `c8`, nicht `-56` und `ffffffffffffffc8`.
#[allow(clippy::too_many_arguments)]
fn number(
    value_of: &crate::expr::Lowered,
    spec: Option<&str>,
    ty: TypeId,
    p: &Program,
    buffer: Reg,
    at_ptr: Reg,
    cap: u32,
    m: &mut Module,
) -> Result<(), NotYet> {
    if value_of.ty.is_float() {
        return Err(NotYet { what: "`float` in einem Formatstring" });
    }
    let LlvmType::Int(bits) = value_of.ty else {
        return Err(NotYet { what: "zusammengesetzter Wert in einem Formatstring" });
    };
    let signed = crate::expr::int_is_signed(ty, p);
    // Auf i64 bringen; die Ziffernrechnung laeuft einheitlich darauf.
    let v = match bits {
        64 => value_of.value.clone(),
        1 => {
            // `bool` wird `true`/`false` (3.9), nicht 1/0.
            let k = m.next_label();
            let (ja, nein, done) = (format!("bool{k}_ja"), format!("bool{k}_nein"), format!("bool{k}_fertig"));
            m.void_inst(&format!("br i1 {}, label %{ja}, label %{nein}", value_of.value));
            m.label(&ja);
            text(b"true", buffer, at_ptr, cap, m);
            m.void_inst(&format!("br label %{done}"));
            m.label(&nein);
            text(b"false", buffer, at_ptr, cap, m);
            m.void_inst(&format!("br label %{done}"));
            m.label(&done);
            return Ok(());
        }
        n => {
            let widen = if signed { "sext" } else { "zext" };
            m.inst(&format!("{widen} i{n} {} to i64", value_of.value)).to_string()
        }
    };
    match spec {
        Some("hex") => digits(&v, 16, false, false, buffer, at_ptr, cap, m),
        Some(s) if s.starts_with('0') => {
            let width: u32 = s.parse().unwrap_or(0);
            digits_padded(&v, width, signed, buffer, at_ptr, cap, m);
        }
        None => digits(&v, 10, signed, signed, buffer, at_ptr, cap, m),
        Some(_) => return Err(NotYet { what: "Formatangabe" }),
    }
    Ok(())
}

/// Die Ziffern einer Zahl zur gegebenen Basis (3.9).
///
/// **Rueckwaerts in einen Zwischenpuffer, dann umgedreht.** Die Zahl der
/// Stellen steht erst fest, wenn man sie gerechnet hat; vorwaerts
/// muesste man sie zweimal rechnen. Zwanzig Byte reichen fuer jeden
/// `i64` samt Vorzeichen. `with_sign` schreibt das Minus; `digits_padded`
/// hat es schon vor die Nullen gesetzt.
#[allow(clippy::too_many_arguments)]
fn digits(v: &str, base: u32, signed: bool, with_sign: bool, buffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let k = m.next_label();
    let tmp = m.alloca("[24 x i8]");
    let n_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 0, ptr {n_ptr}"));

    // **Vorzeichenbehaftet** wird auf dem *negativen* Wert gerechnet und
    // das Zeichen vorangestellt: Der Betrag von `i64::MIN` ist nicht
    // darstellbar (FB-114, derselbe Fall wie bei `parse_int`).
    //
    // **Ohne Vorzeichen** zaehlt das Bitmuster: `{x:hex}` von `-2` ist
    // `fffffffffffffffe`, wie der Interpreter es schreibt (`{:x}` von
    // `*i as u64`). Dafuer rechnet die Schleife mit `udiv`, und die
    // Abbruchbedingung ist „ungleich null" statt „noch negativ".
    let is_neg = if signed { m.inst(&format!("icmp slt i64 {v}, 0")) } else { m.inst("and i1 false, false") };
    let start = if signed {
        let neg = m.inst(&format!("sub i64 0, {v}"));
        m.inst(&format!("select i1 {is_neg}, i64 {v}, i64 {neg}"))
    } else {
        // Ohne Vorzeichen laeuft die Rechnung auf dem Bitmuster selbst.
        m.inst(&format!("add i64 {v}, 0"))
    };
    let rest_ptr = m.alloca("i64");
    m.void_inst(&format!("store i64 {start}, ptr {rest_ptr}"));

    let (head, body, done) = (format!("zi{k}"), format!("zi{k}_rumpf"), format!("zi{k}_fertig"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let rest = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let go_on =
        if signed { m.inst(&format!("icmp slt i64 {rest}, 0")) } else { m.inst(&format!("icmp ne i64 {rest}, 0")) };
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let r = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let q = if signed { m.inst(&format!("sdiv i64 {r}, {base}")) } else { m.inst(&format!("udiv i64 {r}, {base}")) };
    let scaled = m.inst(&format!("mul i64 {q}, {base}"));
    // Vorzeichenbehaftet ist der Rest negativ (die Schleife rechnet dort
    // auf dem negativen Wert), ohne Vorzeichen positiv.
    let digit64 =
        if signed { m.inst(&format!("sub i64 {scaled}, {r}")) } else { m.inst(&format!("sub i64 {r}, {scaled}")) };
    let d = m.inst(&format!("trunc i64 {digit64} to i8"));
    // 0–9 sind '0'+d, 10–15 sind 'a'+d-10.
    let is_lower = m.inst(&format!("icmp slt i8 {d}, 10"));
    let als_ziffer = m.inst(&format!("add i8 {d}, 48"));
    let als_buchst = m.inst(&format!("add i8 {d}, 87"));
    let c = m.inst(&format!("select i1 {is_lower}, i8 {als_ziffer}, i8 {als_buchst}"));
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let slot_at = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 {n}"));
    m.void_inst(&format!("store i8 {c}, ptr {slot_at}"));
    let n1 = m.inst(&format!("add i32 {n}, 1"));
    m.void_inst(&format!("store i32 {n1}, ptr {n_ptr}"));
    m.void_inst(&format!("store i64 {q}, ptr {rest_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    // Die Null hat keine Ziffer erzeugt; sie ist der einzige Fall.
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let empty = m.inst(&format!("icmp eq i32 {n}, 0"));
    let (zero, weiter2) = (format!("zi{k}_null"), format!("zi{k}_weiter"));
    m.void_inst(&format!("br i1 {empty}, label %{zero}, label %{weiter2}"));
    m.label(&zero);
    let slot0 = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 0"));
    m.void_inst(&format!("store i8 48, ptr {slot0}"));
    m.void_inst(&format!("store i32 1, ptr {n_ptr}"));
    m.void_inst(&format!("br label %{weiter2}"));
    m.label(&weiter2);

    // Das Vorzeichen, dann die Ziffern rueckwaerts.
    let show = if with_sign { is_neg } else { m.inst("and i1 false, false") };
    let (minus, ohne) = (format!("zi{k}_minus"), format!("zi{k}_ohne"));
    m.void_inst(&format!("br i1 {show}, label %{minus}, label %{ohne}"));
    m.label(&minus);
    text(b"-", buffer, at_ptr, cap, m);
    m.void_inst(&format!("br label %{ohne}"));
    m.label(&ohne);
    write_reversed(tmp, n_ptr, buffer, at_ptr, cap, m);
}

/// Ziffern mit fester Breite, links mit Nullen gefuellt (`{x:08}`, 3.9).
///
/// Die Nullen kommen vor die Ziffern, also muss ihre Zahl vorher
/// feststehen: erst zaehlen, wie viele Stellen die Zahl hat, dann die
/// Differenz fuellen, dann die Ziffern schreiben.
fn digits_padded(v: &str, width: u32, signed: bool, buffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let places = digit_count(v, signed, m);
    let k = m.next_label();
    // Das Vorzeichen steht vor den Nullen: `{x:04}` von -2 ist `-002`,
    // wie der Interpreter es schreibt (FB-172). Ohne Vorzeichen gibt es
    // keins, auch nicht ueber `i64::MAX`.
    let neg = if signed { m.inst(&format!("icmp slt i64 {v}, 0")) } else { m.inst("and i1 false, false") };
    let (minus, ohne) = (format!("br{k}_minus"), format!("br{k}_ohne"));
    m.void_inst(&format!("br i1 {neg}, label %{minus}, label %{ohne}"));
    m.label(&minus);
    text(b"-", buffer, at_ptr, cap, m);
    m.void_inst(&format!("br label %{ohne}"));
    m.label(&ohne);
    let (head, body, done) = (format!("br{k}"), format!("br{k}_rumpf"), format!("br{k}_fertig"));
    let i_ptr = m.alloca("i32");
    m.void_inst(&format!("store i32 {places}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let missing = m.inst(&format!("icmp slt i32 {i}, {width}"));
    m.void_inst(&format!("br i1 {missing}, label %{body}, label %{done}"));
    m.label(&body);
    text(b"0", buffer, at_ptr, cap, m);
    let i2 = m.inst(&format!("load i32, ptr {i_ptr}"));
    let i3 = m.inst(&format!("add i32 {i2}, 1"));
    m.void_inst(&format!("store i32 {i3}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));
    m.label(&done);
    digits(v, 10, signed, false, buffer, at_ptr, cap, m);
}

/// Wie viele Zeichen die Dezimaldarstellung braucht, das Vorzeichen
/// eingerechnet.
fn digit_count(v: &str, signed: bool, m: &mut Module) -> Reg {
    let k = m.next_label();
    let (head, body, done) = (format!("sz{k}"), format!("sz{k}_rumpf"), format!("sz{k}_fertig"));
    let neg = if signed { m.inst(&format!("icmp slt i64 {v}, 0")) } else { m.inst("and i1 false, false") };
    let n_ptr = m.alloca("i32");
    let start_at = m.inst(&format!("select i1 {neg}, i32 1, i32 0"));
    m.void_inst(&format!("store i32 {start_at}, ptr {n_ptr}"));
    // Wie in `ziffern` vorzeichenbehaftet negativ rechnen: `i64::MIN` hat
    // keinen Betrag. Ohne Vorzeichen zaehlt das Bitmuster selbst.
    let start = if signed {
        let umgekehrt = m.inst(&format!("sub i64 0, {v}"));
        m.inst(&format!("select i1 {neg}, i64 {v}, i64 {umgekehrt}"))
    } else {
        m.inst(&format!("add i64 {v}, 0"))
    };
    let rest_ptr = m.alloca("i64");
    m.void_inst(&format!("store i64 {start}, ptr {rest_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let rest = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let go_on =
        if signed { m.inst(&format!("icmp slt i64 {rest}, 0")) } else { m.inst(&format!("icmp ne i64 {rest}, 0")) };
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let r = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let q = if signed { m.inst(&format!("sdiv i64 {r}, 10")) } else { m.inst(&format!("udiv i64 {r}, 10")) };
    m.void_inst(&format!("store i64 {q}, ptr {rest_ptr}"));
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let n1 = m.inst(&format!("add i32 {n}, 1"));
    m.void_inst(&format!("store i32 {n1}, ptr {n_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    // Die Null hat keine Ziffer erzeugt und braucht doch eine Stelle.
    let keine = m.inst(&format!("icmp eq i32 {n}, 0"));
    m.inst(&format!("select i1 {keine}, i32 1, i32 {n}"))
}

/// Schreibt die Ziffern aus dem Zwischenpuffer rueckwaerts.
fn write_reversed(tmp: Reg, n_ptr: Reg, buffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let k = m.next_label();
    let (head, body, done) = (format!("um{k}"), format!("um{k}_rumpf"), format!("um{k}_fertig"));
    let i_ptr = m.alloca("i32");
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    m.void_inst(&format!("store i32 {n}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&head);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let go_on = m.inst(&format!("icmp sgt i32 {i}, 0"));
    m.void_inst(&format!("br i1 {go_on}, label %{body}, label %{done}"));

    m.label(&body);
    let j = m.inst(&format!("sub i32 {i}, 1"));
    let from_at = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 {j}"));
    let c = m.inst(&format!("load i8, ptr {from_at}"));
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let fits = m.inst(&format!("icmp slt i32 {at}, {cap}"));
    let safe = m.inst(&format!("select i1 {fits}, i32 {at}, i32 0"));
    let after = m.inst(&format!("getelementptr inbounds i8, ptr {buffer}, i32 {safe}"));
    let old = m.inst(&format!("load i8, ptr {after}"));
    let new_val = m.inst(&format!("select i1 {fits}, i8 {c}, i8 {old}"));
    m.void_inst(&format!("store i8 {new_val}, ptr {after}"));
    let a1 = m.inst(&format!("add i32 {at}, 1"));
    let target = m.inst(&format!("select i1 {fits}, i32 {a1}, i32 {at}"));
    m.void_inst(&format!("store i32 {target}, ptr {at_ptr}"));
    m.void_inst(&format!("store i32 {j}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{head}"));

    m.label(&done);
}
