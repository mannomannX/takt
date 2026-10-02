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
    build_inner(p, None, None, ticks, inputs, &[], false)
}

/// Baut den Rahmen fuer alle Maschinen mit virtuellem Schlaf (9.9), wie
/// die Runtime auf einer MCU: Sind alle Maschinen `idle`, rueckt der
/// Rahmen bis zur fruehesten `after`-Frist vor, ohne die Ticks dazwischen
/// auszufuehren. Satz 9.9.1 verlangt denselben Trace wie ohne Schlaf —
/// und der Pfad `_advance` des Codegens laeuft so auch auf dem Wirt
/// (FB-268, FB-273).
pub fn build_sleeping(p: &Program, ticks: u64) -> Harness {
    build_inner(p, None, None, ticks, &[], &[], true)
}

/// Baut den Rahmen mit einer Journal-Nutzlast (5.9).
///
/// `payload` ist die kanonische Form, die der Interpreter ueber
/// `RunOptions::nvm` sieht; der Rahmen reicht sie nach `_init` an
/// `_persist_restore`. Am Ende schreibt er `persist <hex>` — dieselben
/// Bytes, die der Interpreter in seine Zeile schreibt (Satz 9.4.4).
pub fn build_restoring(p: &Program, machine: Option<&str>, ticks: u64, inputs: &[Stimulus], payload: &[u8]) -> Harness {
    build_inner(p, machine, None, ticks, inputs, payload, false)
}

/// Baut den Rahmen mit einem Szenario (13.6): dieselben Maschinen wie
/// `takt test` — die laufenden samt dem gewaehlten Szenario, in
/// Schrittordnung.
pub fn build_scenario(p: &Program, scenario: &str, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, None, Some(scenario), ticks, inputs, &[], false)
}

