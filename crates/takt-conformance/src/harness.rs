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
    build_with(p, machine, ticks, &[])
}

/// Baut den Rahmen fuer *alle* Maschinen des Programms (8.3, 12.1).
///
/// Ein Plant-Modell ist eine gewoehnliche Maschine (8.3): Es schreibt
/// `sim`-Outputs, die an derselben Adresse haengen wie die `hw`-Inputs
/// des Programms. Ein Rahmen, der nur eine Maschine tickt, sieht davon
/// nichts — die Eingaenge bleiben `Bad`, und der Vergleich prueft einen
/// Lauf, den es nicht gibt.
///
/// Darum bekommt der Rahmen die Tickschleife der Runtime statt einer
/// Sonderbehandlung fuer Modelle: alle Maschinen in Deklarations-
/// reihenfolge, jede nach ihrer Periode (7.2), und am Ende des Ticks die
/// Bindung `sim` -> `hw` (8.3). Das Modell braucht dann nichts, was ein
/// gewoehnliches Programm nicht auch braucht.
pub fn build_all(p: &Program, ticks: u64, inputs: &[(u64, String)]) -> Harness {
    build_inner(p, None, ticks, inputs)
}

/// Baut den Rahmen mit Eingaben (12.5).
///
/// `inputs` ist der Stimulus, den auch der Interpreter sieht: je Eintrag
/// ein Tick und ein Command. Damit prueft die Abnahme die *Reaktion* auf
/// Lieferungen und nicht nur den Anfangszustand.
pub fn build_with(p: &Program, machine: &str, ticks: u64, inputs: &[(u64, String)]) -> Harness {
    build_inner(p, Some(machine), ticks, inputs)
}

