//! Der Rahmen, der ein Takt-Programm auf einer MCU ausfuehrt (12.1, 12.3).
//!
//! **Ein Rahmen fuer jedes Ziel** (M11 Schritt 9). Der erzeugte Code
//! spricht die C-ABI, und der Workspace verbietet `extern "C"` (13.4) —
//! also ist der Rahmen C, aus den Bausteinen in [`crate::parts`]. Boards,
//! C- und Rust-Wirte binden ihn, und das Differential faehrt ihn auf dem
//! Wirt mit dem Kern des SDK (`takt_conformance::testhost`). Drei Dinge
//! folgen dabei aus 12.3:
//!
//! 1. **Keine `stdio`.** Auf der MCU gibt es keine libc; die Telemetrie
//!    geht ueber USART, und das Board stellt die Funktion.
//! 2. **Kein `main`.** Die Tickschleife liegt in `takt-rt-baremetal`; der
//!    Rahmen liefert ihr `P_init` und `P_tick` und nichts
//!    weiter.
//! 3. **Die Arena des Wirts.** Der ganze veraenderliche Zustand steht in
//!    einer Arena `struct P_arena` (12.11), die der Wirt anlegt und jedem
//!    Einstieg reicht — statisch, in `.takt_state` oder wo er will (12.3:
//!    „Gesamter Zustand statisch; kein Heap"). Ihren Typ und die Einstiege
//!    nennt der Kopf ([`McuHarness::header`]).
//!
//! **Was der Rahmen nicht ist.** Er ist keine Runtime: Er hat keine Uhr
//! (die kommt vom Timer), keine Treiber (die kommen vom Board) und keine
//! Fault-Behandlung ueber die Abort-Phase hinaus. Er ist das Bindeglied
//! zwischen erzeugtem Code und Tickschleife — die Stelle, an der ein
//! Takt-Programm auf einer MCU zum ersten Mal laeuft.

use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};

use takt_llvm::symbols::Prefix;

use takt_mir::program::Program;

use crate::layout::{Layout, c_type};
use crate::text::Text;

/// Der erzeugte MCU-Rahmen.
pub struct McuHarness {
    /// Der C-Quelltext.
    pub source: String,
    /// Der Kopf fuer den Wirt (`P.h`): die Arena `struct P_arena` mit ihren
    /// Typen, die Einstiege, die Treiber und die Leitung des Boards.
    pub header: String,
    /// Die Speicherform, die er erwartet.
    pub layout: Layout,
    /// Wo in der Arena der Tick steht: als erstes Feld der Runtime, direkt
    /// hinter dem Programmbereich. Ein Pruefgeraet liest ihn ueber den
    /// Debugport, wenn die Konsole schweigt.
    pub tick_at: u64,
    /// Wo in der Arena die Senke des Trace steht (12.11): ein Slot der
    /// Runtime direkt hinter Tick und letztem Tick, also nie im
    /// Programmbereich, den Schutz und Journal decken. `init` setzt ihn.
    pub sink_at: u64,
    /// So viele Bytes braucht der Stack des Job-Kontexts ([`job_stack_bytes`]).
    pub job_stack_bytes: u64,
}

/// Der Abstand der Senke vom Tick: `tick` und `done`, je acht Byte.
const SINK_AFTER_TICK: u64 = 16;

/// Baut den Rahmen fuer alle Maschinen eines Programms.
///
/// Anders als der Linux-Rahmen kennt dieser keine Tickzahl: Die Schleife
/// laeuft, bis das Board ausgeht.
pub fn build(p: &Program) -> McuHarness {
    build_with(p, Frame::default())
}

/// Wie der Rahmen fuer ein Board entsteht.
#[derive(Clone, Debug)]
pub struct Frame<'a> {
    /// Ohne Diagnosen gibt `P_dump` nichts aus, und der Rahmen
    /// traegt kein Schattenlatch.
    pub diagnostics: takt_llvm::Diagnostics,
    /// Die Hardware-Konfiguration gibt jedem Output sein `guard` und
    /// `jitter` (7.5); ohne sie sind beide null wie in der Simulation.
    pub hardware: Option<&'a takt_mir::hardware::Hardware>,
    /// Der Programmbereich der Arena, auf diese Groesse aufgefuellt: So
    /// deckt die Region einer MPU, die ihn ausserhalb des Ticks
    /// schreibschuetzt, genau ihn und nicht die Runtime dahinter (12.3,
    /// 12.11). Den Abschnitt dafuer legt der Wirt an.
    pub protect: Option<u64>,
    /// Das Praefix jedes externen Namens, dasselbe wie im erzeugten Code
    /// (12.11); `takt build --prefix` nennt es.
    pub prefix: Prefix,
    /// Ausdrueckliche Stummel fuer alle Treiber, fuer einen Test, der den
    /// Rahmen ohne Treiber bindet ([`crate::drivers::c_stubs`]).
    pub stubs: bool,
    /// Was der Job-Kontext ueber den groessten `stack`-Vertrag der Jobs
    /// hinaus bekommt (`job_stack_reserve` der Hardware-Konfiguration, 8.10);
    /// ohne Angabe [`JOB_STACK_RESERVE`].
    pub job_stack_reserve: u32,
    /// Das Szenario, das mit den laufenden Maschinen schreitet (13.6), wie
    /// in `takt test`; ohne Angabe keines, wie in jedem Produktbau.
    pub scenario: Option<&'a str>,
}

impl Default for Frame<'_> {
    fn default() -> Self {
        Frame {
            diagnostics: takt_llvm::Diagnostics::Ids,
            hardware: None,
            protect: None,
            prefix: Prefix::default(),
            stubs: false,
            job_stack_reserve: JOB_STACK_RESERVE,
            scenario: None,
        }
    }
}

/// Wie [`build`], fuer ein Board mit seinen Angaben ([`Frame`]).
pub fn build_with(p: &Program, frame: Frame<'_>) -> McuHarness {
    let (diagnostics, hw, x) = (frame.diagnostics, frame.hardware, &frame.prefix);
    let layout = crate::layout::of(p);
    // 7.2: in Schrittordnung, wie der Interpreter; mit Szenario dieses dazu
    // (13.6).
    let chosen = frame
        .scenario
        .and_then(|name| p.machines.iter().position(|m| m.name == name))
        .and_then(|i| u32::try_from(i).ok())
        .map(takt_mir::MachineId);
    let driven: Vec<&takt_mir::machine::Machine> = takt_mir::analysis::schedule::order_with(p, chosen)
        .unwrap_or_else(|_| takt_mir::analysis::schedule::runnable_with(p, chosen))
        .into_iter()
        .map(|id| &p.machines[id.index()])
        .collect();

    let arena = takt_llvm::arena::of(p);
    let tick_at = frame.protect.map_or(arena.bytes, |pad| pad.max(arena.bytes));
    let sink_at = tick_at + SINK_AFTER_TICK;
    let mut t = Text::default();
    // Die Arena stellt der Wirt (12.11); jeder Einstieg bekommt sie.
    let _ = writeln!(
        t.code,
        "_Static_assert(offsetof(struct {x}_arena, tick) == {tick_at}, \"der Tick fuehrt die Runtime an\");"
    );
    let _ = writeln!(
        t.code,
        "_Static_assert(offsetof(struct {x}_arena, sink) == {sink_at}u, \"die Senke folgt Tick und letztem Tick\");\n"
    );
    // Der Tick zuerst: `tick_at` sagt, wo er steht.
    runtime_abi(&mut t, p, x);
    crate::parts::natives(&mut t.code, p);
    // Die Stroeme wie im Linux-Rahmen, ohne Stimulus; ihre Trace-Zeilen
    // gehen an das Board.
    crate::streams::emit(&mut t, p, x);
    // 9.8: die geplanten Schreibvorgaenge, hinter dem Latch, weil
    // `apply_scheduled` ihn schreibt.
    crate::parts::scheduled(&mut t, p, &layout, hw, x);
    crate::parts::jitter(&mut t.code, p, hw, x);
    // 12.10: Registerports lesen den Latch des Modells und schreiben in die
    // Ringe der Stroeme, also hinter beidem.
    crate::ports::emit(&mut t, p, x);
    jobs(&mut t, p, x);
    declarations(&mut t.code, p, &driven, x);
    // 8.10, 12.6: die Treiber, stark gebunden; Stummel nur ausdruecklich.
    let drivers = crate::drivers::of(p, &layout);
    t.code.push_str(&crate::drivers::c_prototypes(&drivers, x));
    if frame.stubs {
        t.code.push_str(&crate::drivers::c_stubs(&drivers, x));
    }
    // 12.6: der Treiberrand vor dem Abtasten, das ihn speist.
    crate::edge::emit(&mut t, p, &layout, &driven, deliveries(p, &layout, x), x);
    init(&mut t.code, p, &layout, &driven, x);
    tick(&mut t, p, &layout, &driven, x);
    telemetry(&mut t, p, &layout, &driven, diagnostics, x);
    // Fuer einen Wirt, der die Arena nicht in C anlegt (12.11, [`arena_layout`]).
    let _ = writeln!(t.code, "const uint64_t {x}_arena_bytes = sizeof(struct {x}_arena);");
    let _ = writeln!(t.code, "const uint64_t {x}_arena_align = _Alignof(struct {x}_arena);");
    // Die Version der Schnittstelle und der Logik-Hash: Die Huelle eines
    // Wirts liest beide Symbole, und passt eines nicht, bindet das Programm
    // nicht; `init` liest sie auch, damit sie in jedem Abbild stehen, das
    // das Programm startet (12.11, `takt check-image`).
    let abi = crate::embed::ABI;
    let _ = writeln!(t.code, "const uint8_t {x}_abi_{abi} = {abi};");
    let _ = writeln!(t.code, "const uint8_t {x}_logic_{} = 1;", logic_hex(p));

    let mut head = String::new();
    prologue(&mut head, p);
    let arena_types = t.header(x, &arena, frame.protect);
    let entries = entry_prototypes(x, &logic_hex(p));
    let source = format!("{head}{arena_types}\n{entries}\n{}", t.code);
    let header = header(x, &arena_types, &entries, &crate::drivers::c_prototypes(&drivers, x));
    let job_stack_bytes = job_stack_bytes(p, frame.job_stack_reserve);
    McuHarness { source, header, layout, tick_at, sink_at, job_stack_bytes }
}

/// Der Logik-Hash des Programms als Hexziffern (5.9): im Symbol
/// `P_logic_<hex>` und im Manifest.
pub fn logic_hex(p: &Program) -> String {
    takt_mir::hash::logic_hash(p).0.iter().map(|b| format!("{b:02x}")).collect()
}

/// Die Einstiege, die im Tick laufen (12.3, 12.11): Schritt, Commit und
/// Trace, Schlaf und Wecken, Tunables, Ende des Laufs, Jobs verteilen,
/// Journal und Toleranz. Unter `xip_flash` muessen sie und alles, was sie
/// rufen, im RAM liegen (`takt check-image`); `init`, `persist_restore`,
/// `end` und die Rechnung der Jobs laufen ausserhalb des Ticks.
pub fn tick_path(x: &Prefix) -> Vec<String> {
    [
        "tick",
        "commit",
        "dump",
        "pc",
        "idle",
        "deadline",
        "advance",
        "woken",
        "wake_sources",
        "tune",
        "next_run",
        "overrun",
        "hardware",
        "tolerance",
        "output_timing",
        "operator_abort",
        "job_dispatch",
        "jobs_busy",
        "persist_snapshot",
    ]
    .iter()
    .map(|e| format!("{x}_{e}"))
    .collect()
}

/// Die oeffentlichen Einstiege des Rahmens (12.11). Sie stehen im Kopf fuer
/// den Wirt und im Rahmen selbst: So prueft der Uebersetzer jede Definition
/// gegen ihre Deklaration.
fn entry_prototypes(x: &Prefix, logic: &str) -> String {
    let a = format!("struct {x}_arena *a");
    let lines = [
        format!("int32_t {x}_init_with({a}, void *user, void *sink, const void *persist, int32_t persist_len);"),
        format!("void {x}_init({a}, void *user, void *sink);"),
        format!("void {x}_tick({a}, int64_t k);"),
        format!("void {x}_commit({a});"),
        format!("uint8_t {x}_idle({a});"),
        format!("int64_t {x}_deadline({a});"),
        format!("void {x}_advance({a}, int64_t n);"),
        format!("int32_t {x}_tune({a}, int64_t k, uint32_t param, const void *value, int32_t len);"),
        format!("uint8_t {x}_woken({a}, int64_t k);"),
        format!("uint8_t {x}_wake_sources(void);"),
        format!("int32_t {x}_persist_snapshot({a}, void *out, int32_t cap);"),
        format!("int32_t {x}_persist_restore({a}, const void *in, int32_t len);"),
        format!("void {x}_dump({a}, int32_t all);"),
        format!("void {x}_pc({a}, int32_t all);"),
        format!("int32_t {x}_next_run({a}, int64_t *delay);"),
        format!("void {x}_end({a});"),
        format!("int64_t {x}_output({a}, int32_t index);"),
        format!("void {x}_overrun({a});"),
        format!("void {x}_hardware({a});"),
        format!("void {x}_operator_abort({a});"),
        format!("void {x}_tolerance(int64_t *ns, uint32_t *runs);"),
        format!("uint8_t {x}_output_timing(void);"),
        format!("int32_t {x}_job_dispatch({a});"),
        format!("void {x}_job_work({a});"),
        format!("int32_t {x}_jobs_busy({a});"),
        format!("extern const int32_t {x}_persist_entries;"),
        format!("extern const int32_t {x}_persist_bound;"),
        format!("extern const uint64_t {x}_arena_bytes;"),
        format!("extern const uint64_t {x}_arena_align;"),
        format!("extern const uint8_t {x}_abi_{};", crate::embed::ABI),
        format!("extern const uint8_t {x}_logic_{logic};"),
    ];
    let mut s = String::from("/* Die Einstiege (12.11): jeder bekommt die Arena des Wirts. */\n");
    for line in lines {
        let _ = writeln!(s, "{line}");
    }
    s
}

/// Der Kopf fuer einen Wirt in C (`P.h`, 12.11): die Arena mit ihren Typen,
/// die Einstiege, die Treiber, die der Wirt stellt, und die Haken des
/// Trace. Jeder Name traegt das Praefix oder gehoert der geteilten
/// Bibliothek und steht hinter einem Guard; so binden mehrere Programme
/// ihre Koepfe in dieselbe Uebersetzungseinheit.
fn header(x: &Prefix, arena_types: &str, entries: &str, drivers: &str) -> String {
    // 12.11 (GEN-019): `TAKT_<PRAEFIX>_H` trifft keinen reservierten Makronamen.
    let guard = format!("TAKT_{}_H", x.as_str().to_uppercase());
    let mut s = String::new();
    let _ = writeln!(s, "/* Der Kopf des Programms `{x}` (12.11); erzeugt von takt-frame. */");
    let _ = writeln!(s, "#ifndef {guard}");
    let _ = writeln!(s, "#define {guard}\n");
    let _ = writeln!(s, "#include <stddef.h>");
    let _ = writeln!(s, "#include <stdint.h>\n");
    s.push_str(arena_types);
    let _ = writeln!(s, "\n/* Die Haken des Trace (12.5, 12.11): stellt der Wirt einmal fuer alle Programme. */");
    let _ = writeln!(s, "#ifndef TAKT_TRACE_HOOKS");
    let _ = writeln!(s, "#define TAKT_TRACE_HOOKS");
    s.push_str(TRACE_HOOKS_C);
    let _ = writeln!(s, "#endif\n");
    s.push_str(entries);
    s.push('\n');
    s.push_str(drivers);
    let _ = writeln!(s, "\n#endif");
    s
}