/// Baut den Rahmen mit Eingaben (12.5).
///
/// `inputs` ist der Stimulus, den auch der Interpreter sieht: je Eintrag
/// ein Tick und ein Command (8.5) oder ein Stromelement (8.6). Damit
/// prueft die Abnahme die *Reaktion* auf Lieferungen und nicht nur den
/// Anfangszustand.
pub fn build_with(p: &Program, machine: &str, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, Some(machine), None, ticks, inputs, &[], false)
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
    sleep: bool,
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
    let _ = writeln!(s, "#include <string.h>");
    // 4.2: FTZ und DAZ aus, ausdruecklich und nicht als Annahme ueber den
    // Zustand, den der Prozess erbt.
    let _ = writeln!(s, "#if defined(__x86_64__) || defined(_M_X64)");
    let _ = writeln!(s, "#include <xmmintrin.h>");
    let _ = writeln!(s, "#define TAKT_IEEE_MODE() _mm_setcsr(_mm_getcsr() & ~0x8040u)");
    let _ = writeln!(s, "#else");
    let _ = writeln!(s, "#define TAKT_IEEE_MODE() ((void)0)");
    let _ = writeln!(s, "#endif\n");
    let _ = writeln!(s, "static void takt_tx_commit(long long);");
    let _ = writeln!(s, "static void takt_int_commit(void);");

    // Die Runtime-Aufrufe (`takt-llvm/src/abi.rs`). Sie schreiben in den
    // Trace, damit der Vergleich sie sieht.
    let _ = writeln!(s, "static long long g_tick = 0;");
    let _ = writeln!(s, "{DURATION_C}");
    scope_flags(&mut s, p);
    // Das Fault-Flag der reinen Funktionen (4.1, `abi::Abi::FAULT_FLAG`):
    // die Art des Faults, den der Aufrufer liest und loescht.
    let _ = writeln!(s, "unsigned int takt_fn_fault = 0;");
    fault_names(&mut s, p);
    alert_table(&mut s, p);
    let _ = writeln!(s, "void takt_alert(int m, int slot, unsigned char on, unsigned char invalid) {{");
    let _ = writeln!(s, "    if (!takt_alert_edge(m, slot, on)) return;");
    let _ = writeln!(
        s,
        "    printf(\"t=%lld alert %s %s%s\\n\", g_tick, takt_machine_name(m), on ? \"on\" : \"off\", invalid ? \" invalid\" : \"\");"
    );
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_log(int m, int site) {{ printf(\"t=%lld log %d %d\\n\", g_tick, m, site); }}");
    // 5.3: der Fault-Uebergang mit Maschine und Art, wie der Interpreter
    // ihn schreibt; der verlassene Zustand steht nicht in dessen Zeile.
    let _ = writeln!(s, "void takt_fault(int m, int from, int code) {{");
    let _ = writeln!(s, "    (void)from;");
    let _ = writeln!(s, "    printf(\"t=%lld fault %s %s\\n\", g_tick, takt_machine_name(m), takt_fault_name(code));");
    let _ = writeln!(s, "}}");
    // 3.3: `now` ist die Dauer seit dem Start des Laufs — die Tickzahl
    // mal T0, wie im Interpreter. Die Runtime fuehrt sie, weil alle
    // Maschinen dieselbe Uhr lesen (12.1).
    let _ = writeln!(s, "long long takt_now(void) {{ return g_tick * {}LL; }}", p.config.tick);
    let _ = writeln!(s, "void takt_measure(int m, int site, double v, unsigned char invalid) {{");
    let _ = writeln!(s, "    if (invalid) printf(\"t=%lld measure %d %d <invalid>\\n\", g_tick, m, site);");
    let _ = writeln!(s, "    else printf(\"t=%lld measure %d %d %.17g\\n\", g_tick, m, site, v);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_verify(int m, int site, unsigned char ok) {{");
    let _ = writeln!(s, "    printf(\"t=%lld verify %d %d %d\\n\", g_tick, m, site, ok ? 1 : 0);");
    let _ = writeln!(s, "}}");
    // 5.4: `abort` merkt den Fault fuer alle Maschinen vor; die
    // Abort-Phase stellt ihn nach den Schritten zu (`abort_phase`).
    raised(&mut s, p);
    let _ = writeln!(s, "void takt_abort(int m, int site) {{");
    let _ = writeln!(s, "    printf(\"t=%lld abort %d %d\\n\", g_tick, m, site);");
    let _ = writeln!(s, "    memset(g_raised, 1, sizeof g_raised);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_verdict(int m, int site, unsigned char pass) {{");
    let _ = writeln!(s, "    printf(\"t=%lld verdict %d %d %d\\n\", g_tick, m, site, pass ? 1 : 0);");
    let _ = writeln!(s, "}}");
    // 13.3: Ein Monitor meldet Index und Position; der Vergleich bildet
    // den Namen aus dem Programm.
    let _ = writeln!(s, "void takt_property(int i, long long at) {{");
    let _ = writeln!(s, "    printf(\"t=%lld property %d %lld\\n\", g_tick, i, at);");
    let _ = writeln!(s, "}}\n");

    // Die Stroeme (`takt-llvm/src/stream.rs`): jeder ueber seinen Ring;
    // die Eingabestroeme speist der Treiberrand.
    crate::streams::emit(&mut s, p, crate::streams::Trace::Stdio);

    natives(&mut s, p);
    machine_declarations(&mut s, p, &driven);
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
    // 7.5: In der Simulation sind `guard` und `jitter` null.
    scheduled(&mut s, p, &layout, None);
    jitter(&mut s, p, None);
    // 12.10: Registerports lesen den Latch des Modells und schreiben in
    // die Ringe der Stroeme, also hinter beidem.
    crate::ports::emit(&mut s, p);
    // 12.6: Der Treiberrand schreibt ins Abbild und in die Ringe; der
    // Stimulus liefert, was auf einem Board die Treiber liefern.
    let mut feed = String::new();
    let most = crate::edge::stimulus(&mut feed, p, &layout, inputs);
    crate::edge::emit(&mut s, p, &layout, &driven, most, crate::streams::Trace::Stdio);
    s.push_str(&feed);

    let _ = writeln!(s, "int main(void) {{");
    let _ = writeln!(s, "    TAKT_IEEE_MODE();");
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
    crate::ports::sample(&mut s, p, "    ");
    // Die Lieferungen des Ticks 0 gehen vor jedem Init durch den Rand, wie
    // `Run::new` den Stimulus vor `Sim::init` einspeist.
    let _ = writeln!(s, "    takt_edge_stimulus(0);");
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
    enter_machines(&mut s, p, &layout, &driven, "    ");
    // 8.8: Auch im Tick 0 holt der Treiber ab, was `enter` gesendet hat.
    commit_sequence(&mut s, p, &driven, "    ", "0");
    crate::ports::sample(&mut s, p, "    ");
    let _ = writeln!(s, "    dump(0);");
    for (i, _) in &monitors {
        let _ = writeln!(s, "    takt_monitor_{i}(monitor_{i}, image, params, latch, 0);");
    }
    // 12.7: Auch der Anfangszustand kann das Kommando setzen.
    platform_end(&mut s, p, &layout, "    ");
    let _ = writeln!(s, "    for (g_tick = 1; g_tick <= {ticks}; g_tick++) {{");
    aging(&mut s, p, &layout, "        ");
    // 12.1: `sample_inputs()` und `validate_and_bound()` nach dem Altern,
    // wie `Run::tick`.
    let _ = writeln!(s, "        takt_edge_stimulus(g_tick);");
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
    // 5.4: Operator-Abort und Runtime-Faults von aussen werden vorgemerkt,
    // wie `apply_stimulus` im Interpreter.
    pended(&mut s, p, inputs, "        ");
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
    steps(&mut s, p, &layout, &driven, "        ", "g_tick");
    abort_phase(&mut s, p, &driven, "        ", "g_tick");
    idle_drops(&mut s, p, &driven, "        ");
    commit_sequence(&mut s, p, &driven, "        ", "g_tick");
    crate::ports::sample(&mut s, p, "        ");
    let _ = writeln!(s, "        dump(g_tick);");
    // 13.3: nach dem Commit, wie `observe_properties` im Interpreter.
    for (i, _) in &monitors {
        let _ = writeln!(s, "        takt_monitor_{i}(monitor_{i}, image, params, latch, g_tick);");
    }
    platform_end(&mut s, p, &layout, "        ");
    if sleep {
        virtual_sleep(&mut s, p, &driven, ticks);
    }
    let _ = writeln!(s, "    }}");
    if next_run_slot(p, &layout).is_some() {
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
    // T2: Eine Dauer steht in ihrer groessten ganzzahligen Einheit.
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
        if let Some(text) = payload_enum_dump(p, slot) {
            dump.push_str(&text);
            continue;
        }
        if let Some(text) = record_dump(p, slot) {
            dump.push_str(&text);
            continue;
        }
        if is_duration(p, &slot.name) {
            let _ = writeln!(
                dump,
                "    {{ long long v = *(long long *)(latch + {}); \
                 printf(\"t=%lld out {} %lld %s\\n\", t, takt_dur_value(v), takt_dur_unit(v)); }}",
                slot.offset, slot.name
            );
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
    let _ = writeln!(dump, "    takt_stream_report(t);");
    let _ = writeln!(dump, "}}");
    // Die Vorwaertsdeklaration muss vor `main` stehen.
    let at = s.find("int main(void)").unwrap_or(0);
    s.insert_str(at, "static void dump(long long t);\n\n");
    s.push_str(&dump);

    Harness { source: s, layout }
}

/// Eine Dauer in der groessten ganzzahligen Einheit, wie `takt_mir::dump::duration`
/// sie schreibt (T2); beide Rahmen nehmen dieselben Funktionen.
pub(crate) const DURATION_C: &str = "\
static const long long takt_dur_factor[7] = { 86400000000000LL, 3600000000000LL, 60000000000LL, 1000000000LL, \
1000000LL, 1000LL, 1LL };
static const char *const takt_dur_name[7] = { \"d\", \"h\", \"min\", \"s\", \"ms\", \"us\", \"ns\" };
static inline int takt_dur_index(long long ns) {
    int i = 0;
    if (ns == 0) return 6;
    while (i < 6 && ns % takt_dur_factor[i] != 0) i++;
    return i;
}
static inline long long takt_dur_value(long long ns) { return ns / takt_dur_factor[takt_dur_index(ns)]; }
static inline const char *takt_dur_unit(long long ns) { return takt_dur_name[takt_dur_index(ns)]; }";

/// Ist der Ausgang eine Dauer (3.3)?
fn is_duration(p: &Program, name: &str) -> bool {
    p.channels
        .iter()
        .find(|c| c.name == name)
        .is_some_and(|c| matches!(p.types.get(c.ty), takt_mir::types::Type::Duration { .. }))
}

/// Das Ende eines Laufs (12.7): ein Wert ausser `NONE` auf `sys/next_run`,
/// nach dem Commit des Ticks.
fn platform_end(s: &mut String, p: &Program, layout: &Layout, indent: &str) {
    let Some(NextRunSlot { slot, ct, ends }) = next_run_slot(p, layout) else { return };
    let _ = writeln!(s, "{indent}switch (*({ct} *)(latch + {})) {{", slot.offset);
    for (d, next) in ends {
        let word = next.word();
        let _ = writeln!(s, "{indent}case {d}: printf(\"t=%lld end {word}\\n\", g_tick); goto ende;");
    }
    let _ = writeln!(s, "{indent}default: break;");
    let _ = writeln!(s, "{indent}}}");
}

/// Ein Enum mit Feldern: `NAME(f1, f2)` wie `value_text` (9.3), die Felder
/// aus ihren 8-Byte-Faechern hinter der Diskriminante (11.2).
fn payload_enum_dump(p: &Program, slot: &crate::layout::Slot) -> Option<String> {
    use takt_mir::types::{FloatWidth, Type};
    let c = p.channels.iter().find(|c| c.name == slot.name)?;
    let Type::Enum(e) = p.types.get(c.ty) else { return None };
    let def = p.enums.get(e.index())?;
    if def.variants.iter().all(|v| v.fields.is_empty()) {
        return None;
    }
    let mut s = String::new();
    let _ = writeln!(
        s,
        "    {{ int d = *(int *)(latch + {}); long long *f = (long long *)(latch + {}); (void)f;",
        slot.offset,
        slot.offset + 8
    );
    let _ = writeln!(s, "    switch (d) {{");
    for v in &def.variants {
        let mut fmt = String::new();
        let mut args = String::new();
        for (k, field) in v.fields.iter().enumerate() {
            let (ff, fa) = match p.types.get(field.ty) {
                Type::Bool => ("%s".to_string(), format!("f[{k}] ? \"true\" : \"false\"")),
                Type::Float { width: FloatWidth::F32, .. } => {
                    ("%.17g".to_string(), format!("(double)*(float *)&f[{k}]"))
                }
                Type::Float { .. } => ("%.17g".to_string(), format!("*(double *)&f[{k}]")),
                Type::Duration { .. } => {
                    ("%lld %s".to_string(), format!("takt_dur_value(f[{k}]), takt_dur_unit(f[{k}])"))
                }
                Type::Int { width, .. } if width.signed() => ("%lld".to_string(), format!("(long long)f[{k}]")),
                Type::Int { .. } => ("%llu".to_string(), format!("(unsigned long long)f[{k}]")),
                Type::Enum(inner) => {
                    let names: String = p.enums[inner.index()]
                        .variants
                        .iter()
                        .map(|w| format!("f[{k}] == {} ? \"{}\" : ", w.discriminant, w.name))
                        .collect();
                    ("%s".to_string(), format!("({names}\"?\")"))
                }
                _ => return None,
            };
            if k > 0 {
                fmt.push_str(", ");
            }
            fmt.push_str(&ff);
            args.push_str(", ");
            args.push_str(&fa);
        }
        let text = if v.fields.is_empty() { v.name.clone() } else { format!("{}({fmt})", v.name) };
        let _ =
            writeln!(s, "    case {}: printf(\"t=%lld out {} {text}\\n\", t{args}); break;", v.discriminant, slot.name);
    }
    let _ = writeln!(s, "    default: printf(\"t=%lld out {} ?\\n\", t);", slot.name);
    let _ = writeln!(s, "    }} }}");
    Some(s)
}

/// Ein Record: `Name(f1, f2)` wie `value_text` (9.3), die Felder an ihren
/// Versaetzen im Latch.
fn record_dump(p: &Program, slot: &crate::layout::Slot) -> Option<String> {
    let c = p.channels.iter().find(|c| c.name == slot.name)?;
    if !matches!(p.types.get(c.ty), takt_mir::types::Type::Record(_)) {
        return None;
    }
    let (fmt, args) = field_text(p, c.ty, &slot.ty, slot.offset)?;
    Some(format!("    printf(\"t=%lld out {} {fmt}\\n\", t{args});\n", slot.name))
}

/// `printf`-Format und Argumente eines Werts an `at` im Latch: Skalare,
/// Enums ohne Felder und Records daraus. Eine Dauer schreibt der
/// Interpreter mit Einheit; sie und alles Uebrige bleiben aussen vor.
fn field_text(p: &Program, ty: takt_mir::TypeId, llvm: &takt_llvm::ty::LlvmType, at: u64) -> Option<(String, String)> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::types::Type;
    match (p.types.get(ty), llvm) {
        (Type::Record(r), LlvmType::Struct(fields)) => {
            let def = p.records.get(r.index())?;
            let (mut fmt, mut args) = (format!("{}(", def.name), String::new());
            for (i, f) in def.fields.iter().enumerate() {
                let (ff, fa) = field_text(p, f.ty, fields.get(i)?, at + llvm.field_offset(i))?;
                fmt.push_str(if i > 0 { ", " } else { "" });
                fmt.push_str(&ff);
                args.push_str(&fa);
            }
            fmt.push(')');
            Some((fmt, args))
        }
        (Type::Bool, LlvmType::Int(1)) => {
            Some(("%s".to_string(), format!(", *(unsigned char *)(latch + {at}) ? \"true\" : \"false\"")))
        }
        (Type::Enum(e), LlvmType::Int(_)) => {
            let def = p.enums.get(e.index())?;
            if def.variants.iter().any(|v| !v.fields.is_empty()) {
                return None;
            }
            let ct = c_type(llvm, true)?;
            let at_value = format!("*({ct} *)(latch + {at})");
            let names: String =
                def.variants.iter().map(|v| format!("{at_value} == {} ? \"{}\" : ", v.discriminant, v.name)).collect();
            Some(("%s".to_string(), format!(", ({names}\"?\")")))
        }
        (Type::Duration { .. }, LlvmType::Int(64)) => {
            let v = format!("*(long long *)(latch + {at})");
            Some(("%lld %s".to_string(), format!(", takt_dur_value({v}), takt_dur_unit({v})")))
        }
        (Type::Int { width, .. }, LlvmType::Int(_)) => {
            let ct = c_type(llvm, width.signed())?;
            let (fmt, cast) = number_format(llvm, width.signed());
            Some((fmt.to_string(), format!(", {cast}(*({ct} *)(latch + {at}))")))
        }
        (Type::Float { .. }, LlvmType::F32 | LlvmType::F64) => {
            let ct = c_type(llvm, true)?;
            let (fmt, cast) = number_format(llvm, true);
            Some((fmt.to_string(), format!(", {cast}(*({ct} *)(latch + {at}))")))
        }
        _ => None,
    }
}

/// Die nativen Funktionen (4.5) liegen in der Runtime: `takt-native-abi`
/// liefert ihre C-Einstiege, fuer den Wirtsrahmen als statische
/// Bibliothek ([`native_library`]), fuer ein Board ueber
/// `takt-mcu-program` (FB-293). Der Rahmen deklariert nur, was er selbst
/// ruft — die Jobs; den erzeugten Code bindet der Linker direkt.
pub(crate) fn natives(s: &mut String, p: &Program) {
    for n in &p.natives {
        if let Some(f) = takt_native::Native::by_name(&n.name).filter(|_| n.from.is_none()) {
            let _ = writeln!(s, "{};", prototype(f));
        }
    }
}

/// Die C-Deklaration eines Einstiegs: ein `bytes<N>`, Record oder Feld
/// als Zeiger und Laenge, ein Ergebnis aus Bytes nach `out`. Ein `bytes<N>`
/// als Ergebnis gibt seine Laenge zurueck, `-1`, wenn es keines gibt
/// (`aes_gcm_decrypt` mit falschem Tag: `Err(FAILED)`, 4.5).
fn prototype(f: takt_native::Native) -> String {
    use takt_native::Kind;
    let sig = f.signature();
    let mut params: Vec<&str> = sig.params.iter().map(|_| "const unsigned char *, int").collect();
    let ret = match sig.ret {
        Kind::U32 => "unsigned int",
        Kind::U16 => "unsigned short",
        Kind::U8 => "unsigned char",
        Kind::Bool => "_Bool",
        Kind::Digest | Kind::Sha256Ctx | Kind::Fixed(_) | Kind::Floats256 => {
            params.push("unsigned char *");
            "void"
        }
        Kind::Bytes => {
            params.push("unsigned char *");
            "int"
        }
    };
    let params = if params.is_empty() { "void".to_string() } else { params.join(", ") };
    format!("{ret} takt_native_{}({params})", f.name())
}

/// Die statische Bibliothek mit den C-Einstiegen der Natives fuer den Wirt
/// (FB-293), einmal je Prozess gebaut: `takt-native-abi` mit Panic-Handler
/// und den Jobs aus `takt-crypto`, im eigenen Zielverzeichnis, damit der
/// Bau nicht auf die Sperre eines laufenden `cargo` wartet.
pub fn native_library() -> Result<std::path::PathBuf, String> {
    static LIB: std::sync::OnceLock<Result<std::path::PathBuf, String>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let out = std::process::Command::new("cargo")
            .args(["rustc", "--release", "--lib", "--crate-type", "staticlib", "--features", "host,ecdsa,rsa,aes-gcm"])
            .arg("--message-format=json-render-diagnostics")
            .arg("--manifest-path")
            .arg(root.join("crates/takt-native-abi/Cargo.toml"))
            .arg("--target-dir")
            .arg(crate::target_dir().join("native-abi"))
            .output()
            .map_err(|e| format!("cargo: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|l| l.contains("\"reason\":\"compiler-artifact\""))
            .filter_map(|l| {
                let rest = &l[l.find("\"filenames\":[\"")? + "\"filenames\":[\"".len()..];
                Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
            })
            .find(|f| f.ends_with(".lib") || f.ends_with(".a"))
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "cargo meldete keine statische Bibliothek".to_string())
    })
    .clone()
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
pub(crate) fn safe_outputs(s: &mut String, p: &Program, layout: &crate::layout::Layout) {
    for slot in &layout.outputs {
        let Some(i) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let Some(safe) = &p.channels[i].attrs.safe else { continue };
        if let Some(text) = safe_payload(p, slot, safe) {
            s.push_str(&text);
            continue;
        }
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

/// Der `safe`-Wert eines Enums mit Nutzlast (11.2): die Diskriminante als
/// `int` vorn, die Felder in ihren 8-Byte-Faechern, ungenutzte Faecher null
/// — `==` vergleicht auch sie.
fn safe_payload(p: &Program, slot: &crate::layout::Slot, safe: &takt_mir::expr::Expr) -> Option<String> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::expr::ExprKind;
    let LlvmType::Struct(_) = &slot.ty else { return None };
    let ExprKind::Variant { enum_id, variant, fields } = &safe.kind else { return None };
    let def = p.enums.get(enum_id.index())?;
    let v = def.variants.get(*variant as usize)?;
    let mut s = format!("    memset(latch + {}, 0, {});\n", slot.offset, slot.size);
    let _ =
        writeln!(s, "    *(int *)(latch + {}) = {}; /* {} auf safe (5.3) */", slot.offset, v.discriminant, slot.name);
    for (k, field) in fields.iter().enumerate() {
        let at = slot.offset + 8 + 8 * k as u64;
        let ct = match takt_llvm::ty::lower(v.fields.get(k)?.ty, p)? {
            LlvmType::F32 => "float",
            LlvmType::F64 => "double",
            _ => "long long",
        };
        let _ = writeln!(s, "    *({ct} *)(latch + {at}) = {};", literal(p, field)?);
    }
    Some(s)
}

/// Die geplanten Schreibvorgaenge (9.8), fuer beide Rahmen.
///
/// `takt_jitter` (7.5): der Jitter je Output aus `hw`, bei einem Output,
/// der nur zu Tickbeginn geschrieben wird, um den Tick mehr (`tick_granular`);
/// ohne Konfiguration null wie in der Simulation.
pub(crate) fn jitter(s: &mut String, p: &Program, hw: Option<&takt_mir::hardware::Hardware>) {
    let _ = writeln!(s, "long long takt_jitter(int o) {{");
    let _ = writeln!(s, "    switch (o) {{");
    for (i, c) in p.channels.iter().enumerate() {
        let (takt_mir::program::Binding::Hw(a), Some(hw)) = (&c.binding, hw) else { continue };
        let Some(entry) = hw.channel(&a.text()) else { continue };
        let granular = if entry.tick_granular == Some(true) { p.config.tick } else { 0 };
        let ns = entry.jitter_ns.unwrap_or(0).saturating_add(granular);
        if ns != 0 {
            let _ = writeln!(s, "    case {i}: return {ns}LL; /* {} */", c.name);
        }
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// **Der Rahmen ist hier die Runtime.** 11.2 legt `sched` in den
/// Runtime-Anteil des Outputs, und 9.8 gibt die Regeln vor: sortiert
/// nach `T`, hoechstens `K_o` Eintraege je Output, gleiche `T`
/// ueberschreiben einander. Eine Warteschlange je Output aus
/// [`queued_outputs`] — so viele, wie `takt size` rechnet (11.5); ein
/// Programm ohne `at`, `pulse` und `cancel` bekommt keine.
///
/// `takt_schedule` liefert `false`, wenn der Zeitpunkt nicht weiter als
/// `guard` in der Zukunft liegt (`TimingFault`) oder die Warteschlange
/// voll ist (`ScheduleOverflow`) — beides Faults der Maschine, die der
/// erzeugte Code an seinem Fault-Pfad behandelt. `guard` ist die gemessene
/// Treiberlatenz des Outputs aus `hw` (7.5, 8.10), ohne Konfiguration
/// null wie in der Simulation. `takt_apply_scheduled` ruft
/// [`commit_sequence`].
pub(crate) fn scheduled(s: &mut String, p: &Program, layout: &Layout, hw: Option<&takt_mir::hardware::Hardware>) {
    let queues = queued_outputs(p);
    if queues.is_empty() {
        return;
    }
    let n = queues.len();
    // K_o aus 7.5; `takt size` rechnet mit derselben Zahl.
    let _ = writeln!(s, "#define TAKT_K_O 4");
    let _ = writeln!(s, "struct takt_sched {{ long long t; long long v; }};");
    let _ = writeln!(s, "static struct takt_sched g_sched[{n}][TAKT_K_O];");
    let _ = writeln!(s, "static int g_sched_n[{n}];");
    let guards: Vec<String> = queues
        .iter()
        .map(|c| {
            let guard = match (&p.channels[c.index()].binding, hw) {
                (takt_mir::program::Binding::Hw(a), Some(hw)) => hw.channel(&a.text()).and_then(|e| e.guard_ns),
                _ => None,
            };
            format!("{}LL", guard.unwrap_or(0))
        })
        .collect();
    let _ = writeln!(s, "static const long long g_guard[{n}] = {{ {} }};", guards.join(", "));
    let _ = writeln!(s, "static int takt_sched_slot(int o) {{");
    let _ = writeln!(s, "    switch (o) {{");
    for (q, c) in queues.iter().enumerate() {
        let _ = writeln!(s, "    case {}: return {q}; /* {} */", c.index(), p.channels[c.index()].name);
    }
    let _ = writeln!(s, "    default: return -1;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    // Das Ergebnis ist null oder die Art des Faults (`abi::fault_code`).
    let timing = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Timing);
    let overflow = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::ScheduleOverflow);
    let _ = writeln!(s, "int takt_schedule(int o, long long t, long long v) {{");
    let _ = writeln!(s, "    int q = takt_sched_slot(o);");
    let _ = writeln!(s, "    if (q < 0) return {overflow};");
    // 7.5, 9.8: `T <= now + guard(o)` ist ein `TimingFault`.
    let _ = writeln!(s, "    if (t <= g_tick * {}LL + g_guard[q]) return {timing};", p.config.tick);
    // Gleiche `T`: die spaetere Anweisung gewinnt (9.8).
    let _ = writeln!(s, "    for (int i = 0; i < g_sched_n[q]; i++)");
    let _ = writeln!(s, "        if (g_sched[q][i].t == t) {{ g_sched[q][i].v = v; return 0; }}");
    let _ = writeln!(s, "    if (g_sched_n[q] >= TAKT_K_O) return {overflow};");
    let _ = writeln!(s, "    g_sched[q][g_sched_n[q]].t = t;");
    let _ = writeln!(s, "    g_sched[q][g_sched_n[q]].v = v;");
    let _ = writeln!(s, "    g_sched_n[q]++;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_cancel(int o) {{ int q = takt_sched_slot(o); if (q >= 0) g_sched_n[q] = 0; }}");
    // 9.9: Schlaf nur, wenn alle `sched[o]` leer sind.
    let _ = writeln!(s, "static _Bool takt_sched_pending(void) {{");
    let _ = writeln!(s, "    for (int q = 0; q < {n}; q++) if (g_sched_n[q]) return 1;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");

    // `apply_scheduled(k)`: Was faellig ist, geht in den Latch. Sind
    // mehrere faellig, gewinnt der spaeteste Zeitpunkt (9.8).
    let _ = writeln!(s, "static void takt_apply_scheduled(long long now) {{");
    let _ = writeln!(s, "    for (int q = 0; q < {n}; q++) {{");
    let _ = writeln!(s, "        long long best_t = -1; long long best_v = 0; int hit = 0;");
    let _ = writeln!(s, "        int k = 0;");
    let _ = writeln!(s, "        for (int i = 0; i < g_sched_n[q]; i++) {{");
    let _ = writeln!(s, "            if (g_sched[q][i].t <= now) {{");
    let _ = writeln!(s, "                if (!hit || g_sched[q][i].t > best_t) {{");
    let _ = writeln!(s, "                    best_t = g_sched[q][i].t; best_v = g_sched[q][i].v; hit = 1;");
    let _ = writeln!(s, "                }}");
    let _ = writeln!(s, "            }} else {{");
    let _ = writeln!(s, "                g_sched[q][k++] = g_sched[q][i];");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        g_sched_n[q] = k;");
    let _ = writeln!(s, "        if (!hit) continue;");
    let _ = writeln!(s, "        switch (q) {{");
    for (q, c) in queues.iter().enumerate() {
        let name = &p.channels[c.index()].name;
        let Some(slot) = layout.outputs.iter().find(|slot| slot.name == *name) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        // Der Wert kam als `i64` an; im Latch steht er in seinem Typ.
        // Ein `double` traegt dieselben Bits, eine Ganzzahl wird
        // verengt — beides genau die Umkehrung von `at` im Codegen.
        let back = if slot.ty.is_float() {
            format!("*({ct} *)(latch + {}) = ({ct})(*(double *)&best_v);", slot.offset)
        } else {
            format!("*({ct} *)(latch + {}) = ({ct})best_v;", slot.offset)
        };
        let _ = writeln!(s, "        case {q}: {back} break; /* {name} */");
    }
    let _ = writeln!(s, "        default: break;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
}

/// Die Outputs mit `sched`-Warteschlange in der Reihenfolge ihrer Plaetze:
/// je Maschine ausser Vorlagen, was `at`, `pulse` und `cancel` treffen
/// (`Layout::output_queues`).
pub(crate) fn queued_outputs(p: &Program) -> Vec<takt_mir::ChannelId> {
    p.machines
        .iter()
        .filter(|m| m.kind != takt_mir::machine::MachineKind::Template)
        .flat_map(|m| m.layout.output_queues.iter().copied())
        .collect()
}

/// `raised[m]` (5.4): ein Abort, den `abort` in diesem Tick erhoben hat;
/// `pending[m]` (5.4, 9.6): ein Fault von aussen — Operator-Abort,
/// Runtime-Fault —, zugestellt zu Beginn des naechsten Schritts oder, wenn
/// die Maschine nicht aktiv ist, in der Abort-Phase. Ein Abort verdraengt
/// einen Runtime-Fault, nicht umgekehrt.
pub(crate) fn raised(s: &mut String, p: &Program) {
    let n = p.machines.len().max(1);
    let abort = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Abort);
    let _ = writeln!(s, "static _Bool g_raised[{n}];");
    let _ = writeln!(s, "static int g_pending[{n}];");
    let _ = writeln!(s, "static void takt_pend(int m, int code) {{");
    let _ = writeln!(s, "    if (g_pending[m] != {abort}) g_pending[m] = code;");
    let _ = writeln!(s, "}}");
}

/// Die Abort-Phase (5.4, 9.4): Nach den Schritten nimmt jede Maschine mit
/// vorgemerktem Abort ihren Fault-Pfad, in statischer Reihenfolge und
/// unabhaengig davon, ob sie in diesem Tick aktiv war.
pub(crate) fn abort_phase(
    s: &mut String,
    p: &Program,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    tick: &str,
) {
    let abort = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Abort);
    let scoped = scoped_of(p);
    for m in driven {
        let Some(i) = p.machines.iter().position(|x| x.name == m.name) else { continue };
        let scope = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, n)) => format!(" && g_scope_{owner}_{n}"),
            None => String::new(),
        };
        let (condition, pending) = (format!("g_raised[{i}]{scope}"), format!("g_pending[{i}]{scope}"));
        let active = match (m.period.max(1), m.phase) {
            (1, _) => "1".to_string(),
            (per, ph) => format!("{tick} % {per} == {ph}"),
        };
        let _ = writeln!(
            s,
            "{indent}if ({condition}) {{ {0}_deliver(state_{0}, image, params, latch, {abort}, {active}); {0}_publish(state_{0}, image); }}",
            m.name
        );
        // Ein vorgemerkter Fault einer Maschine, die in diesem Tick nicht
        // schritt; ein Abort aus `raised` geht vor, der Fault wartet.
        let _ = writeln!(
            s,
            "{indent}else if ({pending}) {{ {0}_deliver(state_{0}, image, params, latch, g_pending[{i}], {active}); g_pending[{i}] = 0; {0}_publish(state_{0}, image); }}",
            m.name
        );
    }
    let _ = writeln!(s, "{indent}memset(g_raised, 0, sizeof g_raised);");
}

