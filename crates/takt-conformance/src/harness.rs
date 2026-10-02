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

use crate::stimulus::Stimulus;
use takt_frame::layout::{Layout, c_type};
use takt_frame::parts::{
    DURATION_C, NextRunSlot, abort_phase, aging, alert_table, commit_sequence, enter_machines, fault_names, idle_drops,
    jitter, job_call, job_tables, machine_declarations, natives, next_run_slot, param_literal, quality_offset,
    queued_outputs, raised, safe_outputs, scheduled, scope_flags, sim_bindings, steps,
};

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
    build_sleeping_with(p, ticks, &[])
}

/// Wie [`build_sleeping`], mit Eingaben: Der Rahmen schlaeft nie ueber
/// einen Tick, in dem der Stimulus liefert. So prueft der Wirt auch die
/// Konjunkte aus 9.9, die an Eingaben haengen — ein voller Wake-Strom haelt
/// das System wach (FB-334).
pub fn build_sleeping_with(p: &Program, ticks: u64, inputs: &[Stimulus]) -> Harness {
    build_inner(p, None, None, ticks, inputs, &[], true)
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
    let layout = takt_frame::layout::of(p);
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
    takt_frame::streams::emit(&mut s, p, takt_frame::streams::Trace::Stdio);

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
        let _ = writeln!(s, "{}", takt_frame::layout::c_buffer(&format!("state_{}", m.name), bytes.max(64)));
    }
    let _ = writeln!(s, "{}", takt_frame::layout::c_buffer("image", layout.image));
    let _ = writeln!(s, "{}", takt_frame::layout::c_buffer("params", layout.params));
    let _ = writeln!(s, "{}", takt_frame::layout::c_buffer("latch", layout.latch));
    for (i, prop) in &monitors {
        let size = takt_llvm::monitor::state_size(prop, p).unwrap_or(1);
        let _ = writeln!(s, "{}", takt_frame::layout::c_buffer(&format!("monitor_{i}"), size));
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
    let most = crate::deliveries::stimulus(&mut feed, p, &layout, inputs);
    takt_frame::edge::emit(&mut s, p, &layout, &driven, most, takt_frame::streams::Trace::Stdio);
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
        virtual_sleep(&mut s, p, &driven, ticks, inputs);
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
fn payload_enum_dump(p: &Program, slot: &takt_frame::layout::Slot) -> Option<String> {
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
fn record_dump(p: &Program, slot: &takt_frame::layout::Slot) -> Option<String> {
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

/// `maybe_sleep()` nach 9.9 am Ende eines Ticks: Sind alle Maschinen
/// `idle`, rueckt der Rahmen bis vor die frueheste `after`-Frist, wie
/// `Runtime::sleep` — `n = d / T0 - 1`, und der Tick an der Frist laeuft.
/// Die Zeitzeile traegt `slept`, wie auf dem Board; der Vergleich liest
/// sie nicht. Ausstehende geplante Ausgaben, ein laufender Job und ein
/// Fault, der hinter einem Abort wartet (`g_pending`), verbieten den
/// Schlaf; ob ein Wake-Strom
/// etwas im Fenster hat, sagt `_idle`. Ein Tick, in dem der Stimulus
/// liefert, laeuft immer: Auf dem Board weckt ein Wake-Ereignis den Kern,
/// hier kennt der Rahmen die Lieferungen im Voraus.
fn virtual_sleep(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], ticks: u64, inputs: &[Stimulus]) {
    let _ = writeln!(s, "        {{");
    let _ = writeln!(s, "            _Bool idle = 1;");
    if !queued_outputs(p).is_empty() {
        let _ = writeln!(s, "            idle = !takt_sched_pending();");
    }
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(s, "            idle = idle && !takt_jobs_active();");
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
    let mut due: Vec<u64> = inputs.iter().map(Stimulus::tick).collect();
    due.sort_unstable();
    due.dedup();
    if !due.is_empty() {
        let list: Vec<String> = due.iter().map(|t| format!("{t}LL")).collect();
        let _ = writeln!(s, "            static const long long due[] = {{ {} }};", list.join(", "));
        let _ = writeln!(s, "            for (unsigned i = 0; i < sizeof due / sizeof due[0]; i++)");
        let _ = writeln!(
            s,
            "                if (due[i] > g_tick) {{ if (g_tick + n >= due[i]) n = due[i] - g_tick - 1; break; }}"
        );
    }
    let _ = writeln!(s, "            if (n > 0) {{");
    for m in driven {
        let _ = writeln!(s, "                {0}_advance(state_{0}, n);", m.name);
    }
    let _ = writeln!(s, "                printf(\"t=%lld time took=0 drift=0 slept=%lld\\n\", g_tick, n);");
    let _ = writeln!(s, "                g_tick += n;");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
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

/// Die Varianten eines Enum-Outputs mit ihren Diskriminanten (3.7).
fn enum_variants(p: &Program, name: &str) -> Option<Vec<(i64, String)>> {
    let c = p.channels.iter().find(|c| c.name == name)?;
    let takt_mir::types::Type::Enum(e) = p.types.list.get(c.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    Some(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect())
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
    // 9.9, Konjunkt 5: Solange ein Job laeuft, schlaeft das System nicht.
    let _ = writeln!(
        s,
        "static _Bool takt_jobs_active(void) {{ int i; for (i = 0; i < {slots}; i++) if (g_jobs[i].active) return 1; return 0; }}"
    );
    let _ = writeln!(s);
}