/// Die Haken des Trace (12.5, 12.11): Der Rahmen schreibt seine Zeilen
/// stueckweise ueber sie, jedes Stueck mit der Senke, die der Wirt bei
/// `init` nannte (`a->sink`). Sie haben keinen Zustand; den haelt die
/// Senke, je Programm und Instanz eine. Ein Haken haengt nichts an: Jeden
/// Trenner schreibt der Rahmen selbst (`grammar/trace.md` T1, FB-690).
pub const TRACE_HOOKS_C: &str = "void takt_trace_text(void *sink, const char *text);
void takt_trace_i64(void *sink, long long value);
void takt_trace_u64(void *sink, unsigned long long value);
void takt_trace_f64(void *sink, double value);
void takt_trace_hex8(void *sink, unsigned char value);
";

/// Groesse und Ausrichtung der Arena auf dem Ziel `triple`, so wie der
/// C-Uebersetzer sie legt (12.11): `P_arena_bytes` und `P_arena_align` aus
/// dem IR des Rahmens. Ein Wirt in Rust braucht sie als Konstanten; ein
/// zweites Layoutmodell neben dem Uebersetzer waere eine zweite Quelle.
/// `flags` sind die des Ziels, etwa `-march`.
pub fn arena_layout(h: &McuHarness, x: &Prefix, triple: &str, flags: &[&str]) -> Result<(u64, u64), String> {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        return Err("clang fehlt; ohne ihn ist die Arena nicht zu bemessen".into());
    };
    let mut child = Command::new(&clang)
        .args(["-x", "c", "-", "-S", "-emit-llvm", "-O0", "-ffreestanding", "-o", "-"])
        .arg(format!("--target={triple}"))
        .args(flags)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", clang.display()))?;
    let mut stdin = child.stdin.take().ok_or("clang ohne Eingabe")?;
    let source = h.source.clone();
    // Schreiben und Lesen zugleich: Sonst fuellt das IR die Leitung, waehrend
    // der Rahmen noch hineingeht.
    let writer = std::thread::spawn(move || stdin.write_all(source.as_bytes()));
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    writer.join().map_err(|_| "Schreiben an clang brach ab".to_string())?.map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    let ir = String::from_utf8_lossy(&out.stdout);
    let constant = |name: &str| -> Result<u64, String> {
        let head = format!("@{x}_{name} = ");
        ir.lines()
            .find(|l| l.starts_with(&head))
            .and_then(|l| l.split("constant i64 ").nth(1))
            .and_then(|v| v.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("`{x}_{name}` fehlt im IR des Rahmens"))
    };
    Ok((constant("arena_bytes")?, constant("arena_align")?))
}

/// Die Arena als Rust-Typ (12.11): Groesse und Ausrichtung aus
/// [`arena_layout`]. `init` beschreibt sie ganz; vorher ist ihr Inhalt
/// ohne Bedeutung.
pub fn rust_arena(bytes: u64, align: u64) -> String {
    format!(
        "/// Die Arena des Programms (12.11): {bytes} Byte, ausgerichtet auf {align}, aus dem Rahmen fuer dieses \
         Ziel.\n#[repr(C, align({align}))]\npub struct Arena(core::mem::MaybeUninit<[u8; {bytes}]>);\n\n\
         impl Arena {{\n    /// Eine Arena vor `init`, das sie ganz beschreibt.\n    pub const fn new() -> Arena {{\n        \
         Arena(core::mem::MaybeUninit::uninit())\n    }}\n}}\n\n\
         impl Default for Arena {{\n    fn default() -> Arena {{\n        Arena::new()\n    }}\n}}\n"
    )
}

/// Kopf und Vorwaertsdeklarationen.
fn prologue(s: &mut String, p: &Program) {
    let _ = writeln!(s, "/* MCU-Rahmen (12.1, 12.3); erzeugt von takt-frame. */");
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
    // Der Wirt stellt sie bereit; der Rahmen ruft sie nur.
    s.push_str(TRACE_HOOKS_C);
    s.push('\n');
    s.push_str(FENV_C);
}

/// Die IEEE-Umgebung an jedem Einstieg (4.2, 12.11): sichern, die Vorgabe
/// herstellen, beim Austritt zurueckgeben. Je FPU-Familie ihr Register;
/// ohne FPU rechnet die Software ohnehin in der Vorgabe.
///
/// **Setzen statt pruefen.** Ein Wirt darf fuer seinen eigenen Code
/// Flush-to-Zero oder einen anderen Rundungsmodus waehlen; ein Fault bei
/// Abweichung machte Takt von fremdem Code abhaengig (plan/m11.md 2.10).
/// Die Ausnahmeflags des Aufrufers kommen mit seinem Register zurueck.
const FENV_C: &str = r#"/* 4.2, 12.11: Jeder Einstieg sichert die Fliesskomma-Umgebung des Aufrufers,
   rechnet in der IEEE-Vorgabe (zur naechsten runden, kein Flush-to-Zero, keine
   Default-NaN, keine Traps) und gibt beim Austritt die des Aufrufers zurueck.
   Die Treiber, die er ruft, rechnen in derselben Umgebung. */
#if defined(__aarch64__)
typedef uint64_t takt_fenv;
/* FPCR: AHP (26), DN (25), FZ (24), RMode (23:22), FZ16 (19), Trap-Freigaben (15, 12:8). */
static inline takt_fenv takt_fenv_enter(void) {
    takt_fenv saved;
    __asm__ volatile("mrs %0, fpcr" : "=r"(saved));
    __asm__ volatile("msr fpcr, %0" : : "r"(saved & ~(takt_fenv)0x07C89F00u) : "memory");
    return saved;
}
static inline void takt_fenv_leave(takt_fenv saved) { __asm__ volatile("msr fpcr, %0" : : "r"(saved) : "memory"); }
#elif defined(__ARM_FP)
typedef uint32_t takt_fenv;
/* FPSCR: AHP (26), DN (25), FZ (24), RMode (23:22), Trap-Freigaben (15, 12:8). */
static inline takt_fenv takt_fenv_enter(void) {
    takt_fenv saved;
    __asm__ volatile("vmrs %0, fpscr" : "=r"(saved));
    __asm__ volatile("vmsr fpscr, %0" : : "r"(saved & ~0x07C09F00u) : "memory");
    return saved;
}
static inline void takt_fenv_leave(takt_fenv saved) { __asm__ volatile("vmsr fpscr, %0" : : "r"(saved) : "memory"); }
#elif defined(__x86_64__) || defined(_M_X64)
typedef uint32_t takt_fenv;
/* MXCSR: FTZ (15), Rundung (14:13), Masken (12:7), DAZ (6); die Flags (5:0) bleiben. */
static inline takt_fenv takt_fenv_enter(void) {
    takt_fenv saved = __builtin_ia32_stmxcsr();
    __builtin_ia32_ldmxcsr((saved & 0x3Fu) | 0x1F80u);
    return saved;
}
static inline void takt_fenv_leave(takt_fenv saved) { __builtin_ia32_ldmxcsr(saved); }
#elif defined(__riscv_flen)
typedef uint32_t takt_fenv;
/* frm: RISC-V kennt weder Flush-to-Zero noch Traps, nur den Rundungsmodus. */
static inline takt_fenv takt_fenv_enter(void) {
    takt_fenv saved;
    __asm__ volatile("csrr %0, frm" : "=r"(saved));
    __asm__ volatile("csrwi frm, 0" : : : "memory");
    return saved;
}
static inline void takt_fenv_leave(takt_fenv saved) { __asm__ volatile("csrw frm, %0" : : "r"(saved) : "memory"); }
#else
typedef uint32_t takt_fenv;
static inline takt_fenv takt_fenv_enter(void) { return 0; }
static inline void takt_fenv_leave(takt_fenv saved) { (void)saved; }
#endif

"#;

/// Die oeffentliche Huelle `P_<name>(arena, ...)` um den Rumpf
/// `takt_<name>`: Sie rechnet ihn in der IEEE-Umgebung (`FENV_C`). `params`
/// und `args` sind die Parameter nach der Arena.
fn guarded(s: &mut String, x: &Prefix, ret: &str, name: &str, params: &str, args: &str) {
    entry(s, x, ret, name, params, args, None);
}

/// Wie [`guarded`], mit der Eintrittssperre (12.11): Ein Eintritt, waehrend
/// ein anderer in dieselbe Arena rechnet — eine ISR, die den Schritt
/// unterbricht und ihn noch einmal ruft —, rechnet nicht, sondern gibt
/// `refused` zurueck und ist `Runtime(Overrun)` im naechsten Tick. Die
/// Sperre ist ein atomarer Tausch; `init` setzt sie zurueck.
fn locked(s: &mut String, x: &Prefix, ret: &str, name: &str, params: &str, args: &str, refused: &str) {
    entry(s, x, ret, name, params, args, Some(refused));
}

fn entry(s: &mut String, x: &Prefix, ret: &str, name: &str, params: &str, args: &str, refused: Option<&str>) {
    let (params, args) = if params.is_empty() {
        (format!("struct {x}_arena *a"), "a".to_string())
    } else {
        (format!("struct {x}_arena *a, {params}"), format!("a, {args}"))
    };
    let call = format!("takt_{name}({args})");
    let (enter, leave) = match refused {
        Some(r) => (
            format!("if (__atomic_exchange_n(&a->entered, 1u, __ATOMIC_ACQUIRE)) {{ a->overrun = 1; return {r}; }} "),
            " __atomic_store_n(&a->entered, 0u, __ATOMIC_RELEASE);".to_string(),
        ),
        None => (String::new(), String::new()),
    };
    let body = if ret == "void" {
        format!("{call}; takt_fenv_leave(f);{leave}")
    } else {
        format!("{ret} r = {call}; takt_fenv_leave(f);{leave} return r;")
    };
    let _ = writeln!(s, "{ret} {x}_{name}({params}) {{ {enter}takt_fenv f = takt_fenv_enter(); {body} }}\n");
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
fn runtime_abi(t: &mut Text, p: &Program, x: &Prefix) {
    let _ = writeln!(t.fields, "    long long tick;");
    // Der zuletzt ausgefuehrte Tick: Im Schlaf rueckt `tick` vor (9.9),
    // die Ausgaben, die der Dump danach schreibt, gehoeren aber zu ihm.
    let _ = writeln!(t.fields, "    long long done;");
    // 12.11: die Senke des Trace, die der Wirt bei jedem `init` nennt; jeder
    // Haken bekommt sie. Ein Slot der Runtime an fester Stelle hinter dem
    // Tick (`sink_at`): nie im Programmbereich, also nie im Journal, unter
    // dem Schutz des Ticks und nur im Schritt gelesen.
    let _ = writeln!(t.fields, "    void *sink;");
    // Der Zeiger, den der Wirt bei `init` uebergab; jeder Treiber bekommt ihn
    // als erstes Argument (8.10, 12.6).
    let _ = writeln!(t.fields, "    void *user;");
    crate::parts::scope_flags(t, p);
    crate::parts::fault_names(&mut t.code, p);
    crate::parts::raised(t, p, x);
    crate::parts::alert_table(t, p, x);
    let s = &mut t.code;

    // 3.3: `now` ist die Dauer seit dem Start — Tickzahl mal T0, gesaettigt.
    crate::parts::now(s, p, x);

    // Jede Beobachtungszeile traegt ihren Tick, wie beim Interpreter
    // (`grammar/trace.md`): Ohne ihn laesst sie sich keinem Tick zuordnen.
    // 5.6: nur die Flanken, mit dem Namen der Maschine wie im Interpreter.
    let _ =
        writeln!(s, "void {x}_alert(struct {x}_arena *a, int m, int slot, unsigned char on, unsigned char invalid) {{");
    let _ = writeln!(s, "    if (!takt_alert_edge(a, m, slot, on)) return;");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" alert \");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, takt_machine_name(m));");
    let _ = writeln!(s, "    takt_trace_text(a->sink, on ? \" on\" : \" off\");");
    let _ = writeln!(s, "    if (invalid) takt_trace_text(a->sink, \" invalid\");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
    let _ = writeln!(s, "}}");
    for (name, args, kind, flags) in [
        ("log", "int m, int site", "log", &[][..]),
        ("verify", "int m, int site, unsigned char ok", "verify", &["ok"]),
        ("verdict", "int m, int site, unsigned char pass", "verdict", &["pass"]),
    ] {
        let _ = writeln!(s, "void {x}_{name}(struct {x}_arena *a, {args}) {{");
        let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
        let _ = writeln!(s, "    takt_trace_text(a->sink, \" {kind} \");");
        let _ = writeln!(s, "    takt_trace_text(a->sink, takt_machine_name(m));");
        let _ = writeln!(s, "    takt_trace_text(a->sink, \" \");");
        let _ = writeln!(s, "    takt_trace_i64(a->sink, site);");
        for f in flags {
            let _ = writeln!(s, "    takt_trace_text(a->sink, {f} ? \" 1\" : \" 0\");");
        }
        let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
        let _ = writeln!(s, "}}");
    }

    // 5.4: `abort` merkt den Fault fuer alle Maschinen vor; die
    // Abort-Phase stellt ihn nach den Schritten zu.
    let _ = writeln!(s, "void {x}_abort(struct {x}_arena *a, int m, int site) {{");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" abort \");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, takt_machine_name(m));");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" \");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, site);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
    let _ = writeln!(s, "    memset(a->raised, 1, sizeof a->raised);");
    let _ = writeln!(s, "}}\n");

    // 5.3: der Fault-Uebergang mit Maschine und Art, wie der Interpreter
    // ihn schreibt.
    let _ = writeln!(s, "void {x}_fault(struct {x}_arena *a, int m, int from, int code) {{");
    let _ = writeln!(s, "    (void)from;");
    crate::parts::note_fault(s, p, "    ");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" fault \");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, takt_machine_name(m));");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" \");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, takt_fault_name(code));");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
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
    let _ = writeln!(s, "void {x}_measure(struct {x}_arena *a, int m, int site, double v, unsigned char invalid) {{");
    let _ = writeln!(s, "    unsigned long long bits;");
    let _ = writeln!(s, "    __builtin_memcpy(&bits, &v, sizeof bits);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" measure \");");
    let _ = writeln!(s, "    takt_trace_text(a->sink, takt_machine_name(m));");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" \");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, site);");
    let _ = writeln!(s, "    if (invalid) {{");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \" <invalid>\\n\");");
    let _ = writeln!(s, "        return;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \" bits \");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, (long long)bits);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
    let _ = writeln!(s, "}}\n");

    // 13.3: Ein Monitor meldet die Verletzung mit Art und Namen der
    // Eigenschaft und ihrer Position, wie der Interpreter (FB-391).
    let names: Vec<String> = p
        .properties
        .iter()
        .map(|q| format!("\" {} {} violated \"", if q.assumption { "assumption" } else { "property" }, q.name))
        .collect();
    let _ = writeln!(s, "void {x}_property(struct {x}_arena *a, int i, long long at) {{");
    if names.is_empty() {
        // Ohne Eigenschaft ruft kein Monitor.
        let _ = writeln!(s, "    (void)a; (void)i; (void)at;");
    } else {
        let _ = writeln!(s, "    static const char *const names[{}] = {{ {} }};", names.len(), names.join(", "));
        let _ = writeln!(s, "    if ((i < 0) || (i >= {})) {{", names.len());
        let _ = writeln!(s, "        return;");
        let _ = writeln!(s, "    }}");
        let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "    takt_trace_i64(a->sink, a->tick);");
        let _ = writeln!(s, "    takt_trace_text(a->sink, names[i]);");
        let _ = writeln!(s, "    takt_trace_i64(a->sink, at);");
        let _ = writeln!(s, "    takt_trace_text(a->sink, \"\\n\");");
    }
    let _ = writeln!(s, "}}\n");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "void {x}_monitor_{i}(struct {x}_arena *a, long long tick);");
    }
}

