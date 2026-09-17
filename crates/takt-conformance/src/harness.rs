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

use takt_mir::MachineId;
use takt_mir::program::Program;

use crate::layout::{Layout, c_type};
use crate::stimulus::Stimulus;

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
pub fn build_all(p: &Program, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, None, None, ticks, inputs, &[])
}

/// Baut den Rahmen mit einer Journal-Nutzlast (5.9).
///
/// `payload` ist die kanonische Form, die der Interpreter ueber
/// `RunOptions::nvm` sieht; der Rahmen reicht sie nach `_init` an
/// `_persist_restore`. Am Ende schreibt er `persist <hex>` — dieselben
/// Bytes, die der Interpreter in seine Zeile schreibt (Satz 9.4.4).
pub fn build_restoring(p: &Program, machine: Option<&str>, ticks: u64, inputs: &[Stimulus], payload: &[u8]) -> Harness {
    build_inner(p, machine, None, ticks, inputs, payload)
}

/// Baut den Rahmen mit einem Szenario (13.6): dieselben Maschinen wie
/// `takt test` — die laufenden samt dem gewaehlten Szenario, in
/// Schrittordnung.
pub fn build_scenario(p: &Program, scenario: &str, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, None, Some(scenario), ticks, inputs, &[])
}

/// Baut den Rahmen mit Eingaben (12.5).
///
/// `inputs` ist der Stimulus, den auch der Interpreter sieht: je Eintrag
/// ein Tick und ein Command (8.5) oder ein Stromelement (8.6). Damit
/// prueft die Abnahme die *Reaktion* auf Lieferungen und nicht nur den
/// Anfangszustand.
pub fn build_with(p: &Program, machine: &str, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, Some(machine), None, ticks, inputs, &[])
}

