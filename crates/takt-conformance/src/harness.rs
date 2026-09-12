//! Der Testrahmen: C-Code, der den erzeugten Tickschritt ausfuehrt.
//!
//! **Warum C und nicht Rust.** Der erzeugte Code spricht die C-ABI, und
//! clang uebersetzt beides in einem Zug. Ein Rust-Gegenstueck braeuchte
//! `extern "C"` und `unsafe` fuer jeden Zeiger — beides verbietet der
//! Workspace, und eine Ausnahme nur fuer den Testrahmen waere eine
//! Ausnahme zu viel.
//!
//! **Was der Rahmen ist und was nicht.** Er ist die Runtime aus 12.1,
//! reduziert auf das, was der Vergleich braucht: Er haelt das
//! Prozessabbild, ruft `init` einmal und `step` je Tick, und schreibt den
//! Latch nach jedem Tick aus. Er ist *keine* Runtime — er hat keine Uhr,
//! keine Treiber und keine Fault-Behandlung. Was er nicht kann, kann der
//! Vergleich nicht pruefen, und das steht in `run`.

use std::fmt::Write as _;

use takt_mir::program::Program;

use crate::layout::{Layout, c_type};

/// Der erzeugte Testrahmen.
pub struct Harness {
    /// Der C-Quelltext.
    pub source: String,
    /// Die Speicherform, die er erwartet.
    pub layout: Layout,
}