/// Der Verwurf im `idle` (5.10, 9.6 `advance_cursors`) nach der
/// Abort-Phase, fuer jede Maschine, die etwas zu verwerfen hat. Eine
/// inaktive gescopte Instanz hoert ohnehin nicht, und ihr Zustand teilt
/// sich den Speicher mit ihren Geschwistern (11.2).
pub(crate) fn idle_drops(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str) {
    let scoped = scoped_of(p);
    for m in driven.iter().filter(|m| takt_llvm::step::drops(m, p)) {
        let scope = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, n)) => format!("if (g_scope_{owner}_{n}) "),
            None => String::new(),
        };
        let _ = writeln!(s, "{indent}{scope}{0}_drop(state_{0});", m.name);
    }
}

/// Die Flankentabelle der Alerts (5.6: „die Runtime protokolliert
/// Flanken"): ein Platz je Maschine, Alert-Stelle und Durchlauf der
/// umgebenden Schleifen, wie `alert_edge` im Interpreter; geschrieben
/// wird nur, was sich aendert.
pub(crate) fn alert_table(s: &mut String, p: &Program) {
    let mut bases = Vec::with_capacity(p.machines.len());
    let mut total = 0u32;
    for m in &p.machines {
        bases.push(total.to_string());
        if m.kind != takt_mir::machine::MachineKind::Template {
            total = total.saturating_add(takt_llvm::machine::counters(m, p).alert_slots());
        }
    }
    let _ = writeln!(s, "static _Bool g_alert[{}];", total.max(1));
    let _ = writeln!(s, "static const int g_alert_base[{}] = {{ {} }};", bases.len().max(1), bases.join(", "));
    let _ = writeln!(s, "static _Bool takt_alert_edge(int m, int slot, unsigned char on) {{");
    let _ = writeln!(s, "    _Bool *was = &g_alert[g_alert_base[m] + slot];");
    let _ = writeln!(s, "    if (*was == (on != 0)) return 0;");
    let _ = writeln!(s, "    *was = on != 0;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
}

/// Merkt Operator-Aborts und Runtime-Faults des Stimulus in ihrem Tick vor:
/// einen `Driver`-Fault beim Besitzer seines Outputs, alles andere bei
/// jeder Maschine.
fn pended(s: &mut String, p: &Program, inputs: &[Stimulus], indent: &str) {
    use takt_mir::machine::{FaultKind, RuntimeKind};
    let every = format!("for (int m = 0; m < {}; m++)", p.machines.len());
    for stim in inputs {
        let (tick, code, owner) = match stim {
            Stimulus::Abort { tick } => (*tick, takt_llvm::abi::fault_code(FaultKind::Abort), None),
            Stimulus::Runtime { tick, kind, output } => {
                let owner = match (kind, output) {
                    (RuntimeKind::Driver, Some(o)) => {
                        p.channels.iter().find(|c| c.name == *o).and_then(|c| c.owner).map(|m| m.index())
                    }
                    _ => None,
                };
                (*tick, takt_llvm::abi::fault_code(FaultKind::Runtime(*kind)), owner)
            }
            _ => continue,
        };
        let _ = match owner {
            Some(m) => writeln!(s, "{indent}if (g_tick == {tick}) takt_pend({m}, {code});"),
            None => writeln!(s, "{indent}if (g_tick == {tick}) {every} takt_pend(m, {code});"),
        };
    }
}

/// Die Namen, mit denen die Rahmen einen Fault schreiben: die Maschine
/// und die Art wie im Trace des Interpreters (`FaultKind::name`), die Art
/// nach ihrer Zahl in der ABI (`abi::fault_code`).
pub(crate) fn fault_names(s: &mut String, p: &Program) {
    let names: Vec<String> = p.machines.iter().map(|m| format!("\"{}\"", m.name)).collect();
    let _ = writeln!(s, "static const char *const takt_machine_names[{}] = {{ {} }};", names.len().max(1), {
        if names.is_empty() { "\"?\"".to_string() } else { names.join(", ") }
    });
    let _ = writeln!(s, "static const char *takt_machine_name(int m) {{");
    let _ = writeln!(s, "    return m >= 0 && m < {} ? takt_machine_names[m] : \"?\";", names.len());
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static const char *takt_fault_name(int code) {{");
    let _ = writeln!(s, "    switch (code) {{");
    for kind in takt_mir::machine::FaultKind::all() {
        let _ = writeln!(s, "    case {}: return \"{}\";", takt_llvm::abi::fault_code(kind), kind.name());
    }
    let _ = writeln!(s, "    default: return \"?\";");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
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
pub(crate) fn sim_bindings(s: &mut String, p: &Program, indent: &str) {
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
        if let Some(age) = age_offset(p, &inp.name) {
            let _ = writeln!(s, "{indent}*(long long *)(image + {age}) = 0;");
        }
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

/// Die `hw`-Eingaenge, die ein `sim`-Output derselben Adresse speist (8.3):
/// Im Sim-Build ist das Modell ihre Quelle, kein Treiber.
pub(crate) fn sim_fed_inputs(p: &Program) -> Vec<usize> {
    use takt_mir::program::{Binding, Direction};
    p.channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dir == Direction::Input)
        .filter(|(_, c)| {
            let Binding::Hw(addr) = &c.binding else { return false };
            p.channels.iter().any(|o| o.dir == Direction::Output && matches!(&o.binding, Binding::Sim(a) if a == addr))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Die Signaturen des erzeugten Codes je Maschine (11.2), fuer beide
/// Rahmen.
pub(crate) fn machine_declarations(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine]) {
    for m in driven {
        let _ = writeln!(s, "void {}_init(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_step(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_publish(void *st, void *in);", m.name);
        let _ =
            writeln!(s, "void {}_deliver(void *st, void *in, void *par, void *out, int code, _Bool active);", m.name);
        let _ = writeln!(s, "void {}_pend(void *st, int code);", m.name);
        if takt_llvm::step::drops(m, p) {
            let _ = writeln!(s, "void {}_drop(void *st);", m.name);
        }
        let _ = writeln!(s, "void {}_init_vars(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_enter(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "_Bool {}_idle(void *st);", m.name);
        let _ = writeln!(s, "long long {}_deadline(void *st);", m.name);
        let _ = writeln!(s, "void {}_advance(void *st, long long n);", m.name);
        if !m.persist.is_empty() {
            let _ = writeln!(s, "int {}_persist_snapshot(void *st, void *out, int cap);", m.name);
            let _ = writeln!(s, "int {}_persist_restore(void *st, const void *in, int len);", m.name);
        }
        // 5.11: das Aktivitaetspraedikat je gescopter Instanz und die
        // `exit:`-Bloecke fuer ihren Austritt.
        for (i, _) in m.states.iter().flat_map(|st| st.instances.iter()).enumerate() {
            let _ = writeln!(s, "_Bool {}_scope_{i}(void *st);", m.name);
        }
        let _ = writeln!(s, "void {}_exit_all(void *st, void *in, void *par, void *out);", m.name);
        if !m.layout.trigger_flags.is_empty() {
            let _ = writeln!(s, "void {}_triggers(void *st, void *in, void *par, void *out);", m.name);
        }
    }
}

/// 5.11: je gescopter Instanz, ob sie zu Beginn des vorigen Ticks aktiv
/// war — der Vergleich liefert Ein- und Austritt.
pub(crate) fn scope_flags(s: &mut String, p: &Program) {
    for (owner, _, i) in scoped_of(p) {
        let _ = writeln!(s, "static _Bool g_scope_{owner}_{i} = 0;");
    }
}

/// Die Eintritte vor dem ersten Tick, in Schrittordnung und nach jedem
/// `publish`, damit Follower schon im Tick 0 frisch lesen (7.2, 9.4).
/// Eine gescopte Instanz betritt nichts, solange ihr Scope nicht steht;
/// `scoped_lifecycle` nach dem `enter` des Besitzers holt sie herein
/// (5.11).
pub(crate) fn enter_machines(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
) {
    let scoped: Vec<String> = scoped_of(p).into_iter().map(|(_, inst, _)| inst).collect();
    for m in driven.iter().filter(|m| !scoped.contains(&m.name)) {
        let _ = writeln!(s, "{indent}{0}_enter(state_{0}, image, params, latch);", m.name);
        let _ = writeln!(s, "{indent}{0}_publish(state_{0}, image);", m.name);
    }
    scoped_lifecycle(s, p, layout, indent);
}

/// Die Schritte eines Ticks, fuer beide Rahmen: erst die Trigger-Phase
/// (7.5, wie im Interpreter zwischen Zustellung und Schritt), dann jede
/// Maschine in Schrittordnung — in jedem `period`-ten Tick mit ihrer
/// Phase (7.2), eine gescopte Instanz nur, solange ihr Scope zu
/// Tick-Beginn steht —, zuletzt der Lebenszyklus der gescopten Instanzen
/// (5.11). `tick` ist der Ausdruck der Tickzahl.
pub(crate) fn steps(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    tick: &str,
) {
    for m in driven {
        if !m.layout.trigger_flags.is_empty() {
            let _ = writeln!(s, "{indent}{0}_triggers(state_{0}, image, params, latch);", m.name);
        }
    }
    let scoped = scoped_of(p);
    for m in driven {
        let condition = match (m.period.max(1), m.phase) {
            (1, _) => String::new(),
            (per, 0) => format!("if ({tick} % {per} == 0) "),
            (per, ph) => format!("if ({tick} % {per} == {ph}) "),
        };
        let condition = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, i)) if condition.is_empty() => format!("if (g_scope_{owner}_{i}) "),
            Some((owner, _, i)) => format!("{} if (g_scope_{owner}_{i}) ", condition.trim_end()),
            None => condition,
        };
        // 9.6: Ein vorgemerkter Fault geht zu Beginn des Schritts in den
        // Zustand; der Schritt nimmt ihn statt seines Rumpfs.
        let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
        let _ = writeln!(
            s,
            "{indent}{condition}{{ if (g_pending[{i}]) {{ {0}_pend(state_{0}, g_pending[{i}]); g_pending[{i}] = 0; }} {0}_step(state_{0}, image, params, latch); {0}_publish(state_{0}, image); }}",
            m.name
        );
    }
    scoped_lifecycle(s, p, layout, indent);
}

/// Die gescopten Instanzen mit ihrem Besitzer und der Nummer, unter der
/// der Codegen das Praedikat `<besitzer>_scope_<n>` erzeugt (5.11).
pub(crate) fn scoped_of(p: &Program) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for owner in &p.machines {
        for (i, si) in owner.states.iter().flat_map(|s| s.instances.iter()).enumerate() {
            out.push((owner.name.clone(), p.machines[si.machine.index()].name.clone(), i));
        }
    }
    out
}

/// Der Lebenszyklus der gescopten Instanzen nach dem Schritt des
/// Besitzers (5.11): Eintritt initialisiert frisch und betritt `initial`,
/// Austritt setzt die Outputs auf `safe` und verwirft den Zustand.
///
/// Das Aktivitaetsbit steht im Rahmen, nicht im Zustands-Struct: Es ist
/// eine Aussage ueber den *Besitzer*, und der Rahmen fragt sie ohnehin
/// vor jedem Schritt ab (`<besitzer>_scope_<n>`).
pub(crate) fn scoped_lifecycle(s: &mut String, p: &Program, layout: &Layout, indent: &str) {
    for (owner, inst, i) in scoped_of(p) {
        let _ = writeln!(s, "{indent}{{ _Bool now = {owner}_scope_{i}(state_{owner});");
        let _ = writeln!(s, "{indent}  if (now && !g_scope_{owner}_{i}) {{");
        let _ = writeln!(s, "{indent}    memset(state_{inst}, 0, sizeof state_{inst});");
        let _ = writeln!(s, "{indent}    {inst}_init_vars(state_{inst}, image, params, latch);");
        let _ = writeln!(s, "{indent}    {inst}_enter(state_{inst}, image, params, latch);");
        let _ = writeln!(s, "{indent}    {inst}_publish(state_{inst}, image);");
        let _ = writeln!(s, "{indent}  }} else if (!now && g_scope_{owner}_{i}) {{");
        // 5.11: erst die `exit:`-Bloecke von innen nach aussen, dann
        // gehen die Outputs auf `safe` — sie ueberschreiben, was ein
        // `exit` an ihnen tat, genau wie im Interpreter.
        let _ = writeln!(s, "{indent}    {inst}_exit_all(state_{inst}, image, params, latch);");
        safe_outputs_of(s, p, layout, &inst, &format!("{indent}    "));
        let _ = writeln!(s, "{indent}    memset(state_{inst}, 0, sizeof state_{inst});");
        let _ = writeln!(s, "{indent}    {inst}_publish(state_{inst}, image);");
        let _ = writeln!(s, "{indent}  }}");
        let _ = writeln!(s, "{indent}  g_scope_{owner}_{i} = now; }}");
    }
}

/// Die Outputs einer Maschine auf ihren `safe`-Wert (5.11, 5.2 Regel 5).
fn safe_outputs_of(s: &mut String, p: &Program, layout: &Layout, machine: &str, indent: &str) {
    let Some(id) = p.machines.iter().position(|m| m.name == machine).map(|i| takt_mir::MachineId(i as u32)) else {
        return;
    };
    for slot in &layout.outputs {
        let Some(c) = p.channels.iter().find(|c| c.name == slot.name) else { continue };
        if c.owner != Some(id) {
            continue;
        }
        let Some(safe) = c.attrs.safe.as_ref().and_then(|e| literal(p, e)) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "{indent}*({ct} *)(latch + {}) = {safe}; /* {} auf safe (5.11) */", slot.offset, slot.name);
    }
}

/// Ψ_{k+1} wird Ψ_k (9.4): die zweite Bank in die erste kopieren, dann
/// `fresh` und die Signale loeschen — ein Signal ist einen Tick sichtbar
/// (5.8), und ohne `fresh` liest ein Follower wieder Ψ_k (7.2).
/// Der Commit eines Ticks, fuer beide Rahmen und fuer Tick 0 dieselbe
/// Folge (12.1): erst Ψ, dann die `sim`-Outputs an ihre `hw`-Inputs (8.3,
/// Unit-Delay wie bei Ψ), dann die Sendepuffer (8.8: gesendet wird beim
/// Commit, und der Treiber holt seine Rate ab, bevor der Latch
/// ausgeschrieben wird) und die internen Ringe (8.6). Zwei Fassungen
/// dieser Folge wichen einmal voneinander ab (FB-269).
pub(crate) fn commit_sequence(
    s: &mut String,
    p: &Program,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    tick: &str,
) {
    psi_commit(s, p, driven, indent);
    // 9.8, 12.1: Was in diesem Tick faellig wird, geht nach den Schritten
    // in den Latch, vor dem Commit — ein geplanter Wert gewinnt gegen eine
    // Zuweisung desselben Ticks, und die `sim`-Bindung sieht ihn.
    if !queued_outputs(p).is_empty() {
        let _ = writeln!(s, "{indent}takt_apply_scheduled({tick} * {}LL);", p.config.tick);
    }
    sim_bindings(s, p, indent);
    let _ = writeln!(s, "{indent}takt_tx_commit({tick});");
    let _ = writeln!(s, "{indent}takt_int_commit();");
}

/// `maybe_sleep()` nach 9.9 am Ende eines Ticks: Sind alle Maschinen
/// `idle`, rueckt der Rahmen bis vor die frueheste `after`-Frist, wie
/// `Runtime::sleep` — `n = d / T0 - 1`, und der Tick an der Frist laeuft.
/// Die Zeitzeile traegt `slept`, wie auf dem Board; der Vergleich liest
/// sie nicht. Wake-Kommandos und Jobs gibt es in diesem Rahmen nicht
/// (keine Eingaben); ausstehende geplante Ausgaben und ein Fault, der
/// hinter einem Abort wartet (`g_pending`), verbieten den Schlaf.
fn virtual_sleep(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], ticks: u64) {
    let _ = writeln!(s, "        {{");
    let _ = writeln!(s, "            _Bool idle = 1;");
    if !queued_outputs(p).is_empty() {
        let _ = writeln!(s, "            idle = !takt_sched_pending();");
    }
    let _ = writeln!(s, "            long long best = -1;");
    for m in driven {
        let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
        let _ = writeln!(s, "            idle = idle && !g_pending[{i}] && {0}_idle(state_{0});", m.name);
    }
    for m in driven {
        let _ = writeln!(s, "            if (idle) {{");
        let _ = writeln!(s, "                long long d = {0}_deadline(state_{0});", m.name);
        let _ = writeln!(s, "                if (d >= 0 && (best < 0 || d < best)) best = d;");
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            long long n = idle && best > 1 ? best - 1 : 0;");
    let _ = writeln!(s, "            if (g_tick + n > {ticks}LL) n = {ticks}LL - g_tick;");
    let _ = writeln!(s, "            if (n > 0) {{");
    for m in driven {
        let _ = writeln!(s, "                {0}_advance(state_{0}, n);", m.name);
    }
    let _ = writeln!(s, "                printf(\"t=%lld time took=0 drift=0 slept=%lld\\n\", g_tick, n);");
    let _ = writeln!(s, "                g_tick += n;");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
}

pub(crate) fn psi_commit(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str) {
    use takt_llvm::psi::{Field, bank_size, field_offset, region_offset};
    let Some(first) = region_offset(takt_mir::MachineId(0), false, p) else { return };
    let Some(next) = region_offset(takt_mir::MachineId(0), true, p) else { return };
    // 8.3: Was eine Maschine ausgibt, lesen die anderen im naechsten Tick
    // aus ihrer Ψ-Bank.
    for (i, c) in p.channels.iter().enumerate() {
        let id = takt_mir::ChannelId(i as u32);
        let (Some(owner), Some(src)) = (c.owner, takt_llvm::image::latch_offset(id, p)) else { continue };
        let (Some(bank), Some(off), Some(ty)) =
            (region_offset(owner, true, p), field_offset(owner, Field::Output(id), p), takt_llvm::ty::lower(c.ty, p))
        else {
            continue;
        };
        let dst = bank + off;
        // Ein Skalar als Zuweisung: `memcpy` unbekannter Ausrichtung wird auf RV32 ein Aufruf je Byte.
        let _ = match c_type(&ty, false) {
            Some(ct) => {
                writeln!(s, "{indent}*({ct} *)(image + {dst}) = *(const {ct} *)(latch + {src}); /* Psi {} */", c.name)
            }
            None => writeln!(s, "{indent}memcpy(image + {dst}, latch + {src}, {}); /* Psi {} */", ty.size(), c.name),
        };
    }
    // Bank und Regionen sind 8-ausgerichtet (psi.rs); die Byte-Schleife kostete auf RV32 rund 10 us je Tick.
    let _ = writeln!(
        s,
        "{indent}for (unsigned i = 0; i < {}; i++) ((unsigned long long *)(image + {first}))[i] = ((const unsigned long long *)(image + {next}))[i]; /* Psi */",
        bank_size(p) / 8
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

/// Der Versatz eines Feldes im Eintrag eines Channels: Wert, Qualitaet,
/// Grund, `age` (`image`).
fn entry_field(p: &Program, name: &str, field: takt_llvm::image::Slot) -> Option<u64> {
    let index = p.channels.iter().position(|c| c.name == name)?;
    let id = takt_mir::ChannelId(index as u32);
    let base = takt_llvm::image::offset_of(id, p)?;
    Some(base + takt_llvm::image::entry_type(id, p)?.field_offset(field as usize))
}

/// Der Versatz des Qualitaetsbytes eines Inputs im Abbild (3.5).
pub(crate) fn quality_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Quality)
}

/// Der Versatz des Grundes (`reason`) im Eintrag eines Channels (3.5).
pub(crate) fn reason_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Reason)
}

