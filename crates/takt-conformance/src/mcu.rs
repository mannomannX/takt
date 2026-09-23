//! Der Rahmen, der ein Takt-Programm auf einer MCU ausfuehrt (12.1, 12.3).
//!
//! **Dieselbe Konstruktion wie [`crate::harness`], ein anderes Ziel.** Der
//! erzeugte Code spricht die C-ABI, und der Workspace verbietet
//! `extern "C"` (13.4) — also erzeugt auch dieser Rahmen C. Was ihn
//! unterscheidet, sind drei Dinge, und jedes folgt aus 12.3:
//!
//! 1. **Keine `stdio`.** Auf der MCU gibt es keine libc; die Telemetrie
//!    geht ueber USART, und das Board stellt die Funktion.
//! 2. **Kein `main`.** Die Tickschleife liegt in `takt-rt-baremetal`; der
//!    Rahmen liefert ihr `takt_mcu_init` und `takt_mcu_tick` und nichts
//!    weiter.
//! 3. **Statischer Speicher.** Prozessabbild, Latch und Maschinenzustaende
//!    stehen in `.bss` (12.3: „Gesamter Zustand statisch; kein Heap").
//!
//! **Was der Rahmen nicht ist.** Er ist keine Runtime: Er hat keine Uhr
//! (die kommt vom Timer), keine Treiber (die kommen vom Board) und keine
//! Fault-Behandlung ueber die Abort-Phase hinaus. Er ist das Bindeglied
//! zwischen erzeugtem Code und Tickschleife — die Stelle, an der ein
//! Takt-Programm auf einer MCU zum ersten Mal laeuft.

use std::fmt::Write as _;

use takt_mir::program::Program;

use crate::layout::{Layout, c_type};

/// Der erzeugte MCU-Rahmen.
pub struct McuHarness {
    /// Der C-Quelltext.
    pub source: String,
    /// Die Speicherform, die er erwartet.
    pub layout: Layout,
}

/// Baut den Rahmen fuer alle Maschinen eines Programms.
///
/// Anders als der Linux-Rahmen kennt dieser keine Tickzahl: Die Schleife
/// laeuft, bis das Board ausgeht.
pub fn build(p: &Program) -> McuHarness {
    build_with(p, takt_llvm::Diagnostics::Ids)
}

/// Wie [`build`], mit Diagnosestufe: ohne sie gibt `takt_mcu_dump` nichts
/// aus und der Rahmen traegt kein Schattenlatch.
pub fn build_with(p: &Program, diagnostics: takt_llvm::Diagnostics) -> McuHarness {
    let layout = crate::layout::of(p);
    // 7.2: in Schrittordnung, wie der Interpreter und der Testrahmen.
    let driven: Vec<&takt_mir::machine::Machine> = takt_mir::analysis::schedule::order(p)
        .unwrap_or_else(|_| takt_mir::analysis::schedule::runnable(p))
        .into_iter()
        .map(|id| &p.machines[id.index()])
        .collect();

    let mut s = String::new();
    prologue(&mut s, p);
    runtime_abi(&mut s, p);
    crate::harness::natives(&mut s, p);
    // Die Stroeme wie im Linux-Rahmen, ohne Stimulus; ihre Trace-Zeilen
    // gehen an das Board.
    crate::streams::emit(&mut s, p, &[], crate::streams::Trace::Board);
    storage(&mut s, p, &layout, &driven);
    declarations(&mut s, &driven);
    init(&mut s, p, &layout, &driven);
    tick(&mut s, p, &layout, &driven);
    telemetry(&mut s, p, &layout, &driven, diagnostics);

    McuHarness { source: s, layout }
}