/// Der gemeinsame Rumpf: `Some(name)` tickt eine Maschine, `None` alle —
/// mit `scenario` dazu das gewaehlte Szenario (13.6).
fn build_inner(
    p: &Program,
    machine: Option<&str>,
    scenario: Option<&str>,
    ticks: u64,
    inputs: &[Stimulus],
    payload: &[u8],
) -> Harness {
    let layout = crate::layout::of(p);
    // Die Maschinen, die der Rahmen fuehrt, in Schrittordnung — dieselbe,
    // die der Interpreter nimmt (7.2: topologisch nach `follows`, sonst
    // Prioritaet; 9.4.1: ohne Kanten semantisch irrelevant).
    let chosen = scenario.and_then(|name| p.machines.iter().position(|m| m.name == name)).map(|i| MachineId(i as u32));
    let driven: Vec<&takt_mir::machine::Machine> = match machine {
        Some(name) => p.machines.iter().filter(|m| m.name == name).collect(),
        None => takt_mir::analysis::schedule::order_with(p, chosen)
            .unwrap_or_else(|_| takt_mir::analysis::schedule::runnable_with(p, chosen))
            .into_iter()
            .map(|id| &p.machines[id.index()])
            .collect(),
    };
    let mut s = String::new();
    let _ = writeln!(s, "/* Testrahmen (13.8); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "#include <stdio.h>");
    let _ = writeln!(s, "#include <string.h>\n");
    let _ = writeln!(s, "static void takt_tx_commit(long long);");
    let _ = writeln!(s, "static void takt_int_commit(void);");
    let _ = writeln!(s, "static void takt_apply_scheduled(long long);");

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
    // 5.3: der Fault-Uebergang, mit Maschine und verlassenem Zustand.
    let _ = writeln!(s, "void takt_fault(int m, int from) {{ printf(\"t=%lld fault %d %d\\n\", g_tick, m, from); }}");
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
    let _ = writeln!(s, "void takt_abort(int m, int site) {{ printf(\"t=%lld abort %d %d\\n\", g_tick, m, site); }}");
    let _ = writeln!(s, "void takt_verdict(int m, int site, unsigned char pass) {{");
    let _ = writeln!(s, "    printf(\"t=%lld verdict %d %d %d\\n\", g_tick, m, site, pass ? 1 : 0);");
    let _ = writeln!(s, "}}");
    // 13.3: Ein Monitor meldet Index und Position; der Vergleich bildet
    // den Namen aus dem Programm.
    let _ = writeln!(s, "void takt_property(int i, long long at) {{");
    let _ = writeln!(s, "    printf(\"t=%lld property %d %lld\\n\", g_tick, i, at);");
    let _ = writeln!(s, "}}\n");

    // Die Stroeme (`takt-llvm/src/stream.rs`): die drei Aufrufe ueber
    // dem Stimulus, der vor dem Lauf feststeht (`streams`).
    crate::streams::emit(&mut s, p, inputs);

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
    if p.natives.iter().any(|n| n.name.starts_with("sha256") || n.name == "hmac_sha256") {
        s.push_str(SHA256_C);
    }
    if p.types.list.iter().any(|t| matches!(t, takt_mir::types::Type::Map { .. })) {
        s.push_str(MAP_C);
    }
    for m in &driven {
        let _ = writeln!(s, "void {}_init(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_step(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_publish(void *st, void *in);", m.name);
        let _ = writeln!(s, "void {}_init_vars(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_enter(void *st, void *in, void *par, void *out);", m.name);
        if !m.persist.is_empty() {
            let _ = writeln!(s, "int {}_persist_snapshot(void *st, void *out, int cap);", m.name);
            let _ = writeln!(s, "int {}_persist_restore(void *st, const void *in, int len);", m.name);
        }
    }
    // 13.3: Laufzeitmonitore laufen nur, wenn der Rahmen alle Maschinen
    // fuehrt — eine Eigenschaft liest jede.
    let monitors: Vec<(usize, &takt_mir::program::Property)> = match machine {
        None => p.properties.iter().enumerate().filter(|(_, prop)| prop.monitor).collect(),
        Some(_) => Vec::new(),
    };
    for (i, _) in &monitors {
        let _ = writeln!(s, "void takt_monitor_{i}(void *st, void *in, void *par, void *out, long long tick);");
    }
    let _ = writeln!(s);
    let persisting: Vec<&takt_mir::machine::Machine> =
        driven.iter().copied().filter(|m| !m.persist.is_empty()).collect();
    if !persisting.is_empty() {
        let bound = takt_mir::persist::max_payload(p).unwrap_or(0).max(1);
        let _ = writeln!(s, "static unsigned char persist_out[{bound}];");
        let bytes: Vec<String> = payload.iter().map(|b| b.to_string()).collect();
        let _ =
            writeln!(s, "static const unsigned char persist_in[{}] = {{{}}};", payload.len().max(1), bytes.join(","));
        let _ = writeln!(s, "static const int persist_in_len = {};", payload.len());
        let _ = writeln!(s);
    }

    // Je Maschine ein eigener Zustand: Sie teilen das Abbild und den
    // Latch, nicht ihren Zustand. Der Struct ist gross genug bemessen;
    // seine genaue Groesse kennt nur der Codegen, und sie zu
    // ueberschaetzen kostet im Test nichts.
    for m in &driven {
        // So gross wie der Zustands-Struct mit Ausrichtung (FB-177): eine
        // feste Zahl hielt, bis die Bindungen eines Stroms Kilobytes wogen.
        let bytes = takt_llvm::machine::state_struct(m, p).map_or(4096, |st| st.aligned_size());
        let _ = writeln!(s, "{}", crate::layout::c_buffer(&format!("state_{}", m.name), bytes.max(64)));
    }
    let _ = writeln!(s, "{}", crate::layout::c_buffer("image", layout.image));
    let _ = writeln!(s, "{}", crate::layout::c_buffer("params", layout.params));
    let _ = writeln!(s, "{}", crate::layout::c_buffer("latch", layout.latch));
    for (i, prop) in &monitors {
        let size = takt_llvm::monitor::state_size(prop, p).unwrap_or(1);
        let _ = writeln!(s, "{}", crate::layout::c_buffer(&format!("monitor_{i}"), size));
    }
    let _ = writeln!(s);
    // 4.5: Die Jobs des Rahmens, hinter den Puffern, weil sie das Abbild schreiben.
    jobs(&mut s, p);

    // 9.8: die geplanten Schreibvorgaenge. Sie gehoeren der Runtime —
    // 11.2 nennt sie „feste Arrays im Runtime-Anteil des Outputs" —,
    // und der Rahmen ist hier die Runtime. Hinter dem Latch, weil
    // `apply_scheduled` ihn schreibt.
    scheduled(&mut s, p, &layout);

    let _ = writeln!(s, "int main(void) {{");
    for m in &driven {
        let _ = writeln!(s, "    memset(state_{0}, 0, sizeof state_{0});", m.name);
    }
    let _ = writeln!(s, "    memset(image, 0, sizeof image);");
    let _ = writeln!(s, "    memset(params, 0, sizeof params);");
    let _ = writeln!(s, "    memset(latch, 0, sizeof latch);");
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(s, "    takt_jobs_init();");
    }
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
    // Wie `Sim::init`: erst die Variablen aller Maschinen, dann die
    // geladenen Werte (5.9), dann die Eintritte in Schrittordnung — und
    // nach jedem `fresh[m] = publish_m(v_m)`, damit ein Follower schon im
    // Tick 0 frisch liest (7.2, 9.4).
    for m in &driven {
        let _ = writeln!(s, "    {0}_init_vars(state_{0}, image, params, latch);", m.name);
    }
    for m in &persisting {
        let _ = writeln!(s, "    {0}_persist_restore(state_{0}, persist_in, persist_in_len);", m.name);
    }
    for m in &driven {
        let _ = writeln!(s, "    {0}_enter(state_{0}, image, params, latch);", m.name);
        let _ = writeln!(s, "    {0}_publish(state_{0}, image);", m.name);
    }
    psi_commit(&mut s, p, &driven, "    ");
    sim_bindings(&mut s, p, "    ");
    // 8.8: Auch im Tick 0 holt der Treiber ab, was `enter` gesendet hat.
    let _ = writeln!(s, "    takt_tx_commit(0);");
    let _ = writeln!(s, "    takt_int_commit();");
    let _ = writeln!(s, "    dump(0);");
    for (i, _) in &monitors {
        let _ = writeln!(s, "    takt_monitor_{i}(monitor_{i}, image, params, latch, 0);");
    }
    let _ = writeln!(s, "    for (g_tick = 1; g_tick <= {ticks}; g_tick++) {{");
    // 9.8: `apply_scheduled(k)` stellt zu Tick-Beginn, was faellig ist —
    // vor jedem Maschinenschritt, damit die Maschinen den Wert im selben
    // Tick lesen. Die Simulation wendet `T` im Tick `ceil(T / T0)` an.
    let _ = writeln!(s, "        takt_apply_scheduled(g_tick * {}LL);", p.config.tick);
    // 4.5: Faellige Jobs werden zu Tick-Beginn sichtbar, wie `poll_jobs` im Interpreter.
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(s, "        takt_jobs_poll();");
    }
    // 8.5: Ein Command gilt einen Tick. Der Rahmen setzt es vor dem
    // Schritt und loescht es danach — wie die Runtime (12.1).
    for (name, slot) in layout.commands.iter().map(|c| (c.name.clone(), c.offset)) {
        let ticks_of: Vec<String> = inputs
            .iter()
            .filter_map(|s| match s {
                Stimulus::Command { tick, name: n } if *n == name => Some(tick.to_string()),
                _ => None,
            })
            .collect();
        if ticks_of.is_empty() {
            continue;
        }
        let condition = ticks_of.iter().map(|t| format!("g_tick == {t}")).collect::<Vec<_>>().join(" || ");
        let _ = writeln!(s, "        image[{slot}] = ({condition}) ? 1 : 0; /* {name} */");
    }
    // 8.4: Ein Tunable gilt ab seiner Tick-Grenze; der Rahmen schreibt den
    // Parametervektor vor dem Schritt, wie `apply_stimulus` im Interpreter.
    for stim in inputs {
        let Stimulus::Tune { tick, name, text } = stim else { continue };
        let Some((i, slot)) = layout.parameters.iter().enumerate().find(|(_, s)| s.name == *name) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = tune_literal(p, i, text) else { continue };
        let _ = writeln!(
            s,
            "        if (g_tick == {tick}) *({ct} *)(params + {}) = {value}; /* tune {name} */",
            slot.offset
        );
    }
    // 7.2: Eine Maschine laeuft in jedem `period`-ten Tick. Ohne die
    // Bedingung liefe ein `every 50 ms`-Modell bei 10 ms Tick fuenfmal
    // zu oft, und sein Wert stuende im Trace an der falschen Stelle.
    for m in &driven {
        let condition = match (m.period.max(1), m.phase) {
            (1, _) => String::new(),
            (per, 0) => format!("if (g_tick % {per} == 0) "),
            (per, ph) => format!("if (g_tick % {per} == {ph}) "),
        };
        let _ = writeln!(
            s,
            "        {condition}{{ {0}_step(state_{0}, image, params, latch); {0}_publish(state_{0}, image); }}",
            m.name
        );
    }
    psi_commit(&mut s, p, &driven, "        ");
    // 8.3: Was ein Modell in diesem Tick auf einen `sim`-Output gestellt
    // hat, liest das Programm im naechsten — Unit-Delay wie bei Ψ.
    sim_bindings(&mut s, p, "        ");
    // 8.8: Gesendet wird beim Commit des Ticks. Der Treiber holt seine
    // Rate ab, bevor der Latch ausgeschrieben wird — sonst stuende die
    // Zeile einen Tick spaeter als beim Interpreter.
    let _ = writeln!(s, "        takt_tx_commit(g_tick);");
    let _ = writeln!(s, "        takt_int_commit();");
    let _ = writeln!(s, "        dump(g_tick);");
    // 13.3: nach dem Commit, wie `observe_properties` im Interpreter.
    for (i, _) in &monitors {
        let _ = writeln!(s, "        takt_monitor_{i}(monitor_{i}, image, params, latch, g_tick);");
    }
    // 12.7: `reboot` beendet den Lauf, danach stehen die Outputs auf `safe`.
    if let Some(RebootSlot { slot, ct, commands }) = reboot_slot(p, &layout) {
        let _ = writeln!(s, "        switch (*({ct} *)(latch + {})) {{", slot.offset);
        for (d, name) in commands {
            let _ = writeln!(s, "        case {d}: printf(\"t=%lld end {name}\\n\", g_tick); goto ende;");
        }
        let _ = writeln!(s, "        default: break;");
        let _ = writeln!(s, "        }}");
    }
    // 12.7: `sys/jump` beendet den Lauf ebenso, sobald der Slot nicht null ist.
    if let Some((slot, ct)) = jump_slot(p, &layout) {
        let _ = writeln!(s, "        if (*({ct} *)(latch + {})) {{", slot.offset);
        let _ = writeln!(s, "            printf(\"t=%lld end boot_jump\\n\", g_tick); goto ende;");
        let _ = writeln!(s, "        }}");
    }
    let _ = writeln!(s, "    }}");
    if reboot_slot(p, &layout).is_some() || jump_slot(p, &layout).is_some() {
        let _ = writeln!(s, "ende:");
        safe_outputs(&mut s, p, &layout);
        let _ = writeln!(s, "    dump(g_tick);");
    }
    if !persisting.is_empty() {
        let _ = writeln!(s, "    printf(\"t=%lld persist \", g_tick);");
        for m in &persisting {
            let _ = writeln!(s, "    {{");
            let _ = writeln!(
                s,
                "        int n = {0}_persist_snapshot(state_{0}, persist_out, sizeof persist_out);",
                m.name
            );
            let _ = writeln!(s, "        for (int i = 0; i < n; i++) printf(\"%02x\", persist_out[i]);");
            let _ = writeln!(s, "    }}");
        }
        let _ = writeln!(s, "    printf(\"\\n\");");
    }
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");

    // `dump` steht hinter `main`, damit die Deklaration oben genuegt.
    let mut dump = String::new();
    let _ = writeln!(dump, "\nstatic void dump(long long t) {{");
    for slot in &layout.outputs {
        // Ein Array als Liste, wie der Interpreter ihn schreibt (T2).
        if let takt_llvm::ty::LlvmType::Array(elem, n) = &slot.ty {
            let Some(ct) = c_type(elem, slot.signed) else { continue };
            let (fmt, cast) = number_format(elem, slot.signed);
            let _ = writeln!(dump, "    printf(\"t=%lld out {} [\", t);", slot.name);
            let _ = writeln!(
                dump,
                "    for (int k = 0; k < {n}; k++) printf(k ? \", {fmt}\" : \"{fmt}\", {cast}(({ct} *)(latch + {}))[k]);",
                slot.offset
            );
            let _ = writeln!(dump, "    printf(\"]\\n\");");
            continue;
        }
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
        let (fmt, cast) = number_format(&slot.ty, slot.signed);
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

/// `printf`-Format und Cast fuer einen Skalar: Fliesskomma mit 17
/// Stellen, damit der Vergleich das Bit trifft (Satz 9.4.4).
fn number_format(ty: &takt_llvm::ty::LlvmType, signed: bool) -> (&'static str, &'static str) {
    match (ty, signed) {
        (takt_llvm::ty::LlvmType::F32 | takt_llvm::ty::LlvmType::F64, _) => ("%.17g", "(double)"),
        (_, true) => ("%lld", "(long long)"),
        (_, false) => ("%llu", "(unsigned long long)"),
    }
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
        // Ein Array elementweise; sein `safe` ist ein Array-Literal (3.6).
        if let (takt_llvm::ty::LlvmType::Array(elem, _), takt_mir::expr::ExprKind::Array(items)) =
            (&slot.ty, &safe.kind)
        {
            let Some(ct) = c_type(elem, slot.signed) else { continue };
            for (k, item) in items.iter().enumerate() {
                let Some(text) = literal(p, item) else { continue };
                let _ = writeln!(
                    s,
                    "    (({ct} *)(latch + {}))[{k}] = {text}; /* {}[{k}] auf safe */",
                    slot.offset, slot.name
                );
            }
            continue;
        }
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(text) = literal(p, safe) else { continue };
        let _ = writeln!(s, "    *({ct} *)(latch + {}) = {text}; /* {} auf safe (5.3) */", slot.offset, slot.name);
    }
}