/// Die Signaturen des erzeugten Codes (11.2).
fn declarations(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(s, "/* Der erzeugte Code (11.2). */");
    crate::parts::machine_declarations(s, p, driven, x);
    let _ = writeln!(s);
}

/// Wie viel Stack der Job-Kontext ueber den groessten `stack`-Vertrag der
/// Jobs hinaus bekommt, wenn die Hardware-Konfiguration nichts anderes sagt
/// (`job_stack_reserve`, 8.10): Rahmen und Verteiler des Jobs, die
/// Umschaltung und — wo Interrupts auf dem Stack des unterbrochenen Fadens
/// laufen — die Interrupts selbst.
pub const JOB_STACK_RESERVE: u32 = 1024;

/// Der Stack des Job-Kontexts (4.5, 12.11): der groesste `stack`-Vertrag
/// der Jobs, `reserve` und 32 Byte fuer den Waechter am unteren Ende (12.3),
/// an 32 Byte ausgerichtet, damit eine MPU-Region oder ein
/// NAPOT-Watchpoint ihn genau abdeckt; 0 ohne Jobs. Ihn stellt der Wirt,
/// statisch fuer die Lebensdauer des Programms wie die Arena, und nur er
/// legt ihn ab: Die Bibliothek haelt ausserhalb der Arena keinen
/// beschreibbaren Speicher (`takt check-image`).
pub fn job_stack_bytes(p: &Program, reserve: u32) -> u64 {
    let slots = crate::parts::job_slots(p);
    let Some(stack) = slots.iter().map(|(_, _, n)| p.natives[n.index()].stack).max() else { return 0 };
    32 + u64::from(stack) + u64::from(reserve)
}

/// Der Typ des Job-Stacks fuer einen Wirt in Rust (12.11): `bytes` Bytes,
/// an 32 Byte ausgerichtet ([`job_stack_bytes`]). Ohne Jobs ist er leer, und
/// kein Wirt fragt nach ihm: Ob ein Programm einen Job-Kontext bekommt, sagt
/// sein Typ (`takt_embed::Work`, FB-385).
pub fn rust_job_stack(x: &Prefix, bytes: u64) -> String {
    format!(
        "/// Die Groesse des Job-Stacks in Bytes (4.5, 12.11); 0 ohne Jobs.
pub const JOB_STACK_BYTES: usize = {bytes};

/// Der Stack des Job-Kontexts (4.5): Der Wirt stellt ihn statisch fuer die
/// Lebensdauer des Programms, wie die Arena; am unteren Ende liegt der
/// Waechter (12.3). Unter dem Namen `{x}_job_stack` prueft `takt
/// check-image` seine Groesse im Abbild.
#[repr(C, align(32))]
pub struct JobStack([u8; JOB_STACK_BYTES]);

impl JobStack {{
    /// Ein Job-Stack; `const` fuer einen statischen.
    pub const fn new() -> JobStack {{
        JobStack([0; JOB_STACK_BYTES])
    }}

    /// Der Stack fuer den Job-Kontext.
    pub fn bytes(&mut self) -> &mut [u8] {{
        &mut self.0
    }}
}}

impl Default for JobStack {{
    fn default() -> JobStack {{
        JobStack::new()
    }}
}}
"
    )
}

/// 4.5: Jobs auf der MCU. Der Start kopiert die Argumente in den Slot und
/// reiht ihn ein; gerechnet wird im Job-Kontext der Runtime, einem Faden
/// mit eigenem Stack, den der Tick unterbricht (12.3). Sichtbar wird das
/// Ergebnis fruehestens nach seiner Dauer (logische Ausfuehrungszeit) —
/// wie im Modell des Interpreters, solange der Job sie haelt.
///
/// **Zwei Faeden, ein Uebergabepunkt.** Die Slots gehoeren der
/// Hauptschleife: Nur sie startet, bricht ab und macht sichtbar. Dem
/// Job-Kontext gehoert ein Auftrag (`work_*`), den `work_state` weiterreicht:
/// Die Hauptschleife fuellt ihn, solange er frei ist, und vergibt ihn
/// (`P_job_dispatch`); der Kontext uebernimmt ihn mit einem Vergleichen und
/// Tauschen, rechnet und meldet ihn fertig; die Hauptschleife holt das
/// Ergebnis ab und gibt den Auftrag frei. Jeder Uebergang ist sequentiell
/// konsistent (MISRA C:2025 R.21.25): Unter POSIX laeuft der Job-Kontext auf
/// einem anderen Kern als der Schritt, und ein Aufruf des Kontexts ohne
/// vergebenen Auftrag liest nichts (FB-528). Ein Neustart oder Abbruch
/// waehrend der Rechnung erhoeht die Generation des Slots, und ein Ergebnis
/// zur alten verfaellt.
///
/// Ohne Jobs bleiben die Einstiege, damit das Board sie ohne Unterschied
/// rufen kann.
///
/// Den Stack des Job-Kontexts stellt der Wirt ([`job_stack_bytes`]).
fn jobs(t: &mut Text, p: &Program, x: &Prefix) {
    let s = &mut t.code;
    let _ = writeln!(s, "/* Jobs (4.5): Slots der Hauptschleife, ein Auftrag fuer den Job-Kontext. */");
    let Some((slots, out_max)) = crate::parts::job_tables(s, p, x) else {
        let _ = writeln!(s, "int32_t {x}_job_dispatch(struct {x}_arena *a) {{ (void)a; return 0; }}");
        let _ = writeln!(s, "void {x}_job_work(struct {x}_arena *a) {{ (void)a; }}");
        let _ = writeln!(s, "int32_t {x}_jobs_busy(struct {x}_arena *a) {{ (void)a; return 0; }}\n");
        return;
    };
    let in_max = crate::parts::job_in_max(p);

    let _ = writeln!(t.code, "enum {{ TAKT_JOB_FREE, TAKT_JOB_WAITING, TAKT_JOB_RUNNING, TAKT_JOB_DONE }};");
    let _ = writeln!(t.code, "enum {{ TAKT_WORK_FREE, TAKT_WORK_ASSIGNED, TAKT_WORK_RUNNING, TAKT_WORK_DONE }};");
    let _ = writeln!(
        t.types,
        "typedef struct {{ unsigned char state, gen, late; int native; long long due, order; int in_len, out_len; unsigned char in[{in_max}], out[{out_max}]; }} {x}_job;"
    );
    // Slots und Auftrag liegen in der Runtime hinter dem Programmbereich:
    // Der Job-Kontext schreibt den Auftrag ausserhalb des Schritts, wenn der
    // Schutz den Programmbereich sperrt (12.3). `work_slot` gehoert der
    // Hauptschleife, `work_state` beiden Faeden und geht nur atomar.
    let f = &mut t.fields;
    let _ = writeln!(f, "    {x}_job jobs[{slots}];");
    let _ = writeln!(f, "    long long job_order;");
    let _ = writeln!(f, "    int work_slot, work_state;");
    let _ = writeln!(f, "    unsigned char work_gen;");
    let _ = writeln!(f, "    int work_native, work_in_len, work_out_len;");
    let _ = writeln!(f, "    unsigned char work_in[{in_max}], work_out[{out_max}];");
    let s = &mut t.code;

    let _ = writeln!(
        s,
        "unsigned char *{x}_job_args(struct {x}_arena *a, int m, int slot) {{ return a->jobs[takt_job_base[m] + slot].in; }}"
    );
    let _ = writeln!(s, "void {x}_job_begin(struct {x}_arena *a, int m, int slot, int native, int len) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; {x}_job *j = &a->jobs[i];");
    let _ = writeln!(s, "    if (len > (int)sizeof j->in) len = (int)sizeof j->in;");
    let _ = writeln!(s, "    j->in_len = len; j->native = native;");
    let _ = writeln!(s, "    j->state = TAKT_JOB_WAITING; j->gen++; j->order = ++a->job_order; j->late = 0;");
    let _ = writeln!(s, "    j->due = a->tick + takt_job_ticks[i];");
    let _ = writeln!(s, "    takt_job_image(a, i, 0, 0, 2); /* Err(PENDING) */");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "void {x}_job_cancel(struct {x}_arena *a, int m, int slot) {{");
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot;");
    let _ = writeln!(s, "    if (a->jobs[i].state == TAKT_JOB_FREE) return;");
    let _ = writeln!(s, "    a->jobs[i].state = TAKT_JOB_FREE; a->jobs[i].gen++;");
    let _ = writeln!(s, "    takt_job_image(a, i, 1, 0, 0); /* Err(CANCELLED) */");
    let _ = writeln!(s, "}}");

    let _ = writeln!(s, "/* Hauptschleife: Ein fertiger Auftrag geht in seinen Slot, wenn der noch auf ihn wartet. */");
    let _ = writeln!(s, "static void takt_jobs_collect(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    {x}_job *j;");
    let _ = writeln!(s, "    if (__atomic_load_n(&a->work_state, __ATOMIC_SEQ_CST) != TAKT_WORK_DONE) return;");
    let _ = writeln!(s, "    j = &a->jobs[a->work_slot];");
    let _ = writeln!(s, "    if (j->state == TAKT_JOB_RUNNING && j->gen == a->work_gen) {{");
    // `-1` ist `Err(FAILED)` ohne Bytes; ein `size_t` daraus kopierte alles.
    let _ = writeln!(
        s,
        "        if (a->work_out_len > 0) memcpy(j->out, a->work_out, (size_t)a->work_out_len); j->out_len = a->work_out_len;"
    );
    let _ = writeln!(s, "        j->state = TAKT_JOB_DONE;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    a->work_slot = -1;");
    let _ = writeln!(s, "    __atomic_store_n(&a->work_state, TAKT_WORK_FREE, __ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "/* Hauptschleife: Der ruhende Kontext bekommt den aeltesten wartenden Job; wahr, wenn er zu rechnen hat. */"
    );
    let _ = writeln!(s, "int32_t {x}_job_dispatch(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i, next = -1;");
    let _ = writeln!(s, "    takt_jobs_collect(a);");
    let _ = writeln!(s, "    if (__atomic_load_n(&a->work_state, __ATOMIC_SEQ_CST) != TAKT_WORK_FREE) return 1;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++)");
    let _ = writeln!(
        s,
        "        if (a->jobs[i].state == TAKT_JOB_WAITING && (next < 0 || a->jobs[i].order < a->jobs[next].order)) next = i;"
    );
    let _ = writeln!(s, "    if (next < 0) return 0;");
    let _ = writeln!(s, "    memcpy(a->work_in, a->jobs[next].in, (size_t)a->jobs[next].in_len);");
    let _ = writeln!(s, "    a->work_in_len = a->jobs[next].in_len; a->work_native = a->jobs[next].native;");
    let _ = writeln!(s, "    a->work_gen = a->jobs[next].gen; a->jobs[next].state = TAKT_JOB_RUNNING;");
    let _ = writeln!(s, "    a->work_slot = next;");
    let _ = writeln!(s, "    __atomic_store_n(&a->work_state, TAKT_WORK_ASSIGNED, __ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "/* Job-Kontext: rechnet den Auftrag, den die Hauptschleife gegeben hat. */");
    let _ = writeln!(s, "static void takt_job_work(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int native, assigned = TAKT_WORK_ASSIGNED;");
    let _ = writeln!(
        s,
        "    if (!__atomic_compare_exchange_n(&a->work_state, &assigned, TAKT_WORK_RUNNING, 0, __ATOMIC_SEQ_CST, __ATOMIC_SEQ_CST)) return;"
    );
    let _ = writeln!(s, "    native = a->work_native;");
    let _ = writeln!(s, "    {{");
    crate::parts::job_call(s, p, "a->work_in", "a->work_in_len", "a->work_out", "a->work_out_len", "        ");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    __atomic_store_n(&a->work_state, TAKT_WORK_DONE, __ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "job_work", "", "");
    let _ = writeln!(
        s,
        "/* 4.5: zu Beginn des Schritts der Maschine `m`. Ein fertiges Ergebnis wird sichtbar, wenn seine Dauer um ist. */"
    );
    let _ = writeln!(s, "static void takt_jobs_poll(struct {x}_arena *a, int m) {{");
    let _ = writeln!(s, "    int i, b;");
    let _ = writeln!(s, "    for (i = takt_job_base[m]; i < takt_job_base[m + 1]; i++) {{");
    // 4.5, FB-476: Ein Lauf, der im Tick seiner Faelligkeit nicht fertig ist,
    // steht dort einmal als `late`; seine Fertigstellung folgt als `done`.
    let _ = writeln!(
        s,
        "        if (a->jobs[i].state != TAKT_JOB_FREE && a->jobs[i].state != TAKT_JOB_DONE && a->jobs[i].due <= a->tick && !a->jobs[i].late) {{"
    );
    let _ = writeln!(s, "            a->jobs[i].late = 1;");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \"t=\"); takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(
        s,
        "            takt_trace_text(a->sink, \" job \"); takt_trace_text(a->sink, takt_job_names[i]); takt_trace_text(a->sink, \" late start=\");"
    );
    let _ = writeln!(
        s,
        "            takt_trace_i64(a->sink, a->jobs[i].due - takt_job_ticks[i]); takt_trace_text(a->sink, \"\\n\");"
    );
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        if (a->jobs[i].state != TAKT_JOB_DONE || a->jobs[i].due > a->tick) continue;");
    let _ = writeln!(s, "        a->jobs[i].state = TAKT_JOB_FREE;");
    let _ = writeln!(
        s,
        "        if (a->jobs[i].out_len < 0) takt_job_image(a, i, 1, 0, 1); /* Err(FAILED) */ else takt_job_image(a, i, 1, 1, 0);"
    );
    let _ = writeln!(
        s,
        "        for (b = 0; b < a->jobs[i].out_len; b++) a->image[takt_job_at[i] + 8 + b] = a->jobs[i].out[b];"
    );
    let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\"); takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(
        s,
        "        takt_trace_text(a->sink, \" job \"); takt_trace_text(a->sink, takt_job_names[i]); takt_trace_text(a->sink, \" done start=\");"
    );
    let _ = writeln!(
        s,
        "        takt_trace_i64(a->sink, a->jobs[i].due - takt_job_ticks[i]); takt_trace_text(a->sink, \"\\n\");"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "/* 9.9: Mit einem Job, der wartet, rechnet oder noch nicht sichtbar ist, schlaeft das System nicht. */"
    );
    let _ = writeln!(s, "int32_t {x}_jobs_busy(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) if (a->jobs[i].state != TAKT_JOB_FREE) return 1;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_jobs_init(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i;");
    let _ = writeln!(
        s,
        "    for (i = 0; i < {slots}; i++) {{ a->jobs[i].state = TAKT_JOB_FREE; takt_job_image(a, i, 0, 0, 2); }}"
    );
    let _ = writeln!(s, "    a->work_slot = -1;");
    let _ = writeln!(s, "    __atomic_store_n(&a->work_state, TAKT_WORK_FREE, __ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "}}\n");
}