/// Kopf und Vorwaertsdeklarationen.
fn prologue(s: &mut String, p: &Program) {
    let _ = writeln!(s, "/* MCU-Rahmen (12.1, 12.3); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "/* Tick: {} ns. Kein Heap, keine libc. */\n", p.config.tick);
    // Nur die Typen, nicht die Funktionen: `stdint.h` ist Teil der
    // freistehenden Umgebung und steht auch ohne libc zur Verfuegung.
    let _ = writeln!(s, "#include <stdint.h>");
    let _ = writeln!(s, "#include <stddef.h>\n");
    // Ohne libc: die vier Speicherroutinen, die Stroeme und `map` brauchen,
    // liefert `compiler_builtins` des Rust-Binaries; hier nur ihre Namen.
    let _ = writeln!(s, "void *memcpy(void *, const void *, size_t);");
    let _ = writeln!(s, "void *memmove(void *, const void *, size_t);");
    let _ = writeln!(s, "void *memset(void *, int, size_t);");
    let _ = writeln!(s, "int memcmp(const void *, const void *, size_t);\n");
    // Das Board stellt sie bereit; der Rahmen ruft sie nur.
    let _ = writeln!(s, "void takt_board_trace(const char *line);");
    let _ = writeln!(s, "void takt_board_trace_i64(long long value);");
    let _ = writeln!(s, "void takt_board_trace_u64(unsigned long long value);");
    let _ = writeln!(s, "void takt_board_trace_f64(double value);");
    let _ = writeln!(s, "void takt_board_trace_hex8(unsigned char value);\n");
}

/// Die Laufzeitmonitore (13.3): alle Eigenschaften mit `monitor`, weil der
/// Rahmen alle Maschinen fuehrt — wie der Linux-Rahmen ohne `--machine`.
fn monitors(p: &Program) -> Vec<(usize, &takt_mir::program::Property)> {
    p.properties.iter().enumerate().filter(|(_, prop)| prop.monitor).collect()
}

/// Die Runtime-Aufrufe aus `takt-llvm/src/abi.rs`.
///
/// **Sie schreiben in die Telemetrie, nicht auf `stdout`.** Was der
/// Linux-Rahmen mit `printf` macht, macht dieser ueber das Board — und
/// weil UART langsam ist, bleibt die Ausgabe knapp: Ereignisart, Maschine,
/// Stelle. Der Vergleich mit dem Interpreter braucht nicht mehr.
fn runtime_abi(s: &mut String, p: &Program) {
    let _ = writeln!(s, "static long long g_tick = 0;");
    let _ = writeln!(s, "unsigned char takt_fn_fault = 0;\n");

    // 3.3: `now` ist die Dauer seit dem Start — Tickzahl mal T0.
    let _ = writeln!(s, "long long takt_now(void) {{ return g_tick * {}LL; }}\n", p.config.tick);

    // Jede Beobachtungszeile traegt ihren Tick, wie beim Interpreter
    // (`grammar/trace.md`): Ohne ihn laesst sie sich keinem Tick zuordnen.
    for (name, args, kind, flag) in [
        ("takt_alert", "int m, int site, unsigned char on", "alert", Some("on")),
        ("takt_log", "int m, int site", "log", None),
        ("takt_fault", "int m, int site", "fault", None),
        ("takt_abort", "int m, int site", "abort", None),
        ("takt_verify", "int m, int site, unsigned char ok", "verify", Some("ok")),
        ("takt_verdict", "int m, int site, unsigned char pass", "verdict", Some("pass")),
    ] {
        let _ = writeln!(s, "void {name}({args}) {{");
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "    takt_board_trace(\"{kind} \");");
        let _ = writeln!(s, "    takt_board_trace_i64(m);");
        let _ = writeln!(s, "    takt_board_trace_i64(site);");
        if let Some(f) = flag {
            let _ = writeln!(s, "    takt_board_trace_i64({f} ? 1 : 0);");
        }
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
        let _ = writeln!(s, "}}");
    }

    // **Das Bitmuster, nicht der gerechnete Wert.** Eine erste Fassung gab
    // `(long long)(v * 1000000.0)` aus — Mikroeinheiten, weil es ohne
    // `printf` kein `%g` gibt. Das kostete auf einem Kern ohne f64-Hardware
    // die ganze Software-Emulation: `__muldf3`, `u64_div_rem` und
    // `__aeabi_d2lz`, zusammen 1778 Byte, und das in einem Binary von
    // 4866 Byte — fuer eine Funktion, die das Programm nie rief (FB-143).
    //
    // Die Bits kosten nichts und sagen mehr: 4.2 verlangt bitgleiche
    // Ergebnisse ueber alle Targets, und `same_number` vergleicht
    // Fliesskomma ohnehin bitweise (9.4.4). Eine Multiplikation waere eine
    // zweite Rundungsquelle vor genau diesem Vergleich.
    //
    // `memcpy` statt eines Zeiger-Casts: Ein `*(long long *)&v` waere ein
    // Verstoss gegen die Aliasing-Regeln von C, und ein Compiler darf ihn
    // wegoptimieren. Fuer acht Byte erzeugt jeder Compiler daraus einen
    // Registertausch.
    let _ = writeln!(s, "void takt_measure(int m, int site, double v) {{");
    let _ = writeln!(s, "    unsigned long long bits;");
    let _ = writeln!(s, "    __builtin_memcpy(&bits, &v, sizeof bits);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"measure \");");
    let _ = writeln!(s, "    takt_board_trace_i64(m);");
    let _ = writeln!(s, "    takt_board_trace_i64(site);");
    let _ = writeln!(s, "    takt_board_trace(\"bits \");");
    let _ = writeln!(s, "    takt_board_trace_i64((long long)bits);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}\n");

    // 13.3: Ein Monitor meldet Index und Position, wie im Linux-Rahmen.
    let _ = writeln!(s, "void takt_property(int i, long long at) {{");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"property \");");
    let _ = writeln!(s, "    takt_board_trace_i64(i);");
    let _ = writeln!(s, "    takt_board_trace_i64(at);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}\n");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "void takt_monitor_{i}(void *st, void *in, void *par, void *out, long long tick);");
    }
}