/// Der Versatz von `age` im Eintrag eines Channels (3.5).
pub(crate) fn age_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Age)
}

/// Die Abtastungen altern um einen Tick; ueber `max_age` werden sie
/// `Stale` (3.5), wie `age_inputs` im Interpreter — vor der Lieferung
/// des Ticks, die das Alter zuruecksetzt.
pub(crate) fn aging(s: &mut String, p: &Program, layout: &Layout, indent: &str) {
    let tick = p.config.tick;
    for slot in &layout.inputs {
        let (Some(q), Some(age), Some(reason)) = (
            quality_offset(p, &slot.name),
            age_offset(p, &slot.name),
            entry_field(p, &slot.name, takt_llvm::image::Slot::Reason),
        ) else {
            continue;
        };
        let stale = match p.channels.iter().find(|c| c.name == slot.name).and_then(|c| c.attrs.max_age) {
            Some(max) => format!(" if (*a > {max}LL && image[{q}] != 3) {{ image[{q}] = 2; image[{reason}] = 0; }}"),
            None => String::new(),
        };
        let _ = writeln!(
            s,
            "{indent}{{ long long *a = (long long *)(image + {age}); *a = *a > {}LL ? {}LL : *a + {tick}LL;{stale} }}",
            i64::MAX - tick,
            i64::MAX
        );
    }
}

