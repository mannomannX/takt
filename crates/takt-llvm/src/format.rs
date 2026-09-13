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

use takt_mir::pattern::{Format, FormatPiece};
use takt_mir::program::Program;

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
    ziel: Reg,
    ty: &LlvmType,
    cap: u32,
    p: &Program,
    m: &mut Module,
    vars: &dyn crate::expr::Vars,
) -> Result<(), NotYet> {
    let puffer = m.inst(&format!("getelementptr inbounds {ty}, ptr {ziel}, i32 0, i32 1"));
    // `at` ist die Schreibstelle; sie waechst mit jedem Baustein.
    let at_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {at_ptr}"));
    for piece in &f.pieces {
        match piece {
            FormatPiece::Text(t) => text(t.as_bytes(), puffer, at_ptr, cap, m),
            FormatPiece::Expr { expr, spec } => {
                let wert = crate::expr::lower(expr, p, m, vars)?;
                zahl(&wert, spec.as_deref(), puffer, at_ptr, cap, m)?;
            }
        }
    }
    let len = m.inst(&format!("load i32, ptr {at_ptr}"));
    let len_ptr = m.inst(&format!("getelementptr inbounds {ty}, ptr {ziel}, i32 0, i32 0"));
    m.void_inst(&format!("store i32 {len}, ptr {len_ptr}"));
    Ok(())
}

/// Literaler Text: Die Bytes stehen fest, also wird abgerollt.
fn text(bytes: &[u8], puffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    for b in bytes {
        let at = m.inst(&format!("load i32, ptr {at_ptr}"));
        // Am Rand wird nicht geschrieben; die Stelle wird geklemmt und
        // die Laenge waechst nicht weiter.
        let passt = m.inst(&format!("icmp slt i32 {at}, {cap}"));
        let safe = m.inst(&format!("select i1 {passt}, i32 {at}, i32 0"));
        let p = m.inst(&format!("getelementptr inbounds i8, ptr {puffer}, i32 {safe}"));
        let alt = m.inst(&format!("load i8, ptr {p}"));
        let neu = m.inst(&format!("select i1 {passt}, i8 {b}, i8 {alt}"));
        m.void_inst(&format!("store i8 {neu}, ptr {p}"));
        let weiter = m.inst(&format!("add i32 {at}, 1"));
        let ziel = m.inst(&format!("select i1 {passt}, i32 {weiter}, i32 {at}"));
        m.void_inst(&format!("store i32 {ziel}, ptr {at_ptr}"));
    }
}

/// Ein Ausdruck als Text (3.9).
fn zahl(
    wert: &crate::expr::Lowered,
    spec: Option<&str>,
    puffer: Reg,
    at_ptr: Reg,
    cap: u32,
    m: &mut Module,
) -> Result<(), NotYet> {
    if wert.ty.is_float() {
        return Err(NotYet { what: "`float` in einem Formatstring" });
    }
    let LlvmType::Int(bits) = wert.ty else {
        return Err(NotYet { what: "zusammengesetzter Wert in einem Formatstring" });
    };
    // Auf i64 bringen; die Ziffernrechnung laeuft einheitlich darauf.
    let v = match bits {
        64 => wert.value.clone(),
        1 => {
            // `bool` wird `true`/`false` (3.9), nicht 1/0.
            let k = m.next_label();
            let (ja, nein, fertig) = (format!("bool{k}_ja"), format!("bool{k}_nein"), format!("bool{k}_fertig"));
            m.void_inst(&format!("br i1 {}, label %{ja}, label %{nein}", wert.value));
            m.label(&ja);
            text(b"true", puffer, at_ptr, cap, m);
            m.void_inst(&format!("br label %{fertig}"));
            m.label(&nein);
            text(b"false", puffer, at_ptr, cap, m);
            m.void_inst(&format!("br label %{fertig}"));
            m.label(&fertig);
            return Ok(());
        }
        n => m.inst(&format!("sext i{n} {} to i64", wert.value)).to_string(),
    };
    match spec {
        Some("hex") => ziffern(&v, 16, false, puffer, at_ptr, cap, m),
        Some(s) if s.starts_with('0') => {
            let breite: u32 = s.parse().unwrap_or(0);
            ziffern_breit(&v, breite, puffer, at_ptr, cap, m);
        }
        None => ziffern(&v, 10, true, puffer, at_ptr, cap, m),
        Some(_) => return Err(NotYet { what: "Formatangabe" }),
    }
    Ok(())
}