/// Prozessabbild, Latch und Maschinenzustaende — alles statisch.
///
/// 12.3: „Gesamter Zustand statisch in `.bss`; kein Heap." Die Groessen
/// kommen aus `takt size` (11.5), also aus derselben Rechnung, die der
/// Compiler gegen das Speicherbudget haelt.
fn storage(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Statischer Zustand (12.3), ausgerichtet fuer die ABI. */");
    let _ = writeln!(s, "{}", crate::layout::c_buffer("image", layout.image));
    for (i, prop) in monitors(p) {
        let size = takt_llvm::monitor::state_size(prop, p).unwrap_or(1);
        let _ = writeln!(s, "{}", crate::layout::c_buffer(&format!("monitor_{i}"), size));
    }
    let _ = writeln!(s, "{}", crate::layout::c_buffer("latch", layout.latch));
    let _ = writeln!(s, "{}", crate::layout::c_buffer("params", layout.params));
    for m in driven {
        // Die Zustandsgroesse kennt der Rahmen nicht genau; er nimmt die
        // Obergrenze aus dem Overlay (11.2). Zu gross ist verschwendeter
        // RAM, zu klein waere ein Ueberschreiben — darum grosszuegig.
        // So gross wie der Zustands-Struct des Codegens mit Ausrichtung
        // (FB-177, FB-194): Eine Schranke aus Variablen- und Zustandszahl
        // uebersah Bloecke und Puffer, und der erzeugte Code schrieb ueber
        // den Puffer hinaus — auf dem Board bis in die Stack-Wache.
        let bytes = takt_llvm::machine::state_struct(m, p).map_or(4096, |st| st.aligned_size());
        let _ = writeln!(s, "{}", crate::layout::c_buffer(&format!("state_{}", m.name), bytes.max(64)));
    }
    let _ = writeln!(s);
}

/// Die Signaturen des erzeugten Codes (11.2).
fn declarations(s: &mut String, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Der erzeugte Code (11.2). */");
    for m in driven {
        let _ = writeln!(s, "void {}_init(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_step(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_publish(void *st, void *in);", m.name);
        let _ = writeln!(s, "void {}_init_vars(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_enter(void *st, void *in, void *par, void *out);", m.name);
        if !m.persist.is_empty() {
            let _ = writeln!(s, "int {}_persist_snapshot(void *st, void *out, int cap);", m.name);
            let _ = writeln!(s, "int {}_persist_restore(void *st, const void *in, int len);", m.name);
        }
        let _ = writeln!(s, "_Bool {}_idle(void *st);", m.name);
        let _ = writeln!(s, "long long {}_deadline(void *st);", m.name);
        let _ = writeln!(s, "void {}_advance(void *st, long long n);", m.name);
    }
    let _ = writeln!(s);
}