/// Der gemeinsame Rumpf: `Some(name)` tickt eine Maschine, `None` alle.
fn build_inner(p: &Program, machine: Option<&str>, ticks: u64, inputs: &[(u64, String)]) -> Harness {
    let layout = crate::layout::of(p);
    // Die Maschinen, die der Rahmen fuehrt, in Deklarationsreihenfolge —
    // dieselbe, die der Interpreter nimmt (9.4: ohne `follows` ist sie
    // semantisch irrelevant, aber der Trace soll gleich aussehen).
    let gefuehrt: Vec<&takt_mir::machine::Machine> = match machine {
        Some(name) => p.machines.iter().filter(|m| m.name == name).collect(),
        None => p.machines.iter().filter(|m| m.kind != takt_mir::machine::MachineKind::Template).collect(),
    };
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
    // 3.3: `now` ist die Dauer seit dem Start des Laufs — die Tickzahl
    // mal T0, wie im Interpreter. Die Runtime fuehrt sie, weil alle
    // Maschinen dieselbe Uhr lesen (12.1).
    let _ = writeln!(s, "long long takt_now(void) {{ return g_tick * {}LL; }}", p.config.tick);
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
    for m in &gefuehrt {
        let _ = writeln!(s, "void {}_init(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_step(void *st, void *in, void *par, void *out);", m.name);
    }
    let _ = writeln!(s);

    // Je Maschine ein eigener Zustand: Sie teilen das Abbild und den
    // Latch, nicht ihren Zustand. Der Struct ist gross genug bemessen;
    // seine genaue Groesse kennt nur der Codegen, und sie zu
    // ueberschaetzen kostet im Test nichts.
    for m in &gefuehrt {
        let _ = writeln!(s, "static char state_{}[4096];", m.name);
    }
    let _ = writeln!(s, "static char image[{}];", layout.image.max(1));
    let _ = writeln!(s, "static char params[{}];", layout.params.max(1));
    let _ = writeln!(s, "static char latch[{}];\n", layout.latch.max(1));

    let _ = writeln!(s, "int main(void) {{");
    for m in &gefuehrt {
        let _ = writeln!(s, "    memset(state_{0}, 0, sizeof state_{0});", m.name);
    }
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

    // 9.4: Der Lauf beginnt mit den Outputs auf `safe` (5.3) — vor
    // jedem Init, wie in `Sim::new`. Danach erst bindet die Simulation,
    // damit ein `enter:`-Block schon den sicheren Wert sieht.
    safe_outputs(&mut s, p, &layout);
    sim_bindings(&mut s, p, "    ");
    for m in &gefuehrt {
        let _ = writeln!(s, "    {0}_init(state_{0}, image, params, latch);", m.name);
    }
    sim_bindings(&mut s, p, "    ");
    let _ = writeln!(s, "    dump(0);");
    let _ = writeln!(s, "    for (g_tick = 1; g_tick <= {ticks}; g_tick++) {{");
    // 8.5: Ein Command gilt einen Tick. Der Rahmen setzt es vor dem
    // Schritt und loescht es danach — wie die Runtime (12.1).
    for (name, slot) in layout.commands.iter().map(|c| (c.name.clone(), c.offset)) {
        let ticks_of: Vec<String> = inputs.iter().filter(|(_, n)| *n == name).map(|(t, _)| t.to_string()).collect();
        if ticks_of.is_empty() {
            continue;
        }
        let bedingung = ticks_of.iter().map(|t| format!("g_tick == {t}")).collect::<Vec<_>>().join(" || ");
        let _ = writeln!(s, "        image[{slot}] = ({bedingung}) ? 1 : 0; /* {name} */");
    }
    // 7.2: Eine Maschine laeuft in jedem `period`-ten Tick. Ohne die
    // Bedingung liefe ein `every 50 ms`-Modell bei 10 ms Tick fuenfmal
    // zu oft, und sein Wert stuende im Trace an der falschen Stelle.
    for m in &gefuehrt {
        let bedingung = match (m.period.max(1), m.phase) {
            (1, _) => String::new(),
            (per, 0) => format!("if (g_tick % {per} == 0) "),
            (per, ph) => format!("if (g_tick % {per} == {ph}) "),
        };
        let _ = writeln!(s, "        {bedingung}{0}_step(state_{0}, image, params, latch);", m.name);
    }
    // 8.3: Was ein Modell in diesem Tick auf einen `sim`-Output gestellt
    // hat, liest das Programm im naechsten — Unit-Delay wie bei Ψ.
    sim_bindings(&mut s, p, "        ");
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
/// Setzt jeden Output auf seinen `safe`-Wert (5.3, 9.4).
///
/// Der Interpreter tut das in `Sim::new`, bevor eine Maschine laeuft:
/// Ein Lauf beginnt im sicheren Zustand, nicht bei null. Der Unterschied
/// faellt auf, sobald ein Wert nicht zufaellig 0 ist — ein Modell mit
/// `safe = 22 degC` lieferte sonst 0, und jeder Vergleich daran haengt.
///
/// Nur Literale: Ein berechneter `safe`-Wert braeuchte den Interpreter,
/// und der Rahmen soll ohne ihn auskommen (dieselbe Grenze wie bei den
/// Parametern).
fn safe_outputs(s: &mut String, p: &Program, layout: &crate::layout::Layout) {
    for slot in &layout.outputs {
        let Some(i) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let Some(safe) = &p.channels[i].attrs.safe else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(text) = literal(safe) else { continue };
        let _ = writeln!(s, "    *({ct} *)(latch + {}) = {text}; /* {} auf safe (5.3) */", slot.offset, slot.name);
    }
}

/// Speist die `sim`-Outputs in die `hw`-Inputs derselben Adresse (8.3).
///
/// Ein Plant-Modell schreibt `output p_sim : float[bar] @ sim("daq1/ai0")`,
/// das Programm liest `input p : float[bar] @ hw("daq1/ai0")`. Die
/// Adresse ist die Naht, und der Interpreter zieht sie in
/// `Image::apply_sim_bindings`. Hier steht dieselbe Naht in C.
///
/// Der Wert geht aus dem Latch in den Wertteil des Abbild-Eintrags, und
/// die Qualitaet wird `Good` (0): Der Eingang hat eine Quelle, also ist
/// er nicht mehr `Bad` (3.5). Ohne das bliebe er `Bad`, und jeder
/// Lesezugriff faultete.
fn sim_bindings(s: &mut String, p: &Program, einzug: &str) {
    use takt_mir::program::{Binding, Direction};
    let adresse = |b: &Binding| match b {
        Binding::Hw(a) | Binding::Sim(a) => Some(a.clone()),
        Binding::None => None,
    };
    for (i, out) in p.channels.iter().enumerate() {
        if out.dir != Direction::Output {
            continue;
        }
        let Binding::Sim(_) = &out.binding else { continue };
        let Some(addr) = adresse(&out.binding) else { continue };
        // Der Eingang an derselben Adresse; ein Strom wird gesendet, nicht
        // gestellt (8.8) und bleibt hier aussen vor.
        let Some((j, inp)) = p
            .channels
            .iter()
            .enumerate()
            .find(|(_, c)| c.dir == Direction::Input && matches!(&c.binding, Binding::Hw(a) if *a == addr))
        else {
            continue;
        };
        if matches!(p.types.list.get(inp.ty.index()), Some(takt_mir::types::Type::Stream(_))) {
            continue;
        }
        let from = takt_mir::ChannelId(i as u32);
        let to = takt_mir::ChannelId(j as u32);
        let (Some(src), Some(dst)) = (takt_llvm::image::latch_offset(from, p), takt_llvm::image::offset_of(to, p))
        else {
            continue;
        };
        let Some(size) = takt_llvm::ty::lower(inp.ty, p).map(|t| t.size()) else { continue };
        let _ = writeln!(s, "{einzug}memcpy(image + {dst}, latch + {src}, {size}); /* {} -> {} */", out.name, inp.name);
        // Qualitaet `Good` (3.5): Der Eingang hat jetzt eine Quelle.
        if let Some(q) = quality_offset(p, &inp.name) {
            let _ = writeln!(s, "{einzug}image[{q}] = 0;");
        }
    }
}

fn param_literal(p: &Program, index: usize) -> Option<String> {
    literal(&p.params.get(index)?.default)
}

/// Ein Literal als C-Text; alles andere braeuchte den Interpreter.
fn literal(e: &takt_mir::expr::Expr) -> Option<String> {
    match &e.kind {
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
