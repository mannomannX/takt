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
    /// Wie viele Byte der Programmzustand in `.bss.takt_state` belegt (12.3):
    /// Abbild, Latch, Parameter, Monitore und die Zustaende der Maschinen.
    pub state_bytes: u64,
}

/// Baut den Rahmen fuer alle Maschinen eines Programms.
///
/// Anders als der Linux-Rahmen kennt dieser keine Tickzahl: Die Schleife
/// laeuft, bis das Board ausgeht.
pub fn build(p: &Program) -> McuHarness {
    build_with(p, Frame::default())
}

/// Wie der Rahmen fuer ein Board entsteht.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// Ohne Diagnosen gibt `takt_mcu_dump` nichts aus, und der Rahmen
    /// traegt kein Schattenlatch.
    pub diagnostics: takt_llvm::Diagnostics,
    /// Die Hardware-Konfiguration gibt jedem Output sein `guard` und
    /// `jitter` (7.5); ohne sie sind beide null wie in der Simulation.
    pub hardware: Option<&'a takt_mir::hardware::Hardware>,
    /// Der Programmzustand in einem eigenen Abschnitt `.takt_state`, den ein
    /// Board mit MPU ausserhalb des Ticks schreibschuetzt (12.3); sonst liegt
    /// er in `.bss`.
    pub protected: bool,
}

impl Default for Frame<'_> {
    fn default() -> Self {
        Frame { diagnostics: takt_llvm::Diagnostics::Ids, hardware: None, protected: false }
    }
}

/// Wie [`build`], fuer ein Board mit seinen Angaben ([`Frame`]).
pub fn build_with(p: &Program, frame: Frame<'_>) -> McuHarness {
    let (diagnostics, hw) = (frame.diagnostics, frame.hardware);
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
    crate::streams::emit(&mut s, p, crate::streams::Trace::Board);
    let state_bytes = storage(&mut s, p, &layout, &driven, frame.protected);
    // 9.8: die geplanten Schreibvorgaenge, hinter dem Latch, weil
    // `apply_scheduled` ihn schreibt.
    crate::harness::scheduled(&mut s, p, &layout, hw);
    crate::harness::jitter(&mut s, p, hw);
    jobs(&mut s, p);
    declarations(&mut s, p, &driven);
    // 12.6: der Treiberrand vor dem Abtasten, das ihn speist.
    crate::edge::emit(&mut s, p, &layout, &driven, deliveries(p, &layout), crate::streams::Trace::Board);
    init(&mut s, p, &layout, &driven);
    tick(&mut s, p, &layout, &driven);
    telemetry(&mut s, p, &layout, &driven, diagnostics);

    McuHarness { source: s, layout, state_bytes }
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
    // Der zuletzt ausgefuehrte Tick: Im Schlaf rueckt `g_tick` vor (9.9),
    // die Ausgaben, die der Dump danach schreibt, gehoeren aber zu ihm.
    let _ = writeln!(s, "static long long g_done = 0;");
    crate::harness::scope_flags(s, p);
    let _ = writeln!(s, "unsigned int takt_fn_fault = 0;\n");
    crate::harness::fault_names(s, p);
    crate::harness::raised(s, p);

    // 3.3: `now` ist die Dauer seit dem Start — Tickzahl mal T0.
    let _ = writeln!(s, "long long takt_now(void) {{ return g_tick * {}LL; }}\n", p.config.tick);

    // Jede Beobachtungszeile traegt ihren Tick, wie beim Interpreter
    // (`grammar/trace.md`): Ohne ihn laesst sie sich keinem Tick zuordnen.
    // 5.6: nur die Flanken, mit dem Namen der Maschine wie im Interpreter.
    crate::harness::alert_table(s, p);
    let _ = writeln!(s, "void takt_alert(int m, int slot, unsigned char on, unsigned char invalid) {{");
    let _ = writeln!(s, "    if (!takt_alert_edge(m, slot, on)) return;");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"alert \");");
    let _ = writeln!(s, "    takt_board_trace(takt_machine_name(m));");
    let _ = writeln!(s, "    takt_board_trace(on ? \" on\" : \" off\");");
    let _ = writeln!(s, "    if (invalid) takt_board_trace(\" invalid\");");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}");
    for (name, args, kind, flags) in [
        ("takt_log", "int m, int site", "log", &[][..]),
        ("takt_verify", "int m, int site, unsigned char ok", "verify", &["ok"]),
        ("takt_verdict", "int m, int site, unsigned char pass", "verdict", &["pass"]),
    ] {
        let _ = writeln!(s, "void {name}({args}) {{");
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "    takt_board_trace(\"{kind} \");");
        let _ = writeln!(s, "    takt_board_trace_i64(m);");
        let _ = writeln!(s, "    takt_board_trace_i64(site);");
        for f in flags {
            let _ = writeln!(s, "    takt_board_trace_i64({f} ? 1 : 0);");
        }
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
        let _ = writeln!(s, "}}");
    }

    // 5.4: `abort` merkt den Fault fuer alle Maschinen vor; die
    // Abort-Phase stellt ihn nach den Schritten zu.
    let _ = writeln!(s, "void takt_abort(int m, int site) {{");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"abort \");");
    let _ = writeln!(s, "    takt_board_trace_i64(m);");
    let _ = writeln!(s, "    takt_board_trace_i64(site);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "    memset(g_raised, 1, sizeof g_raised);");
    let _ = writeln!(s, "}}\n");

    // 5.3: der Fault-Uebergang mit Maschine und Art, wie der Interpreter
    // ihn schreibt.
    let _ = writeln!(s, "void takt_fault(int m, int from, int code) {{");
    let _ = writeln!(s, "    (void)from;");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"fault \");");
    let _ = writeln!(s, "    takt_board_trace(takt_machine_name(m));");
    let _ = writeln!(s, "    takt_board_trace(\" \");");
    let _ = writeln!(s, "    takt_board_trace(takt_fault_name(code));");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}\n");

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
    let _ = writeln!(s, "void takt_measure(int m, int site, double v, unsigned char invalid) {{");
    let _ = writeln!(s, "    unsigned long long bits;");
    let _ = writeln!(s, "    __builtin_memcpy(&bits, &v, sizeof bits);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"measure \");");
    let _ = writeln!(s, "    takt_board_trace_i64(m);");
    let _ = writeln!(s, "    takt_board_trace_i64(site);");
    let _ = writeln!(s, "    if (invalid) {{");
    let _ = writeln!(s, "        takt_board_trace(\"<invalid>\\n\");");
    let _ = writeln!(s, "        return;");
    let _ = writeln!(s, "    }}");
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
///
/// **Der Programmzustand steht auf Wunsch in einem eigenen Abschnitt**
/// (12.3): Ein Board mit MPU legt `.takt_state` in eine Region, die nur
/// waehrend des Ticks beschreibbar ist; ohne ihn liegt er in `.bss`.
/// Liefert, wie viele Byte er belegt.
fn storage(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    protected: bool,
) -> u64 {
    let _ = writeln!(s, "/* Statischer Zustand (12.3), ausgerichtet fuer die ABI. */");
    let section = if protected { " __attribute__((section(\".takt_state\")))" } else { "" };
    let mut total = 0u64;
    let mut buffer = |s: &mut String, name: &str, bytes: u64| {
        let bytes = bytes.max(1);
        total += bytes.div_ceil(8) * 8;
        let _ = writeln!(s, "static _Alignas(8) unsigned char {name}[{bytes}]{section};");
    };
    buffer(s, "image", layout.image);
    for (i, prop) in monitors(p) {
        let size = takt_llvm::monitor::state_size(prop, p).unwrap_or(1);
        buffer(s, &format!("monitor_{i}"), size);
    }
    buffer(s, "latch", layout.latch);
    buffer(s, "params", layout.params);
    for m in driven {
        // Die Zustandsgroesse kennt der Rahmen nicht genau; er nimmt die
        // Obergrenze aus dem Overlay (11.2). Zu gross ist verschwendeter
        // RAM, zu klein waere ein Ueberschreiben — darum grosszuegig.
        // So gross wie der Zustands-Struct des Codegens mit Ausrichtung
        // (FB-177, FB-194): Eine Schranke aus Variablen- und Zustandszahl
        // uebersah Bloecke und Puffer, und der erzeugte Code schrieb ueber
        // den Puffer hinaus — auf dem Board bis in die Stack-Wache.
        let bytes = takt_llvm::machine::state_struct(m, p).map_or(4096, |st| st.aligned_size());
        buffer(s, &format!("state_{}", m.name), bytes.max(64));
    }
    let _ = writeln!(s);
    total
}

