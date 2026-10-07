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

use takt_llvm::symbols::Prefix;
use takt_mir::MachineId;
use takt_mir::program::Program;

use crate::stimulus::Stimulus;
use takt_frame::layout::{Layout, c_type};
use takt_frame::parts::{
    DURATION_C, NextRunSlot, abort_phase, aging, alert_table, commit_sequence, enter_machines, fault_names, idle_drops,
    jitter, job_call, job_tables, machine_declarations, natives, next_run_slot, param_literal, quality_offset,
    queued_outputs, raised, safe_outputs, scheduled, scope_flags, sim_bindings, steps,
};
use takt_frame::text::Text;

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

/// Das Programm mit den Parameterwerten des Profils `name` (8.4): Defaults,
/// dann das Profil, wie `eval_params` im Interpreter. Der Rahmen setzt den
/// Parametervektor aus den Defaults; ohne diesen Schritt liefe ein Szenario
/// mit Profil nativ mit anderen Parametern als im Interpreter. `None`, wenn
/// es das Profil nicht gibt.
pub fn with_profile(p: &Program, name: &str) -> Option<Program> {
    let profile = p.profiles.iter().find(|x| x.name == name)?;
    let mut out = p.clone();
    for (id, value) in &profile.assignments {
        out.params[id.index()].default = value.clone();
    }
    Some(out)
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
    // Die Einstiege heissen wie im erzeugten Code ohne Angabe eines Praefixes
    // (`Prefix::default`, 12.11): Der Testrahmen bindet ein Programm.
    let x = &Prefix::default();
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
    let arena = takt_llvm::arena::of(p);
    let mut s = String::new();
    let _ = writeln!(s, "/* Testrahmen (13.8); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "#include <stdint.h>");
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
    let head = s;
    let mut t = Text::default();
    // Die eine Arena des Laufs (12.11); `main` reicht sie an jede Funktion.
    let _ = writeln!(t.code, "static struct {x}_arena g_arena;");
    let _ = writeln!(t.code, "static void takt_tx_commit(struct {x}_arena *a, long long);");
    let _ = writeln!(t.code, "static void takt_int_commit(struct {x}_arena *a);");

    // Die Runtime-Aufrufe (`takt-llvm/src/abi.rs`). Sie schreiben in den
    // Trace, damit der Vergleich sie sieht.
    let _ = writeln!(t.fields, "    long long tick;");
    let _ = writeln!(t.code, "{DURATION_C}");
    scope_flags(&mut t, p);
    fault_names(&mut t.code, p);
    alert_table(&mut t, p, x);
    let _ = writeln!(
        t.code,
        "void {x}_alert(struct {x}_arena *a, int m, int slot, unsigned char on, unsigned char invalid) {{"
    );
    let _ = writeln!(t.code, "    if (!takt_alert_edge(a, m, slot, on)) return;");
    let _ = writeln!(
        t.code,
        "    printf(\"t=%lld alert %s %s%s\\n\", a->tick, takt_machine_name(m), on ? \"on\" : \"off\", invalid ? \" invalid\" : \"\");"
    );
    let _ = writeln!(t.code, "}}");
    let _ = writeln!(
        t.code,
        "void {x}_log(struct {x}_arena *a, int m, int site) {{ printf(\"t=%lld log %d %d\\n\", a->tick, m, site); }}"
    );
    // 5.3: der Fault-Uebergang mit Maschine und Art, wie der Interpreter
    // ihn schreibt; der verlassene Zustand steht nicht in dessen Zeile.
    let _ = writeln!(t.code, "void {x}_fault(struct {x}_arena *a, int m, int from, int code) {{");
    let _ = writeln!(t.code, "    (void)from;");
    let _ = writeln!(
        t.code,
        "    printf(\"t=%lld fault %s %s\\n\", a->tick, takt_machine_name(m), takt_fault_name(code));"
    );
    let _ = writeln!(t.code, "}}");
    // 3.3: `now` ist die Dauer seit dem Start des Laufs — die Tickzahl
    // mal T0, wie im Interpreter. Die Runtime fuehrt sie, weil alle
    // Maschinen dieselbe Uhr lesen (12.1).
    let _ = writeln!(t.code, "long long {x}_now(struct {x}_arena *a) {{ return a->tick * {}LL; }}", p.config.tick);
    let _ =
        writeln!(t.code, "void {x}_measure(struct {x}_arena *a, int m, int site, double v, unsigned char invalid) {{");
    let _ = writeln!(t.code, "    if (invalid) printf(\"t=%lld measure %d %d <invalid>\\n\", a->tick, m, site);");
    let _ = writeln!(t.code, "    else printf(\"t=%lld measure %d %d %.17g\\n\", a->tick, m, site, v);");
    let _ = writeln!(t.code, "}}");
    let _ = writeln!(t.code, "void {x}_verify(struct {x}_arena *a, int m, int site, unsigned char ok) {{");
    let _ = writeln!(t.code, "    printf(\"t=%lld verify %d %d %d\\n\", a->tick, m, site, ok ? 1 : 0);");
    let _ = writeln!(t.code, "}}");
    // 5.4: `abort` merkt den Fault fuer alle Maschinen vor; die
    // Abort-Phase stellt ihn nach den Schritten zu (`abort_phase`).
    raised(&mut t, p, x);
    let _ = writeln!(t.code, "void {x}_abort(struct {x}_arena *a, int m, int site) {{");
    let _ = writeln!(t.code, "    printf(\"t=%lld abort %d %d\\n\", a->tick, m, site);");
    let _ = writeln!(t.code, "    memset(a->raised, 1, sizeof a->raised);");
    let _ = writeln!(t.code, "}}");
    let _ = writeln!(t.code, "void {x}_verdict(struct {x}_arena *a, int m, int site, unsigned char pass) {{");
    let _ = writeln!(t.code, "    printf(\"t=%lld verdict %d %d %d\\n\", a->tick, m, site, pass ? 1 : 0);");
    let _ = writeln!(t.code, "}}");
    // 13.3: Ein Monitor meldet Index und Position; der Vergleich bildet
    // den Namen aus dem Programm.
    let _ = writeln!(t.code, "void {x}_property(struct {x}_arena *a, int i, long long at) {{");
    let _ = writeln!(t.code, "    printf(\"t=%lld property %d %lld\\n\", a->tick, i, at);");
    let _ = writeln!(t.code, "}}\n");

    // Die Stroeme (`takt-llvm/src/stream.rs`): jeder ueber seinen Ring;
    // die Eingabestroeme speist der Treiberrand.
    takt_frame::streams::emit(&mut t, p, takt_frame::streams::Trace::Stdio, x);

    natives(&mut t.code, p);
    machine_declarations(&mut t.code, p, &driven, x);
    // 13.3: Laufzeitmonitore laufen nur, wenn der Rahmen alle Maschinen
    // fuehrt — eine Eigenschaft liest jede.
    let monitors: Vec<(usize, &takt_mir::program::Property)> = match machine {
        None => p.properties.iter().enumerate().filter(|(_, prop)| prop.monitor).collect(),
        Some(_) => Vec::new(),
    };
    for (i, _) in &monitors {
        let _ = writeln!(t.code, "void {x}_monitor_{i}(struct {x}_arena *a, long long tick);");
    }
    let _ = writeln!(t.code);
    let persisting: Vec<&takt_mir::machine::Machine> =
        driven.iter().copied().filter(|m| !m.persist.is_empty()).collect();
    if !persisting.is_empty() {
        let bound = takt_mir::persist::max_payload(p).unwrap_or(0).max(1);
        let _ = writeln!(t.code, "static unsigned char persist_out[{bound}];");
        let bytes: Vec<String> = payload.iter().map(|b| b.to_string()).collect();
        let _ = writeln!(
            t.code,
            "static const unsigned char persist_in[{}] = {{{}}};",
            payload.len().max(1),
            bytes.join(",")
        );
        let _ = writeln!(t.code, "static const int persist_in_len = {};", payload.len());
        let _ = writeln!(t.code);
    }

    // 4.5: Die Jobs des Rahmens; sie schreiben das Abbild.
    jobs(&mut t, p, x);

    // 9.8: die geplanten Schreibvorgaenge. Sie gehoeren der Runtime —
    // 11.2 nennt sie „feste Arrays im Runtime-Anteil des Outputs" —,
    // und der Rahmen ist hier die Runtime. Hinter dem Latch, weil
    // `apply_scheduled` ihn schreibt.
    // 7.5: In der Simulation sind `guard` und `jitter` null.
    scheduled(&mut t, p, &layout, None, x);
    jitter(&mut t.code, p, None, x);
    // 12.10: Registerports lesen den Latch des Modells und schreiben in
    // die Ringe der Stroeme, also hinter beidem.
    crate::ports::emit(&mut t, p, x);
    // 12.6: Der Treiberrand schreibt ins Abbild und in die Ringe; der
    // Stimulus liefert, was auf einem Board die Treiber liefern.
    let mut feed = String::new();
    let most = crate::deliveries::stimulus(&mut feed, p, &layout, inputs, x);
    takt_frame::edge::emit(&mut t, p, &layout, &driven, most, takt_frame::streams::Trace::Stdio, x);
    t.code.push_str(&feed);

    // 8.4: der Einstieg fuer Tunables, derselbe wie im Produktrahmen.
    if inputs.iter().any(|s| matches!(s, Stimulus::Tune { .. })) {
        takt_frame::parts::tune(&mut t.code, p, &layout, x);
    }

    let _ = writeln!(t.code, "int main(void) {{");
    let _ = writeln!(t.code, "    struct {x}_arena *const a = &g_arena;");
    let _ = writeln!(t.code, "    TAKT_IEEE_MODE();");
    let _ = writeln!(t.code, "    memset(a, 0, sizeof *a);");
    let _ = writeln!(t.code, "    takt_edge_init(a);");
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(t.code, "    takt_jobs_init(a);");
    }
    // 3.5: Ein Input ohne Treiber ist `Bad`. Ein genullter Abbild-Eintrag
    // hiesse `Good` (die Skala beginnt dort), und der Vergleich pruefte
    // dann einen Lauf, den es nicht gibt — der Interpreter faultet in
    // diesem Fall. Der Rahmen setzt die Qualitaet darum ausdruecklich.
    for slot in &layout.inputs {
        let Some(entry) = quality_offset(p, &slot.name) else { continue };
        let _ = writeln!(t.code, "    a->image[{entry}] = {}; /* {} ist Bad (3.5) */", 3, slot.name);
    }

    // Die Parameter stehen fuer den Lauf fest (8.4); sie werden einmal
    // gesetzt.
    for (i, slot) in layout.parameters.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = param_literal(p, i) else { continue };
        let _ = writeln!(t.code, "    *({ct} *)(a->params + {}) = {value}; /* {} */", slot.offset, slot.name);
    }

    // 9.4: Der Lauf beginnt mit den Outputs auf `safe` (5.3) — vor
    // jedem Init, wie in `Sim::new`. Danach erst bindet die Simulation,
    // damit ein `enter:`-Block schon den sicheren Wert sieht.
    safe_outputs(&mut t.code, p, &layout);
    sim_bindings(&mut t.code, p, "    ");
    crate::ports::sample(&mut t.code, p, "    ");
    // Die Lieferungen des Ticks 0 gehen vor jedem Init durch den Rand, wie
    // `Run::new` den Stimulus vor `Sim::init` einspeist.
    let _ = writeln!(t.code, "    takt_edge_stimulus(a, 0);");
    // Wie `Sim::init`: erst die Variablen aller Maschinen, dann die
    // geladenen Werte (5.9), dann die Eintritte in Schrittordnung — und
    // nach jedem `fresh[m] = publish_m(v_m)`, damit ein Follower schon im
    // Tick 0 frisch liest (7.2, 9.4).
    for m in &driven {
        let _ = writeln!(t.code, "    {x}_{0}_init_vars(a);", m.name);
    }
    for m in &persisting {
        let _ = writeln!(t.code, "    {x}_{0}_persist_restore(a, persist_in, persist_in_len);", m.name);
    }
    enter_machines(&mut t.code, p, &layout, &driven, "    ", x);
    // 8.8: Auch im Tick 0 holt der Treiber ab, was `enter` gesendet hat.
    commit_sequence(&mut t.code, p, &driven, "    ", "0");
    crate::ports::sample(&mut t.code, p, "    ");
    let _ = writeln!(t.code, "    dump(a, 0);");
    for (i, _) in &monitors {
        let _ = writeln!(t.code, "    {x}_monitor_{i}(a, 0);");
    }
    // 12.7: Auch der Anfangszustand kann das Kommando setzen.
    platform_end(&mut t.code, p, &layout, "    ");
    let _ = writeln!(t.code, "    for (a->tick = 1; a->tick <= {ticks}; a->tick++) {{");
    aging(&mut t.code, p, &layout, "        ");
    // 12.1: `sample_inputs()` und `validate_and_bound()` nach dem Altern,
    // wie `Run::tick`.
    let _ = writeln!(t.code, "        takt_edge_stimulus(a, a->tick);");
    // 4.5: Faellige Jobs werden zu Tick-Beginn sichtbar, wie `poll_jobs` im Interpreter.
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(t.code, "        takt_jobs_poll(a);");
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
        let condition = ticks_of.iter().map(|t| format!("a->tick == {t}")).collect::<Vec<_>>().join(" || ");
        let _ = writeln!(t.code, "        a->image[{slot}] = ({condition}) ? 1 : 0; /* {name} */");
    }
    // 5.4: Operator-Abort und Runtime-Faults von aussen werden vorgemerkt,
    // wie `apply_stimulus` im Interpreter.
    pended(&mut t.code, p, inputs, "        ");
    // 8.4: Ein Tunable gilt ab seiner Tick-Grenze; der Rahmen gibt ihn vor
    // dem Schritt an `takt_tune_value`, wie die Schleife des Produktrahmens
    // (`Runtime::service_with`), und `takt_tune_value` prueft Typ und Range wie
    // `apply_stimulus` im Interpreter.
    for stim in inputs {
        let Stimulus::Tune { tick, name, text } = stim else { continue };
        let Some(i) = p.params.iter().position(|q| q.name == *name) else { continue };
        let Some(bytes) = tune_bytes(p, i, text) else { continue };
        let list: Vec<String> = bytes.iter().map(u8::to_string).collect();
        let _ = writeln!(
            t.code,
            "        if (a->tick == {tick}) {{ static const unsigned char v[] = {{ {} }}; \
             (void)takt_tune_value(a, {i}u, v, {}); }} /* tune {name} */",
            list.join(", "),
            bytes.len()
        );
    }
    steps(&mut t.code, p, &layout, &driven, "        ", "a->tick", x);
    abort_phase(&mut t.code, p, &driven, "        ", "a->tick", x);
    idle_drops(&mut t.code, p, &driven, "        ", x);
    commit_sequence(&mut t.code, p, &driven, "        ", "a->tick");
    crate::ports::sample(&mut t.code, p, "        ");
    let _ = writeln!(t.code, "        dump(a, a->tick);");
    // 13.3: nach dem Commit, wie `observe_properties` im Interpreter.
    for (i, _) in &monitors {
        let _ = writeln!(t.code, "        {x}_monitor_{i}(a, a->tick);");
    }
    platform_end(&mut t.code, p, &layout, "        ");
    if sleep {
        virtual_sleep(&mut t.code, p, &layout, &driven, ticks, inputs, x);
    }
    let _ = writeln!(t.code, "    }}");
    if next_run_slot(p, &layout).is_some() {
        let _ = writeln!(t.code, "ende:");
        safe_outputs(&mut t.code, p, &layout);
        let _ = writeln!(t.code, "    dump(a, a->tick);");
    }
    if !persisting.is_empty() {
        let _ = writeln!(t.code, "    printf(\"t=%lld persist \", a->tick);");
        for m in &persisting {
            let _ = writeln!(t.code, "    {{");
            let _ = writeln!(
                t.code,
                "        int n = {x}_{0}_persist_snapshot(a, persist_out, sizeof persist_out);",
                m.name
            );
            let _ = writeln!(t.code, "        for (int i = 0; i < n; i++) printf(\"%02x\", persist_out[i]);");
            let _ = writeln!(t.code, "    }}");
        }
        let _ = writeln!(t.code, "    printf(\"\\n\");");
    }
    // Wie die Boards: Die letzte Zeile nennt, bis wohin der Lauf kam — nach
    // der Schleife `ticks + 1`, nach dem Ende eines Laufs (12.7) dessen Tick.
    let _ = writeln!(t.code, "    printf(\"takt end %lld\\n\", a->tick);");
    let _ = writeln!(t.code, "    return 0;");
    let _ = writeln!(t.code, "}}");

    // `dump` steht hinter `main`, damit die Deklaration oben genuegt.
    // T2: Eine Dauer steht in ihrer groessten ganzzahligen Einheit.
    let mut dump = String::new();
    let _ = writeln!(dump, "\nstatic void dump(struct {x}_arena *a, long long t) {{");
    for slot in &layout.outputs {
        // Ein Array als Liste, wie der Interpreter ihn schreibt (T2).
        if let takt_llvm::ty::LlvmType::Array(elem, n) = &slot.ty {
            let Some(ct) = c_type(elem, slot.signed) else { continue };
            let (fmt, cast) = number_format(elem, slot.signed);
            let _ = writeln!(dump, "    printf(\"t=%lld out {} [\", t);", slot.name);
            let _ = writeln!(
                dump,
                "    for (int k = 0; k < {n}; k++) printf(k ? \", {fmt}\" : \"{fmt}\", {cast}(({ct} *)(a->latch + {}))[k]);",
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
                "    {{ long long v = *(long long *)(a->latch + {}); \
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
            let _ = writeln!(dump, "    switch (*({ct} *)(a->latch + {})) {{", slot.offset);
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
            "    printf(\"t=%lld out {} {fmt}\\n\", t, {cast}(*({ct} *)(a->latch + {})));",
            slot.name, slot.offset
        );
    }
    let _ = writeln!(dump, "    takt_stream_report(a, t);");
    let _ = writeln!(dump, "}}");
    // Die Vorwaertsdeklaration muss vor `main` stehen.
    let at = t.code.find("int main(void)").unwrap_or(0);
    t.code.insert_str(at, &format!("static void dump(struct {x}_arena *a, long long t);\n\n"));
    t.code.push_str(&dump);

    Harness { source: t.assemble(&head, x, &arena, None), layout }
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
    let _ = writeln!(s, "{indent}switch (*({ct} *)(a->latch + {})) {{", slot.offset);
    for (d, next) in ends {
        let word = next.word();
        let _ = writeln!(s, "{indent}case {d}: printf(\"t=%lld end {word}\\n\", a->tick); goto ende;");
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
        "    {{ int d = *(int *)(a->latch + {}); long long *f = (long long *)(a->latch + {}); (void)f;",
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
/// Enums ohne Felder, Arrays und Records daraus (ein Padding `_ : [3] u8`
/// schreibt der Interpreter mit). Eine Dauer schreibt der Interpreter mit
/// Einheit; sie und alles Uebrige bleiben aussen vor.
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
        (Type::Array { elem, len }, LlvmType::Array(inner, n)) if n == len => {
            let (mut fmt, mut args) = ("[".to_string(), String::new());
            for i in 0..u64::from(*n) {
                let (ff, fa) = field_text(p, *elem, inner, at + i * inner.aligned_size())?;
                fmt.push_str(if i > 0 { ", " } else { "" });
                fmt.push_str(&ff);
                args.push_str(&fa);
            }
            fmt.push(']');
            Some((fmt, args))
        }
        (Type::Bool, LlvmType::Int(1)) => {
            Some(("%s".to_string(), format!(", *(unsigned char *)(a->latch + {at}) ? \"true\" : \"false\"")))
        }
        (Type::Enum(e), LlvmType::Int(_)) => {
            let def = p.enums.get(e.index())?;
            if def.variants.iter().any(|v| !v.fields.is_empty()) {
                return None;
            }
            let ct = c_type(llvm, true)?;
            let at_value = format!("*({ct} *)(a->latch + {at})");
            let names: String =
                def.variants.iter().map(|v| format!("{at_value} == {} ? \"{}\" : ", v.discriminant, v.name)).collect();
            Some(("%s".to_string(), format!(", ({names}\"?\")")))
        }
        (Type::Duration { .. }, LlvmType::Int(64)) => {
            let v = format!("*(long long *)(a->latch + {at})");
            Some(("%lld %s".to_string(), format!(", takt_dur_value({v}), takt_dur_unit({v})")))
        }
        (Type::Int { width, .. }, LlvmType::Int(_)) => {
            let ct = c_type(llvm, width.signed())?;
            let (fmt, cast) = number_format(llvm, width.signed());
            Some((fmt.to_string(), format!(", {cast}(*({ct} *)(a->latch + {at}))")))
        }
        (Type::Float { .. }, LlvmType::F32 | LlvmType::F64) => {
            let ct = c_type(llvm, true)?;
            let (fmt, cast) = number_format(llvm, true);
            Some((fmt.to_string(), format!(", {cast}(*({ct} *)(a->latch + {at}))")))
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
    LIB.get_or_init(|| native_library_for(None)).clone()
}

/// Dieselbe Bibliothek fuer ein Ziel (`Some(triple)`, etwa die Boards) oder
/// den Wirt (`None`), ohne Zwischenspeicher im Prozess.
pub fn native_library_for(triple: Option<&str>) -> Result<std::path::PathBuf, String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut cargo = std::process::Command::new("cargo");
    cargo.args(["rustc", "--release", "--lib", "--crate-type", "staticlib", "--features", "host,ecdsa,rsa,aes-gcm"]);
    if let Some(triple) = triple {
        cargo.args(["--target", triple]);
    }
    let out = cargo
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
            Some(m) => writeln!(s, "{indent}if (a->tick == {tick}) takt_pend(a, {m}, {code});"),
            None => writeln!(s, "{indent}if (a->tick == {tick}) {every} takt_pend(a, m, {code});"),
        };
    }
}