/// `takt_mcu_init`: einmal vor dem ersten Tick.
fn init(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Einmal vor dem ersten Tick (12.1, Schritt 1). */");
    let _ = writeln!(s, "int takt_mcu_persist_restore(const void *in, int len);");
    let _ = writeln!(s, "int takt_mcu_init_with(const void *persist, int persist_len) {{");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof image; i++) image[i] = 0;");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof latch; i++) latch[i] = 0;");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof params; i++) params[i] = 0;");
    for m in driven {
        let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof state_{0}; i++) state_{0}[i] = 0;", m.name);
    }

    // 3.5: Ein Input ohne Treiber ist `Bad`. Ein genullter Eintrag hiesse
    // `Good`, und das waere eine Zusage, die kein Treiber gegeben hat.
    for slot in &layout.inputs {
        if let Some(entry) = crate::harness::quality_offset(p, &slot.name) {
            let _ = writeln!(s, "    image[{entry}] = 3; /* {} ist Bad (3.5) */", slot.name);
        }
    }

    // Die Parameter stehen fuer den Lauf fest (8.4).
    for (i, slot) in layout.parameters.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = crate::harness::param_literal(p, i) else { continue };
        let _ = writeln!(s, "    *({ct} *)(params + {}) = {value}; /* {} */", slot.offset, slot.name);
    }

    // 5.9: Defaults, dann die geladenen Werte, dann erst enter: — wie
    // der Interpreter zwischen init_vars und machine::init laedt; nach
    // jedem Eintritt `publish`, damit Follower schon im Tick 0 frisch
    // lesen (7.2, 9.4).
    for m in driven {
        let _ = writeln!(s, "    {0}_init_vars(state_{0}, image, params, latch);", m.name);
    }
    let _ = writeln!(s, "    int restored = takt_mcu_persist_restore(persist, persist_len);");
    for m in driven {
        let _ = writeln!(s, "    {0}_enter(state_{0}, image, params, latch);", m.name);
        let _ = writeln!(s, "    {0}_publish(state_{0}, image);", m.name);
    }
    crate::harness::psi_commit(s, p, driven, "    ");
    let _ = writeln!(s, "    takt_tx_commit(0);");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    takt_monitor_{i}(monitor_{i}, image, params, latch, 0);");
    }
    let _ = writeln!(s, "    return restored;");
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_tick`: ein Tick, von der Schleife gerufen.
fn tick(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Ein Tick (12.1, Schritte 2 bis 10). */");
    let _ = writeln!(s, "void takt_mcu_sample(void);");
    let _ = writeln!(s, "void takt_mcu_tick(long long k) {{");
    let _ = writeln!(s, "    g_tick = k;");
    let _ = writeln!(s, "    takt_fn_fault = 0;");
    crate::harness::aging(s, p, layout, "    ");
    let _ = writeln!(s, "    takt_mcu_sample();");
    for m in driven {
        // Die Periode: Eine Maschine mit `n_m > 1` laeuft nur jeden
        // n-ten Tick (7.2, Zaehler-Scheduling); danach `publish` (9.4).
        let condition = if m.period > 1 { format!("if (k % {} == 0) ", m.period) } else { String::new() };
        let _ = writeln!(
            s,
            "    {condition}{{ {0}_step(state_{0}, image, params, latch); {0}_publish(state_{0}, image); }}",
            m.name
        );
    }
    crate::harness::psi_commit(s, p, driven, "    ");
    // Dieselbe Folge wie im Linux-Rahmen: `sim`-Outputs an ihre `hw`-Inputs
    // (8.3), dann die Sendepuffer und die internen Ringe, dann die Monitore.
    crate::harness::sim_bindings(s, p, "    ");
    let _ = writeln!(s, "    takt_tx_commit(k);");
    let _ = writeln!(s, "    takt_int_commit();");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    takt_monitor_{i}(monitor_{i}, image, params, latch, k);");
    }
    let _ = writeln!(s, "}}\n");

    sleep(s, p.config.tick, layout, p, driven);
}

/// `takt_mcu_idle` und `takt_mcu_deadline`: darf geschlafen werden (9.9)?
///
/// 9.9 nennt sechs Konjunkte. Je Maschine beantwortet der erzeugte Code
/// zwei (`idle`-Zustand, kein `pending`); ein anliegendes Wake-Kommando
/// prueft der Rahmen, weil er das Prozessabbild besitzt. Die uebrigen drei
/// sind auf der MCU gegenstandslos: Es gibt dort keine geplanten Ausgaben,
/// keine Jobs und keine Stroeme — also auch keine Wake-Fenster.
fn sleep(s: &mut String, tick: i64, layout: &Layout, p: &Program, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Systemschlaf (9.9). */");
    let _ = writeln!(s, "_Bool takt_mcu_idle(void) {{");
    if driven.is_empty() {
        let _ = writeln!(s, "    return 0;");
    } else {
        // Ein anliegendes Wake-Kommando beendet den Schlaf, bevor er
        // beginnt — sonst schliefe das System darueber hinweg.
        for slot in &layout.commands {
            if p.commands.iter().any(|c| c.name == slot.name && c.wake) {
                let _ = writeln!(s, "    if (image[{}]) return 0; /* {} weckt */", slot.offset, slot.name);
            }
        }
        for m in driven {
            let _ = writeln!(s, "    if (!{0}_idle(state_{0})) return 0;", m.name);
        }
        let _ = writeln!(s, "    return 1;");
    }
    let _ = writeln!(s, "}}\n");

    // Die frueheste Frist ueber alle Maschinen, als absoluter Zeitpunkt in
    // Nanosekunden — so erwartet `Program::next_deadline` sie. Die
    // Maschinen rechnen in Ticks, weil `t_in_state` sie zaehlt; die
    // Umrechnung steht hier, wo `takt_now` ohnehin die Zeitquelle ist.
    let _ = writeln!(s, "long long takt_mcu_deadline(void) {{");
    let _ = writeln!(s, "    long long best = -1;");
    for m in driven {
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "        long long d = {0}_deadline(state_{0});", m.name);
        let _ = writeln!(s, "        if (d >= 0 && (best < 0 || d < best)) best = d;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    if (best < 0) return -1;");
    let _ = writeln!(s, "    return takt_now() + best * {tick}LL;");
    let _ = writeln!(s, "}}\n");

    // 9.9: „fuer jede Maschine: time_in_state += n*T0". Ein
    // uebersprungener Tick ruft kein `_step`; ohne das feuerte jede
    // `after`-Frist um die geschlafenen Ticks zu spaet.
    let _ = writeln!(s, "void takt_mcu_init(void) {{ (void)takt_mcu_init_with(0, 0); }}\n");
    let _ = writeln!(s, "void takt_mcu_advance(long long n) {{");
    let _ = writeln!(s, "    g_tick += n;");
    for m in driven {
        let _ = writeln!(s, "    {0}_advance(state_{0}, n);", m.name);
    }
    let _ = writeln!(s, "}}\n");

    // 5.9: Der Board-Treiber sieht nur Bytes. Snapshot reiht die Nutzlast
    // aller Maschinen, Restore verteilt sie; die Rueckgabe zaehlt die
    // uebernommenen Eintraege, der Rest ist PersistReset.
    let persisting: Vec<&takt_mir::machine::Machine> =
        driven.iter().copied().filter(|m| !m.persist.is_empty()).collect();
    let _ = writeln!(s, "int takt_mcu_persist_snapshot(void *out, int cap) {{");
    let _ = writeln!(s, "    int n = 0;");
    for m in &persisting {
        let _ = writeln!(s, "    {{");
        let _ =
            writeln!(s, "        int k = {0}_persist_snapshot(state_{0}, (unsigned char *)out + n, cap - n);", m.name);
        let _ = writeln!(s, "        if (k == 0) return 0;");
        let _ = writeln!(s, "        n += k;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    (void)out; (void)cap;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");
    let _ = writeln!(s, "int takt_mcu_persist_restore(const void *in, int len) {{");
    let _ = writeln!(s, "    int n = 0;");
    for m in &persisting {
        let _ = writeln!(s, "    n += {0}_persist_restore(state_{0}, in, len);", m.name);
    }
    let _ = writeln!(s, "    (void)in; (void)len;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");
    let entries: usize = persisting.iter().map(|m| m.persist.len()).sum();
    let _ = writeln!(s, "const int takt_mcu_persist_entries = {entries};");
    let _ = writeln!(s, "const int takt_mcu_persist_bound = {};\n", takt_mir::persist::max_payload(p).unwrap_or(0));
}

/// `takt_mcu_dump`: den Latch ausgeben, fuer den Vergleich — dieselben
/// Zeilen wie `dump` im Linux-Rahmen (grammar/trace.md), damit
/// `compare` beide lesen kann. Ohne `all` nur, was sich seit der letzten
/// Ausgabe geaendert hat (9.3). Eine Tabelle je Ausgang statt Code je
/// Ausgang: So war die Funktion die groesste des Rahmens.
fn telemetry(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    diagnostics: takt_llvm::Diagnostics,
) {
    if diagnostics == takt_llvm::Diagnostics::None {
        let _ = writeln!(s, "void takt_mcu_dump(int all) {{ (void)all; }}\n");
        program_counters(s, p, driven);
        sample(s, p, layout);
        commit(s, layout);
        outputs(s, layout);
        return;
    }
    let _ = writeln!(s, "/* Die Ausgaenge als Trace-Zeilen (grammar/trace.md); ohne `all` nur die geaenderten. */");
    let _ = writeln!(s, "{}", crate::layout::c_buffer("g_shown", layout.latch));
    let _ = writeln!(s, "struct takt_variant {{ long long d; const char *name; }};");
    let _ = writeln!(
        s,
        "struct takt_out {{ const char *name; const struct takt_variant *variants; unsigned short off, size, count; unsigned char kind, n_variants; }};"
    );
    let mut rows = Vec::new();
    let mut kinds: Vec<u8> = Vec::new();
    for (i, slot) in layout.outputs.iter().enumerate() {
        let (elem, count) = match &slot.ty {
            takt_llvm::ty::LlvmType::Array(elem, n) => (elem.as_ref(), *n),
            t => (t, 0),
        };
        let Some(kind) = value_kind(elem, slot.signed) else { continue };
        kinds.push(kind);
        let variants = enum_variants(p, &slot.name).unwrap_or_default();
        let mut vptr = "0".to_string();
        if !variants.is_empty() {
            let list: Vec<String> = variants.iter().map(|(d, name)| format!("{{ {d}LL, \"{name}\" }}")).collect();
            let _ = writeln!(s, "static const struct takt_variant g_out{i}_v[] = {{ {} }};", list.join(", "));
            vptr = format!("g_out{i}_v");
        }
        rows.push(format!(
            "    {{ \"{}\", {vptr}, {}, {}, {count}, {kind}, {} }},",
            slot.name,
            slot.offset,
            slot.size,
            variants.len()
        ));
    }
    kinds.sort_unstable();
    kinds.dedup();
    let n = rows.len();
    if rows.is_empty() {
        rows.push("    { \"\", 0, 0, 0, 0, 0, 0 },".to_string());
    }
    let _ = writeln!(s, "static const struct takt_out g_outs[] = {{\n{}\n}};", rows.join("\n"));
    let _ = writeln!(s, "static int takt_same(const unsigned char *a, const unsigned char *b, unsigned n) {{");
    let _ = writeln!(s, "    while (n--) if (*a++ != *b++) return 0;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static long long takt_load(unsigned char kind, const unsigned char *v) {{");
    let _ = writeln!(s, "    switch (kind) {{");
    for kind in kinds.iter().filter(|k| *k & 0x80 == 0) {
        let ct = kind_c_type(*kind);
        let _ = writeln!(s, "    case {kind}: return (long long)*(const {ct} *)v;");
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_dump_value(const struct takt_out *o, const unsigned char *v) {{");
    let _ = writeln!(s, "    if (o->variants) {{");
    let _ = writeln!(s, "        long long d = takt_load(o->kind, v);");
    let _ = writeln!(s, "        for (unsigned i = 0; i < o->n_variants; i++)");
    let _ = writeln!(s, "            if (o->variants[i].d == d) {{ takt_board_trace(o->variants[i].name); return; }}");
    let _ = writeln!(s, "        takt_board_trace(\"?\");");
    let _ = writeln!(s, "        return;");
    let _ = writeln!(s, "    }}");
    for kind in kinds.iter().filter(|k| *k & 0x80 != 0) {
        let ct = kind_c_type(*kind);
        let _ = writeln!(s, "    if (o->kind == {kind}) {{ takt_board_trace_f64((double)*(const {ct} *)v); return; }}");
    }
    let _ = writeln!(s, "    if (o->kind & 0x40) takt_board_trace_u64((unsigned long long)takt_load(o->kind, v));");
    let _ = writeln!(s, "    else takt_board_trace_i64(takt_load(o->kind, v));");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_mcu_dump(int all) {{");
    let _ = writeln!(s, "    for (unsigned i = 0; i < {n}; i++) {{");
    let _ = writeln!(s, "        const struct takt_out *o = &g_outs[i];");
    let _ = writeln!(s, "        const unsigned char *v = latch + o->off;");
    let _ = writeln!(s, "        if (!all && takt_same(v, g_shown + o->off, o->size)) continue;");
    let _ = writeln!(s, "        memcpy(g_shown + o->off, v, o->size);");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "        takt_board_trace(\"out \");");
    let _ = writeln!(s, "        takt_board_trace(o->name);");
    let _ = writeln!(s, "        if (o->count) {{");
    let _ = writeln!(s, "            unsigned w = o->size / o->count;");
    let _ = writeln!(s, "            takt_board_trace(\" [\");");
    let _ = writeln!(s, "            for (unsigned k = 0; k < o->count; k++) {{");
    let _ = writeln!(s, "                if (k) takt_board_trace(\", \");");
    let _ = writeln!(s, "                takt_dump_value(o, v + k * w);");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "            takt_board_trace(\"]\\n\");");
    let _ = writeln!(s, "        }} else {{");
    let _ = writeln!(s, "            takt_board_trace(\" \");");
    let _ = writeln!(s, "            takt_dump_value(o, v);");
    let _ = writeln!(s, "            takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
    program_counters(s, p, driven);
    sample(s, p, layout);
    commit(s, layout);
    outputs(s, layout);
}

/// `takt_mcu_pc`: wo jede Maschine steht (11.2, Instrumentierung
/// `statements`). Ohne sie bleibt die Funktion leer.
fn program_counters(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Der Programmzaehler je Maschine (11.2). */");
    let _ = writeln!(s, "void takt_mcu_pc(void) {{");
    for m in driven {
        let Some(at) =
            takt_llvm::machine::state_struct(m, p).and_then(|st| st.byte_offset(takt_llvm::machine::Role::Pc, 0))
        else {
            continue;
        };
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "    takt_board_trace(\"pc {} \");", m.name);
        let _ = writeln!(s, "    takt_board_trace_i64(*(int *)(state_{} + {at}));", m.name);
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    }
    let _ = writeln!(s, "}}\n");
}

/// Die Art eines Werts in der Ausgabetabelle: Breite in Bytes, `0x40`
/// ohne Vorzeichen, `0x80` Fliesskomma.
fn value_kind(ty: &takt_llvm::ty::LlvmType, signed: bool) -> Option<u8> {
    use takt_llvm::ty::LlvmType;
    Some(match (ty, signed) {
        (LlvmType::Int(1), _) => 0x41,
        (LlvmType::Int(bits), true) => u8::try_from(bits / 8).ok()?,
        (LlvmType::Int(bits), false) => 0x40 | u8::try_from(bits / 8).ok()?,
        (LlvmType::F32, _) => 0x84,
        (LlvmType::F64, _) => 0x88,
        _ => return None,
    })
}

/// Der C-Typ zu einer Art.
fn kind_c_type(kind: u8) -> &'static str {
    match kind {
        0x01 => "signed char",
        0x02 => "short",
        0x04 => "int",
        0x08 => "long long",
        0x41 => "unsigned char",
        0x42 => "unsigned short",
        0x44 => "unsigned int",
        0x48 => "unsigned long long",
        0x84 => "float",
        _ => "double",
    }
}

/// Die Varianten eines Enum-Ausgangs mit ihren Diskriminanten.
///
/// Dieselbe Abfrage wie im Linux-Rahmen: Beide muessen den Namen
/// schreiben, den der Interpreter schreibt (9.3), sonst vergliche der
/// Exit-Test Schreibweisen statt Werte.
fn enum_variants(p: &Program, name: &str) -> Option<Vec<(i64, String)>> {
    let c = p.channels.iter().find(|c| c.name == name)?;
    let takt_mir::types::Type::Enum(e) = p.types.list.get(c.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    Some(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect())
}

/// `takt_mcu_sample`: die Treiber an das Abbild (12.1, Schritt 2).
///
/// **Das Gegenstueck zu [`commit`], und aus demselben Grund ein Symbol.**
/// Aus `hw("ui/button")` wird `takt_in_ui_button(&value, &quality)`; wer
/// den Kanal bindet und keinen Treiber stellt, bekommt einen Linkfehler
/// mit dem Namen darin. Eine Registrierung zur Laufzeit waere flexibler,
/// aber ein nicht eingetragener Eingang fiele still aus — und ein
/// Eingang, der still `Bad` bleibt, ist schlimmer als einer, der fehlt.
///
/// **Warum die Qualitaet mitkommt.** 12.6 laesst Eingaenge degradieren
/// statt zu faulten: Ein Treiber, der nichts Frisches hat, sagt `Stale`,
/// einer mit unplausiblem Wert `Suspect`. Ohne diesen Rueckweg koennte er
/// nur luegen oder schweigen. Die schwache Voreinstellung antwortet
/// nicht und laesst den Eintrag, wie `init` ihn gesetzt hat: `Bad` (3.5).
///
/// Eingaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf; sie
/// stellt das Modell im selben Tick (8.3).
fn sample(s: &mut String, p: &Program, layout: &Layout) {
    let bound: Vec<(&crate::layout::Slot, String, u64)> = layout
        .inputs
        .iter()
        .filter_map(|slot| {
            let name = slot.address.as_ref().map(|a| format!("takt_in_{}", a.ident()))?;
            Some((slot, name, crate::harness::quality_offset(p, &slot.name)?))
        })
        .collect();

    let _ = writeln!(s, "/* Die Treiber, die das Board liest (8.10, 12.1). */");
    for (slot, fname, _) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "_Bool {fname}({ct} *value, unsigned char *quality); /* {} */", slot.name);
    }
    if bound.is_empty() {
        let _ = writeln!(s, "/*   keine — kein Eingang ist an Hardware gebunden */");
    }
    for (slot, fname, _) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "__attribute__((weak)) _Bool {fname}({ct} *value, unsigned char *quality)");
        let _ = writeln!(s, "{{ (void)value; (void)quality; return 0; }}");
    }

    let _ = writeln!(s, "\n/* Schritt 2: die Geraete gehen in das Abbild (12.1). */");
    let _ = writeln!(s, "void takt_mcu_sample(void) {{");
    for (slot, fname, quality) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "        {ct} v = *({ct} *)(image + {});", slot.offset);
        let _ = writeln!(s, "        unsigned char q = 0;");
        let _ = writeln!(s, "        if ({fname}(&v, &q)) {{");
        let _ = writeln!(s, "            *({ct} *)(image + {}) = v;", slot.offset);
        let _ = writeln!(s, "            image[{quality}] = q;");
        if let Some(age) = crate::harness::age_offset(p, &slot.name) {
            let _ = writeln!(s, "            *(long long *)(image + {age}) = 0;");
        }
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_commit`: den Latch an die Treiber geben (12.1).
///
/// **Der Pfad aus `@ hw(...)` wird zum Symbolnamen.** 8.10 verlangt, dass
/// die Abbildung auf Geraete ausserhalb des Programms steht — „gehoeren in
/// die Hardware-Konfiguration, nicht in die Steuerlogik" —, und nennt den
/// Pfad einen symbolischen Verweis dorthin. Der Rahmen loest ihn nicht
/// auf: Er erzeugt aus `hw("ui/led")` einen Aufruf von
/// `takt_out_ui_led(...)` und ueberlaesst die Peripherie dem, der sie
/// besitzt (9.5 fuehrt Treiber in der TCB, den Rahmen nicht).
///
/// **Warum ein Symbol und keine Tabelle.** Eine Registrierung zur Laufzeit
/// waere flexibler, aber ein nicht eingetragener Ausgang fiele still aus —
/// dieselbe Fehlerklasse, die FB-137 und FB-139 gekostet haben. Als Symbol
/// prueft der Linker die Vollstaendigkeit: Wer einen Ausgang bindet und
/// keinen Treiber stellt, bekommt einen Linkfehler mit dem Namen darin,
/// und zwar bevor etwas laeuft.
///
/// Ausgaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf: Zu
/// ihnen gehoert kein Geraet. Der Latch bleibt trotzdem lesbar, dafuer ist
/// [`outputs`] da.
fn commit(s: &mut String, layout: &Layout) {
    let bound: Vec<(&crate::layout::Slot, String)> = layout
        .outputs
        .iter()
        .filter_map(|slot| slot.address.as_ref().map(|a| (slot, format!("takt_out_{}", a.ident()))))
        .collect();

    let _ = writeln!(s, "/* Die Treiber, die das Board stellt (8.10, 12.1). */");
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "void {fname}({ct} value); /* {} */", slot.name);
    }
    if bound.is_empty() {
        let _ = writeln!(s, "/*   keine — kein Ausgang ist an Hardware gebunden */");
    }
    // Schwach gebunden: Ein Ausgang ohne Treiber am Board geht ins Leere,
    // der Trace zeigt ihn trotzdem — so laeuft jedes Programm des Korpus,
    // und ein Board ueberschreibt nur, was es verdrahtet hat.
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "__attribute__((weak)) void {fname}({ct} value) {{ (void)value; }}");
    }

    let _ = writeln!(s, "\n/* Schritt 10: der Latch geht an die Geraete (12.1). */");
    let _ = writeln!(s, "void takt_mcu_commit(void) {{");
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    {fname}(*({ct} *)(latch + {}));", slot.offset);
    }
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_output`: einen Ausgang lesen, nach Stellung.
///
/// **Fuer Diagnose, nicht fuer Treiber.** Das Stellen macht
/// [`commit`] ueber benannte Symbole; diese Funktion ist der Weg, einen
/// Latch-Wert anzusehen, ohne ihn zu stellen — der Bring-up nutzt sie,
/// bevor ein Treiber existiert, und ein Testrahmen, der den Latch prueft.
///
/// Der Index ist die Stellung in `layout.outputs`; die Zuordnung steht im
/// Kopf des erzeugten Textes, damit sie nachlesbar ist.
fn outputs(s: &mut String, layout: &Layout) {
    let _ = writeln!(s, "/* Die Ausgaenge nach Stellung, fuer Diagnose. */");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let _ = writeln!(s, "/*   {i} = {} */", slot.name);
    }
    let _ = writeln!(s, "long long takt_mcu_output(int index) {{");
    let _ = writeln!(s, "    switch (index) {{");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    case {i}: return (long long)*({ct} *)(latch + {});", slot.offset);
    }
    // Ein unbekannter Index ist kein Absturz: Der Aufrufer bekommt eine
    // Null und der Lauf geht weiter. 4.1 verlangt Totalitaet, und ein
    // Treiber, der nach einem entfallenen Ausgang fragt, ist ein
    // Uebersetzungsfehler — keiner, der zur Laufzeit stehen bleiben darf.
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}