/// Die Ziffern einer Zahl zur gegebenen Basis (3.9).
///
/// **Rueckwaerts in einen Zwischenpuffer, dann umgedreht.** Die Zahl der
/// Stellen steht erst fest, wenn man sie gerechnet hat; vorwaerts
/// muesste man sie zweimal rechnen. Zwanzig Byte reichen fuer jeden
/// `i64` samt Vorzeichen.
fn ziffern(v: &str, basis: u32, signed: bool, puffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let k = m.next_label();
    let tmp = m.inst("alloca [24 x i8]");
    let n_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 0, ptr {n_ptr}"));

    // **Vorzeichenbehaftet** wird auf dem *negativen* Wert gerechnet und
    // das Zeichen vorangestellt: Der Betrag von `i64::MIN` ist nicht
    // darstellbar (FB-114, derselbe Fall wie bei `parse_int`).
    //
    // **Ohne Vorzeichen** zaehlt das Bitmuster: `{x:hex}` von `-2` ist
    // `fffffffffffffffe`, wie der Interpreter es schreibt (`{:x}` von
    // `*i as u64`). Dafuer rechnet die Schleife mit `udiv`, und die
    // Abbruchbedingung ist „ungleich null" statt „noch negativ".
    let ist_neg = if signed { m.inst(&format!("icmp slt i64 {v}, 0")) } else { m.inst("and i1 false, false") };
    let start = if signed {
        let neg = m.inst(&format!("sub i64 0, {v}"));
        m.inst(&format!("select i1 {ist_neg}, i64 {v}, i64 {neg}"))
    } else {
        // Ohne Vorzeichen laeuft die Rechnung auf dem Bitmuster selbst.
        m.inst(&format!("add i64 {v}, 0"))
    };
    let rest_ptr = m.inst("alloca i64");
    m.void_inst(&format!("store i64 {start}, ptr {rest_ptr}"));

    let (kopf, rumpf, fertig) = (format!("zi{k}"), format!("zi{k}_rumpf"), format!("zi{k}_fertig"));
    m.void_inst(&format!("br label %{kopf}"));
    m.label(&kopf);
    let rest = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let weiter =
        if signed { m.inst(&format!("icmp slt i64 {rest}, 0")) } else { m.inst(&format!("icmp ne i64 {rest}, 0")) };
    m.void_inst(&format!("br i1 {weiter}, label %{rumpf}, label %{fertig}"));

    m.label(&rumpf);
    let r = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let q = if signed { m.inst(&format!("sdiv i64 {r}, {basis}")) } else { m.inst(&format!("udiv i64 {r}, {basis}")) };
    let mal = m.inst(&format!("mul i64 {q}, {basis}"));
    // Vorzeichenbehaftet ist der Rest negativ (die Schleife rechnet dort
    // auf dem negativen Wert), ohne Vorzeichen positiv.
    let ziffer64 = if signed { m.inst(&format!("sub i64 {mal}, {r}")) } else { m.inst(&format!("sub i64 {r}, {mal}")) };
    let d = m.inst(&format!("trunc i64 {ziffer64} to i8"));
    // 0–9 sind '0'+d, 10–15 sind 'a'+d-10.
    let ist_klein = m.inst(&format!("icmp slt i8 {d}, 10"));
    let als_ziffer = m.inst(&format!("add i8 {d}, 48"));
    let als_buchst = m.inst(&format!("add i8 {d}, 87"));
    let c = m.inst(&format!("select i1 {ist_klein}, i8 {als_ziffer}, i8 {als_buchst}"));
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let stelle = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 {n}"));
    m.void_inst(&format!("store i8 {c}, ptr {stelle}"));
    let n1 = m.inst(&format!("add i32 {n}, 1"));
    m.void_inst(&format!("store i32 {n1}, ptr {n_ptr}"));
    m.void_inst(&format!("store i64 {q}, ptr {rest_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&fertig);
    // Die Null hat keine Ziffer erzeugt; sie ist der einzige Fall.
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let leer = m.inst(&format!("icmp eq i32 {n}, 0"));
    let (null, weiter2) = (format!("zi{k}_null"), format!("zi{k}_weiter"));
    m.void_inst(&format!("br i1 {leer}, label %{null}, label %{weiter2}"));
    m.label(&null);
    let stelle0 = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 0"));
    m.void_inst(&format!("store i8 48, ptr {stelle0}"));
    m.void_inst(&format!("store i32 1, ptr {n_ptr}"));
    m.void_inst(&format!("br label %{weiter2}"));
    m.label(&weiter2);

    // Das Vorzeichen, dann die Ziffern rueckwaerts.
    let (minus, ohne) = (format!("zi{k}_minus"), format!("zi{k}_ohne"));
    m.void_inst(&format!("br i1 {ist_neg}, label %{minus}, label %{ohne}"));
    m.label(&minus);
    text(b"-", puffer, at_ptr, cap, m);
    m.void_inst(&format!("br label %{ohne}"));
    m.label(&ohne);
    umdrehen(tmp, n_ptr, puffer, at_ptr, cap, m);
}

/// Ziffern mit fester Breite, links mit Nullen gefuellt (`{x:08}`, 3.9).
///
/// Die Nullen kommen vor die Ziffern, also muss ihre Zahl vorher
/// feststehen: erst zaehlen, wie viele Stellen die Zahl hat, dann die
/// Differenz fuellen, dann die Ziffern schreiben.
fn ziffern_breit(v: &str, breite: u32, puffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let stellen = stellenzahl(v, m);
    let k = m.next_label();
    let (kopf, rumpf, fertig) = (format!("br{k}"), format!("br{k}_rumpf"), format!("br{k}_fertig"));
    let i_ptr = m.inst("alloca i32");
    m.void_inst(&format!("store i32 {stellen}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));
    m.label(&kopf);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let fehlt = m.inst(&format!("icmp slt i32 {i}, {breite}"));
    m.void_inst(&format!("br i1 {fehlt}, label %{rumpf}, label %{fertig}"));
    m.label(&rumpf);
    text(b"0", puffer, at_ptr, cap, m);
    let i2 = m.inst(&format!("load i32, ptr {i_ptr}"));
    let i3 = m.inst(&format!("add i32 {i2}, 1"));
    m.void_inst(&format!("store i32 {i3}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));
    m.label(&fertig);
    ziffern(v, 10, true, puffer, at_ptr, cap, m);
}

/// Wie viele Zeichen die Dezimaldarstellung braucht, das Vorzeichen
/// eingerechnet.
fn stellenzahl(v: &str, m: &mut Module) -> Reg {
    let k = m.next_label();
    let (kopf, rumpf, fertig) = (format!("sz{k}"), format!("sz{k}_rumpf"), format!("sz{k}_fertig"));
    let neg = m.inst(&format!("icmp slt i64 {v}, 0"));
    let n_ptr = m.inst("alloca i32");
    let anfang = m.inst(&format!("select i1 {neg}, i32 1, i32 0"));
    m.void_inst(&format!("store i32 {anfang}, ptr {n_ptr}"));
    // Wie in `ziffern` negativ rechnen: `i64::MIN` hat keinen Betrag.
    let umgekehrt = m.inst(&format!("sub i64 0, {v}"));
    let start = m.inst(&format!("select i1 {neg}, i64 {v}, i64 {umgekehrt}"));
    let rest_ptr = m.inst("alloca i64");
    m.void_inst(&format!("store i64 {start}, ptr {rest_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&kopf);
    let rest = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let weiter = m.inst(&format!("icmp slt i64 {rest}, 0"));
    m.void_inst(&format!("br i1 {weiter}, label %{rumpf}, label %{fertig}"));

    m.label(&rumpf);
    let r = m.inst(&format!("load i64, ptr {rest_ptr}"));
    let q = m.inst(&format!("sdiv i64 {r}, 10"));
    m.void_inst(&format!("store i64 {q}, ptr {rest_ptr}"));
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    let n1 = m.inst(&format!("add i32 {n}, 1"));
    m.void_inst(&format!("store i32 {n1}, ptr {n_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&fertig);
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    // Die Null hat keine Ziffer erzeugt und braucht doch eine Stelle.
    let keine = m.inst(&format!("icmp eq i32 {n}, 0"));
    m.inst(&format!("select i1 {keine}, i32 1, i32 {n}"))
}

/// Schreibt die Ziffern aus dem Zwischenpuffer rueckwaerts.
fn umdrehen(tmp: Reg, n_ptr: Reg, puffer: Reg, at_ptr: Reg, cap: u32, m: &mut Module) {
    let k = m.next_label();
    let (kopf, rumpf, fertig) = (format!("um{k}"), format!("um{k}_rumpf"), format!("um{k}_fertig"));
    let i_ptr = m.inst("alloca i32");
    let n = m.inst(&format!("load i32, ptr {n_ptr}"));
    m.void_inst(&format!("store i32 {n}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&kopf);
    let i = m.inst(&format!("load i32, ptr {i_ptr}"));
    let weiter = m.inst(&format!("icmp sgt i32 {i}, 0"));
    m.void_inst(&format!("br i1 {weiter}, label %{rumpf}, label %{fertig}"));

    m.label(&rumpf);
    let j = m.inst(&format!("sub i32 {i}, 1"));
    let von = m.inst(&format!("getelementptr inbounds [24 x i8], ptr {tmp}, i32 0, i32 {j}"));
    let c = m.inst(&format!("load i8, ptr {von}"));
    let at = m.inst(&format!("load i32, ptr {at_ptr}"));
    let passt = m.inst(&format!("icmp slt i32 {at}, {cap}"));
    let safe = m.inst(&format!("select i1 {passt}, i32 {at}, i32 0"));
    let nach = m.inst(&format!("getelementptr inbounds i8, ptr {puffer}, i32 {safe}"));
    let alt = m.inst(&format!("load i8, ptr {nach}"));
    let neu = m.inst(&format!("select i1 {passt}, i8 {c}, i8 {alt}"));
    m.void_inst(&format!("store i8 {neu}, ptr {nach}"));
    let a1 = m.inst(&format!("add i32 {at}, 1"));
    let ziel = m.inst(&format!("select i1 {passt}, i32 {a1}, i32 {at}"));
    m.void_inst(&format!("store i32 {ziel}, ptr {at_ptr}"));
    m.void_inst(&format!("store i32 {j}, ptr {i_ptr}"));
    m.void_inst(&format!("br label %{kopf}"));

    m.label(&fertig);
}