/// Die Varianten eines Enum-Outputs mit ihren Diskriminanten (3.7).
fn enum_variants(p: &Program, name: &str) -> Option<Vec<(i64, String)>> {
    let c = p.channels.iter().find(|c| c.name == name)?;
    let takt_mir::types::Type::Enum(e) = p.types.list.get(c.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    Some(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect())
}

/// Wo `sys/next_run` im Latch steht und was jede Diskriminante verlangt
/// (12.7). Die Bedeutungen kommen aus `takt_mir::sys`, derselben Tabelle,
/// die der Interpreter fragt.
pub(crate) struct NextRunSlot<'a> {
    pub(crate) slot: &'a crate::layout::Slot,
    /// Der C-Typ der Diskriminante: `int` vorn im Struct, weil `NextRun`
    /// mit `AFTER(delay)` Felder hat (11.2).
    pub(crate) ct: &'static str,
    /// Diskriminante und Bedeutung jeder Variante, die den Lauf beendet.
    pub(crate) ends: Vec<(i64, takt_mir::sys::NextRun)>,
}

/// Der Latch-Platz von `sys/next_run`, wenn das Programm den Kanal bindet.
pub(crate) fn next_run_slot<'a>(p: &Program, layout: &'a Layout) -> Option<NextRunSlot<'a>> {
    let (output, variants) = takt_mir::sys::next_run(p)?;
    let slot = layout.outputs.iter().find(|s| s.name == p.channels[output].name)?;
    let ct = match &slot.ty {
        takt_llvm::ty::LlvmType::Struct(_) => "int",
        t => c_type(t, slot.signed)?,
    };
    let ends = variants.into_iter().filter_map(|(d, next)| Some((d, next?))).collect();
    Some(NextRunSlot { slot, ct, ends })
}