/// Die geplanten Schreibvorgaenge (9.8).
///
/// **Der Rahmen ist hier die Runtime.** 11.2 legt `sched` in den
/// Runtime-Anteil des Outputs, und 9.8 gibt die Regeln vor: sortiert
/// nach `T`, hoechstens `K_o` Eintraege je Output, gleiche `T`
/// ueberschreiben einander, und `apply_scheduled(k)` stellt zu
/// Tick-Beginn, was faellig ist.
///
/// `takt_schedule` liefert `false`, wenn der Zeitpunkt nicht in der
/// Zukunft liegt (`TimingFault`) oder die Warteschlange voll ist
/// (`ScheduleOverflow`) — beides Faults der Maschine, die der erzeugte
/// Code an seinem Fault-Pfad behandelt.
fn scheduled(s: &mut String, p: &Program, layout: &Layout) {
    // K_o aus 7.5; `takt size` rechnet mit derselben Zahl.
    let _ = writeln!(s, "#define TAKT_K_O 4");
    let _ = writeln!(s, "struct takt_sched {{ long long t; long long v; }};");
    let n = p.channels.len().max(1);
    let _ = writeln!(s, "static struct takt_sched g_sched[{n}][TAKT_K_O];");
    let _ = writeln!(s, "static int g_sched_n[{n}];");
    let _ = writeln!(s, "_Bool takt_schedule(int o, long long t, long long v) {{");
    let _ = writeln!(s, "    if (o < 0 || o >= {n}) return 0;");
    // 9.8: `T <= now` ist ein `TimingFault`; in der Simulation ist
    // `guard` null.
    let _ = writeln!(s, "    if (t <= g_tick * {}LL) return 0;", p.config.tick);
    // Gleiche `T`: die spaetere Anweisung gewinnt (9.8).
    let _ = writeln!(s, "    for (int i = 0; i < g_sched_n[o]; i++)");
    let _ = writeln!(s, "        if (g_sched[o][i].t == t) {{ g_sched[o][i].v = v; return 1; }}");
    let _ = writeln!(s, "    if (g_sched_n[o] >= TAKT_K_O) return 0;");
    let _ = writeln!(s, "    g_sched[o][g_sched_n[o]].t = t;");
    let _ = writeln!(s, "    g_sched[o][g_sched_n[o]].v = v;");
    let _ = writeln!(s, "    g_sched_n[o]++;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_cancel(int o) {{ if (o >= 0 && o < {n}) g_sched_n[o] = 0; }}");

    // `apply_scheduled(k)`: Was faellig ist, geht in den Latch. Sind
    // mehrere faellig, gewinnt der spaeteste Zeitpunkt (9.8).
    let _ = writeln!(s, "static void takt_apply_scheduled(long long now) {{");
    let _ = writeln!(s, "    for (int o = 0; o < {n}; o++) {{");
    let _ = writeln!(s, "        long long best_t = -1; long long best_v = 0; int hit = 0;");
    let _ = writeln!(s, "        int k = 0;");
    let _ = writeln!(s, "        for (int i = 0; i < g_sched_n[o]; i++) {{");
    let _ = writeln!(s, "            if (g_sched[o][i].t <= now) {{");
    let _ = writeln!(s, "                if (!hit || g_sched[o][i].t > best_t) {{");
    let _ = writeln!(s, "                    best_t = g_sched[o][i].t; best_v = g_sched[o][i].v; hit = 1;");
    let _ = writeln!(s, "                }}");
    let _ = writeln!(s, "            }} else {{");
    let _ = writeln!(s, "                g_sched[o][k++] = g_sched[o][i];");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        g_sched_n[o] = k;");
    let _ = writeln!(s, "        if (!hit) continue;");
    let _ = writeln!(s, "        switch (o) {{");
    for slot in &layout.outputs {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(id) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        // Der Wert kam als `i64` an; im Latch steht er in seinem Typ.
        // Ein `double` traegt dieselben Bits, eine Ganzzahl wird
        // verengt — beides genau die Umkehrung von `at` im Codegen.
        let back = if slot.ty.is_float() {
            format!("*({ct} *)(latch + {}) = ({ct})(*(double *)&best_v);", slot.offset)
        } else {
            format!("*({ct} *)(latch + {}) = ({ct})best_v;", slot.offset)
        };
        let _ = writeln!(s, "        case {id}: {back} break; /* {} */", slot.name);
    }
    let _ = writeln!(s, "        default: break;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
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
fn sim_bindings(s: &mut String, p: &Program, indent: &str) {
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
        let _ = writeln!(s, "{indent}memcpy(image + {dst}, latch + {src}, {size}); /* {} -> {} */", out.name, inp.name);
        // Qualitaet `Good` (3.5): Der Eingang hat jetzt eine Quelle — sofern
        // der Wert in der deklarierten Range liegt; sonst `Bad` (12.6). Der
        // Interpreter prueft am Rand auch `max_slew` und `debounce`; das
        // bleibt hier aussen vor (LIMITS).
        let Some(q) = quality_offset(p, &inp.name) else { continue };
        match range_check(p, inp.ty) {
            Some((ct, lo, hi)) => {
                let _ = writeln!(
                    s,
                    "{indent}{{ {ct} v = *({ct} *)(image + {dst}); image[{q}] = (v < {lo} || v > {hi}) ? 3 : 0; }}"
                );
            }
            None => {
                let _ = writeln!(s, "{indent}image[{q}] = 0;");
            }
        }
    }
}

/// C-Typ und Grenzen eines Skalars mit deklarierter Range (3.4).
fn range_check(p: &Program, ty: takt_mir::TypeId) -> Option<(&'static str, String, String)> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::types::{Const, FloatWidth, Type};
    let literal = |c: &Const| match c {
        Const::Int(i) | Const::Duration(i) => format!("{i}LL"),
        Const::Float(f) => format!("{f:?}"),
        Const::Bool(b) => u8::from(*b).to_string(),
    };
    let (ct, r) = match p.types.list.get(ty.index())? {
        Type::Int { width, range: Some(r), .. } => {
            (crate::layout::c_type(&LlvmType::Int(width.bits()), width.signed())?, r)
        }
        Type::Float { width, range: Some(r), .. } => {
            let t = if *width == FloatWidth::F32 { LlvmType::F32 } else { LlvmType::F64 };
            (crate::layout::c_type(&t, true)?, r)
        }
        _ => return None,
    };
    Some((ct, literal(&r.lo), literal(&r.hi)))
}

/// Ψ_{k+1} wird Ψ_k (9.4): die zweite Bank in die erste kopieren, dann
/// `fresh` und die Signale loeschen — ein Signal ist einen Tick sichtbar
/// (5.8), und ohne `fresh` liest ein Follower wieder Ψ_k (7.2).
pub(crate) fn psi_commit(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str) {
    use takt_llvm::psi::{Field, bank_size, field_offset, region_offset};
    let Some(first) = region_offset(takt_mir::MachineId(0), false, p) else { return };
    let Some(next) = region_offset(takt_mir::MachineId(0), true, p) else { return };
    let _ = writeln!(
        s,
        "{indent}for (unsigned i = 0; i < {}; i++) image[{first} + i] = image[{next} + i]; /* Psi */",
        bank_size(p)
    );
    for m in driven {
        let Some(id) = p.machines.iter().position(|x| x.name == m.name) else { continue };
        let id = takt_mir::MachineId(id as u32);
        let Some(base) = region_offset(id, true, p) else { continue };
        let _ = writeln!(s, "{indent}image[{base}] = 0; /* fresh {} */", m.name);
        for i in 0..m.signals.len() {
            if let Some(off) = field_offset(id, Field::Signal(takt_mir::SignalId(i as u32)), p) {
                let _ = writeln!(s, "{indent}image[{}] = 0; /* Signal {}.{} */", base + off, m.name, m.signals[i].name);
            }
        }
    }
}

pub(crate) fn param_literal(p: &Program, index: usize) -> Option<String> {
    literal(p, &p.params.get(index)?.default)
}

/// Der Wert einer `tune`-Zeile als C-Text (8.4): ausserhalb der Range
/// verworfen, wie im Interpreter.
fn tune_literal(p: &Program, index: usize, text: &str) -> Option<String> {
    use takt_interp::Value;
    let param = p.params.get(index)?;
    let v = takt_interp::trace::parse_value(text, param.ty, p).ok()?;
    let range = match p.types.get(param.ty) {
        takt_mir::types::Type::Int { range, .. }
        | takt_mir::types::Type::Float { range, .. }
        | takt_mir::types::Type::Duration { range } => *range,
        _ => None,
    };
    if range.is_some_and(|r| !takt_interp::in_range(&v, &r)) {
        return None;
    }
    Some(match v {
        Value::Int(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::Bool(b) => u8::from(b).to_string(),
        Value::Duration(ns) => ns.to_string(),
        Value::F64(f) => format!("{f:?}"),
        Value::F32(f) => format!("{f:?}f"),
        Value::Enum { variant, .. } => {
            let takt_mir::types::Type::Enum(e) = p.types.get(param.ty) else { return None };
            p.enums.get(e.index())?.variants.get(variant as usize)?.discriminant.to_string()
        }
        _ => return None,
    })
}

/// Ein Literal als C-Text; alles andere braeuchte den Interpreter.
fn literal(p: &Program, e: &takt_mir::expr::Expr) -> Option<String> {
    match &e.kind {
        takt_mir::expr::ExprKind::Int(n) => Some(n.to_string()),
        takt_mir::expr::ExprKind::Duration(d) => Some(d.to_string()),
        takt_mir::expr::ExprKind::Bool(b) => Some(u8::from(*b).to_string()),
        takt_mir::expr::ExprKind::Float(f) => Some(format!("{f:?}")),
        // Eine feldlose Variante ist ihre Diskriminante — die kann
        // explizit gesetzt sein und von der Nummer abweichen.
        takt_mir::expr::ExprKind::Variant { enum_id, variant, fields } if fields.is_empty() => {
            Some(p.enums.get(enum_id.index())?.variants.get(*variant as usize)?.discriminant.to_string())
        }
        _ => None,
    }
}

/// Der Versatz des Qualitaetsbytes eines Inputs im Abbild (3.5).
pub(crate) fn quality_offset(p: &Program, name: &str) -> Option<u64> {
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

/// Wo `sys/jump` im Latch steht (12.7).
fn jump_slot<'a>(p: &Program, layout: &'a Layout) -> Option<(&'a crate::layout::Slot, &'static str)> {
    let slot = layout.outputs.iter().find(|s| {
        p.channels.iter().any(|c| {
            c.name == s.name && matches!(&c.binding, takt_mir::program::Binding::Hw(a) if a.text() == "sys/jump")
        })
    })?;
    Some((slot, c_type(&slot.ty, slot.signed)?))
}

/// Wo `sys/reboot` im Latch steht und welche Kommandos es kennt.
struct RebootSlot<'a> {
    slot: &'a crate::layout::Slot,
    ct: &'static str,
    commands: Vec<(i64, &'static str)>,
}

/// Der Latch-Platz von `sys/reboot` mit seinen Kommandos (12.7).
///
/// `RESTART` und `DEEP_SLEEP` beenden den Lauf; die Namen stehen klein im
/// Trace, wie der Interpreter sie schreibt.
fn reboot_slot<'a>(p: &Program, layout: &'a Layout) -> Option<RebootSlot<'a>> {
    let slot = layout.outputs.iter().find(|s| {
        p.channels.iter().any(|c| {
            c.name == s.name && matches!(&c.binding, takt_mir::program::Binding::Hw(a) if a.text() == "sys/reboot")
        })
    })?;
    let ct = c_type(&slot.ty, slot.signed)?;
    // 12.7: `RebootCmd` ist vordefiniert; ein fremdes Enum an derselben
    // Adresse ist kein Kommando. Derselbe Test wie im Interpreter.
    let takt_mir::types::Type::Enum(e) =
        p.types.list.get(p.channels.iter().find(|c| c.name == slot.name)?.ty.index())?
    else {
        return None;
    };
    if p.enums.get(e.index())?.name != "RebootCmd" {
        return None;
    }
    let variants = enum_variants(p, &slot.name)?;
    let commands: Vec<(i64, &'static str)> = variants
        .iter()
        .filter_map(|(d, name)| match name.as_str() {
            "RESTART" => Some((*d, "restart")),
            "DEEP_SLEEP" => Some((*d, "deep_sleep")),
            _ => None,
        })
        .collect();
    (!commands.is_empty()).then_some(RebootSlot { slot, ct, commands })
}

/// SHA-256 (FIPS 180-4) und HMAC ueber die kanonische Form von `Sha256Ctx`
/// (5.9): `h` als acht u32, die Laenge und die gefuellten Bytes, `total`
/// als u64, alles little-endian. Dieselbe Rechnung wie `takt-native`,
/// damit der Vergleich sie mitprueft.
const SHA256_C: &str = r#"
typedef struct { unsigned int h[8]; unsigned char buf[64]; unsigned int len; unsigned long long total; } takt_sha;
static const unsigned int takt_sha_k[64] = {
    0x428a2f98u, 0x71374491u, 0xb5c0fbcfu, 0xe9b5dba5u, 0x3956c25bu, 0x59f111f1u, 0x923f82a4u, 0xab1c5ed5u,
    0xd807aa98u, 0x12835b01u, 0x243185beu, 0x550c7dc3u, 0x72be5d74u, 0x80deb1feu, 0x9bdc06a7u, 0xc19bf174u,
    0xe49b69c1u, 0xefbe4786u, 0x0fc19dc6u, 0x240ca1ccu, 0x2de92c6fu, 0x4a7484aau, 0x5cb0a9dcu, 0x76f988dau,
    0x983e5152u, 0xa831c66du, 0xb00327c8u, 0xbf597fc7u, 0xc6e00bf3u, 0xd5a79147u, 0x06ca6351u, 0x14292967u,
    0x27b70a85u, 0x2e1b2138u, 0x4d2c6dfcu, 0x53380d13u, 0x650a7354u, 0x766a0abbu, 0x81c2c92eu, 0x92722c85u,
    0xa2bfe8a1u, 0xa81a664bu, 0xc24b8b70u, 0xc76c51a3u, 0xd192e819u, 0xd6990624u, 0xf40e3585u, 0x106aa070u,
    0x19a4c116u, 0x1e376c08u, 0x2748774cu, 0x34b0bcb5u, 0x391c0cb3u, 0x4ed8aa4au, 0x5b9cca4fu, 0x682e6ff3u,
    0x748f82eeu, 0x78a5636fu, 0x84c87814u, 0x8cc70208u, 0x90befffau, 0xa4506cebu, 0xbef9a3f7u, 0xc67178f2u };
static unsigned int takt_rotr(unsigned int x, int n) { return (x >> n) | (x << (32 - n)); }
static void takt_sha_compress(unsigned int *h, const unsigned char *b) {
    unsigned int w[64], a, bb, c, d, e, f, g, hh; int i;
    for (i = 0; i < 16; i++)
        w[i] = ((unsigned int)b[4 * i] << 24) | ((unsigned int)b[4 * i + 1] << 16) | ((unsigned int)b[4 * i + 2] << 8) | (unsigned int)b[4 * i + 3];
    for (i = 16; i < 64; i++) {
        unsigned int s0 = takt_rotr(w[i - 15], 7) ^ takt_rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
        unsigned int s1 = takt_rotr(w[i - 2], 17) ^ takt_rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    a = h[0]; bb = h[1]; c = h[2]; d = h[3]; e = h[4]; f = h[5]; g = h[6]; hh = h[7];
    for (i = 0; i < 64; i++) {
        unsigned int s1 = takt_rotr(e, 6) ^ takt_rotr(e, 11) ^ takt_rotr(e, 25);
        unsigned int ch = (e & f) ^ (~e & g);
        unsigned int t1 = hh + s1 + ch + takt_sha_k[i] + w[i];
        unsigned int s0 = takt_rotr(a, 2) ^ takt_rotr(a, 13) ^ takt_rotr(a, 22);
        unsigned int maj = (a & bb) ^ (a & c) ^ (bb & c);
        unsigned int t2 = s0 + maj;
        hh = g; g = f; f = e; e = d + t1; d = c; c = bb; bb = a; a = t1 + t2;
    }
    h[0] += a; h[1] += bb; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
}
static void takt_sha_init(takt_sha *c) {
    static const unsigned int h0[8] = { 0x6a09e667u, 0xbb67ae85u, 0x3c6ef372u, 0xa54ff53au, 0x510e527fu, 0x9b05688cu, 0x1f83d9abu, 0x5be0cd19u };
    int i;
    for (i = 0; i < 8; i++) c->h[i] = h0[i];
    c->len = 0; c->total = 0;
}
static void takt_sha_update(takt_sha *c, const unsigned char *d, int n) {
    int i;
    for (i = 0; i < n; i++) {
        c->buf[c->len++] = d[i];
        if (c->len == 64) { takt_sha_compress(c->h, c->buf); c->len = 0; }
    }
    c->total += (unsigned long long)n;
}
static void takt_sha_final(takt_sha *c, unsigned char *out) {
    unsigned long long bits = c->total * 8u;
    unsigned char pad = 0x80, zero = 0, be[8];
    int i;
    takt_sha_update(c, &pad, 1);
    while (c->len != 56) takt_sha_update(c, &zero, 1);
    for (i = 0; i < 8; i++) be[i] = (unsigned char)(bits >> (56 - 8 * i));
    takt_sha_update(c, be, 8);
    for (i = 0; i < 8; i++) {
        out[4 * i] = (unsigned char)(c->h[i] >> 24); out[4 * i + 1] = (unsigned char)(c->h[i] >> 16);
        out[4 * i + 2] = (unsigned char)(c->h[i] >> 8); out[4 * i + 3] = (unsigned char)c->h[i];
    }
}
static unsigned int takt_le32(const unsigned char *b) {
    return (unsigned int)b[0] | ((unsigned int)b[1] << 8) | ((unsigned int)b[2] << 16) | ((unsigned int)b[3] << 24);
}
static void takt_put32(unsigned char *b, unsigned int v) {
    b[0] = (unsigned char)v; b[1] = (unsigned char)(v >> 8); b[2] = (unsigned char)(v >> 16); b[3] = (unsigned char)(v >> 24);
}
/* Die kanonische Form lesen; ein fremder Puffer ergibt den leeren Zustand. */
static void takt_sha_from(takt_sha *c, const unsigned char *b, int n) {
    unsigned int len; int i;
    takt_sha_init(c);
    if (n < 44) return;
    len = takt_le32(b + 32);
    if (len > 63u || n != (int)(44u + len)) return;
    for (i = 0; i < 8; i++) c->h[i] = takt_le32(b + 4 * i);
    for (i = 0; i < (int)len; i++) c->buf[i] = b[36 + i];
    c->len = len;
    c->total = (unsigned long long)takt_le32(b + 36 + len) | ((unsigned long long)takt_le32(b + 40 + len) << 32);
}
static void takt_sha_to(const takt_sha *c, unsigned char *out) {
    int i;
    for (i = 0; i < 8; i++) takt_put32(out + 4 * i, c->h[i]);
    takt_put32(out + 32, c->len);
    for (i = 0; i < (int)c->len; i++) out[36 + i] = c->buf[i];
    takt_put32(out + 36 + c->len, (unsigned int)c->total);
    takt_put32(out + 40 + c->len, (unsigned int)(c->total >> 32));
}
static void takt_put_digest(unsigned char *out, const unsigned char *d) {
    int i;
    takt_put32(out, 32u);
    for (i = 0; i < 32; i++) out[4 + i] = d[i];
}
void takt_native_sha256(const unsigned char *b, int n, unsigned char *out) {
    takt_sha c; unsigned char d[32];
    takt_sha_init(&c); takt_sha_update(&c, b, n); takt_sha_final(&c, d); takt_put_digest(out, d);
}
void takt_native_hmac_sha256(const unsigned char *key, int kn, const unsigned char *msg, int mn, unsigned char *out) {
    unsigned char k[64], pad[64], d[32]; takt_sha c; int i;
    for (i = 0; i < 64; i++) k[i] = 0;
    if (kn > 64) { takt_sha_init(&c); takt_sha_update(&c, key, kn); takt_sha_final(&c, k); }
    else { for (i = 0; i < kn; i++) k[i] = key[i]; }
    for (i = 0; i < 64; i++) pad[i] = k[i] ^ 0x36;
    takt_sha_init(&c); takt_sha_update(&c, pad, 64); takt_sha_update(&c, msg, mn); takt_sha_final(&c, d);
    for (i = 0; i < 64; i++) pad[i] = k[i] ^ 0x5c;
    takt_sha_init(&c); takt_sha_update(&c, pad, 64); takt_sha_update(&c, d, 32); takt_sha_final(&c, d);
    takt_put_digest(out, d);
}
void takt_native_sha256_init(unsigned char *out) { takt_sha c; takt_sha_init(&c); takt_sha_to(&c, out); }
void takt_native_sha256_update(const unsigned char *cb, int cn, const unsigned char *d, int n, unsigned char *out) {
    takt_sha c; takt_sha_from(&c, cb, cn); takt_sha_update(&c, d, n); takt_sha_to(&c, out);
}
void takt_native_sha256_final(const unsigned char *cb, int cn, unsigned char *out) {
    takt_sha c; unsigned char d[32];
    takt_sha_from(&c, cb, cn); takt_sha_final(&c, d); takt_put_digest(out, d);
}
"#;

/// 4.5: Jobs im Rahmen. Das Ergebnis der reinen Funktion steht beim Start
/// fest; der Slot im Abbild wird `done`, sobald `duration` in Ticks
/// vergangen ist — wie das Modell des Interpreters. Ein Argument kommt als
/// Folge kanonischer Bloecke (`u32` Laenge, Bytes), ein `bytes<N>` darin
/// als Laenge und Daten (5.9).
fn jobs(s: &mut String, p: &Program) {
    use takt_mir::fns::NativeKind;
    let slots: Vec<(usize, usize, takt_mir::NativeId)> = p
        .machines
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| m.layout.job_slots.iter().enumerate().map(move |(j, s)| (mi, j, s.native)))
        .collect();
    if slots.is_empty() || !p.natives.iter().any(|n| n.kind == NativeKind::Job) {
        return;
    }
    let t0 = p.config.tick.max(1);
    let out_max = slots
        .iter()
        .filter_map(|(_, _, n)| takt_llvm::image::job_entry_size(*n, p))
        .map(|e| e.saturating_sub(8))
        .max()
        .unwrap_or(8)
        .max(8);
    let at: Vec<String> = slots
        .iter()
        .map(|(mi, j, _)| takt_llvm::image::job_offset(takt_mir::MachineId(*mi as u32), *j, p).unwrap_or(0).to_string())
        .collect();
    let ticks: Vec<String> = slots
        .iter()
        .map(|(_, _, n)| {
            let d = p.natives[n.index()].duration.unwrap_or(0).max(0);
            (d.saturating_add(t0 - 1) / t0).to_string()
        })
        .collect();
    let mut base = Vec::new();
    let mut acc = 0usize;
    for m in &p.machines {
        base.push(acc.to_string());
        acc += m.layout.job_slots.len();
    }
    let _ = writeln!(
        s,
        "typedef struct {{ int active; long long due; int out_len; unsigned char out[{out_max}]; }} takt_job;"
    );
    let _ = writeln!(s, "static takt_job g_jobs[{}];", slots.len());
    let _ = writeln!(s, "static const long long takt_job_at[{}] = {{ {} }};", slots.len(), at.join(", "));
    let _ = writeln!(s, "static const int takt_job_ticks[{}] = {{ {} }};", slots.len(), ticks.join(", "));
    let _ = writeln!(s, "static const int takt_job_base[{}] = {{ {} }};", base.len().max(1), base.join(", "));
    s.push_str(JOBS_C);
    let _ = writeln!(s, "void takt_job_begin(int m, int slot, int native, const unsigned char *args, int len) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; takt_job *j = &g_jobs[i];");
    let _ = writeln!(s, "    const unsigned char *a[8]; int n[8]; int k = 0, p = 0;");
    let _ = writeln!(s, "    for (k = 0; k < 8; k++) {{ a[k] = args; n[k] = 0; }}");
    let _ = writeln!(
        s,
        "    for (k = 0; k < 8 && p + 4 <= len; k++) {{ n[k] = (int)takt_job_le32(args + p); a[k] = args + p + 4; p += 4 + n[k]; }}"
    );
    let _ = writeln!(s, "    j->out_len = 0;");
    let _ = writeln!(s, "    switch (native) {{");
    for (idx, n) in p.natives.iter().enumerate() {
        if n.kind != NativeKind::Job {
            continue;
        }
        if let Some(case) = job_case(n) {
            let _ = writeln!(s, "    case {idx}: {{ {case} }} break;");
        }
    }
    let _ = writeln!(s, "    default: break;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    j->active = 1; j->due = g_tick + takt_job_ticks[i];");
    let _ = writeln!(s, "    takt_job_image(i, 0, 0, 2); /* Err(PENDING) */");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_job_cancel(int m, int slot) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot;");
    let _ = writeln!(
        s,
        "    if (g_jobs[i].active) {{ g_jobs[i].active = 0; takt_job_image(i, 1, 0, 0); /* Err(CANCELLED) */ }}"
    );
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_jobs_poll(void) {{");
    let _ = writeln!(s, "    int i, b;");
    let _ = writeln!(s, "    for (i = 0; i < {}; i++) {{", slots.len());
    let _ = writeln!(s, "        if (!g_jobs[i].active || g_jobs[i].due > g_tick) continue;");
    let _ = writeln!(s, "        g_jobs[i].active = 0; takt_job_image(i, 1, 1, 0);");
    let _ = writeln!(
        s,
        "        for (b = 0; b < g_jobs[i].out_len; b++) image[takt_job_at[i] + 8 + b] = g_jobs[i].out[b];"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_jobs_init(void) {{ int i; for (i = 0; i < {}; i++) takt_job_image(i, 0, 0, 2); }}",
        slots.len()
    );
    let _ = writeln!(s);
}

/// Der Aufruf einer `native job` im Rahmen: Argumente nach Art, das
/// Ergebnis in kanonischer Form nach `j->out`.
fn job_case(n: &takt_mir::fns::Native) -> Option<String> {
    use takt_native::Kind;
    let sig = takt_native::Native::by_name(&n.name)?.signature();
    let args: Vec<String> = sig
        .params
        .iter()
        .enumerate()
        .map(|(k, kind)| match kind {
            Kind::Bytes => format!("a[{k}] + 4, (int)takt_job_le32(a[{k}])"),
            _ => format!("a[{k}], n[{k}]"),
        })
        .collect();
    let call = format!("takt_native_{}({})", n.name, args.join(", "));
    Some(match sig.ret {
        Kind::U32 => format!("unsigned int v = {call}; takt_job_put32(j->out, v); j->out_len = 4;"),
        Kind::U16 => format!(
            "unsigned short v = {call}; j->out[0] = (unsigned char)v; j->out[1] = (unsigned char)(v >> 8); j->out_len = 2;"
        ),
        Kind::U8 | Kind::Bool => format!("j->out[0] = {call}; j->out_len = 1;"),
        Kind::Digest => format!("takt_native_{}({}, j->out); j->out_len = 36;", n.name, args.join(", ")),
        Kind::Sha256Ctx => format!(
            "takt_native_{}({}, j->out); j->out_len = 44 + (int)takt_job_le32(j->out + 32);",
            n.name,
            args.join(", ")
        ),
        Kind::Bytes => return None,
    })
}

const JOBS_C: &str = r#"
static unsigned int takt_job_le32(const unsigned char *b) {
    return (unsigned int)b[0] | ((unsigned int)b[1] << 8) | ((unsigned int)b[2] << 16) | ((unsigned int)b[3] << 24);
}
static void takt_job_put32(unsigned char *b, unsigned int v) {
    b[0] = (unsigned char)v; b[1] = (unsigned char)(v >> 8); b[2] = (unsigned char)(v >> 16); b[3] = (unsigned char)(v >> 24);
}
/* done, ok, err (Diskriminante von JobErr: CANCELLED 0, FAILED 1, PENDING 2) */
static void takt_job_image(int i, int done, int ok, int err) {
    unsigned char *e = image + takt_job_at[i];
    e[0] = (unsigned char)done; e[1] = (unsigned char)ok; takt_job_put32(e + 4, (unsigned int)err);
}
"#;

/// `map<K, V, N>` im Rahmen (3.9): dieselbe Sondierung wie `takt_native::map`
/// — FNV-1a ueber den auf K Byte aufgefuellten Schluessel, lineare
/// Sondierung, Entfernen per Rueckwaertsverschiebung. Der Slot ist
/// `belegt, Schluessel, Wert`.
const MAP_C: &str = r#"
static unsigned int takt_map_hash(const unsigned char *key, int klen) {
    unsigned int h = 0x811c9dc5u; int i;
    for (i = 0; i < klen; i++) { h ^= key[i]; h *= 0x01000193u; }
    return h;
}
static int takt_map_find(const unsigned char *s, int cap, int klen, int vlen, const unsigned char *key, unsigned int h) {
    int size = 1 + klen + vlen, n, i;
    if (cap <= 0) return -1;
    i = (int)(h % (unsigned int)cap);
    for (n = 0; n < cap; n++) {
        const unsigned char *slot = s + i * size;
        if (!slot[0]) return -1;
        if (takt_map_hash(slot + 1, klen) == h && memcmp(slot + 1, key, (size_t)klen) == 0) return i;
        i = (i + 1) % cap;
    }
    return -1;
}
static int takt_map_free(const unsigned char *s, int cap, int klen, int vlen, unsigned int h) {
    int size = 1 + klen + vlen, n, i;
    if (cap <= 0) return -1;
    i = (int)(h % (unsigned int)cap);
    for (n = 0; n < cap; n++) { if (!s[i * size]) return i; i = (i + 1) % cap; }
    return -1;
}
int takt_native_map_len(const unsigned char *s, int cap, int klen, int vlen) {
    int size = 1 + klen + vlen, n = 0, i;
    for (i = 0; i < cap; i++) if (s[i * size]) n++;
    return n;
}
unsigned char takt_native_map_insert(unsigned char *s, int cap, int klen, int vlen, const unsigned char *key, const unsigned char *val) {
    int size = 1 + klen + vlen;
    unsigned int h = takt_map_hash(key, klen);
    int i = takt_map_find(s, cap, klen, vlen, key, h);
    if (i < 0) i = takt_map_free(s, cap, klen, vlen, h);
    if (i < 0) return 0;
    s[i * size] = 1;
    memcpy(s + i * size + 1, key, (size_t)klen);
    memcpy(s + i * size + 1 + klen, val, (size_t)vlen);
    return 1;
}
unsigned char takt_native_map_get(const unsigned char *s, int cap, int klen, int vlen, const unsigned char *key, unsigned char *out) {
    int size = 1 + klen + vlen;
    int i = takt_map_find(s, cap, klen, vlen, key, takt_map_hash(key, klen));
    if (i < 0) return 0;
    memcpy(out, s + i * size + 1 + klen, (size_t)vlen);
    return 1;
}
unsigned char takt_native_map_remove(unsigned char *s, int cap, int klen, int vlen, const unsigned char *key) {
    int size = 1 + klen + vlen, hole, j, n;
    int i = takt_map_find(s, cap, klen, vlen, key, takt_map_hash(key, klen));
    if (i < 0) return 0;
    s[i * size] = 0;
    hole = i; j = i;
    for (n = 0; n < cap; n++) {
        int home, stays;
        j = (j + 1) % cap;
        if (!s[j * size]) break;
        home = (int)(takt_map_hash(s + j * size + 1, klen) % (unsigned int)cap);
        stays = hole <= j ? (home > hole && home <= j) : (home > hole || home <= j);
        if (stays) continue;
        memcpy(s + hole * size, s + j * size, (size_t)size);
        s[j * size] = 0;
        hole = j;
    }
    return 1;
}
"#;