/// `maybe_sleep()` nach 9.9 am Ende eines Ticks: Sind alle Maschinen
/// `idle`, rueckt der Rahmen bis vor die frueheste `after`-Frist, wie
/// `Runtime::sleep` — `n = d / T0 - 1`, und der Tick an der Frist laeuft.
/// Dieselbe Annahme wie der Interpreter (`Run::earliest_deadline`, FB-429):
/// `<m>_deadline` zaehlt die Basis-Ticks ab jetzt bis zu der Aktivierung, an
/// der die Frist faellt — bei einer Periode ueber einem Tick also bis zu
/// einem Tick der Maschine —, und `<m>_advance(n)` nimmt die vollen `n`
/// Basis-Ticks und behaelt den Rest von `n / Periode`.
/// Die Zeitzeile traegt `slept`, wie auf dem Board; der Vergleich liest
/// sie nicht. Ausstehende geplante Ausgaben, ein laufender Job und ein
/// Fault, der hinter einem Abort wartet (`pending`), verbieten den
/// Schlaf; ob ein Wake-Strom
/// etwas im Fenster hat, sagt `_idle`. Ein Tick, in dem der Stimulus
/// liefert, laeuft immer: Auf dem Board weckt ein Wake-Ereignis den Kern,
/// hier kennt der Rahmen die Lieferungen im Voraus.
fn virtual_sleep(
    s: &mut String,
    p: &Program,
    layout: &takt_frame::layout::Layout,
    driven: &[&takt_mir::machine::Machine],
    ticks: u64,
    inputs: &[Stimulus],
    x: &Prefix,
) {
    let _ = writeln!(s, "        {{");
    let _ = writeln!(s, "            _Bool idle = 1;");
    if !queued_outputs(p).is_empty() {
        let _ = writeln!(s, "            idle = !takt_sched_pending(a);");
    }
    if p.machines.iter().any(|m| !m.layout.job_slots.is_empty()) {
        let _ = writeln!(s, "            idle = idle && !takt_jobs_active(a);");
    }
    let _ = writeln!(s, "            long long best = -1;");
    for m in driven {
        let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
        let _ = writeln!(s, "            idle = idle && !a->pending[{i}] && {x}_{0}_idle(a);", m.name);
    }
    for m in driven {
        let _ = writeln!(s, "            if (idle) {{");
        let _ = writeln!(s, "                long long d = {x}_{0}_deadline(a);", m.name);
        let _ = writeln!(s, "                if (d >= 0 && (best < 0 || d < best)) best = d;");
        let _ = writeln!(s, "            }}");
    }
    let _ = writeln!(s, "            long long n = idle && best > 1 ? best - 1 : 0;");
    let _ = writeln!(s, "            if (a->tick + n > {ticks}LL) n = {ticks}LL - a->tick;");
    let mut due: Vec<u64> = inputs.iter().map(Stimulus::tick).collect();
    due.sort_unstable();
    due.dedup();
    if !due.is_empty() {
        let list: Vec<String> = due.iter().map(|t| format!("{t}LL")).collect();
        let _ = writeln!(s, "            static const long long due[] = {{ {} }};", list.join(", "));
        let _ = writeln!(s, "            for (unsigned i = 0; i < sizeof due / sizeof due[0]; i++)");
        let _ = writeln!(
            s,
            "                if (due[i] > a->tick) {{ if (a->tick + n >= due[i]) n = due[i] - a->tick - 1; break; }}"
        );
    }
    let _ = writeln!(s, "            if (n > 0) {{");
    for m in driven {
        let _ = writeln!(s, "                {x}_{0}_advance(a, n);", m.name);
    }
    // Die Abtastungen altern wie in leeren Schritten (`Run::skip`).
    takt_frame::parts::aging_slept(s, p, layout, "                ");
    let _ = writeln!(s, "                printf(\"t=%lld time took=0 drift=0 slept=%lld\\n\", a->tick, n);");
    let _ = writeln!(s, "                a->tick += n;");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
}