/// Baut den Rahmen fuer ein Programm.
///
/// `machine` ist der Name der Maschine, deren Schritt gerufen wird; ihre
/// Funktionen heissen `<name>_init` und `<name>_step` (11.2).
pub fn build(p: &Program, machine: &str, ticks: u64) -> Harness {
    let layout = crate::layout::of(p);
    let mut s = String::new();
    let _ = writeln!(s, "/* Testrahmen (13.8); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "#include <stdio.h>");
    let _ = writeln!(s, "#include <string.h>\n");

    // Die Runtime-Aufrufe (`takt-llvm/src/abi.rs`). Sie schreiben in den
    // Trace, damit der Vergleich sie sieht.
    let _ = writeln!(s, "static long long g_tick = 0;");
    // Das Fault-Flag der reinen Funktionen (4.1, `abi::Abi::FAULT_FLAG`).
    // Es gehoert der Runtime; der Rahmen stellt es bereit und setzt es je
    // Tick zurueck, wie es die Abort-Phase taete.
    let _ = writeln!(s, "unsigned char takt_fn_fault = 0;");
    let _ = writeln!(s, "void takt_alert(int m, int site, unsigned char on) {{");
    let _ = writeln!(s, "    printf(\"t=%lld alert %d %d %d\\n\", g_tick, m, site, on ? 1 : 0);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_log(int m, int site) {{ printf(\"t=%lld log %d %d\\n\", g_tick, m, site); }}");
    let _ = writeln!(s, "void takt_measure(int m, int site, double v) {{");
    let _ = writeln!(s, "    printf(\"t=%lld measure %d %d %.17g\\n\", g_tick, m, site, v);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_verify(int m, int site, unsigned char ok) {{");
    let _ = writeln!(s, "    printf(\"t=%lld verify %d %d %d\\n\", g_tick, m, site, ok ? 1 : 0);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_abort(int m, int site) {{ printf(\"t=%lld abort %d %d\\n\", g_tick, m, site); }}\n");

    // Die Stroeme (`takt-llvm/src/stream.rs`). Der Rahmen liefert keine
    // Elemente; ein leeres Fenster ist der Fall, den jedes Programm
    // aushalten muss.
    let _ = writeln!(s, "int takt_stream_count(int s, long long cur) {{ (void)s; (void)cur; return 0; }}");
    let _ = writeln!(s, "long long takt_stream_at(int s, long long cur, int i, void *out) {{");
    let _ = writeln!(s, "    (void)s; (void)cur; (void)i; (void)out; return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_stream_examined(int s, long long seq) {{ (void)s; (void)seq; }}\n");

    // 4.5: Die nativen Funktionen liegen in der Runtime. Der Rahmen
    // liefert sie in C — dieselbe Rechnung wie `takt-native`, damit der
    // Vergleich sie mitprueft statt sie zu umgehen.
    if p.natives.iter().any(|n| n.name == "crc32") {
        let _ = writeln!(s, "unsigned int takt_native_crc32(const unsigned char *b, int n) {{");
        let _ = writeln!(s, "    unsigned int c = 0xFFFFFFFFu;");
        let _ = writeln!(s, "    for (int i = 0; i < n; i++) {{");
        let _ = writeln!(s, "        c ^= b[i];");
        let _ = writeln!(s, "        for (int k = 0; k < 8; k++)");
        let _ = writeln!(s, "            c = (c & 1u) ? ((c >> 1) ^ 0xEDB88320u) : (c >> 1);");
        let _ = writeln!(s, "    }}");
        let _ = writeln!(s, "    return c ^ 0xFFFFFFFFu;");
        let _ = writeln!(s, "}}");
    }
    if p.natives.iter().any(|n| n.name == "sum8") {
        let _ = writeln!(s, "unsigned char takt_native_sum8(const unsigned char *b, int n) {{");
        let _ = writeln!(s, "    unsigned char s = 0;");
        let _ = writeln!(s, "    for (int i = 0; i < n; i++) s = (unsigned char)(s + b[i]);");
        let _ = writeln!(s, "    return s;");
        let _ = writeln!(s, "}}");
    }
    let _ = writeln!(s, "void {machine}_init(void *st, void *in, void *par, void *out);");
    let _ = writeln!(s, "void {machine}_step(void *st, void *in, void *par, void *out);\n");

    // Der Zustands-Struct ist gross genug bemessen; seine genaue Groesse
    // kennt nur der Codegen, und sie ueberzuschaetzen kostet im Test
    // nichts.
    let _ = writeln!(s, "static char state[4096];");
    let _ = writeln!(s, "static char image[{}];", layout.image.max(1));
    let _ = writeln!(s, "static char params[{}];", layout.params.max(1));
    let _ = writeln!(s, "static char latch[{}];\n", layout.latch.max(1));

    let _ = writeln!(s, "int main(void) {{");
    let _ = writeln!(s, "    memset(state, 0, sizeof state);");
    let _ = writeln!(s, "    memset(image, 0, sizeof image);");
    let _ = writeln!(s, "    memset(params, 0, sizeof params);");
    let _ = writeln!(s, "    memset(latch, 0, sizeof latch);");
    // 3.5: Ein Input ohne Treiber ist `Bad`. Ein genullter Abbild-Eintrag
    // hiesse `Good` (die Skala beginnt dort), und der Vergleich pruefte
    // dann einen Lauf, den es nicht gibt — der Interpreter faultet in
    // diesem Fall. Der Rahmen setzt die Qualitaet darum ausdruecklich.
    for slot in &layout.inputs {
        let Some(entry) = quality_offset(p, &slot.name) else { continue };
        let _ = writeln!(s, "    image[{entry}] = {}; /* {} ist Bad (3.5) */", 3, slot.name);
    }

    // Die Parameter stehen fuer den Lauf fest (8.4); sie werden einmal
    // gesetzt.
    for (i, slot) in layout.parameters.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = param_literal(p, i) else { continue };
        let _ = writeln!(s, "    *({ct} *)(params + {}) = {value}; /* {} */", slot.offset, slot.name);
    }

    let _ = writeln!(s, "    {machine}_init(state, image, params, latch);");
    let _ = writeln!(s, "    dump(0);");
    let _ = writeln!(s, "    for (g_tick = 1; g_tick <= {ticks}; g_tick++) {{");
    let _ = writeln!(s, "        {machine}_step(state, image, params, latch);");
    let _ = writeln!(s, "        dump(g_tick);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");

    // `dump` steht hinter `main`, damit die Deklaration oben genuegt.
    let mut dump = String::new();
    let _ = writeln!(dump, "\nstatic void dump(long long t) {{");
    for slot in &layout.outputs {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        // Ein Enum wird mit seinem Variantennamen ausgegeben, nicht mit
        // der Diskriminante: Der Interpreter schreibt den Namen (9.3), und
        // ein Vergleich von `CLOSED` gegen `0` waere ein Unterschied in
        // der Schreibweise, nicht in der Semantik.
        if let Some(varianten) = enum_variants(p, &slot.name) {
            let _ = writeln!(dump, "    switch (*({ct} *)(latch + {})) {{", slot.offset);
            for (d, name) in varianten {
                let _ = writeln!(dump, "    case {d}: printf(\"t=%lld out {} {name}\\n\", t); break;", slot.name);
            }
            let _ = writeln!(dump, "    default: printf(\"t=%lld out {} ?\\n\", t);", slot.name);
            let _ = writeln!(dump, "    }}");
            continue;
        }
        // Vorzeichenlose Werte werden vorzeichenlos ausgegeben: Der
        // Interpreter schreibt die Zahl, die der Typ meint, und `%lld`
        // auf einem `u32` gaebe 2286445522 als -2008521774.
        let (fmt, cast) = match (&slot.ty, slot.signed) {
            (takt_llvm::ty::LlvmType::F32 | takt_llvm::ty::LlvmType::F64, _) => ("%.17g", "(double)"),
            (_, true) => ("%lld", "(long long)"),
            (_, false) => ("%llu", "(unsigned long long)"),
        };
        let _ = writeln!(
            dump,
            "    printf(\"t=%lld out {} {fmt}\\n\", t, {cast}(*({ct} *)(latch + {})));",
            slot.name, slot.offset
        );
    }
    let _ = writeln!(dump, "}}");
    // Die Vorwaertsdeklaration muss vor `main` stehen.
    let at = s.find("int main(void)").unwrap_or(0);
    s.insert_str(at, "static void dump(long long t);\n\n");
    s.push_str(&dump);

    Harness { source: s, layout }
}