/// `P_init`: einmal vor dem ersten Tick.
fn init(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(s, "/* Einmal vor dem ersten Tick (12.1, Schritt 1). */");
    let _ = writeln!(s, "static int32_t takt_persist_restore(struct {x}_arena *a, const void *in, int32_t len);");
    let _ = writeln!(s, "static void takt_sample(struct {x}_arena *a);");
    let _ = writeln!(
        s,
        "static int32_t takt_init_with(struct {x}_arena *a, void *user, void *sink, const void *persist, \
         int32_t persist_len) {{"
    );
    // 12.11: `init` beschreibt die ganze Arena und verlaesst sich nicht auf
    // genullten Speicher.
    let _ = writeln!(s, "    memset(a, 0, sizeof *a);");
    // 12.11: ABI-Version und Logik-Hash stehen in jedem Abbild, das das
    // Programm startet; `takt check-image` liest sie dort.
    let _ = writeln!(s, "    (void)*(volatile const uint8_t *)&{x}_abi_{};", crate::embed::ABI);
    let _ = writeln!(s, "    (void)*(volatile const uint8_t *)&{x}_logic_{};", logic_hex(p));
    // 12.11: Senke und Treiberobjekt nennt der Wirt bei jedem `init`; keines
    // kommt aus dem Journal, und vor ihnen schreibt nichts in den Trace.
    let _ = writeln!(s, "    a->sink = sink;");
    let _ = writeln!(s, "    a->user = user;");
    let _ = writeln!(s, "    takt_edge_init(a);");

    // 3.5: Ein Input ohne Treiber ist `Bad`. Ein genullter Eintrag hiesse
    // `Good`, und das waere eine Zusage, die kein Treiber gegeben hat.
    for slot in &layout.inputs {
        if let Some(entry) = crate::parts::quality_offset(p, &slot.name) {
            let _ = writeln!(s, "    a->image[{entry}] = 3; /* {} ist Bad (3.5) */", slot.name);
        }
    }

    // Die Parameter stehen fuer den Lauf fest (8.4).
    for (i, slot) in layout.parameters.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = crate::parts::param_literal(p, i) else { continue };
        let _ = writeln!(
            s,
            "    __builtin_memcpy(a->params + {}, &({ct}){{ {value} }}, sizeof({ct})); /* {} */",
            slot.offset, slot.name
        );
    }
    if !crate::parts::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_init(a);");
    }
    // 9.4: Der Lauf beginnt mit den Outputs auf `safe`, vor jedem Init —
    // wie `Sim::new`. Danach bindet die Simulation,
    // damit Tick 0 die `safe`-Werte eines Modells liest (8.3).
    crate::parts::safe_outputs(s, p, layout);
    crate::parts::sim_bindings(s, p, "    ");
    crate::ports::sample(s, p, "    ");

    // 5.9: Defaults, dann die geladenen Werte, dann erst enter: — wie
    // der Interpreter zwischen init_vars und machine::init laedt; nach
    // jedem Eintritt `publish`, damit Follower schon im Tick 0 frisch
    // lesen (7.2, 9.4).
    for m in driven {
        let _ = writeln!(s, "    {x}_{0}_init_vars(a);", m.name);
    }
    let _ = writeln!(s, "    int restored = takt_persist_restore(a, persist, persist_len);");
    // 9.4: Auch Tick 0 beginnt mit `I_0 = sample()`; ein `enter:` des
    // Anfangszustands liest die Eingaenge wie im Interpreter (FB-316).
    let _ = writeln!(s, "    takt_sample(a);");
    let _ = writeln!(s, "    a->outer = 0;");
    crate::parts::outer_fault(s, p, "    ");
    crate::parts::enter_machines(s, p, layout, driven, "    ", x);
    // Was `enter` und das erste `loop:` im Tick 0 senden, wird hier
    // sichtbar (FB-269).
    crate::parts::commit_sequence(s, p, driven, "    ", "0");
    crate::ports::sample(s, p, "    ");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    {x}_monitor_{i}(a, 0);");
    }
    let _ = writeln!(s, "    return restored;");
    let _ = writeln!(s, "}}");
    guarded(
        s,
        x,
        "int32_t",
        "init_with",
        "void *user, void *sink, const void *persist, int32_t persist_len",
        "user, sink, persist, persist_len",
    );
}

/// `P_tick`: ein Tick, von der Schleife gerufen.
fn tick(t: &mut Text, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(t.fields, "    _Bool overrun, hardware, operator_abort;");
    // 12.11: die Eintrittssperre ([`locked`]).
    let _ = writeln!(t.fields, "    unsigned int entered;");
    let _ = writeln!(t.fields, "    _Bool driver_fault[{}];", driver_outputs(p, layout).len().max(1));
    let s = &mut t.code;
    let _ = writeln!(s, "/* Ein Tick (12.1, Schritte 2 bis 10). */");
    let _ = writeln!(s, "static void takt_sample(struct {x}_arena *a);");
    // 7.3: Die Schleife meldet einen Ueberlauf vor dem naechsten Tick
    // (`Program::raise_overrun`); er wirkt fuer alle Maschinen in diesem
    // Tick und steht als Zeile `runtime` im Trace, damit der Lauf sich
    // nachspielen laesst (12.5).
    let _ = writeln!(s, "void {x}_overrun(struct {x}_arena *a) {{ a->overrun = 1; }}");
    // 12.3: Der Speicherschutz hat einen Zugriff der TCB abgewiesen.
    let _ = writeln!(s, "void {x}_hardware(struct {x}_arena *a) {{ a->hardware = 1; }}");
    // 5.4, 9.4: Der Bediener bricht ab; der Kern meldet es vor dem naechsten
    // Tick wie einen Ueberlauf, und die Zeile `abort` zeichnet es auf (12.5).
    let _ = writeln!(s, "void {x}_operator_abort(struct {x}_arena *a) {{ a->operator_abort = 1; }}");
    // 7.1, 12.6 Zeile 7: die Toleranz der Tickquelle fuer die Schleife.
    let (ns, runs) = p.config.tolerance();
    let _ = writeln!(s, "void {x}_tolerance(int64_t *ns, uint32_t *runs) {{ *ns = {ns}LL; *runs = {runs}u; }}");
    // 1.4: `output_timing` fuer die Schleife; 1 (`boundary`) haelt den
    // Commit bis zur naechsten Tickgrenze.
    let boundary = u8::from(p.config.output_timing == takt_mir::program::OutputTiming::Boundary);
    let _ = writeln!(s, "uint8_t {x}_output_timing(void) {{ return {boundary}u; }}");
    // 12.6 Zeile 6: Was der Commit an Treiberfehlern gesehen hat, wirkt im
    // naechsten Tick, wie ein Ueberlauf.
    let driven_out = driver_outputs(p, layout);
    let _ = writeln!(s, "static void takt_tick(struct {x}_arena *a, int64_t k) {{");
    let _ = writeln!(s, "    a->tick = k;");
    let _ = writeln!(s, "    a->done = k;");
    let _ = writeln!(s, "    if (a->overrun) {{");
    let _ = writeln!(s, "        a->overrun = 0;");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "        takt_trace_i64(a->sink, k);");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \" runtime Overrun\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(a, m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Overrun))
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if (a->hardware) {{");
    let _ = writeln!(s, "        a->hardware = 0;");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "        takt_trace_i64(a->sink, k);");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \" runtime Hardware\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(a, m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Hardware))
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if (a->operator_abort) {{");
    let _ = writeln!(s, "        a->operator_abort = 0;");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "        takt_trace_i64(a->sink, k);");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \" abort\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(a, m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Abort)
    );
    let _ = writeln!(s, "    }}");
    let driver =
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Driver));
    for (i, out) in driven_out.iter().enumerate() {
        let pend = match out.owner {
            Some(m) => format!("takt_pend(a, {m}, {driver});"),
            None => format!("for (int m = 0; m < {}; m++) takt_pend(a, m, {driver});", p.machines.len()),
        };
        let _ = writeln!(s, "    if (a->driver_fault[{i}]) {{");
        let _ = writeln!(s, "        a->driver_fault[{i}] = 0;");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "        takt_trace_i64(a->sink, k);");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \" runtime Driver {}\\n\");", out.name);
        let _ = writeln!(s, "        {pend}");
        let _ = writeln!(s, "    }}");
    }
    crate::parts::aging(s, p, layout, "    ");
    crate::parts::outer_fault(s, p, "    ");
    // 4.5: Was der Job-Kontext fertig hat, holt der Tickbeginn ab; sichtbar
    // wird es im Schritt seiner Maschine (`parts::steps`).
    if !crate::parts::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_collect(a);");
    }
    let _ = writeln!(s, "    takt_sample(a);");
    // 8.3: die `sim`-Kopplung der Stroeme nach dem Rand, wie
    // `apply_sim_bindings` im Interpreter.
    let _ = writeln!(s, "    takt_sim_streams(a, takt_ticks_ns(k));");
    crate::parts::steps(s, p, layout, driven, "    ", "k", x);
    crate::parts::abort_phase(s, p, layout, driven, "    ", "k", x);
    crate::parts::signal_lines(s, p, driven, "    ", "k");
    crate::parts::idle_drops(s, p, driven, "    ", x);
    crate::parts::commit_sequence(s, p, driven, "    ", "k");
    crate::ports::sample(s, p, "    ");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    {x}_monitor_{i}(a, k);");
    }
    let _ = writeln!(s, "}}");
    locked(s, x, "void", "tick", "int64_t k", "k", "");

    sleep(s, layout, p, driven, x);
    platform(s, p, layout, x);
    wake(t, p, layout, x);
}