/// Der Wert einer `tune`-Zeile in kanonischer Byteform (5.9), wie ihn
/// `takt_tune_value` nimmt; `None`, wenn der Text kein Wert des Parametertyps ist.
/// Die Range prueft `takt_tune_value` selbst, wie im Produktrahmen.
pub fn tune_bytes(p: &Program, index: usize, text: &str) -> Option<Vec<u8>> {
    let ty = p.params.get(index)?.ty;
    let v = takt_interp::trace::parse_value(text, ty, p).ok()?;
    takt_interp::bytes::encode(p, &v, ty).ok()
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
fn jobs(t: &mut Text, p: &Program, x: &Prefix) {
    let Some((slots, out_max)) = job_tables(&mut t.code, p, x) else { return };
    let in_max = takt_frame::parts::job_in_max(p);
    let _ = writeln!(
        t.types,
        "typedef struct {{ int active; long long due; int out_len; unsigned char in[{in_max}], out[{out_max}]; }} takt_job;"
    );
    let _ = writeln!(t.fields, "    takt_job jobs[{slots}];");
    let s = &mut t.code;
    let _ = writeln!(
        s,
        "unsigned char *{x}_job_args(struct {x}_arena *a, int m, int slot) {{ return a->jobs[takt_job_base[m] + slot].in; }}"
    );
    let _ = writeln!(s, "void {x}_job_begin(struct {x}_arena *a, int m, int slot, int native, int len) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; takt_job *j = &a->jobs[i];");
    job_call(s, p, "j->in", "len", "j->out", "j->out_len", "    ");
    let _ = writeln!(s, "    j->active = 1; j->due = a->tick + takt_job_ticks[i];");
    let _ = writeln!(s, "    takt_job_image(a, i, 0, 0, 2); /* Err(PENDING) */");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void {x}_job_cancel(struct {x}_arena *a, int m, int slot) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot;");
    let _ = writeln!(
        s,
        "    if (a->jobs[i].active) {{ a->jobs[i].active = 0; takt_job_image(a, i, 1, 0, 0); /* Err(CANCELLED) */ }}"
    );
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_jobs_poll(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i, b;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) {{");
    let _ = writeln!(s, "        if (!a->jobs[i].active || a->jobs[i].due > a->tick) continue;");
    let _ = writeln!(s, "        a->jobs[i].active = 0;");
    let _ = writeln!(
        s,
        "        if (a->jobs[i].out_len < 0) {{ takt_job_image(a, i, 1, 0, 1); continue; }} /* Err(FAILED) */"
    );
    let _ = writeln!(s, "        takt_job_image(a, i, 1, 1, 0);");
    let _ = writeln!(
        s,
        "        for (b = 0; b < a->jobs[i].out_len; b++) a->image[takt_job_at[i] + 8 + b] = a->jobs[i].out[b];"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_jobs_init(struct {x}_arena *a) {{ int i; for (i = 0; i < {slots}; i++) takt_job_image(a, i, 0, 0, 2); }}"
    );
    // 9.9, Konjunkt 5: Solange ein Job laeuft, schlaeft das System nicht.
    let _ = writeln!(
        s,
        "static _Bool takt_jobs_active(struct {x}_arena *a) {{ int i; for (i = 0; i < {slots}; i++) if (a->jobs[i].active) return 1; return 0; }}"
    );
    let _ = writeln!(s);
}