/// Die Signaturen des erzeugten Codes (11.2).
fn declarations(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Der erzeugte Code (11.2). */");
    crate::harness::machine_declarations(s, p, driven);
    let _ = writeln!(s);
}

/// Wie viel Stack der Job-Kontext ueber den groessten `stack`-Vertrag der
/// Jobs hinaus bekommt, wenn das Board nichts anderes sagt: Rahmen und
/// Verteiler des Jobs, die Umschaltung und — wo Interrupts auf dem Stack
/// des unterbrochenen Fadens laufen — die Interrupts selbst.
// TODO(M10 Schritt 12): die Reserve aus der Hardware-Konfiguration (8.10),
// wie die des Hauptstacks.
const JOB_STACK_RESERVE: u32 = 1024;

/// 4.5: Jobs auf der MCU. Der Start kopiert die Argumente in den Slot und
/// reiht ihn ein; gerechnet wird im Job-Kontext der Runtime, einem Faden
/// mit eigenem Stack, den der Tick unterbricht (12.3). Sichtbar wird das
/// Ergebnis fruehestens nach seiner Dauer (logische Ausfuehrungszeit) —
/// wie im Modell des Interpreters, solange der Job sie haelt.
///
/// **Zwei Faeden, ein Uebergabepunkt.** Die Slots gehoeren der
/// Hauptschleife: Nur sie startet, bricht ab und macht sichtbar. Dem
/// Job-Kontext gehoert ein Auftrag (`g_work_*`): Die Hauptschleife fuellt
/// ihn, solange der Kontext ruht (`takt_mcu_job_dispatch`), der Kontext
/// rechnet ihn und meldet `g_work_finished`, die Hauptschleife holt das
/// Ergebnis ab. Ein Neustart oder Abbruch waehrend der Rechnung erhoeht
/// die Generation des Slots, und ein Ergebnis zur alten verfaellt.
///
/// Ohne Jobs bleiben die Einstiege, damit das Board sie ohne Unterschied
/// rufen kann.
///
/// **Unter dem Stack liegt der Waechter** (12.3): 32 Byte am unteren Ende,
/// zusaetzlich zu Vertrag und Reserve und an 32 Byte ausgerichtet, damit
/// eine MPU-Region oder ein NAPOT-Watchpoint ihn genau abdeckt.
fn jobs(s: &mut String, p: &Program) {
    let _ = writeln!(s, "/* Jobs (4.5): Slots der Hauptschleife, ein Auftrag fuer den Job-Kontext. */");
    let Some((slots, out_max)) = crate::harness::job_tables(s, p) else {
        let _ = writeln!(s, "int takt_mcu_job_dispatch(void) {{ return 0; }}");
        let _ = writeln!(s, "void takt_mcu_job_work(void) {{}}");
        let _ = writeln!(s, "int takt_mcu_jobs_busy(void) {{ return 0; }}");
        let _ = writeln!(s, "unsigned char *takt_mcu_job_stack(unsigned int *size) {{ *size = 0; return 0; }}\n");
        return;
    };
    // So gross wie der Puffer, den der erzeugte Code fuer die Argumente anlegt.
    let in_max = crate::harness::job_slots(p)
        .iter()
        .map(|(_, _, n)| {
            let params = &p.natives[n.index()].params;
            params.iter().map(|q| 4 + u64::from(takt_mir::bytes::max_size(p, q.ty).unwrap_or(0))).sum::<u64>()
        })
        .max()
        .unwrap_or(0)
        .max(4);
    let stack = crate::harness::job_slots(p).iter().map(|(_, _, n)| p.natives[n.index()].stack).max().unwrap_or(0);
    let names: Vec<String> = crate::harness::job_slots(p)
        .iter()
        .map(|(mi, j, _)| {
            let m = &p.machines[*mi];
            let handle = m.layout.job_slots[*j].handle;
            format!("\"{} {}\"", m.name, m.vars.get(handle.index()).map_or("?", |v| v.name.as_str()))
        })
        .collect();

    let _ = writeln!(s, "enum {{ TAKT_JOB_FREE, TAKT_JOB_WAITING, TAKT_JOB_RUNNING, TAKT_JOB_DONE }};");
    let _ = writeln!(
        s,
        "typedef struct {{ unsigned char state, gen; int native; long long due, order; int in_len, out_len; unsigned char in[{in_max}], out[{out_max}]; }} takt_mcu_job;"
    );
    let _ = writeln!(s, "static takt_mcu_job g_jobs[{slots}];");
    let _ = writeln!(s, "static long long g_job_order;");
    let _ = writeln!(s, "static const char *const takt_job_names[{slots}] = {{ {} }};", names.join(", "));
    let _ = writeln!(s, "static volatile int g_work_slot = -1, g_work_finished;");
    let _ = writeln!(s, "static unsigned char g_work_gen;");
    let _ = writeln!(s, "static int g_work_native, g_work_in_len, g_work_out_len;");
    let _ = writeln!(s, "static unsigned char g_work_in[{in_max}], g_work_out[{out_max}];");
    let _ = writeln!(s, "#ifndef TAKT_JOB_STACK_RESERVE");
    let _ = writeln!(s, "#define TAKT_JOB_STACK_RESERVE {JOB_STACK_RESERVE}");
    let _ = writeln!(s, "#endif");
    let _ = writeln!(
        s,
        "static unsigned char takt_job_stack_mem[32 + {stack} + TAKT_JOB_STACK_RESERVE] __attribute__((aligned(32)));"
    );
    let _ = writeln!(s, "unsigned char *takt_mcu_job_stack(unsigned int *size) {{");
    let _ = writeln!(s, "    *size = sizeof takt_job_stack_mem; return takt_job_stack_mem;");
    let _ = writeln!(s, "}}");

    let _ = writeln!(s, "void takt_job_begin(int m, int slot, int native, const unsigned char *args, int len) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; takt_mcu_job *j = &g_jobs[i];");
    let _ = writeln!(s, "    if (len > (int)sizeof j->in) len = (int)sizeof j->in;");
    let _ = writeln!(s, "    memcpy(j->in, args, (size_t)len); j->in_len = len; j->native = native;");
    let _ = writeln!(s, "    j->state = TAKT_JOB_WAITING; j->gen++; j->order = ++g_job_order;");
    let _ = writeln!(s, "    j->due = g_tick + takt_job_ticks[i];");
    let _ = writeln!(s, "    takt_job_image(i, 0, 0, 2); /* Err(PENDING) */");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void takt_job_cancel(int m, int slot) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot;");
    let _ = writeln!(s, "    if (g_jobs[i].state == TAKT_JOB_FREE) return;");
    let _ = writeln!(s, "    g_jobs[i].state = TAKT_JOB_FREE; g_jobs[i].gen++;");
    let _ = writeln!(s, "    takt_job_image(i, 1, 0, 0); /* Err(CANCELLED) */");
    let _ = writeln!(s, "}}");

    let _ = writeln!(s, "/* Hauptschleife: Ein fertiger Auftrag geht in seinen Slot, wenn der noch auf ihn wartet. */");
    let _ = writeln!(s, "static void takt_jobs_collect(void) {{");
    let _ = writeln!(s, "    takt_mcu_job *j;");
    let _ = writeln!(s, "    if (g_work_slot < 0 || !g_work_finished) return;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    j = &g_jobs[g_work_slot];");
    let _ = writeln!(s, "    if (j->state == TAKT_JOB_RUNNING && j->gen == g_work_gen) {{");
    // `-1` ist `Err(FAILED)` ohne Bytes; ein `size_t` daraus kopierte alles.
    let _ = writeln!(
        s,
        "        if (g_work_out_len > 0) memcpy(j->out, g_work_out, (size_t)g_work_out_len); j->out_len = g_work_out_len;"
    );
    let _ = writeln!(s, "        j->state = TAKT_JOB_DONE;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    g_work_finished = 0; g_work_slot = -1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "/* Hauptschleife: Der ruhende Kontext bekommt den aeltesten wartenden Job; wahr, wenn er zu rechnen hat. */"
    );
    let _ = writeln!(s, "int takt_mcu_job_dispatch(void) {{");
    let _ = writeln!(s, "    int i, next = -1;");
    let _ = writeln!(s, "    takt_jobs_collect();");
    let _ = writeln!(s, "    if (g_work_slot >= 0) return 1;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++)");
    let _ = writeln!(
        s,
        "        if (g_jobs[i].state == TAKT_JOB_WAITING && (next < 0 || g_jobs[i].order < g_jobs[next].order)) next = i;"
    );
    let _ = writeln!(s, "    if (next < 0) return 0;");
    let _ = writeln!(s, "    memcpy(g_work_in, g_jobs[next].in, (size_t)g_jobs[next].in_len);");
    let _ = writeln!(s, "    g_work_in_len = g_jobs[next].in_len; g_work_native = g_jobs[next].native;");
    let _ = writeln!(s, "    g_work_gen = g_jobs[next].gen; g_jobs[next].state = TAKT_JOB_RUNNING;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    g_work_finished = 0; g_work_slot = next;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "/* Job-Kontext: rechnet den Auftrag, den die Hauptschleife gegeben hat. */");
    let _ = writeln!(s, "void takt_mcu_job_work(void) {{");
    let _ = writeln!(s, "    int native;");
    let _ = writeln!(s, "    if (g_work_slot < 0 || g_work_finished) return;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    native = g_work_native;");
    let _ = writeln!(s, "    {{");
    crate::harness::job_call(s, p, "g_work_in", "g_work_in_len", "g_work_out", "g_work_out_len", "        ");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    g_work_finished = 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "/* 12.1: zu Tickbeginn. Ein fertiges Ergebnis wird sichtbar, wenn seine Dauer um ist. */");
    let _ = writeln!(s, "static void takt_jobs_poll(void) {{");
    let _ = writeln!(s, "    int i, b;");
    let _ = writeln!(s, "    takt_jobs_collect();");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) {{");
    let _ = writeln!(s, "        if (g_jobs[i].state != TAKT_JOB_DONE || g_jobs[i].due > g_tick) continue;");
    let _ = writeln!(s, "        g_jobs[i].state = TAKT_JOB_FREE;");
    let _ = writeln!(
        s,
        "        if (g_jobs[i].out_len < 0) takt_job_image(i, 1, 0, 1); /* Err(FAILED) */ else takt_job_image(i, 1, 1, 0);"
    );
    let _ = writeln!(
        s,
        "        for (b = 0; b < g_jobs[i].out_len; b++) image[takt_job_at[i] + 8 + b] = g_jobs[i].out[b];"
    );
    let _ = writeln!(s, "        takt_board_trace(\"t=\"); takt_board_trace_i64(g_tick);");
    let _ = writeln!(
        s,
        "        takt_board_trace(\" job \"); takt_board_trace(takt_job_names[i]); takt_board_trace(\" done\\n\");"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "/* 9.9: Mit einem Job, der wartet, rechnet oder noch nicht sichtbar ist, schlaeft das System nicht. */"
    );
    let _ = writeln!(s, "int takt_mcu_jobs_busy(void) {{");
    let _ = writeln!(s, "    int i;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) if (g_jobs[i].state != TAKT_JOB_FREE) return 1;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_jobs_init(void) {{");
    let _ = writeln!(s, "    int i;");
    let _ = writeln!(
        s,
        "    for (i = 0; i < {slots}; i++) {{ g_jobs[i].state = TAKT_JOB_FREE; takt_job_image(i, 0, 0, 2); }}"
    );
    let _ = writeln!(s, "    g_work_slot = -1; g_work_finished = 0;");
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_init`: einmal vor dem ersten Tick.
fn init(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Einmal vor dem ersten Tick (12.1, Schritt 1). */");
    let _ = writeln!(s, "int takt_mcu_persist_restore(const void *in, int len);");
    let _ = writeln!(s, "void takt_mcu_sample(void);");
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
    if !crate::harness::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_init();");
    }
    // 9.4: Der Lauf beginnt mit den Outputs auf `safe`, vor jedem Init —
    // wie `Sim::new` und der Wirtsrahmen. Danach bindet die Simulation,
    // damit Tick 0 die `safe`-Werte eines Modells liest (8.3).
    crate::harness::safe_outputs(s, p, layout);
    crate::harness::sim_bindings(s, p, "    ");

    // 5.9: Defaults, dann die geladenen Werte, dann erst enter: — wie
    // der Interpreter zwischen init_vars und machine::init laedt; nach
    // jedem Eintritt `publish`, damit Follower schon im Tick 0 frisch
    // lesen (7.2, 9.4).
    for m in driven {
        let _ = writeln!(s, "    {0}_init_vars(state_{0}, image, params, latch);", m.name);
    }
    let _ = writeln!(s, "    int restored = takt_mcu_persist_restore(persist, persist_len);");
    // 9.4: Auch Tick 0 beginnt mit `I_0 = sample()`; ein `enter:` des
    // Anfangszustands liest die Eingaenge wie im Interpreter (FB-316).
    let _ = writeln!(s, "    takt_mcu_sample();");
    crate::harness::enter_machines(s, p, layout, driven, "    ");
    // Was `enter` und das erste `loop:` im Tick 0 senden, wird hier
    // sichtbar (FB-269).
    crate::harness::commit_sequence(s, p, driven, "    ", "0");
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
    // 7.3: Die Schleife meldet einen Ueberlauf vor dem naechsten Tick
    // (`Program::raise_overrun`); er wirkt fuer alle Maschinen in diesem
    // Tick und steht als Zeile `runtime` im Trace, damit der Lauf sich
    // nachspielen laesst (12.5).
    let _ = writeln!(s, "static _Bool g_overrun;");
    let _ = writeln!(s, "void takt_mcu_overrun(void) {{ g_overrun = 1; }}");
    // 12.3: Der Speicherschutz hat einen Zugriff der TCB abgewiesen.
    let _ = writeln!(s, "static _Bool g_hardware;");
    let _ = writeln!(s, "void takt_mcu_hardware(void) {{ g_hardware = 1; }}");
    // 7.1, 12.6 Zeile 7: die Toleranz der Tickquelle fuer die Schleife.
    let (ns, runs) = p.config.tolerance();
    let _ = writeln!(s, "void takt_mcu_tolerance(long long *ns, unsigned *runs) {{ *ns = {ns}LL; *runs = {runs}u; }}");
    // Der Tick, den der Rahmen gerade rechnet: fuer ein Pruefgeraet, das
    // nach Tick liefert (`takt_board_support::edge_probe`).
    let _ = writeln!(s, "long long takt_mcu_current_tick(void) {{ return g_tick; }}");
    // 12.6 Zeile 6: Was der Commit an Treiberfehlern gesehen hat, wirkt im
    // naechsten Tick, wie ein Ueberlauf.
    let driven_out = driver_outputs(p, layout);
    let _ = writeln!(s, "static _Bool g_driver_fault[{}];", driven_out.len().max(1));
    let _ = writeln!(s, "void takt_mcu_tick(long long k) {{");
    let _ = writeln!(s, "    g_tick = k;");
    let _ = writeln!(s, "    g_done = k;");
    let _ = writeln!(s, "    if (g_overrun) {{");
    let _ = writeln!(s, "        g_overrun = 0;");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(k);");
    let _ = writeln!(s, "        takt_board_trace(\"runtime Overrun\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Overrun))
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if (g_hardware) {{");
    let _ = writeln!(s, "        g_hardware = 0;");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(k);");
    let _ = writeln!(s, "        takt_board_trace(\"runtime Hardware\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Hardware))
    );
    let _ = writeln!(s, "    }}");
    let driver =
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Driver));
    for (i, out) in driven_out.iter().enumerate() {
        let pend = match out.owner {
            Some(m) => format!("takt_pend({m}, {driver});"),
            None => format!("for (int m = 0; m < {}; m++) takt_pend(m, {driver});", p.machines.len()),
        };
        let _ = writeln!(s, "    if (g_driver_fault[{i}]) {{");
        let _ = writeln!(s, "        g_driver_fault[{i}] = 0;");
        let _ = writeln!(s, "        takt_board_trace(\"t=\");");
        let _ = writeln!(s, "        takt_board_trace_i64(k);");
        let _ = writeln!(s, "        takt_board_trace(\"runtime Driver {}\\n\");", out.name);
        let _ = writeln!(s, "        {pend}");
        let _ = writeln!(s, "    }}");
    }
    crate::harness::aging(s, p, layout, "    ");
    // 4.5: Was fertig und faellig ist, wird zu Tickbeginn sichtbar, wie
    // `poll_jobs` im Interpreter und im Wirtsrahmen.
    if !crate::harness::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_poll();");
    }
    let _ = writeln!(s, "    takt_mcu_sample();");
    crate::harness::steps(s, p, layout, driven, "    ", "k");
    crate::harness::abort_phase(s, p, driven, "    ", "k");
    crate::harness::idle_drops(s, p, driven, "    ");
    crate::harness::commit_sequence(s, p, driven, "    ", "k");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    takt_monitor_{i}(monitor_{i}, image, params, latch, k);");
    }
    let _ = writeln!(s, "}}\n");

    sleep(s, p.config.tick, layout, p, driven);
    platform(s, p, layout);
}

/// Wie `takt_mcu_next_run` das Ende eines Laufs meldet: die Nummer ist die
/// Stelle plus eins, 0 heisst weiter. `takt-mcu-program` liest dieselbe
/// Folge.
const NEXT_RUN_CODES: [takt_mir::sys::NextRun; 4] = [
    takt_mir::sys::NextRun::Now,
    takt_mir::sys::NextRun::After,
    takt_mir::sys::NextRun::OnWake,
    takt_mir::sys::NextRun::OnStart,
];

/// `takt_mcu_next_run` und `takt_mcu_end`: das Ende des Laufs (12.7), wie im
/// Wirtsrahmen — die Zeile `end`, dann alle Ausgaenge auf `safe`. Was
/// zwischen zwei Laeufen geschieht, fuehrt das Board aus.
fn platform(s: &mut String, p: &Program, layout: &Layout) {
    let next = crate::harness::next_run_slot(p, layout);
    let _ = writeln!(s, "/* 12.7: 0 weiter, 1 NOW, 2 AFTER (`delay` in ns), 3 ON_WAKE, 4 ON_START. */");
    let _ = writeln!(s, "int takt_mcu_next_run(long long *delay) {{");
    let _ = writeln!(s, "    *delay = -1;");
    if let Some(n) = &next {
        let _ = writeln!(s, "    switch (*({} *)(latch + {})) {{", n.ct, n.slot.offset);
        for (d, end) in &n.ends {
            let code = NEXT_RUN_CODES.iter().position(|c| c == end).unwrap_or(0) + 1;
            // `AFTER(delay)`: die Dauer im ersten Fach hinter der Diskriminante (11.2).
            let delay = match end {
                takt_mir::sys::NextRun::After => format!("*delay = *(long long *)(latch + {}); ", n.slot.offset + 8),
                _ => String::new(),
            };
            let _ = writeln!(s, "    case {d}: {delay}return {code};");
        }
        let _ = writeln!(s, "    default: break;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let words: Vec<String> = NEXT_RUN_CODES.iter().map(|c| format!("\"end {}\\n\"", c.word())).collect();
    let _ = writeln!(s, "void takt_mcu_end(void) {{");
    let _ = writeln!(s, "    static const char *const words[] = {{ {} }};", words.join(", "));
    let _ = writeln!(s, "    long long delay;");
    let _ = writeln!(s, "    int c = takt_mcu_next_run(&delay);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_done);");
    let _ = writeln!(s, "    if (c >= 1 && c <= {}) takt_board_trace(words[c - 1]);", NEXT_RUN_CODES.len());
    crate::harness::safe_outputs(s, p, layout);
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_idle` und `takt_mcu_deadline`: darf geschlafen werden (9.9)?
///
/// 9.9 nennt sechs Konjunkte. Je Maschine beantwortet der erzeugte Code
/// drei (`idle`-Zustand, keine Zustellung, leere Wake-Stroeme); ein
/// anliegendes Wake-Kommando, ausstehende geplante Ausgaben und ein
/// Fault, der hinter einem Abort wartet, prueft der Rahmen, weil ihm
/// Prozessabbild, Warteschlangen und `g_pending` gehoeren — und laufende
/// Jobs (4.5): Mit ihnen schlaeft das System nicht.
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
        if !crate::harness::queued_outputs(p).is_empty() {
            let _ = writeln!(s, "    if (takt_sched_pending()) return 0;");
        }
        let _ = writeln!(s, "    if (takt_mcu_jobs_busy()) return 0;");
        for m in driven {
            let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
            let _ = writeln!(s, "    if (g_pending[{i}]) return 0;");
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
        commit(s, p, layout);
        outputs(s, layout);
        return;
    }
    let _ = writeln!(s, "/* Die Ausgaenge als Trace-Zeilen (grammar/trace.md); ohne `all` nur die geaenderten. */");
    let _ = writeln!(s, "{}", crate::harness::DURATION_C);
    let _ = writeln!(s, "{}", crate::layout::c_buffer("g_shown", layout.latch));
    let _ = writeln!(s, "struct takt_variant;");
    let _ = writeln!(
        s,
        "struct takt_field {{ const struct takt_variant *names; unsigned short off; unsigned char kind, n_names; }};"
    );
    let _ = writeln!(
        s,
        "struct takt_variant {{ long long d; const char *name; const struct takt_field *fields; unsigned char n_fields; }};"
    );
    let _ = writeln!(
        s,
        "struct takt_out {{ const char *name; const struct takt_variant *variants; unsigned short off, size, count; unsigned char kind, n_variants; }};"
    );
    let mut rows = Vec::new();
    let mut kinds: Vec<u8> = Vec::new();
    let mut named = std::collections::BTreeSet::new();
    for (i, slot) in layout.outputs.iter().enumerate() {
        let (elem, count) = match &slot.ty {
            takt_llvm::ty::LlvmType::Array(elem, n) => (elem.as_ref(), *n),
            t => (t, 0),
        };
        let payload = payload_variants(p, &slot.name);
        // FB-312: Ein Record ist eine Variante mit seinem Namen; `takt_load`
        // liefert fuer seine Art null und trifft sie.
        let record = p
            .channels
            .iter()
            .find(|c| c.name == slot.name)
            .map(|c| match p.types.get(c.ty) {
                takt_mir::types::Type::Array { elem, .. } => *elem,
                _ => c.ty,
            })
            .and_then(|ty| record_variant(s, p, ty, elem, &mut named, &mut kinds));
        if let Some(table) = record {
            rows.push(format!(
                "    {{ \"{}\", {table}, {}, {}, {count}, {RECORD}, 1 }},",
                slot.name, slot.offset, slot.size
            ));
            continue;
        }
        let duration = p
            .channels
            .iter()
            .find(|c| c.name == slot.name)
            .is_some_and(|c| matches!(p.types.get(c.ty), takt_mir::types::Type::Duration { .. }));
        let kind = if duration { Some(DURATION) } else { value_kind(elem, slot.signed) };
        let Some(kind) = kind.or(payload.as_ref().map(|_| 4)) else { continue };
        kinds.push(kind);
        let variants = enum_variants(p, &slot.name).unwrap_or_default();
        let mut vptr = "0".to_string();
        if !variants.is_empty() {
            let mut list = Vec::with_capacity(variants.len());
            for (j, (d, name)) in variants.iter().enumerate() {
                let fields = payload.as_ref().map_or(&[][..], |v| &v[j][..]);
                let mut fptr = "0".to_string();
                if !fields.is_empty() {
                    let mut items = Vec::with_capacity(fields.len());
                    for (k, (kind, names)) in fields.iter().enumerate() {
                        kinds.push(*kind);
                        let table = match names {
                            Some((enum_name, table)) => {
                                if named.insert(enum_name.clone()) {
                                    let _ = writeln!(
                                        s,
                                        "static const struct takt_variant g_enum_{enum_name}[] = {{ {table} }};"
                                    );
                                }
                                format!("g_enum_{enum_name}")
                            }
                            None => "0".to_string(),
                        };
                        let n_names = names.as_ref().map_or(0, |(_, t)| t.matches("{ ").count());
                        // Die Nutzlast liegt in Worten zu acht Byte hinter der Diskriminante.
                        items.push(format!("{{ {table}, {}, {kind}, {n_names} }}", 8 + 8 * k));
                    }
                    let _ =
                        writeln!(s, "static const struct takt_field g_out{i}_v{j}_f[] = {{ {} }};", items.join(", "));
                    fptr = format!("g_out{i}_v{j}_f");
                }
                list.push(format!("{{ {d}LL, \"{name}\", {fptr}, {} }}", fields.len()));
            }
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
    let _ = writeln!(s, "static void takt_dump_fields(const struct takt_variant *x, const unsigned char *at);");
    let _ = writeln!(s, "static void takt_dump_field(const struct takt_field *f, const unsigned char *at) {{");
    let _ = writeln!(s, "    if (f->kind == {RECORD}) {{ takt_dump_fields(f->names, at); return; }}");
    let _ = writeln!(s, "    long long v = takt_load(f->kind, at);");
    let _ = writeln!(s, "    if (f->names) {{");
    let _ = writeln!(s, "        for (unsigned i = 0; i < f->n_names; i++)");
    let _ = writeln!(s, "            if (f->names[i].d == v) {{ takt_board_trace(f->names[i].name); return; }}");
    let _ = writeln!(s, "        takt_board_trace(\"?\");");
    let _ = writeln!(s, "    }} else if (f->kind == {BOOL}) takt_board_trace(v ? \"true\" : \"false\");");
    let _ = writeln!(s, "    else if (f->kind == 0x84) takt_board_trace_f64((double)*(const float *)at);");
    let _ = writeln!(s, "    else if (f->kind == 0x88) takt_board_trace_f64(*(const double *)at);");
    let _ = writeln!(
        s,
        "    else if (f->kind == {DURATION}) {{ takt_board_trace_i64(takt_dur_value(v)); takt_board_trace(takt_dur_unit(v)); }}"
    );
    let _ = writeln!(s, "    else if (f->kind & 0x40) takt_board_trace_u64((unsigned long long)v);");
    let _ = writeln!(s, "    else takt_board_trace_i64(v);");
    let _ = writeln!(s, "}}");
    // `Name(f1, f2)` wie `value_text` im Interpreter (9.3).
    let _ = writeln!(s, "static void takt_dump_fields(const struct takt_variant *x, const unsigned char *at) {{");
    let _ = writeln!(s, "    takt_board_trace(x->name);");
    let _ = writeln!(s, "    if (!x->n_fields) return;");
    let _ = writeln!(s, "    takt_board_trace(\"(\");");
    let _ = writeln!(s, "    for (unsigned k = 0; k < x->n_fields; k++) {{");
    let _ = writeln!(s, "        if (k) takt_board_trace(\", \");");
    let _ = writeln!(s, "        takt_dump_field(&x->fields[k], at + x->fields[k].off);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_board_trace(\")\");");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_dump_value(const struct takt_out *o, const unsigned char *v) {{");
    let _ = writeln!(s, "    if (o->variants) {{");
    let _ = writeln!(s, "        long long d = takt_load(o->kind, v);");
    let _ = writeln!(s, "        for (unsigned i = 0; i < o->n_variants; i++) {{");
    let _ = writeln!(s, "            const struct takt_variant *x = &o->variants[i];");
    let _ = writeln!(s, "            if (x->d != d) continue;");
    let _ = writeln!(s, "            takt_dump_fields(x, v);");
    let _ = writeln!(s, "            return;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        takt_board_trace(\"?\");");
    let _ = writeln!(s, "        return;");
    let _ = writeln!(s, "    }}");
    for kind in kinds.iter().filter(|k| *k & 0x80 != 0) {
        let ct = kind_c_type(*kind);
        let _ = writeln!(s, "    if (o->kind == {kind}) {{ takt_board_trace_f64((double)*(const {ct} *)v); return; }}");
    }
    if kinds.contains(&DURATION) {
        let _ = writeln!(s, "    if (o->kind == {DURATION}) {{");
        let _ = writeln!(s, "        long long d = takt_load(o->kind, v);");
        let _ = writeln!(s, "        takt_board_trace_i64(takt_dur_value(d));");
        let _ = writeln!(s, "        takt_board_trace(takt_dur_unit(d));");
        let _ = writeln!(s, "        return;");
        let _ = writeln!(s, "    }}");
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
    let _ = writeln!(s, "        takt_board_trace_i64(g_done);");
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
    let _ = writeln!(s, "    takt_stream_report(g_done);");
    let _ = writeln!(s, "}}\n");
    program_counters(s, p, driven);
    sample(s, p, layout);
    commit(s, p, layout);
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
        let _ = writeln!(s, "    takt_board_trace_i64(g_done);");
        let _ = writeln!(s, "    takt_board_trace(\"pc {} \");", m.name);
        let _ = writeln!(s, "    takt_board_trace_i64(*(int *)(state_{} + {at}));", m.name);
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    }
    let _ = writeln!(s, "}}\n");
}

/// Die Art einer Dauer: acht Byte mit Vorzeichen, geschrieben in ihrer
/// groessten ganzzahligen Einheit wie im Interpreter (T2).
const DURATION: u8 = 0x28;

/// Ein Record in der Ausgabetabelle (FB-312): Seine Felder stehen in der
/// Variante, auf die `names` zeigt. `takt_load` liefert fuer ihn null.
const RECORD: u8 = 0x10;

/// Ein Wahrheitswert als Feld: ein Byte, geschrieben als `true`/`false`
/// wie im Interpreter. Als Ausgang bleibt er `0x41` und erscheint als
/// Zahl, die `compare` gleich liest.
const BOOL: u8 = 0x11;

/// Die Art eines Werts in der Ausgabetabelle: Breite in Bytes, `0x40`
/// ohne Vorzeichen, `0x80` Fliesskomma, `0x20` Dauer.
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
        0x08 | DURATION => "long long",
        0x41 | BOOL => "unsigned char",
        0x42 => "unsigned short",
        0x44 => "unsigned int",
        0x48 => "unsigned long long",
        0x84 => "float",
        _ => "double",
    }
}

/// Die Felder je Variante eines Enum-Ausgangs mit Nutzlast: Art wie
/// [`value_kind`], bei einem Enum-Feld dazu Name und Tabelle seiner Namen.
#[allow(clippy::type_complexity)]
fn payload_variants(p: &Program, name: &str) -> Option<Vec<Vec<(u8, Option<(String, String)>)>>> {
    use takt_mir::types::Type;
    let c = p.channels.iter().find(|c| c.name == name)?;
    let Type::Enum(e) = p.types.get(c.ty) else { return None };
    let def = p.enums.get(e.index())?;
    if def.variants.iter().all(|v| v.fields.is_empty()) {
        return None;
    }
    let mut out = Vec::with_capacity(def.variants.len());
    for v in &def.variants {
        let mut fields = Vec::with_capacity(v.fields.len());
        for f in &v.fields {
            let signed = matches!(p.types.get(f.ty), Type::Int { width, .. } if width.signed());
            let kind = match p.types.get(f.ty) {
                Type::Duration { .. } => DURATION,
                Type::Bool => BOOL,
                Type::Enum(inner) => {
                    let def = p.enums.get(inner.index())?;
                    let table: Vec<String> = def
                        .variants
                        .iter()
                        .map(|w| format!("{{ {}LL, \"{}\", 0, 0 }}", w.discriminant, w.name))
                        .collect();
                    fields.push((4u8, Some((def.name.clone(), table.join(", ")))));
                    continue;
                }
                _ => value_kind(&takt_llvm::ty::lower(f.ty, p)?, signed)?,
            };
            fields.push((kind, None));
        }
        out.push(fields);
    }
    Some(out)
}

/// Die Variante eines Record-Typs fuer die Ausgabetabelle (FB-312):
/// Name, Felder an ihren Versaetzen im Struct, ein Record-Feld als Zeiger
/// auf seine eigene Variante. Derselbe Umfang wie `field_text` im
/// Wirtsrahmen; `None` fuer alles andere, dann faellt der Ausgang aus der
/// Tabelle wie dort. Jeder Record-Typ steht einmal in der Tabelle.
fn record_variant(
    s: &mut String,
    p: &Program,
    ty: takt_mir::TypeId,
    llvm: &takt_llvm::ty::LlvmType,
    named: &mut std::collections::BTreeSet<String>,
    kinds: &mut Vec<u8>,
) -> Option<String> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::types::Type;
    let Type::Record(r) = p.types.get(ty) else { return None };
    let def = p.records.get(r.index())?;
    let LlvmType::Struct(parts) = llvm else { return None };
    let symbol = format!("g_rec{}", r.index());
    if named.contains(&symbol) {
        return Some(symbol);
    }
    let mut items = Vec::with_capacity(def.fields.len());
    for (i, f) in def.fields.iter().enumerate() {
        let (part, off) = (parts.get(i)?, llvm.field_offset(i));
        let (names, kind, n_names) = match (p.types.get(f.ty), part) {
            (Type::Record(_), _) => (record_variant(s, p, f.ty, part, named, kinds)?, RECORD, 1),
            (Type::Bool, LlvmType::Int(1)) => ("0".to_string(), BOOL, 0),
            (Type::Duration { .. }, LlvmType::Int(64)) => ("0".to_string(), DURATION, 0),
            (Type::Enum(e), LlvmType::Int(_)) => {
                let edef = p.enums.get(e.index())?;
                if edef.variants.iter().any(|v| !v.fields.is_empty()) {
                    return None;
                }
                let table = format!("g_enum_{}", edef.name);
                if named.insert(edef.name.clone()) {
                    let rows: Vec<String> = edef
                        .variants
                        .iter()
                        .map(|w| format!("{{ {}LL, \"{}\", 0, 0 }}", w.discriminant, w.name))
                        .collect();
                    let _ = writeln!(s, "static const struct takt_variant {table}[] = {{ {} }};", rows.join(", "));
                }
                (table, value_kind(part, true)?, edef.variants.len())
            }
            (Type::Int { width, .. }, LlvmType::Int(_)) => ("0".to_string(), value_kind(part, width.signed())?, 0),
            (Type::Float { .. }, LlvmType::F32 | LlvmType::F64) => ("0".to_string(), value_kind(part, true)?, 0),
            _ => return None,
        };
        if kind != RECORD {
            kinds.push(kind);
        }
        items.push(format!("{{ {names}, {off}, {kind}, {n_names} }}"));
    }
    let _ = writeln!(s, "static const struct takt_field {symbol}_f[] = {{ {} }};", items.join(", "));
    let _ = writeln!(
        s,
        "static const struct takt_variant {symbol}[] = {{ {{ 0LL, \"{}\", {symbol}_f, {} }} }};",
        def.name,
        items.len()
    );
    named.insert(symbol.clone());
    Some(symbol)
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
/// Eingaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf, und
/// ebenso keiner, den ein `sim`-Output derselben Adresse speist: Im
/// Sim-Build ist das Modell seine Quelle, und ein Treiber, der zu
/// Tickbeginn laese, ueberschriebe es (8.3).
fn sample(s: &mut String, p: &Program, layout: &Layout) {
    let tick = p.config.tick;
    let scalars = bound_scalars(p, layout);
    let streams = bound_streams(p);
    let _ =
        writeln!(s, "/* Die Treiber, die das Board liest (8.10, 12.1); Zeitstempel in ns seit dem Start (12.6). */");
    if !streams.is_empty() {
        let _ =
            writeln!(s, "/* Ein Strom liefert je Aufruf ein Element; ein laengeres als `cap` kuerzt der Treiber. */");
    }
    for b in &scalars {
        let _ = writeln!(
            s,
            "_Bool {}({} *value, unsigned char *quality, long long *t); /* {} */",
            b.function, b.ct, b.name
        );
    }
    for b in &streams {
        let _ = writeln!(
            s,
            "_Bool {}(unsigned char *buf, int cap, int *len, long long *t, long long *seq); /* {} */",
            b.function, b.name
        );
    }
    if scalars.is_empty() && streams.is_empty() {
        let _ = writeln!(s, "/*   keine — kein Eingang ist an Hardware gebunden */");
    }
    for b in &scalars {
        let _ = writeln!(
            s,
            "__attribute__((weak)) _Bool {}({} *value, unsigned char *quality, long long *t)",
            b.function, b.ct
        );
        let _ = writeln!(s, "{{ (void)value; (void)quality; (void)t; return 0; }}");
    }
    for b in &streams {
        let _ = writeln!(
            s,
            "__attribute__((weak)) _Bool {}(unsigned char *buf, int cap, int *len, long long *t, long long *seq)",
            b.function
        );
        let _ = writeln!(s, "{{ (void)buf; (void)cap; (void)len; (void)t; (void)seq; return 0; }}");
    }
    // Ein Platz je Element, das ein Strom in einem Tick liefern darf, und
    // eines mehr, damit der Rand `MAXPT` pruefen kann (12.6 Zeile 2).
    let pool: u64 = streams.iter().map(|b| u64::from(b.polls) * u64::from(b.cap)).sum();
    let _ = writeln!(s, "static _Alignas(8) unsigned char g_edge_pool[{}];", pool.max(1));

    record(s, p);

    let _ = writeln!(s, "\n/* Schritt 2: die Lieferungen an den Rand, der Rand ins Abbild (12.1, 12.6). */");
    let _ = writeln!(s, "void takt_mcu_sample(void) {{");
    let _ = writeln!(s, "    long long now = g_tick * {tick}LL;");
    for b in &scalars {
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        {} v = 0;", b.ct);
        let _ = writeln!(s, "        unsigned char q = 0;");
        let _ = writeln!(s, "        long long t = now;");
        let _ = writeln!(s, "        if ({}(&v, &q, &t))", b.function);
        // `Bad` kommt ohne Wert (12.6 Zeile 2); sein Grund ist der Treiber.
        let _ = writeln!(
            s,
            "            takt_edge_reading({}, &v, (int)sizeof v, {}, q, q == 3 ? 3 : 0, q != 3, t, 0LL);",
            b.channel, b.number
        );
        let _ = writeln!(s, "    }}");
    }
    let mut off = 0u64;
    for b in &streams {
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        long long last = g_edge_tracks[{}].last_seq;", b.channel);
        let _ = writeln!(s, "        long long next = last == (-9223372036854775807LL - 1) ? 0 : last + 1;");
        let _ = writeln!(s, "        for (int i = 0; i < {}; i++) {{", b.polls);
        let _ = writeln!(s, "            unsigned char *buf = g_edge_pool + {off} + i * {};", b.cap);
        let _ = writeln!(s, "            int len = 0;");
        let _ = writeln!(s, "            long long t = now, seq = next;");
        let _ = writeln!(s, "            if (!{}(buf, {}, &len, &t, &seq)) break;", b.function, b.cap);
        let _ = writeln!(s, "            if (len < 0) len = 0;");
        let _ = writeln!(s, "            if (len > {0}) len = {0};", b.cap);
        let _ = writeln!(s, "            next = seq + 1;");
        let _ = writeln!(s, "            takt_edge_element({}, buf, len, t, seq);", b.channel);
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
        off += u64::from(b.polls) * u64::from(b.cap);
    }
    if !p.recorded.is_empty() {
        let _ = writeln!(s, "    takt_mcu_record(now);");
    }
    let _ = writeln!(s, "    takt_edge_commit(g_tick);");
    let _ = writeln!(s, "}}\n");
}

/// Die importierten Inputs, die das Programm nicht liest (8.2): je Tick
/// ueber ihren Treiber gelesen und als Metazeile `rec` aufgezeichnet, in der
/// Form einer `in`-Zeile — ein Skalar in Tick 0 und bei jeder Aenderung
/// (T4), ein Strom je Element. Die Qualitaet steht wie am Rand: `Bad` mit
/// Grund `Driver`; eine Diskriminante, die das Enum nicht kennt, ist `Bad`
/// mit Grund `OutOfRange` (12.6 Zeile 3).
fn record(s: &mut String, p: &Program) {
    if p.recorded.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n/* Aufgezeichnete Inputs, die das Programm nicht liest (8.2, 12.5). */");
    for (i, r) in p.recorded.iter().enumerate() {
        let ident = r.address.ident();
        if let Some(st) = r.stream {
            let f = format!("takt_poll_{ident}");
            let _ = writeln!(
                s,
                "_Bool {f}(unsigned char *buf, int cap, int *len, long long *t, long long *seq); /* rec {} */",
                r.name
            );
            let _ = writeln!(
                s,
                "__attribute__((weak)) _Bool {f}(unsigned char *buf, int cap, int *len, long long *t, long long *seq)"
            );
            let _ = writeln!(s, "{{ (void)buf; (void)cap; (void)len; (void)t; (void)seq; return 0; }}");
            // Ein Byte mehr als jedes gueltige Element, wie bei einem gebundenen Strom.
            let _ = writeln!(s, "static _Alignas(8) unsigned char g_rec_{i}[{}];", st.bytes + 1);
            continue;
        }
        let (f, ct) = (format!("takt_in_{ident}"), recorded_c_type(r.value));
        let _ = writeln!(s, "_Bool {f}({ct} *value, unsigned char *quality, long long *t); /* rec {} */", r.name);
        let _ = writeln!(s, "__attribute__((weak)) _Bool {f}({ct} *value, unsigned char *quality, long long *t)");
        let _ = writeln!(s, "{{ (void)value; (void)quality; (void)t; return 0; }}");
    }
    let _ = writeln!(s, "static void takt_mcu_record(long long now) {{");
    for (i, r) in p.recorded.iter().enumerate() {
        if let Some(st) = r.stream {
            record_stream(s, p, i, r, st);
            continue;
        }
        let (f, ct) = (format!("takt_in_{}", r.address.ident()), recorded_c_type(r.value));
        let _ = writeln!(s, "    {{ /* {} */", r.name);
        let _ = writeln!(s, "        static {ct} last;");
        let _ = writeln!(s, "        static unsigned char last_q, seen;");
        let _ = writeln!(s, "        {ct} v = 0;");
        let _ = writeln!(s, "        unsigned char q = 0;");
        let _ = writeln!(s, "        long long t = now;");
        let _ = writeln!(
            s,
            "        if ({f}(&v, &q, &t) && (!seen || memcmp(&v, &last, sizeof v) != 0 || q != last_q || t != now)) {{"
        );
        let _ = writeln!(s, "            seen = 1; last = v; last_q = q;");
        let _ = writeln!(s, "            takt_board_trace(\"t=\");");
        let _ = writeln!(s, "            takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "            takt_board_trace(\"rec {} \");", r.name);
        let _ = writeln!(s, "            if (q == 1) takt_board_trace(\"suspect \");");
        let _ = writeln!(s, "            else if (q == 2) takt_board_trace(\"stale \");");
        let _ = writeln!(s, "            else if (q == 3) takt_board_trace(\"bad reason=Driver \");");
        let _ = writeln!(s, "            else {}", recorded_value(p, r));
        let _ = writeln!(s, "            if (t != now) {{ takt_board_trace(\"t=\"); takt_board_trace_i64(t); }}");
        let _ = writeln!(s, "            takt_board_trace(\"\\n\");");
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}");
}

/// Ein aufgezeichneter Strom: je Tick bis zu `MAXPT + 1` Elemente, jedes
/// eine Zeile `rec` — ein `u8` als Zahl, jedes andere Element in seiner
/// Drahtform `0x…`; `t=` und `seq=` nur, wo sie von der Tickgrenze und der
/// lueckenlosen Folge abweichen (`grammar/trace.md`). Ein `u8`-Element
/// anderer Laenge kann keine `in`-Zeile tragen und wird nicht aufgezeichnet.
fn record_stream(
    s: &mut String,
    p: &Program,
    i: usize,
    r: &takt_mir::program::Recorded,
    st: takt_mir::program::RecordedStream,
) {
    let polls = takt_hal::edge::maxpt(st.max_rate_hz, p.config.tick).unwrap_or(1).saturating_add(1);
    let cap = st.bytes + 1;
    let f = format!("takt_poll_{}", r.address.ident());
    let _ = writeln!(s, "    {{ /* {} */", r.name);
    let _ = writeln!(s, "        static long long next;");
    let _ = writeln!(s, "        for (int i = 0; i < {polls}; i++) {{");
    let _ = writeln!(s, "            int len = 0;");
    let _ = writeln!(s, "            long long t = now, seq = next, expected = next;");
    let _ = writeln!(s, "            if (!{f}(g_rec_{i}, {cap}, &len, &t, &seq)) break;");
    let _ = writeln!(s, "            if (len < 0) len = 0;");
    let _ = writeln!(s, "            if (len > {cap}) len = {cap};");
    let _ = writeln!(s, "            next = seq + 1;");
    let wire = r.value == takt_mir::program::RecordedValue::Wire;
    if !wire {
        let _ = writeln!(s, "            if (len != 1) continue;");
    }
    let _ = writeln!(s, "            takt_board_trace(\"t=\");");
    let _ = writeln!(s, "            takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "            takt_board_trace(\"rec {} \");", r.name);
    if wire {
        // Zwei Ziffern je Byte und Aufruf: `takt_board_trace_hex8` schreibt
        // je Byte ein `0x`, und eine Zeile am Stueck kann laenger sein, als
        // `takt_board_trace` liest.
        let _ = writeln!(s, "            static const char digits[] = \"0123456789abcdef\";");
        let _ = writeln!(s, "            takt_board_trace(\"0x\");");
        let _ = writeln!(s, "            for (int b = 0; b < len; b++) {{");
        let _ =
            writeln!(s, "                char d[3] = {{ digits[g_rec_{i}[b] >> 4], digits[g_rec_{i}[b] & 15], 0 }};");
        let _ = writeln!(s, "                takt_board_trace(d);");
        let _ = writeln!(s, "            }}");
        let _ = writeln!(s, "            takt_board_trace(\" \");");
    } else {
        let _ = writeln!(s, "            takt_board_trace_i64((long long)g_rec_{i}[0]);");
    }
    let _ = writeln!(s, "            if (t != now) {{ takt_board_trace(\"t=\"); takt_board_trace_i64(t); }}");
    let _ =
        writeln!(s, "            if (seq != expected) {{ takt_board_trace(\"seq=\"); takt_board_trace_i64(seq); }}");
    let _ = writeln!(s, "            takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
}

/// Der C-Typ, in dem der Treiber eines aufgezeichneten Inputs liefert —
/// derselbe wie fuer einen gebundenen dieses Typs.
fn recorded_c_type(value: takt_mir::program::RecordedValue) -> &'static str {
    use takt_mir::program::RecordedValue;
    use takt_mir::types::FloatWidth;
    match value {
        RecordedValue::Bool => "unsigned char",
        RecordedValue::Int(w) => c_type(&takt_llvm::ty::LlvmType::Int(w.bits()), w.signed()).unwrap_or("long long"),
        RecordedValue::Float(FloatWidth::F32) => "float",
        RecordedValue::Float(FloatWidth::F64) => "double",
        RecordedValue::Enum(_) => "unsigned int",
        RecordedValue::Wire => "unsigned char",
    }
}

/// Die Anweisung, die den Wert `v` eines aufgezeichneten Inputs in die
/// Zeile `rec` schreibt, in der Literalform der Sprache (`grammar/trace.md` T2).
fn recorded_value(p: &Program, r: &takt_mir::program::Recorded) -> String {
    use takt_mir::program::RecordedValue;
    match r.value {
        RecordedValue::Bool => "takt_board_trace(v ? \"true \" : \"false \");".to_string(),
        RecordedValue::Int(w) if !w.signed() && w.bits() == 64 => {
            "takt_board_trace_u64((unsigned long long)v);".to_string()
        }
        RecordedValue::Int(_) => "takt_board_trace_i64((long long)v);".to_string(),
        RecordedValue::Float(_) => match &r.unit {
            Some(unit) => format!("{{ takt_board_trace_f64((double)v); takt_board_trace(\"{unit} \"); }}"),
            None => "takt_board_trace_f64((double)v);".to_string(),
        },
        RecordedValue::Enum(id) => {
            let cases: String = p.enums[id.index()]
                .variants
                .iter()
                .map(|x| format!(" case {}: takt_board_trace(\"{} \"); break;", x.discriminant, x.name))
                .collect();
            format!("switch (v) {{{cases} default: takt_board_trace(\"bad reason=OutOfRange \"); }}")
        }
        RecordedValue::Wire => String::new(),
    }
}

/// Ein Skalar, den ein Treiber des Boards liefert.
struct BoundScalar {
    name: String,
    channel: usize,
    function: String,
    ct: &'static str,
    /// Art, `as_i64` und `as_f64` des Werts `v` fuer den Rand.
    number: String,
}

/// Die an Hardware gebundenen Skalare, ohne die, die ein `sim`-Output
/// derselben Adresse speist (8.3).
fn bound_scalars(p: &Program, layout: &Layout) -> Vec<BoundScalar> {
    use takt_mir::types::Type;
    let fed = crate::harness::sim_fed_inputs(p);
    layout
        .inputs
        .iter()
        .filter_map(|slot| {
            let channel = p.channels.iter().position(|c| c.name == slot.name)?;
            if fed.contains(&channel) {
                return None;
            }
            let function = format!("takt_in_{}", slot.address.as_ref()?.ident());
            let ct = c_type(&slot.ty, slot.signed)?;
            let number = match p.types.list.get(p.channels[channel].ty.index())? {
                Type::Int { width, .. } if !width.signed() && width.bits() == 64 => {
                    "v <= 9223372036854775807ULL ? 1 : 2, (long long)v, (double)v".to_string()
                }
                Type::Int { .. } | Type::Duration { .. } => "1, (long long)v, (double)v".to_string(),
                Type::Float { .. } => "2, 0LL, (double)v".to_string(),
                _ => "0, 0LL, 0.0".to_string(),
            };
            Some(BoundScalar { name: slot.name.clone(), channel, function, ct, number })
        })
        .collect()
}

/// Ein Eingabestrom, dessen Elemente ein Treiber des Boards liefert.
struct BoundStream {
    name: String,
    channel: usize,
    function: String,
    /// Die Bytes eines Platzes: eines mehr als jedes gueltige Element, damit
    /// ein ueberlanges, das der Treiber auf `cap` kuerzt, als `malformed`
    /// zaehlt (12.6 Zeile 5) und nicht als gueltiges gekuerztes.
    cap: u32,
    /// `MAXPT + 1`: so oft fragt der Rahmen je Tick.
    polls: u32,
}

/// Die an Hardware gebundenen Eingabestroeme, ohne die gekoppelten (8.3).
fn bound_streams(p: &Program) -> Vec<BoundStream> {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    p.channels
        .iter()
        .enumerate()
        .filter_map(|(channel, c)| {
            let Some(Type::Stream(elem)) = p.types.list.get(c.ty.index()) else { return None };
            let Binding::Hw(addr) = &c.binding else { return None };
            if c.dir != Direction::Input || crate::streams::coupled_input(p, channel) {
                return None;
            }
            Some(BoundStream {
                name: c.name.clone(),
                channel,
                function: format!("takt_poll_{}", addr.ident()),
                cap: crate::streams::payload_cap(p, *elem).saturating_add(1),
                polls: takt_hal::edge::maxpt_of(c, p.config.tick).unwrap_or(1).saturating_add(1),
            })
        })
        .collect()
}

/// Wie viele Lieferungen ein Tick hoechstens bringt: je Skalar eine, je
/// Strom `MAXPT + 1`.
fn deliveries(p: &Program, layout: &Layout) -> usize {
    bound_scalars(p, layout).len() + bound_streams(p).iter().map(|b| b.polls as usize).sum::<usize>()
}

/// Ein Ausgang an einem Treiber des Boards (12.6 Zeile 6).
struct DriverOutput {
    name: String,
    /// Der Besitzer; ohne ihn trifft der Fault jede Maschine.
    owner: Option<usize>,
    /// Das Geraet, dessen Heartbeat zaehlt.
    device: String,
}

/// Die an Hardware gebundenen Ausgaenge, Skalare in der Reihenfolge des
/// Latch, dann die Ausgabestroeme.
fn driver_outputs(p: &Program, layout: &Layout) -> Vec<DriverOutput> {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    let of = |c: &takt_mir::program::Channel| DriverOutput {
        name: c.name.clone(),
        owner: c.owner.map(|m| m.index()),
        device: takt_hal::edge::driver_of(c),
    };
    let scalars =
        layout.outputs.iter().filter(|slot| slot.address.is_some() && c_type(&slot.ty, slot.signed).is_some());
    let mut out: Vec<DriverOutput> =
        scalars.filter_map(|slot| p.channels.iter().find(|c| c.name == slot.name)).map(of).collect();
    out.extend(
        p.channels
            .iter()
            .filter(|c| c.dir == Direction::Output && matches!(c.binding, Binding::Hw(_)))
            .filter(|c| matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
            .map(of),
    );
    out
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
fn commit(s: &mut String, p: &Program, layout: &Layout) {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    let bound: Vec<(&crate::layout::Slot, String)> = layout
        .outputs
        .iter()
        .filter(|slot| c_type(&slot.ty, slot.signed).is_some())
        .filter_map(|slot| slot.address.as_ref().map(|a| (slot, format!("takt_out_{}", a.ident()))))
        .collect();
    let streams: Vec<(&takt_mir::program::Channel, String)> = p
        .channels
        .iter()
        .filter(|c| c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
        .filter_map(|c| match &c.binding {
            Binding::Hw(a) => Some((c, format!("takt_free_{}", a.ident()))),
            _ => None,
        })
        .collect();
    let outputs = driver_outputs(p, layout);
    let mut devices: Vec<&str> = outputs.iter().map(|o| o.device.as_str()).collect();
    devices.sort_unstable();
    devices.dedup();
    let alive = |d: &str| format!("takt_alive_{}", takt_mir::pattern::Address::simple(d).ident());

    let _ = writeln!(s, "/* Die Treiber, die das Board stellt (8.10, 12.1); sie bestaetigen (12.6 Zeile 6). */");
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "_Bool {fname}({ct} value); /* {} */", slot.name);
    }
    for (c, fname) in &streams {
        let _ = writeln!(s, "int {fname}(void); /* freier Platz von {} (8.8) */", c.name);
    }
    for d in &devices {
        let _ = writeln!(s, "_Bool {}(void); /* Heartbeat (12.4) */", alive(d));
    }
    if bound.is_empty() && streams.is_empty() {
        let _ = writeln!(s, "/*   keine — kein Ausgang ist an Hardware gebunden */");
    }
    // Schwach gebunden: Ein Ausgang ohne Treiber am Board geht ins Leere und
    // gilt als bestaetigt, ein Geraet ohne Heartbeat als lebendig, ein
    // Sendepuffer ohne Auskunft als unbekannt — so laeuft jedes Programm des
    // Korpus, und ein Board ueberschreibt nur, was es verdrahtet hat.
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "__attribute__((weak)) _Bool {fname}({ct} value) {{ (void)value; return 1; }}");
    }
    for (_, fname) in &streams {
        let _ = writeln!(s, "__attribute__((weak)) int {fname}(void) {{ return -1; }}");
    }
    for d in &devices {
        let _ = writeln!(s, "__attribute__((weak)) _Bool {}(void) {{ return 1; }}", alive(d));
    }
    let _ = writeln!(s, "_Bool takt_edge_output(_Bool confirmed, _Bool alive, int free, int capacity);");

    let _ = writeln!(
        s,
        "\n/* Schritt 10: der Latch geht an die Geraete (12.1); was scheitert, faultet im naechsten Tick. */"
    );
    let _ = writeln!(s, "void takt_mcu_commit(void) {{");
    for d in &devices {
        let _ = writeln!(s, "    _Bool alive_{} = {}();", takt_mir::pattern::Address::simple(d).ident(), alive(d));
    }
    let index = |name: &str| outputs.iter().position(|o| o.name == name);
    for (slot, fname) in &bound {
        let (Some(ct), Some(i)) = (c_type(&slot.ty, slot.signed), index(&slot.name)) else { continue };
        let device = takt_mir::pattern::Address::simple(&outputs[i].device).ident();
        let _ = writeln!(
            s,
            "    if (takt_edge_output({fname}(*({ct} *)(latch + {})), alive_{device}, -1, -1)) g_driver_fault[{i}] = 1;",
            slot.offset
        );
    }
    for (c, fname) in &streams {
        let Some(i) = index(&c.name) else { continue };
        let device = takt_mir::pattern::Address::simple(&outputs[i].device).ident();
        let cap = c.attrs.capacity_bytes.map_or(-1, i64::from);
        let _ = writeln!(s, "    if (takt_edge_output(1, alive_{device}, {fname}(), {cap})) g_driver_fault[{i}] = 1;");
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