/// Die Job-Slots eines Programms, flach ueber die Maschinen wie im Abbild:
/// Maschine, Slot in ihr, native Funktion.
pub(crate) fn job_slots(p: &Program) -> Vec<(usize, usize, takt_mir::NativeId)> {
    p.machines
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| m.layout.job_slots.iter().enumerate().map(move |(j, s)| (mi, j, s.native)))
        .collect()
}

/// Was beide Rahmen ueber ihre Jobs wissen (4.5): Versatz jedes Slots im
/// Abbild, seine Dauer in Ticks, der erste Slot je Maschine und die Helfer
/// aus `JOBS_C`. Gibt die Zahl der Slots und die groesste Ergebnislaenge
/// zurueck; `None`, wenn das Programm keine Jobs startet.
pub(crate) fn job_tables(s: &mut String, p: &Program) -> Option<(usize, u64)> {
    use takt_mir::fns::NativeKind;
    let slots = job_slots(p);
    if slots.is_empty() || !p.natives.iter().any(|n| n.kind == NativeKind::Job) {
        return None;
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
    let _ = writeln!(s, "static const long long takt_job_at[{}] = {{ {} }};", slots.len(), at.join(", "));
    let _ = writeln!(s, "static const int takt_job_ticks[{}] = {{ {} }};", slots.len(), ticks.join(", "));
    let _ = writeln!(s, "static const int takt_job_base[{}] = {{ {} }};", base.len().max(1), base.join(", "));
    s.push_str(JOBS_C);
    Some((slots.len(), out_max))
}

/// Ruft die native Funktion eines Jobs (4.5): `native` waehlt sie, `args`
/// und `len` sind die Folge kanonischer Bloecke (`u32` Laenge, Bytes), ein
/// `bytes<N>` darin als Laenge und Daten (5.9); das Ergebnis geht in
/// kanonischer Form nach `out`, seine Laenge nach `out_len`.
pub(crate) fn job_call(s: &mut String, p: &Program, args: &str, len: &str, out: &str, out_len: &str, indent: &str) {
    let _ = writeln!(s, "{indent}const unsigned char *a[8]; int n[8]; int k = 0, p = 0;");
    let _ = writeln!(s, "{indent}for (k = 0; k < 8; k++) {{ a[k] = {args}; n[k] = 0; }}");
    let _ = writeln!(
        s,
        "{indent}for (k = 0; k < 8 && p + 4 <= {len}; k++) {{ n[k] = (int)takt_job_le32({args} + p); a[k] = {args} + p + 4; p += 4 + n[k]; }}"
    );
    let _ = writeln!(s, "{indent}{out_len} = 0;");
    let _ = writeln!(s, "{indent}switch (native) {{");
    for (idx, n) in p.natives.iter().enumerate() {
        if n.kind != takt_mir::fns::NativeKind::Job {
            continue;
        }
        if let Some(case) = job_case(n, out, out_len) {
            let _ = writeln!(s, "{indent}case {idx}: {{ {case} }} break;");
        }
    }
    let _ = writeln!(s, "{indent}default: break;");
    let _ = writeln!(s, "{indent}}}");
}

/// 4.5: Jobs im Rahmen. Das Ergebnis der reinen Funktion steht beim Start
/// fest; der Slot im Abbild wird `done`, sobald `duration` in Ticks
/// vergangen ist — wie das Modell des Interpreters.
fn jobs(s: &mut String, p: &Program) {
    let Some((slots, out_max)) = job_tables(s, p) else { return };
    let _ = writeln!(
        s,
        "typedef struct {{ int active; long long due; int out_len; unsigned char out[{out_max}]; }} takt_job;"
    );
    let _ = writeln!(s, "static takt_job g_jobs[{slots}];");
    let _ = writeln!(s, "void takt_job_begin(int m, int slot, int native, const unsigned char *args, int len) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; takt_job *j = &g_jobs[i];");
    job_call(s, p, "args", "len", "j->out", "j->out_len", "    ");
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
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) {{");
    let _ = writeln!(s, "        if (!g_jobs[i].active || g_jobs[i].due > g_tick) continue;");
    let _ = writeln!(s, "        g_jobs[i].active = 0;");
    let _ =
        writeln!(s, "        if (g_jobs[i].out_len < 0) {{ takt_job_image(i, 1, 0, 1); continue; }} /* Err(FAILED) */");
    let _ = writeln!(s, "        takt_job_image(i, 1, 1, 0);");
    let _ = writeln!(
        s,
        "        for (b = 0; b < g_jobs[i].out_len; b++) image[takt_job_at[i] + 8 + b] = g_jobs[i].out[b];"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_jobs_init(void) {{ int i; for (i = 0; i < {slots}; i++) takt_job_image(i, 0, 0, 2); }}"
    );
    let _ = writeln!(s);
}

/// Der Aufruf einer `native job` im Rahmen: Argumente nach Art, das
/// Ergebnis in kanonischer Form nach `out`, seine Laenge nach `out_len`;
/// `-1` dort heisst `Err(FAILED)`.
fn job_case(n: &takt_mir::fns::Native, out: &str, out_len: &str) -> Option<String> {
    use takt_native::Kind;
    let sig = takt_native::Native::by_name(&n.name)?.signature();
    let args: Vec<String> = sig
        .params
        .iter()
        .enumerate()
        .map(|(k, kind)| match kind {
            Kind::Bytes | Kind::Digest | Kind::Fixed(_) => format!("a[{k}] + 4, (int)takt_job_le32(a[{k}])"),
            _ => format!("a[{k}], n[{k}]"),
        })
        .collect();
    let call = format!("takt_native_{}({})", n.name, args.join(", "));
    Some(match sig.ret {
        Kind::U32 => format!("unsigned int v = {call}; takt_job_put32({out}, v); {out_len} = 4;"),
        Kind::U16 => format!(
            "unsigned short v = {call}; {out}[0] = (unsigned char)v; {out}[1] = (unsigned char)(v >> 8); {out_len} = 2;"
        ),
        Kind::U8 | Kind::Bool => format!("{out}[0] = {call}; {out_len} = 1;"),
        Kind::Digest => format!("takt_native_{}({}, {out}); {out_len} = 36;", n.name, args.join(", ")),
        Kind::Sha256Ctx => format!(
            "takt_native_{}({}, {out}); {out_len} = 44 + (int)takt_job_le32({out} + 32);",
            n.name,
            args.join(", ")
        ),
        // Ein Feld traegt keine Laenge; es ist so lang wie die Eingabe.
        Kind::Floats256 => format!("takt_native_{}({}, {out}); {out_len} = n[0];", n.name, args.join(", ")),
        Kind::Fixed(_) => return None,
        Kind::Bytes => format!(
            "int r = takt_native_{}({}, {out} + 4); if (r < 0) {{ {out_len} = -1; }} else {{ takt_job_put32({out}, (unsigned int)r); {out_len} = 4 + r; }}",
            n.name,
            args.join(", ")
        ),
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