/// `P_woken` und `P_wake_sources`: die Weckereignisse im Schlaf (5.10, 9.9).
///
/// Der Kern fragt an jeder geschlafenen Grenze, ob der Schritt dort etwas
/// anderes saehe als der Tick vor dem Schlaf. Der Rahmen tastet dafuer die
/// Wake-Quellen ueber ihre Treiber ab, wie `takt_sample` es an dieser
/// Grenze taete: ein Wake-Command, das der Wirt meldet; ein Eingang, dessen
/// Treiber anders liefert als im letzten Tick (geliefert oder nicht, Wert,
/// Qualitaet, Grund, Alter) oder dessen Abtastung ohne Lieferung veraltet
/// (3.5); ein Strom, dessen Treiber ein Element bringt. Das Command und das
/// Element haelt der Rahmen vor, und der Tick an dieser Grenze liefert sie
/// zuerst. Eine Uhr ohne diese Frage weckte erst an der Frist, und ein
/// Taster wirkte Stunden spaeter (FB-388).
fn wake(t: &mut Text, p: &Program, layout: &Layout, x: &Prefix) {
    let scalars: Vec<BoundScalar> = bound_scalars(p, layout, x).into_iter().filter(|b| b.wake.is_some()).collect();
    let streams: Vec<BoundStream> = bound_streams(p, x).into_iter().filter(|b| b.wake.is_some()).collect();
    let commands: Vec<BoundCommand> = bound_commands(p, layout, x).into_iter().filter(|c| c.wake.is_some()).collect();
    if !scalars.is_empty() {
        let n = scalars.len();
        let _ = writeln!(t.fields, "    unsigned char wake_v[{n}][8];");
        let _ = writeln!(t.fields, "    uint8_t wake_q[{n}], wake_r[{n}], wake_got[{n}];");
        let _ = writeln!(t.fields, "    int64_t wake_age[{n}];");
        for b in scalars.iter().filter(|b| b.ct.is_none()) {
            let _ = writeln!(t.fields, "    _Alignas(8) unsigned char wake_b{}[{}];", b.wake.unwrap_or(0), b.size);
        }
    }
    if !commands.is_empty() {
        let _ = writeln!(t.fields, "    _Bool cmd_held[{}];", commands.len());
    }
    if !streams.is_empty() {
        let n = streams.len();
        let _ = writeln!(t.fields, "    _Bool wake_held[{n}];");
        let _ = writeln!(t.fields, "    int32_t wake_len[{n}];");
        let _ = writeln!(t.fields, "    int64_t wake_t[{n}], wake_seq[{n}];");
    }
    let s = &mut t.code;
    let any = u8::from(!scalars.is_empty() || !streams.is_empty() || !commands.is_empty());
    let _ = writeln!(s, "/* Weckereignisse im Schlaf (9.9). */");
    let _ = writeln!(s, "uint8_t {x}_wake_sources(void) {{ return {any}u; }}");
    let _ = writeln!(s, "static uint8_t takt_woken(struct {x}_arena *a, int64_t k) {{");
    let _ = writeln!(s, "    int64_t now = takt_ticks_ns(k);");
    for c in &commands {
        let w = c.wake.unwrap_or(0);
        let _ = writeln!(s, "    if (!a->cmd_held[{w}] && {}(a->user, now)) a->cmd_held[{w}] = 1;", c.function);
        let _ = writeln!(s, "    if (a->cmd_held[{w}]) return 1; /* {} */", c.name);
    }
    for b in &scalars {
        let w = b.wake.unwrap_or(0);
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        uint8_t q = 0, r = 0;");
        let _ = writeln!(s, "        int64_t age = 0, t = now;");
        let same = match b.ct {
            Some(ct) => {
                let _ = writeln!(s, "        {ct} v = 0;");
                let _ = writeln!(s, "        uint8_t got = {}(a->user, now, &v, &q, &r, &age, &t);", b.function);
                format!("memcmp(&v, a->wake_v[{w}], sizeof v)")
            }
            None => {
                let buf = format!("a->edge_in{}", b.channel);
                let _ = writeln!(
                    s,
                    "        uint8_t got = {}(a->user, now, {buf}, {}, &q, &r, &age, &t);",
                    b.function, b.size
                );
                format!("memcmp({buf}, a->wake_b{w}, {})", b.size)
            }
        };
        let _ = writeln!(s, "        if (got != a->wake_got[{w}]) return 1;");
        let _ = writeln!(
            s,
            "        if (got != 0 && (q != a->wake_q[{w}] || r != a->wake_r[{w}] || age != a->wake_age[{w}] || (got == 1 && {same}))) return 1;"
        );
        let name = &p.channels[b.channel].name;
        let aged = (
            p.channels[b.channel].attrs.max_age,
            crate::parts::age_offset(p, name),
            crate::parts::quality_offset(p, name),
        );
        if let (Some(max), Some(age), Some(q)) = aged {
            // Wie `aging` an dieser Grenze: alle Ticks seit dem letzten.
            let _ = writeln!(s, "        if (got == 0 && a->image[{q}] != 2 && a->image[{q}] != 3) {{");
            let _ = writeln!(s, "            long long at; __builtin_memcpy(&at, a->image + {age}, sizeof at);");
            let _ = writeln!(s, "            long long d = takt_ticks_ns(k - a->tick);");
            let _ = writeln!(s, "            if (at > {}LL - d || at + d > {max}LL) return 1;", i64::MAX);
            let _ = writeln!(s, "        }}");
        }
        let _ = writeln!(s, "    }}");
    }
    for b in &streams {
        let w = b.wake.unwrap_or(0);
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        if (a->wake_held[{w}]) return 1;");
        let _ = writeln!(s, "        long long last = a->edge_tracks[{}].last_seq;", b.channel);
        let _ = writeln!(s, "        int64_t t = now;");
        let _ = writeln!(s, "        int64_t seq = last == (-9223372036854775807LL - 1) ? 0 : last + 1;");
        let _ = writeln!(s, "        int32_t len = 0;");
        let _ = writeln!(
            s,
            "        if ({}(a->user, now, a->edge_pool + {}, {}, &len, &t, &seq)) {{",
            b.function, b.offset, b.cap
        );
        let _ = writeln!(
            s,
            "            a->wake_held[{w}] = 1; a->wake_len[{w}] = len; a->wake_t[{w}] = t; a->wake_seq[{w}] = seq;"
        );
        let _ = writeln!(s, "            return 1;");
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    (void)a; (void)now;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    // Ein Eintritt in einen laufenden weckt: Frueher zu wecken ist nie falsch.
    locked(s, x, "uint8_t", "woken", "int64_t k", "k", "1");
}

/// Die Nummer, mit der `P_next_run` ein Ende meldet: die von
/// `takt-rt-core`, deren `NextRun::from_code` jede Huelle zuruecklesen
/// laesst (GEN-029).
fn next_run_code(end: takt_mir::sys::NextRun) -> i32 {
    use takt_mir::sys::NextRun as Mir;
    use takt_rt_core::NextRun as Rt;
    match end {
        Mir::Now => Rt::Now,
        Mir::After => Rt::After(0),
        Mir::OnWake => Rt::OnWake,
        Mir::OnStart => Rt::OnStart,
    }
    .code()
}

/// `P_next_run` und `P_end`: das Ende des Laufs (12.7), wie im
/// Interpreter — die Zeile `end`, dann alle Ausgaenge auf `safe`. Was
/// zwischen zwei Laeufen geschieht, fuehrt das Board aus.
fn platform(s: &mut String, p: &Program, layout: &Layout, x: &Prefix) {
    let next = crate::parts::next_run_slot(p, layout);
    let _ = writeln!(s, "/* 12.7: 0 weiter, 1 NOW, 2 AFTER (`delay` in ns), 3 ON_WAKE, 4 ON_START. */");
    let _ = writeln!(s, "int32_t {x}_next_run(struct {x}_arena *a, int64_t *delay) {{");
    let _ = writeln!(s, "    *delay = -1;");
    if let Some(n) = &next {
        let _ = writeln!(s, "    {} which;", n.ct);
        let _ = writeln!(s, "    __builtin_memcpy(&which, a->latch + {}, sizeof which);", n.slot.offset);
        let _ = writeln!(s, "    switch (which) {{");
        for (d, end) in &n.ends {
            let code = next_run_code(*end);
            // `AFTER(delay)`: die Dauer im ersten Fach hinter der Diskriminante (11.2).
            let delay = match end {
                takt_mir::sys::NextRun::After => {
                    format!("__builtin_memcpy(delay, a->latch + {}, sizeof *delay); ", n.slot.offset + 8)
                }
                _ => String::new(),
            };
            let _ = writeln!(s, "    case {d}: {delay}return {code};");
        }
        let _ = writeln!(s, "    default: break;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let mut ends = takt_mir::sys::NextRun::ALL;
    ends.sort_by_key(|e| next_run_code(*e));
    let words: Vec<String> = ends.iter().map(|c| format!("\" end {}\\n\"", c.word())).collect();
    let _ = writeln!(s, "void {x}_end(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    static const char *const words[] = {{ {} }};", words.join(", "));
    let _ = writeln!(s, "    int64_t delay;");
    let _ = writeln!(s, "    int c = {x}_next_run(a, &delay);");
    let _ = writeln!(s, "    takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "    takt_trace_i64(a->sink, a->done);");
    let _ = writeln!(s, "    if (c >= 1 && c <= {}) takt_trace_text(a->sink, words[c - 1]);", ends.len());
    crate::parts::safe_outputs(s, p, layout);
    let _ = writeln!(s, "}}\n");
}

/// `P_idle` und `P_deadline`: darf geschlafen werden (9.9)?
///
/// 9.9 nennt sechs Konjunkte. Je Maschine beantwortet der erzeugte Code
/// drei (`idle`-Zustand, keine Zustellung, leere Wake-Stroeme); ein
/// anliegendes Wake-Kommando, ausstehende geplante Ausgaben und ein
/// Fault, der hinter einem Abort wartet, prueft der Rahmen, weil ihm
/// Prozessabbild, Warteschlangen und `pending` gehoeren — und laufende
/// Jobs (4.5): Mit ihnen schlaeft das System nicht.
fn sleep(s: &mut String, layout: &Layout, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(s, "/* Systemschlaf (9.9). */");
    let _ = writeln!(s, "static uint8_t takt_idle(struct {x}_arena *a) {{");
    if driven.is_empty() {
        let _ = writeln!(s, "    return 0;");
    } else {
        // Ein anliegendes Wake-Kommando beendet den Schlaf, bevor er
        // beginnt — sonst schliefe das System darueber hinweg.
        for slot in &layout.commands {
            if p.commands.iter().any(|c| c.name == slot.name && c.wake) {
                let _ = writeln!(s, "    if (a->image[{}]) return 0; /* {} weckt */", slot.offset, slot.name);
            }
        }
        if !crate::parts::queued_outputs(p).is_empty() {
            let _ = writeln!(s, "    if (takt_sched_pending(a)) return 0;");
        }
        let _ = writeln!(s, "    if ({x}_jobs_busy(a)) return 0;");
        // 9.6: Was an einen Strom mit `drop_oldest` gesendet ist, wird im
        // naechsten Tick zugestellt; ein Schlaf verschoebe die Zustellung.
        if crate::streams::stages(p) {
            let _ = writeln!(s, "    if (takt_int_staged(a)) return 0;");
        }
        for m in driven {
            let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
            let _ = writeln!(s, "    if (a->pending[{i}]) return 0;");
            let _ = writeln!(s, "    if (!{x}_{0}_idle(a)) return 0;", m.name);
        }
        let _ = writeln!(s, "    return 1;");
    }
    let _ = writeln!(s, "}}");
    guarded(s, x, "uint8_t", "idle", "", "");

    // Die frueheste Frist ueber alle Maschinen, als absoluter Zeitpunkt in
    // Nanosekunden — so erwartet `Program::next_deadline` sie. Die
    // Maschinen rechnen in Ticks, weil `t_in_state` sie zaehlt; die
    // Umrechnung steht hier, wo `takt_now` ohnehin die Zeitquelle ist.
    let _ = writeln!(s, "static int64_t takt_deadline(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    long long best = -1;");
    for m in driven {
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "        long long d = {x}_{0}_deadline(a);", m.name);
        let _ = writeln!(s, "        if (d >= 0 && (best < 0 || d < best)) best = d;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    if (best < 0) return -1;");
    let _ = writeln!(s, "    return takt_add_ns({x}_now(a), takt_ticks_ns(best));");
    let _ = writeln!(s, "}}");
    guarded(s, x, "int64_t", "deadline", "", "");

    // 9.9: „fuer jede Maschine: time_in_state += n*T0". Ein
    // uebersprungener Tick ruft kein `_step`; ohne das feuerte jede
    // `after`-Frist um die geschlafenen Ticks zu spaet.
    let _ = writeln!(
        s,
        "void {x}_init(struct {x}_arena *a, void *user, void *sink) {{ (void){x}_init_with(a, user, sink, 0, 0); }}\n"
    );
    // Die Maschinen zaehlen ihre Aktivierungen in den Ticks `tick + 1 ..
    // tick + n` aus dem Tick des Rahmens (`P_now`, FB-429); er rueckt darum
    // erst nach ihnen vor.
    // Die Abtastungen altern in den geschlafenen Ticks wie in leeren
    // Schritten (`Run::skip` im Interpreter): Ein Input ohne Lieferung ist
    // danach so alt, wie er ohne Schlaf waere.
    let _ = writeln!(s, "static void takt_advance(struct {x}_arena *a, int64_t n) {{");
    for m in driven {
        let _ = writeln!(s, "    {x}_{0}_advance(a, n);", m.name);
    }
    crate::parts::aging_slept(s, p, layout, "    ");
    let _ = writeln!(s, "    a->tick += n;");
    let _ = writeln!(s, "}}");
    locked(s, x, "void", "advance", "int64_t n", "n", "");

    // 8.4: die Tunables ueber den Weg der Schleife (`Program::tune`), mit
    // ihrer Zeile im Golden-Trace an der Grenze von Tick `k`.
    crate::parts::tune(s, p, layout, x);
    let _ = writeln!(
        s,
        "static void takt_tune_trace(void *sink, int64_t k, uint32_t param, const unsigned char *v, int32_t len, int32_t accepted);"
    );
    let _ = writeln!(
        s,
        "static int32_t takt_tune(struct {x}_arena *a, int64_t k, uint32_t param, const unsigned char *v, int32_t len) {{"
    );
    let _ = writeln!(s, "    int32_t r = takt_tune_value(a, param, v, len);");
    let _ = writeln!(s, "    takt_tune_trace(a->sink, k, param, v, len, r);");
    let _ = writeln!(s, "    return r;");
    let _ = writeln!(s, "}}");
    locked(
        s,
        x,
        "int32_t",
        "tune",
        "int64_t k, uint32_t param, const void *value, int32_t len",
        "k, param, (const unsigned char *)value, len",
        "0",
    );

    // 5.9: Der Board-Treiber sieht nur Bytes. Snapshot reiht die Nutzlast
    // aller Maschinen, Restore verteilt sie; die Rueckgabe zaehlt die
    // uebernommenen Eintraege, der Rest ist PersistReset.
    let persisting: Vec<&takt_mir::machine::Machine> =
        driven.iter().copied().filter(|m| !m.persist.is_empty()).collect();
    let _ = writeln!(s, "static int32_t takt_persist_snapshot(struct {x}_arena *a, void *out, int32_t cap) {{");
    let _ = writeln!(s, "    int n = 0;");
    for m in &persisting {
        let _ = writeln!(s, "    {{");
        let _ = writeln!(s, "        int k = {x}_{0}_persist_snapshot(a, (unsigned char *)out + n, cap - n);", m.name);
        let _ = writeln!(s, "        if (k == 0) return 0;");
        let _ = writeln!(s, "        n += k;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    (void)out; (void)cap;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}");
    locked(s, x, "int32_t", "persist_snapshot", "void *out, int32_t cap", "out, cap", "0");
    let _ = writeln!(s, "static int32_t takt_persist_restore(struct {x}_arena *a, const void *in, int32_t len) {{");
    let _ = writeln!(s, "    int n = 0;");
    for m in &persisting {
        let _ = writeln!(s, "    n += {x}_{0}_persist_restore(a, in, len);", m.name);
    }
    let _ = writeln!(s, "    (void)in; (void)len;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}");
    locked(s, x, "int32_t", "persist_restore", "const void *in, int32_t len", "in, len", "0");
    let entries: usize = persisting.iter().map(|m| m.persist.len()).sum();
    let _ = writeln!(s, "const int32_t {x}_persist_entries = {entries};");
    let _ = writeln!(s, "const int32_t {x}_persist_bound = {};\n", takt_mir::persist::max_payload(p).unwrap_or(0));
}

/// `P_dump`: den Latch ausgeben, fuer den Vergleich — dieselben
/// Zeilen wie `dump` im Linux-Rahmen (grammar/trace.md), damit
/// `compare` beide lesen kann. Ohne `all` nur, was sich seit der letzten
/// Ausgabe geaendert hat (9.3). Eine Tabelle je Ausgang statt Code je
/// Ausgang: So war die Funktion die groesste des Rahmens.
fn telemetry(
    t: &mut Text,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    diagnostics: takt_llvm::Diagnostics,
    x: &Prefix,
) {
    if diagnostics == takt_llvm::Diagnostics::None {
        let _ = writeln!(t.code, "void {x}_dump(struct {x}_arena *a, int32_t all) {{ (void)a; (void)all; }}\n");
        let _ = writeln!(
            t.code,
            "static void takt_tune_trace(void *sink, int64_t k, uint32_t param, const unsigned char *v, int32_t len, \
             int32_t accepted) {{ (void)sink; (void)k; (void)param; (void)v; (void)len; (void)accepted; }}\n"
        );
        program_counters(t, p, driven, x);
        sample(t, p, layout, x);
        commit(&mut t.code, p, layout, x);
        outputs(&mut t.code, layout, x);
        return;
    }
    let _ = writeln!(t.fields, "    _Alignas(8) unsigned char shown[{}];", layout.latch.max(1));
    let s = &mut t.code;
    let _ = writeln!(s, "/* Die Ausgaenge als Trace-Zeilen (grammar/trace.md); ohne `all` nur die geaenderten. */");
    let _ = writeln!(s, "{}", crate::parts::DURATION_C);
    crate::parts::tune_trace(s, p, layout);
    let _ = writeln!(s, "struct takt_variant;");
    let _ = writeln!(
        s,
        "struct takt_field {{ const struct takt_variant *names; unsigned short off; unsigned char kind; unsigned int n; }};"
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
    let bytes = kinds.contains(&BYTES);
    let arrays = named.iter().any(|n| n.starts_with("g_arr"));
    kinds.retain(|k| *k != BYTES);
    let _ = writeln!(s, "static int takt_same(const unsigned char *x, const unsigned char *y, unsigned n) {{");
    let _ = writeln!(s, "    while (n--) if (*x++ != *y++) return 0;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static long long takt_load(unsigned char kind, const unsigned char *v) {{");
    let _ = writeln!(s, "    switch (kind) {{");
    for kind in kinds.iter().filter(|k| *k & 0x80 == 0) {
        let ct = kind_c_type(*kind);
        let _ = writeln!(s, "    case {kind}: {{ {ct} x; __builtin_memcpy(&x, v, sizeof x); return (long long)x; }}");
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    if bytes {
        // `bytes<N>` liegt als Laenge (`i32`) und `N` Bytes ab Versatz 4
        // (`takt_llvm::ty::lower`); geschrieben wie `value_text` (9.3).
        let _ = writeln!(s, "static void takt_dump_bytes(void *sink, const unsigned char *at, unsigned int cap) {{");
        let _ = writeln!(s, "    int len;");
        let _ = writeln!(s, "    __builtin_memcpy(&len, at, sizeof len);");
        let _ = writeln!(s, "    unsigned int n = len < 0 ? 0u : (unsigned int)len;");
        let _ = writeln!(s, "    if (n > cap) n = cap;");
        let _ = writeln!(s, "    takt_trace_text(sink, \"[\");");
        let _ = writeln!(s, "    for (unsigned int i = 0; i < n; i++) {{");
        let _ = writeln!(s, "        if (i) takt_trace_text(sink, \", \");");
        let _ = writeln!(s, "        takt_trace_hex8(sink, at[4 + i]);");
        let _ = writeln!(s, "    }}");
        let _ = writeln!(s, "    takt_trace_text(sink, \"]\");");
        let _ = writeln!(s, "}}");
    }
    let _ =
        writeln!(s, "static void takt_dump_fields(void *sink, const struct takt_variant *x, const unsigned char *at);");
    let _ =
        writeln!(s, "static void takt_dump_field(void *sink, const struct takt_field *f, const unsigned char *at) {{");
    let _ = writeln!(s, "    if (f->kind == {RECORD}) {{ takt_dump_fields(sink, f->names, at); return; }}");
    if bytes {
        let _ = writeln!(s, "    if (f->kind == {BYTES}) {{ takt_dump_bytes(sink, at, f->n); return; }}");
    }
    if arrays {
        let _ = writeln!(s, "    if (f->kind == {ARRAY}) {{");
        let _ = writeln!(s, "        const struct takt_field *e = &f->names->fields[0];");
        let _ = writeln!(s, "        takt_trace_text(sink, \"[\");");
        let _ = writeln!(s, "        for (unsigned int i = 0; i < f->n; i++) {{");
        let _ = writeln!(s, "            if (i) takt_trace_text(sink, \", \");");
        let _ = writeln!(s, "            takt_dump_field(sink, e, at + (size_t)i * e->off);");
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "        takt_trace_text(sink, \"]\");");
        let _ = writeln!(s, "        return;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    long long v = takt_load(f->kind, at);");
    let _ = writeln!(s, "    if (f->names) {{");
    let _ = writeln!(s, "        for (unsigned i = 0; i < f->n; i++)");
    let _ = writeln!(s, "            if (f->names[i].d == v) {{ takt_trace_text(sink, f->names[i].name); return; }}");
    let _ = writeln!(s, "        takt_trace_text(sink, \"?\");");
    let _ = writeln!(s, "    }} else if (f->kind == {BOOL}) takt_trace_text(sink, v ? \"true\" : \"false\");");
    let _ = writeln!(
        s,
        "    else if (f->kind == 0x84) {{ float x; __builtin_memcpy(&x, at, sizeof x); takt_trace_f64(sink, (double)x); }}"
    );
    let _ = writeln!(
        s,
        "    else if (f->kind == 0x88) {{ double x; __builtin_memcpy(&x, at, sizeof x); takt_trace_f64(sink, x); }}"
    );
    let _ = writeln!(s, "    else if (f->kind == {DURATION}) {{ takt_dur_trace(sink, v); }}");
    let _ = writeln!(s, "    else if (f->kind & 0x40) takt_trace_u64(sink, (unsigned long long)v);");
    let _ = writeln!(s, "    else takt_trace_i64(sink, v);");
    let _ = writeln!(s, "}}");
    // `Name(f1, f2)` wie `value_text` im Interpreter (9.3).
    let _ = writeln!(
        s,
        "static void takt_dump_fields(void *sink, const struct takt_variant *x, const unsigned char *at) {{"
    );
    let _ = writeln!(s, "    takt_trace_text(sink, x->name);");
    let _ = writeln!(s, "    if (!x->n_fields) return;");
    let _ = writeln!(s, "    takt_trace_text(sink, \"(\");");
    let _ = writeln!(s, "    for (unsigned k = 0; k < x->n_fields; k++) {{");
    let _ = writeln!(s, "        if (k) takt_trace_text(sink, \", \");");
    let _ = writeln!(s, "        takt_dump_field(sink, &x->fields[k], at + x->fields[k].off);");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_trace_text(sink, \")\");");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_dump_value(void *sink, const struct takt_out *o, const unsigned char *v) {{");
    let _ = writeln!(s, "    if (o->variants) {{");
    let _ = writeln!(s, "        long long d = takt_load(o->kind, v);");
    let _ = writeln!(s, "        for (unsigned i = 0; i < o->n_variants; i++) {{");
    let _ = writeln!(s, "            const struct takt_variant *x = &o->variants[i];");
    let _ = writeln!(s, "            if (x->d != d) continue;");
    let _ = writeln!(s, "            takt_dump_fields(sink, x, v);");
    let _ = writeln!(s, "            return;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        takt_trace_text(sink, \"?\");");
    let _ = writeln!(s, "        return;");
    let _ = writeln!(s, "    }}");
    for kind in kinds.iter().filter(|k| *k & 0x80 != 0) {
        let ct = kind_c_type(*kind);
        let _ = writeln!(
            s,
            "    if (o->kind == {kind}) {{ {ct} x; __builtin_memcpy(&x, v, sizeof x); takt_trace_f64(sink, (double)x); return; }}"
        );
    }
    if kinds.contains(&DURATION) {
        let _ = writeln!(s, "    if (o->kind == {DURATION}) {{");
        let _ = writeln!(s, "        long long d = takt_load(o->kind, v);");
        let _ = writeln!(s, "        takt_dur_trace(sink, d);");
        let _ = writeln!(s, "        return;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    if (o->kind & 0x40) takt_trace_u64(sink, (unsigned long long)takt_load(o->kind, v));");
    let _ = writeln!(s, "    else takt_trace_i64(sink, takt_load(o->kind, v));");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_dump(struct {x}_arena *a, int32_t all) {{");
    let _ = writeln!(s, "    for (unsigned i = 0; i < {n}; i++) {{");
    let _ = writeln!(s, "        const struct takt_out *o = &g_outs[i];");
    let _ = writeln!(s, "        const unsigned char *v = a->latch + o->off;");
    let _ = writeln!(s, "        if (!all && takt_same(v, a->shown + o->off, o->size)) continue;");
    let _ = writeln!(s, "        memcpy(a->shown + o->off, v, o->size);");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "        takt_trace_i64(a->sink, a->done);");
    let _ = writeln!(s, "        takt_trace_text(a->sink, \" out \");");
    let _ = writeln!(s, "        takt_trace_text(a->sink, o->name);");
    let _ = writeln!(s, "        if (o->count) {{");
    let _ = writeln!(s, "            unsigned w = o->size / o->count;");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \" [\");");
    let _ = writeln!(s, "            for (unsigned k = 0; k < o->count; k++) {{");
    let _ = writeln!(s, "                if (k) takt_trace_text(a->sink, \", \");");
    let _ = writeln!(s, "                takt_dump_value(a->sink, o, v + (size_t)k * w);");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \"]\\n\");");
    let _ = writeln!(s, "        }} else {{");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \" \");");
    let _ = writeln!(s, "            takt_dump_value(a->sink, o, v);");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \"\\n\");");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_stream_report(a, a->done);");
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "dump", "int32_t all", "all");
    program_counters(t, p, driven, x);
    sample(t, p, layout, x);
    commit(&mut t.code, p, layout, x);
    outputs(&mut t.code, layout, x);
}

/// `P_pc`: wo jede Maschine steht (11.2, Instrumentierung `statements`
/// oder `states`), wie ein Zustand nur, wenn er sich aendert, und mit `all`
/// jede (grammar/trace.md, T4). Ohne Instrumentierung bleibt die Funktion
/// leer.
fn program_counters(t: &mut Text, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let counters: Vec<(&str, u64)> = driven
        .iter()
        .filter_map(|m| {
            let st = takt_llvm::machine::state_struct(m, p)?;
            Some((m.name.as_str(), st.byte_offset(takt_llvm::machine::Role::Pc, 0)?))
        })
        .collect();
    if !counters.is_empty() {
        let _ = writeln!(t.fields, "    int32_t pc_shown[{}];", counters.len());
    }
    let s = &mut t.code;
    let _ = writeln!(s, "/* Der Programmzaehler je Maschine (11.2), nur bei Aenderung (T4). */");
    let _ = writeln!(s, "void {x}_pc(struct {x}_arena *a, int32_t all) {{");
    let _ = writeln!(s, "    (void)a; (void)all;");
    for (i, (name, at)) in counters.iter().enumerate() {
        let state = takt_llvm::arena::state_name(name);
        let _ = writeln!(s, "    {{ int32_t pc; __builtin_memcpy(&pc, a->{state} + {at}, sizeof pc);");
        let _ = writeln!(s, "    if (all || pc != a->pc_shown[{i}]) {{");
        let _ = writeln!(s, "        a->pc_shown[{i}] = pc;");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "        takt_trace_i64(a->sink, a->done);");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \" pc {name} \");");
        let _ = writeln!(s, "        takt_trace_i64(a->sink, a->pc_shown[{i}]);");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \"\\n\");");
        let _ = writeln!(s, "    }} }}");
    }
    let _ = writeln!(s, "}}\n");
}

/// Die Art einer Dauer: acht Byte mit Vorzeichen, geschrieben in ihrer
/// groessten ganzzahligen Einheit wie im Interpreter (T2).
const DURATION: u8 = 0x28;

/// Ein Record in der Ausgabetabelle (FB-312): Seine Felder stehen in der
/// Variante, auf die `names` zeigt. `takt_load` liefert fuer ihn null.
const RECORD: u8 = 0x10;

/// Ein Feld `bytes<N>` (FB-402): Laenge und Bytes, geschrieben als Liste
/// wie im Interpreter; `n` der Feldbeschreibung ist `N`.
const BYTES: u8 = 0x12;

/// Ein Feld `[N] T`: `N` Elemente als Liste wie im Interpreter. Die
/// Variante, auf die `names` zeigt, beschreibt das Element in ihrem einzigen
/// Feld; dessen Versatz ist der Abstand der Elemente, `n` ist `N`.
const ARRAY: u8 = 0x13;

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
/// auf seine eigene Variante, ein Feld `bytes<N>` mit seiner Kapazitaet
/// (FB-402). `None` fuer alles andere, dann faellt der Ausgang aus der
/// Tabelle. Jeder Record-Typ steht einmal in der Tabelle.
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
        let (names, kind, n) = field_entry(s, p, f.ty, part, named, kinds)?;
        items.push(format!("{{ {names}, {off}, {kind}, {n} }}"));
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

/// Ein Feld eines Records in der Ausgabetabelle: Tabelle, Art und `n` wie
/// in `struct takt_field`. Ein Feld `[N] T` zeigt auf eine Variante mit dem
/// Element als einzigem Feld, dessen Versatz der Abstand der Elemente ist
/// ([`ARRAY`]). `None` fuer einen Typ ohne Schreibweise in der Tabelle.
fn field_entry(
    s: &mut String,
    p: &Program,
    ty: takt_mir::TypeId,
    part: &takt_llvm::ty::LlvmType,
    named: &mut std::collections::BTreeSet<String>,
    kinds: &mut Vec<u8>,
) -> Option<(String, u8, usize)> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::types::Type;
    let entry = match (p.types.get(ty), part) {
        (Type::Record(_), _) => (record_variant(s, p, ty, part, named, kinds)?, RECORD, 1),
        (Type::Array { elem, .. }, LlvmType::Array(inner, len)) => {
            let (names, kind, n) = field_entry(s, p, *elem, inner, named, kinds)?;
            let symbol = format!("g_arr{}", named.len());
            let _ = writeln!(
                s,
                "static const struct takt_field {symbol}_f[] = {{ {{ {names}, {}, {kind}, {n} }} }};",
                inner.aligned_size()
            );
            let _ = writeln!(s, "static const struct takt_variant {symbol}[] = {{ {{ 0LL, \"\", {symbol}_f, 1 }} }};");
            named.insert(symbol.clone());
            (symbol, ARRAY, usize::try_from(*len).ok()?)
        }
        (Type::Bytes { cap }, LlvmType::Struct(_)) => ("0".to_string(), BYTES, *cap as usize),
        (Type::Bool, LlvmType::Int(1)) => ("0".to_string(), BOOL, 0),
        (Type::Duration { .. }, LlvmType::Int(64)) => ("0".to_string(), DURATION, 0),
        (Type::Enum(e), LlvmType::Int(_)) => {
            let edef = p.enums.get(e.index())?;
            if edef.variants.iter().any(|v| !v.fields.is_empty()) {
                return None;
            }
            let table = format!("g_enum_{}", edef.name);
            if named.insert(edef.name.clone()) {
                let rows: Vec<String> =
                    edef.variants.iter().map(|w| format!("{{ {}LL, \"{}\", 0, 0 }}", w.discriminant, w.name)).collect();
                let _ = writeln!(s, "static const struct takt_variant {table}[] = {{ {} }};", rows.join(", "));
            }
            (table, value_kind(part, true)?, edef.variants.len())
        }
        (Type::Int { width, .. }, LlvmType::Int(_)) => ("0".to_string(), value_kind(part, width.signed())?, 0),
        (Type::Float { .. }, LlvmType::F32 | LlvmType::F64) => ("0".to_string(), value_kind(part, true)?, 0),
        _ => return None,
    };
    if entry.1 != RECORD && entry.1 != ARRAY {
        kinds.push(entry.1);
    }
    Some(entry)
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

/// `takt_sample`: die Treiber an das Abbild (12.1, Schritt 2).
///
/// **Das Gegenstueck zu [`commit`], und aus demselben Grund ein Symbol.**
/// Aus `hw("ui/button")` wird `P_in_ui_button(user, now, &value, &quality,
/// &reason, &age, &t)`; wer den Kanal bindet und keinen Treiber stellt,
/// bekommt einen Linkfehler mit dem Namen darin. Eine Registrierung zur
/// Laufzeit waere flexibler, aber ein nicht eingetragener Eingang fiele still
/// aus — und ein Eingang, der still `Bad` bleibt, ist schlimmer als einer,
/// der fehlt.
///
/// **Warum Qualitaet, Grund und Alter mitkommen.** 12.6 laesst Eingaenge
/// degradieren statt zu faulten: Ein Treiber, der nichts Frisches hat, sagt
/// `Stale` mit dem Alter seines Werts, einer mit unplausiblem Wert `Suspect`
/// oder `Bad` mit Grund; Zeile 2 prueft, dass der Messzeitpunkt `t - age`
/// nicht faellt. Ohne diesen Rueckweg koennte er nur luegen oder schweigen.
/// Liefert er nichts, bleibt der Eintrag, wie `init` ihn gesetzt hat: `Bad`
/// (3.5).
///
/// Ein Eingang zusammengesetzten Typs liefert in einen Puffer der Arena, der
/// bis zum Abgleich des Rands haelt (`edge_in<kanal>`). Je Command fragt der
/// Rahmen den Wirt, ob es in diesem Tick anliegt (8.5).
///
/// Eingaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf. Einen
/// Eingang, den im Sim-Build ein `sim`-Output derselben Adresse speist,
/// fragt der Rahmen nach der Kopplung im Commit: Liefert sein Treiber in
/// diesem Tick, gilt die Lieferung statt des Modells, wie ein Stimulus im
/// Interpreter (8.3); liefert er nichts, bleibt der Wert des Modells.
fn sample(t: &mut Text, p: &Program, layout: &Layout, x: &Prefix) {
    let scalars = bound_scalars(p, layout, x);
    let streams = bound_streams(p, x);
    let commands = bound_commands(p, layout, x);
    for b in scalars.iter().filter(|b| b.ct.is_none()) {
        let _ = writeln!(t.fields, "    _Alignas(8) unsigned char edge_in{}[{}];", b.channel, b.size);
    }
    // Ein Platz je Element, das ein Strom in einem Tick liefern darf, und
    // eines mehr, damit der Rand `MAXPT` pruefen kann (12.6 Zeile 2).
    let pool: u64 = streams.iter().map(|b| u64::from(b.polls) * u64::from(b.cap)).sum();
    if pool > 0 {
        let _ = writeln!(t.fields, "    _Alignas(8) unsigned char edge_pool[{pool}];");
    }
    record_fields(t, p);
    let reported = tx_slots(p, &crate::drivers::tx_read(p));
    if let Some(n) = reported.iter().map(|(_, slot)| slot + 1).max() {
        let _ = writeln!(t.fields, "    int tx_last_free[{n}];");
        let _ = writeln!(t.fields, "    unsigned char tx_last_idle[{n}], tx_seen[{n}];");
    }
    let s = &mut t.code;
    record(s, p, x);

    let _ = writeln!(s, "\n/* Schritt 2: die Lieferungen an den Rand, der Rand ins Abbild (12.1, 12.6). */");
    let _ = writeln!(s, "static void takt_sample(struct {x}_arena *a) {{");
    let body = s.len();
    for b in &scalars {
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        uint8_t q = 0, r = 0;");
        let _ = writeln!(s, "        int64_t age = 0, t = now;");
        // `Bad` kommt ohne Wert (12.6 Zeile 2).
        let buf = format!("a->edge_in{}", b.channel);
        let value = match b.ct {
            Some(ct) => {
                let _ = writeln!(s, "        {ct} v = 0;");
                let _ = writeln!(s, "        uint8_t got = {}(a->user, now, &v, &q, &r, &age, &t);", b.function);
                "&v, (int)sizeof v".to_string()
            }
            None => {
                let _ = writeln!(
                    s,
                    "        uint8_t got = {}(a->user, now, {buf}, {}, &q, &r, &age, &t);",
                    b.function, b.size
                );
                format!("{buf}, {}", b.size)
            }
        };
        let _ = writeln!(
            s,
            "        if (got != 0) takt_edge_reading(a, {}, {value}, {}, q, r, got == 1, t, age);",
            b.channel, b.number
        );
        // 9.9: Was dieser Tick las, vergleicht `P_woken` im Schlaf.
        if let Some(w) = b.wake {
            let kept = match b.ct {
                Some(_) => format!("memcpy(a->wake_v[{w}], &v, sizeof v);"),
                None => format!("memcpy(a->wake_b{w}, {buf}, {});", b.size),
            };
            let _ = writeln!(
                s,
                "        a->wake_got[{w}] = got; a->wake_q[{w}] = q; a->wake_r[{w}] = r; a->wake_age[{w}] = age; {kept}"
            );
        }
        let _ = writeln!(s, "    }}");
    }
    for b in &streams {
        let off = b.offset;
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        long long last = a->edge_tracks[{}].last_seq;", b.channel);
        let _ = writeln!(s, "        long long next = last == (-9223372036854775807LL - 1) ? 0 : last + 1;");
        let _ = writeln!(s, "        int i = 0;");
        // 9.9: Das Element, das `P_woken` an dieser Grenze las, kommt zuerst;
        // es steht schon auf dem ersten Platz.
        if let Some(w) = b.wake {
            let _ = writeln!(s, "        if (a->wake_held[{w}]) {{");
            let _ = writeln!(s, "            int32_t len = a->wake_len[{w}];");
            let _ = writeln!(s, "            a->wake_held[{w}] = 0;");
            let _ = writeln!(s, "            if (len < 0) len = 0;");
            let _ = writeln!(s, "            if (len > {0}) len = {0};", b.keep);
            let _ = writeln!(s, "            next = a->wake_seq[{w}] + 1;");
            let _ = writeln!(
                s,
                "            takt_edge_element(a, {}, a->edge_pool + {off}, len, a->wake_t[{w}], a->wake_seq[{w}]);",
                b.channel
            );
            let _ = writeln!(s, "            i = 1;");
            let _ = writeln!(s, "        }}");
        }
        let _ = writeln!(s, "        for (; i < {}; i++) {{", b.polls);
        let _ = writeln!(s, "            unsigned char *buf = a->edge_pool + {off} + (ptrdiff_t)i * {};", b.cap);
        let _ = writeln!(s, "            int32_t len = 0;");
        let _ = writeln!(s, "            int64_t t = now, seq = next;");
        let _ = writeln!(s, "            if (!{}(a->user, now, buf, {}, &len, &t, &seq)) break;", b.function, b.cap);
        let _ = writeln!(s, "            if (len < 0) len = 0;");
        let _ = writeln!(s, "            if (len > {0}) len = {0};", b.keep);
        let _ = writeln!(s, "            next = seq + 1;");
        let _ = writeln!(s, "            takt_edge_element(a, {}, buf, len, t, seq);", b.channel);
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
    }
    // 8.8, FB-124: `tx.idle` zu Tickbeginn — der eigene Puffer leer
    // (`tx_busy`, beim Commit gesetzt) und der Sender fertig nach seinem
    // Treiber. Wer es nicht beantworten kann (-1), haelt die Leitung.
    for (channel, slot) in tx_slots(p, &crate::drivers::idle_read(p)) {
        let takt_mir::program::Binding::Hw(addr) = &p.channels[channel].binding else { continue };
        let _ = writeln!(s, "    a->tx_hold[{slot}] = {x}_idle_{}(a->user, now) != 1;", addr.ident());
    }
    // 12.5, FB-435: Was der Treiber meldet, ist ein Input; der Lauf zeichnet
    // es als `tx` auf, im Tick 0 und bei jeder Aenderung.
    for (channel, slot) in &reported {
        let _ = writeln!(s, "    {{ int free = takt_tx_cap({channel}) - a->tx_n[{slot}];");
        let _ = writeln!(s, "      unsigned char idle = !a->tx_busy[{slot}] && !a->tx_hold[{slot}];");
        let _ = writeln!(
            s,
            "      if (!a->tx_seen[{slot}] || free != a->tx_last_free[{slot}] || idle != a->tx_last_idle[{slot}]) {{"
        );
        let _ = writeln!(s, "        takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "        takt_trace_i64(a->sink, a->tick);");
        let _ = writeln!(s, "        takt_trace_text(a->sink, \" tx {} free=\");", p.channels[*channel].name);
        let _ = writeln!(s, "        takt_trace_i64(a->sink, free);");
        let _ = writeln!(s, "        takt_trace_text(a->sink, idle ? \" idle=true\\n\" : \" idle=false\\n\");");
        let _ = writeln!(s, "        a->tx_seen[{slot}] = 1;");
        let _ = writeln!(s, "        a->tx_last_free[{slot}] = free;");
        let _ = writeln!(s, "        a->tx_last_idle[{slot}] = idle;");
        let _ = writeln!(s, "      }} }}");
    }
    if !p.recorded.is_empty() {
        let _ = writeln!(s, "    takt_record(a, now);");
    }
    // 8.5: Ein Command gilt genau den Tick, an dessen Grenze der Wirt es
    // meldet; ein Wake-Command, das `P_woken` schon las, zuerst.
    for c in &commands {
        let asked = format!("{}(a->user, now)", c.function);
        let _ = match c.wake {
            Some(w) => writeln!(
                s,
                "    a->image[{}] = (a->cmd_held[{w}] || {asked}) ? 1 : 0; a->cmd_held[{w}] = 0; /* {} */",
                c.offset, c.name
            ),
            None => writeln!(s, "    a->image[{}] = {asked} ? 1 : 0; /* {} */", c.offset, c.name),
        };
    }
    let _ = writeln!(s, "    takt_edge_commit(a, a->tick);");
    let _ = writeln!(s, "}}\n");
    with_now(s, body, "takt_ticks_ns(a->tick)");
}

/// Stellt `now`, die Zeit des Ticks, an den Anfang des Rumpfs ab `body`,
/// wenn er sie nennt; ein ungelesener Wert waere toter Code (MISRA C:2012
/// 2.2, plan/cert.md „Der C-Teil“).
fn with_now(s: &mut String, body: usize, value: &str) {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let code = &s[body..];
    let named = code.match_indices("now").any(|(at, _)| {
        !code[..at].chars().next_back().is_some_and(ident) && !code[at + 3..].chars().next().is_some_and(ident)
    });
    if named {
        s.insert_str(body, &format!("    int64_t now = {value};\n"));
    }
}

/// Die hw-gebundenen Ausgabestroeme unter `read` mit ihrem Platz im
/// Sendepuffer des Rahmens (`takt_tx_slot`: die Ausgabestroeme in der
/// Reihenfolge der Channels, streams.rs).
fn tx_slots(p: &Program, read: &[usize]) -> Vec<(usize, usize)> {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    p.channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
        .enumerate()
        .filter(|(_, (i, c))| read.contains(i) && matches!(c.binding, Binding::Hw(_)))
        .map(|(slot, (i, _))| (i, slot))
        .collect()
}

/// Die importierten Inputs, die das Programm nicht liest (8.2): je Tick
/// ueber ihren Treiber gelesen und als Metazeile `rec` aufgezeichnet, in der
/// Form einer `in`-Zeile — ein Skalar in Tick 0 und bei jeder Aenderung
/// (T4), ein Strom je Element. Die Qualitaet steht wie am Rand: `Bad` mit
/// Grund `Driver`; eine Diskriminante, die das Enum nicht kennt, ist `Bad`
/// mit Grund `OutOfRange` (12.6 Zeile 3).
fn record(s: &mut String, p: &Program, x: &Prefix) {
    if p.recorded.is_empty() {
        return;
    }
    let _ = writeln!(s, "\n/* Aufgezeichnete Inputs, die das Programm nicht liest (8.2, 12.5). */");
    let _ = writeln!(s, "static void takt_record(struct {x}_arena *a, int64_t now) {{");
    for (i, r) in p.recorded.iter().enumerate() {
        if let Some(st) = r.stream {
            record_stream(s, p, i, r, st, x);
            continue;
        }
        let (f, ct) = (format!("{x}_in_{}", r.address.ident()), crate::drivers::recorded_value(r.value).c);
        let _ = writeln!(s, "    {{ /* {} */", r.name);
        let _ = writeln!(s, "        {ct} v = 0;");
        let _ = writeln!(s, "        uint8_t q = 0, r = 0;");
        let _ = writeln!(s, "        int64_t age = 0, t = now;");
        let _ = writeln!(
            s,
            "        if ({f}(a->user, now, &v, &q, &r, &age, &t) && (!a->rec_seen_{i} || memcmp(&v, &a->rec_last_{i}, sizeof v) != 0 || q != a->rec_last_q_{i} || t != now)) {{"
        );
        let _ = writeln!(s, "            a->rec_seen_{i} = 1; a->rec_last_{i} = v; a->rec_last_q_{i} = q;");
        let _ = writeln!(s, "            takt_trace_text(a->sink, \"t=\");");
        let _ = writeln!(s, "            takt_trace_i64(a->sink, a->tick);");
        let _ = writeln!(s, "            takt_trace_text(a->sink, \" rec {} \");", r.name);
        let _ = writeln!(s, "            if (q == 1) takt_trace_text(a->sink, \"suspect\");");
        let _ = writeln!(s, "            else if (q == 2) takt_trace_text(a->sink, \"stale\");");
        // Der Grund, den der Treiber nennt (`Reason` der Prelude); ohne ihn keiner.
        let _ = writeln!(
            s,
            "            else if (q == 3) {{ static const char *const why[] = {{ \"bad\", \"bad reason=OutOfRange\", \"bad reason=Implausible\", \"bad reason=Driver\", \"bad reason=Node\" }}; takt_trace_text(a->sink, r < 5 ? why[r] : \"bad\"); }}"
        );
        let _ = writeln!(s, "            else {}", recorded_value(p, r));
        let _ = writeln!(
            s,
            "            if (t != now) {{ takt_trace_text(a->sink, \" t=\"); takt_trace_i64(a->sink, t); }}"
        );
        let _ = writeln!(s, "            takt_trace_text(a->sink, \"\\n\");");
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}");
}

/// Die Felder der Aufzeichnung (8.2) in der Arena: je Strom sein Puffer und
/// die naechste Folgenummer, je Skalar der zuletzt geschriebene Wert.
fn record_fields(t: &mut Text, p: &Program) {
    for (i, r) in p.recorded.iter().enumerate() {
        if let Some(st) = r.stream {
            // Ein Byte mehr als jedes gueltige Element, wie bei einem gebundenen Strom.
            let _ = writeln!(t.fields, "    _Alignas(8) unsigned char rec_{i}[{}];", st.bytes + 1);
            let _ = writeln!(t.fields, "    long long rec_next_{i};");
            continue;
        }
        let ct = crate::drivers::recorded_value(r.value).c;
        let _ = writeln!(t.fields, "    {ct} rec_last_{i};");
        let _ = writeln!(t.fields, "    unsigned char rec_last_q_{i}, rec_seen_{i};");
    }
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
    x: &Prefix,
) {
    let polls = takt_hal::edge::maxpt(st.max_rate_hz, p.config.tick).unwrap_or(1).saturating_add(1);
    let cap = st.bytes + 1;
    let f = format!("{x}_poll_{}", r.address.ident());
    let _ = writeln!(s, "    {{ /* {} */", r.name);
    let _ = writeln!(s, "        for (int i = 0; i < {polls}; i++) {{");
    let _ = writeln!(s, "            int32_t len = 0;");
    let _ = writeln!(s, "            int64_t t = now, seq = a->rec_next_{i}, expected = a->rec_next_{i};");
    let _ = writeln!(s, "            if (!{f}(a->user, now, a->rec_{i}, {cap}, &len, &t, &seq)) break;");
    let _ = writeln!(s, "            if (len < 0) len = 0;");
    let _ = writeln!(s, "            if (len > {cap}) len = {cap};");
    let _ = writeln!(s, "            a->rec_next_{i} = seq + 1;");
    let wire = r.value == takt_mir::program::RecordedValue::Wire;
    if !wire {
        let _ = writeln!(s, "            if (len != 1) continue;");
    }
    let _ = writeln!(s, "            takt_trace_text(a->sink, \"t=\");");
    let _ = writeln!(s, "            takt_trace_i64(a->sink, a->tick);");
    let _ = writeln!(s, "            takt_trace_text(a->sink, \" rec {} \");", r.name);
    if wire {
        // Zwei Ziffern je Byte und Aufruf: `takt_trace_hex8` schreibt je Byte
        // ein `0x`, und eine Zeile am Stueck kann laenger sein, als ein Haken
        // auf einmal liest.
        let _ = writeln!(s, "            static const char digits[] = \"0123456789abcdef\";");
        let _ = writeln!(s, "            takt_trace_text(a->sink, \"0x\");");
        let _ = writeln!(s, "            for (int b = 0; b < len; b++) {{");
        let _ =
            writeln!(s, "                char d[3] = {{ digits[a->rec_{i}[b] >> 4], digits[a->rec_{i}[b] & 15], 0 }};");
        let _ = writeln!(s, "                takt_trace_text(a->sink, d);");
        let _ = writeln!(s, "            }}");
    } else {
        let _ = writeln!(s, "            takt_trace_i64(a->sink, (long long)a->rec_{i}[0]);");
    }
    let _ =
        writeln!(s, "            if (t != now) {{ takt_trace_text(a->sink, \" t=\"); takt_trace_i64(a->sink, t); }}");
    let _ = writeln!(
        s,
        "            if (seq != expected) {{ takt_trace_text(a->sink, \" seq=\"); takt_trace_i64(a->sink, seq); }}"
    );
    let _ = writeln!(s, "            takt_trace_text(a->sink, \"\\n\");");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
}

/// Die Anweisung, die den Wert `v` eines aufgezeichneten Inputs in die
/// Zeile `rec` schreibt, in der Literalform der Sprache (`grammar/trace.md` T2).
fn recorded_value(p: &Program, r: &takt_mir::program::Recorded) -> String {
    use takt_mir::program::RecordedValue;
    match r.value {
        RecordedValue::Bool => "takt_trace_text(a->sink, v ? \"true\" : \"false\");".to_string(),
        RecordedValue::Int(w) if !w.signed() && w.bits() == 64 => {
            "takt_trace_u64(a->sink, (unsigned long long)v);".to_string()
        }
        RecordedValue::Int(_) => "takt_trace_i64(a->sink, (long long)v);".to_string(),
        RecordedValue::Float(_) => match &r.unit {
            Some(unit) => format!("{{ takt_trace_f64(a->sink, (double)v); takt_trace_text(a->sink, \" {unit}\"); }}"),
            None => "takt_trace_f64(a->sink, (double)v);".to_string(),
        },
        RecordedValue::Enum(id) => {
            let cases: String = p.enums[id.index()]
                .variants
                .iter()
                .map(|x| format!(" case {}: takt_trace_text(a->sink, \"{}\"); break;", x.discriminant, x.name))
                .collect();
            format!("switch (v) {{{cases} default: takt_trace_text(a->sink, \"bad reason=OutOfRange\"); }}")
        }
        RecordedValue::Wire => String::new(),
    }
}

/// Ein Eingang mit Platz im Abbild, den ein Treiber des Boards liefert.
struct BoundScalar {
    name: String,
    channel: usize,
    function: String,
    /// Der C-Typ des Werts; `None` fuer einen Eingang zusammengesetzten
    /// Typs, der als Bytes in seiner Speicherform kommt (`drivers::Kind::Block`).
    ct: Option<&'static str>,
    /// Die Bytes des Werts.
    size: u64,
    /// Art, `as_i64` und `as_f64` des Werts `v` fuer den Rand.
    number: String,
    /// Der Platz unter den Wake-Quellen (5.10), wenn der Kanal eine ist.
    wake: Option<usize>,
}

/// Die an Hardware gebundenen Eingaenge mit Platz im Abbild — dieselben wie
/// in [`crate::drivers::of`]. Speist im Sim-Build ein Modell den Eingang,
/// fragt der Rand nach der Kopplung den Treiber: Was er liefert, gilt in
/// diesem Tick statt des Modells, wie ein Stimulus im Interpreter (8.3,
/// `Image::apply_sim_bindings`).
fn bound_scalars(p: &Program, layout: &Layout, x: &Prefix) -> Vec<BoundScalar> {
    use takt_mir::types::Type;
    let mut out: Vec<BoundScalar> = layout
        .inputs
        .iter()
        .filter_map(|slot| {
            let channel = p.channels.iter().position(|c| c.name == slot.name)?;
            // `sys/outer_fault` stellt der Kern (`parts::outer_fault`).
            if takt_mir::sys::outer_fault(p) == Some(channel) {
                return None;
            }
            let address = slot.address.as_ref()?;
            let function = crate::drivers::input_symbol(address, x);
            let ct = c_type(&slot.ty, slot.signed);
            // Das Geraet `sys` hat nur Skalare (12.7).
            if ct.is_none() && takt_mir::sys::is_sys(&address.text()) {
                return None;
            }
            let number = match (ct, p.types.list.get(p.channels[channel].ty.index())?) {
                (None, _) => "0, 0LL, 0.0".to_string(),
                (_, Type::Int { width, .. }) if !width.signed() && width.bits() == 64 => {
                    "v <= 9223372036854775807ULL ? 1 : 2, (long long)v, (double)v".to_string()
                }
                (_, Type::Int { .. } | Type::Duration { .. }) => "1, (long long)v, (double)v".to_string(),
                (_, Type::Float { .. }) => "2, 0LL, (double)v".to_string(),
                _ => "0, 0LL, 0.0".to_string(),
            };
            let name = slot.name.clone();
            Some(BoundScalar { name, channel, function, ct, size: slot.size, number, wake: None })
        })
        .collect();
    number_wake(p, out.iter_mut().map(|b| (b.channel, &mut b.wake)));
    out
}

/// Ein Command, das der Wirt meldet (8.5).
struct BoundCommand {
    name: String,
    function: String,
    /// Sein Byte im Abbild.
    offset: u64,
    /// Der Platz unter den Wake-Commands (5.10), wenn es eines ist.
    wake: Option<usize>,
}

/// Die Commands des Programms mit ihrem Eintrag des Wirts (`P_cmd_<name>`).
fn bound_commands(p: &Program, layout: &Layout, x: &Prefix) -> Vec<BoundCommand> {
    let mut n = 0;
    layout
        .commands
        .iter()
        .map(|slot| {
            let woken = p.commands.iter().any(|c| c.name == slot.name && c.wake);
            let wake = woken.then(|| {
                n += 1;
                n - 1
            });
            BoundCommand {
                name: slot.name.clone(),
                function: x.name(&crate::drivers::method(crate::drivers::Kind::Command, &slot.name)),
                offset: slot.offset,
                wake,
            }
        })
        .collect()
}

/// Numeriert die Wake-Quellen unter `bound` in ihrer Reihenfolge.
fn number_wake<'a>(p: &Program, bound: impl Iterator<Item = (usize, &'a mut Option<usize>)>) {
    let mut n = 0;
    for (channel, wake) in bound {
        if p.channels[channel].attrs.wake {
            *wake = Some(n);
            n += 1;
        }
    }
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
    /// So viele Bytes gehen an den Rand: `str<N>` und `bytes<N>` kuerzt er
    /// auf ihr `N` (3.9); ein Record behaelt das eine Byte mehr fuer Zeile 5,
    /// ein `line<N>` fuer `.truncated`, das der Rand setzt.
    keep: u32,
    /// `MAXPT + 1`: so oft fragt der Rahmen je Tick.
    polls: u32,
    /// Der Versatz seiner Plaetze in `edge_pool`.
    offset: u64,
    /// Der Platz unter den Wake-Quellen (5.10), wenn der Strom eine ist.
    wake: Option<usize>,
}

/// Die an Hardware gebundenen Eingabestroeme; in einen gekoppelten liefert
/// der Treiber vor den Elementen des Modells, wie der Stimulus im
/// Interpreter (8.3).
fn bound_streams(p: &Program, x: &Prefix) -> Vec<BoundStream> {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    let mut out: Vec<BoundStream> = p
        .channels
        .iter()
        .enumerate()
        .filter_map(|(channel, c)| {
            let Some(Type::Stream(elem)) = p.types.list.get(c.ty.index()) else { return None };
            let Binding::Hw(addr) = &c.binding else { return None };
            if c.dir != Direction::Input {
                return None;
            }
            let n = crate::streams::payload_cap(p, *elem);
            // `line<N>` kuerzt erst der Rand, der es fuer `.truncated` merkt
            // (3.9, KON2-028); `str<N>` und `bytes<N>` schon hier.
            let text = matches!(p.types.list.get(elem.index()), Some(Type::Str { .. } | Type::Bytes { .. }));
            Some(BoundStream {
                name: c.name.clone(),
                channel,
                function: format!("{x}_poll_{}", addr.ident()),
                cap: n.saturating_add(1),
                keep: if text { n } else { n.saturating_add(1) },
                polls: takt_hal::edge::maxpt_of(c, p.config.tick).unwrap_or(1).saturating_add(1),
                offset: 0,
                wake: None,
            })
        })
        .collect();
    let mut offset = 0u64;
    for b in &mut out {
        b.offset = offset;
        offset += u64::from(b.polls) * u64::from(b.cap);
    }
    number_wake(p, out.iter_mut().map(|b| (b.channel, &mut b.wake)));
    out
}

/// Wie viele Lieferungen ein Tick hoechstens bringt: je Skalar eine, je
/// Strom `MAXPT + 1`.
fn deliveries(p: &Program, layout: &Layout, x: &Prefix) -> usize {
    bound_scalars(p, layout, x).len() + bound_streams(p, x).iter().map(|b| b.polls as usize).sum::<usize>()
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

/// `P_commit`: den Latch an die Treiber geben (12.1).
///
/// **Der Pfad aus `@ hw(...)` wird zum Symbolnamen.** 8.10 verlangt, dass
/// die Abbildung auf Geraete ausserhalb des Programms steht — „gehoeren in
/// die Hardware-Konfiguration, nicht in die Steuerlogik" —, und nennt den
/// Pfad einen symbolischen Verweis dorthin. Der Rahmen loest ihn nicht
/// auf: Er erzeugt aus `hw("ui/led")` einen Aufruf von
/// `P_out_ui_led(user, ...)` und ueberlaesst die Peripherie dem, der sie
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
fn commit(s: &mut String, p: &Program, layout: &Layout, x: &Prefix) {
    use takt_mir::program::{Binding, Direction};
    use takt_mir::types::Type;
    let bound: Vec<(&crate::layout::Slot, String)> = layout
        .outputs
        .iter()
        .filter(|slot| c_type(&slot.ty, slot.signed).is_some())
        .filter_map(|slot| slot.address.as_ref().map(|a| (slot, format!("{x}_out_{}", a.ident()))))
        .collect();
    let streams: Vec<(&takt_mir::program::Channel, String)> = p
        .channels
        .iter()
        .filter(|c| c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
        .filter_map(|c| match &c.binding {
            Binding::Hw(a) => Some((c, format!("{x}_free_{}", a.ident()))),
            _ => None,
        })
        .collect();
    let outputs = driver_outputs(p, layout);
    let mut devices: Vec<&str> = outputs.iter().map(|o| o.device.as_str()).collect();
    devices.sort_unstable();
    devices.dedup();
    let alive = |d: &str| format!("{x}_alive_{}", takt_mir::pattern::Address::simple(d).ident());

    let _ = writeln!(s, "{}", crate::edge::OUTPUT);

    let _ = writeln!(
        s,
        "\n/* Schritt 10: der Latch geht an die Geraete (12.1); was scheitert, faultet im naechsten Tick. */"
    );
    let _ = writeln!(s, "static void takt_commit(struct {x}_arena *a) {{");
    let body = s.len();
    for d in &devices {
        let _ = writeln!(
            s,
            "    _Bool alive_{} = {}(a->user, now);",
            takt_mir::pattern::Address::simple(d).ident(),
            alive(d)
        );
    }
    let index = |name: &str| outputs.iter().position(|o| o.name == name);
    for (slot, fname) in &bound {
        let (Some(ct), Some(i)) = (c_type(&slot.ty, slot.signed), index(&slot.name)) else { continue };
        let device = takt_mir::pattern::Address::simple(&outputs[i].device).ident();
        let _ = writeln!(
            s,
            "    {{ {ct} v; __builtin_memcpy(&v, a->latch + {}, sizeof v); if (takt_edge_output({fname}(a->user, now, v), alive_{device}, -1, -1, -1)) a->driver_fault[{i}] = 1; }}",
            slot.offset
        );
    }
    for (c, fname) in &streams {
        let Some(i) = index(&c.name) else { continue };
        let device = takt_mir::pattern::Address::simple(&outputs[i].device).ident();
        // 8.8: Der Sendepuffer eines Ausgabestroms ist `capacity` Bytes,
        // Default 256 — dieselbe Zahl wie `takt_tx_cap` (streams.rs).
        let cap = i64::from(c.attrs.capacity.unwrap_or(256));
        // 8.8, FB-124: `idle` prueft der Rand nur, wo das Programm es liest
        // und der Treiber darum da ist; sonst unbekannt.
        let idle = match (
            &c.binding,
            tx_slots(p, &crate::drivers::idle_read(p)).iter().any(|(ch, _)| p.channels[*ch].name == c.name),
        ) {
            (Binding::Hw(a), true) => format!("{x}_idle_{}(a->user, now)", a.ident()),
            _ => "-1".to_string(),
        };
        let _ = writeln!(
            s,
            "    if (takt_edge_output(1, alive_{device}, {fname}(a->user, now), {cap}, {idle})) a->driver_fault[{i}] = 1;"
        );
    }
    let _ = writeln!(s, "}}");
    // Die Grenze des gerechneten Ticks: Ein Schlaf (9.9) rueckt `tick` schon vor dem Commit vor.
    with_now(s, body, "takt_ticks_ns(a->done)");
    locked(s, x, "void", "commit", "", "", "");
}

/// `P_output`: einen Ausgang lesen, nach Stellung.
///
/// **Fuer Diagnose, nicht fuer Treiber.** Das Stellen macht
/// [`commit`] ueber benannte Symbole; diese Funktion ist der Weg, einen
/// Latch-Wert anzusehen, ohne ihn zu stellen — der Bring-up nutzt sie,
/// bevor ein Treiber existiert, und ein Testrahmen, der den Latch prueft.
///
/// Der Index ist die Stellung in `layout.outputs`; die Zuordnung steht im
/// Kopf des erzeugten Textes, damit sie nachlesbar ist.
fn outputs(s: &mut String, layout: &Layout, x: &Prefix) {
    let _ = writeln!(s, "/* Die Ausgaenge nach Stellung, fuer Diagnose. */");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let _ = writeln!(s, "/*   {i} = {} */", slot.name);
    }
    let _ = writeln!(s, "int64_t {x}_output(struct {x}_arena *a, int32_t index) {{");
    let _ = writeln!(s, "    switch (index) {{");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(
            s,
            "    case {i}: {{ {ct} v; __builtin_memcpy(&v, a->latch + {}, sizeof v); return (long long)v; }}",
            slot.offset
        );
    }
    // Ein unbekannter Index ist kein Absturz: Der Aufrufer bekommt eine
    // Null und der Lauf geht weiter. 4.1 verlangt Totalitaet, und ein
    // Treiber, der nach einem entfallenen Ausgang fragt, ist ein
    // Uebersetzungsfehler — keiner, der zur Laufzeit stehen bleiben darf.
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}