/// Der Wert eines Parameters als C-Literal (8.4).
///
/// Nur Literale: Ein berechneter Default braeuchte den Interpreter, und
/// der Rahmen soll nichts auswerten, was der Vergleich pruefen soll.
fn param_literal(p: &Program, index: usize) -> Option<String> {
    let param = p.params.get(index)?;
    match &param.default.kind {
        takt_mir::expr::ExprKind::Int(n) => Some(n.to_string()),
        takt_mir::expr::ExprKind::Duration(d) => Some(d.to_string()),
        takt_mir::expr::ExprKind::Bool(b) => Some(u8::from(*b).to_string()),
        takt_mir::expr::ExprKind::Float(f) => Some(format!("{f:?}")),
        _ => None,
    }
}

/// Der Versatz des Qualitaetsbytes eines Inputs im Abbild (3.5).
fn quality_offset(p: &Program, name: &str) -> Option<u64> {
    let index = p.channels.iter().position(|c| c.name == name)?;
    let id = takt_mir::ChannelId(index as u32);
    let base = takt_llvm::image::offset_of(id, p)?;
    let takt_llvm::ty::LlvmType::Struct(fields) = takt_llvm::image::entry_type(id, p)? else { return None };
    // Der Wert steht zuerst, die Qualitaet dahinter (`image`).
    Some(base + fields.first()?.size())
}

/// Die Varianten eines Enum-Outputs mit ihren Diskriminanten (3.7).
fn enum_variants(p: &Program, name: &str) -> Option<Vec<(i64, String)>> {
    let c = p.channels.iter().find(|c| c.name == name)?;
    let takt_mir::types::Type::Enum(e) = p.types.list.get(c.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    Some(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect())
}
