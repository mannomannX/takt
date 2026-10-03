//! Der Rahmen, der ein Takt-Programm auf einer MCU ausfuehrt (12.1, 12.3).
//!
//! **Dieselbe Konstruktion wie der Wirtsrahmen, ein anderes Ziel.** Der
//! erzeugte Code spricht die C-ABI, und der Workspace verbietet
//! `extern "C"` (13.4) — also erzeugen beide Rahmen C, aus denselben
//! Bausteinen ([`crate::parts`]). Was diesen Rahmen vom Wirtsrahmen des
//! Differentials (`takt-conformance`) unterscheidet, sind drei Dinge, und
//! jedes folgt aus 12.3:
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
}

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
}

impl Default for Frame<'_> {
    fn default() -> Self {
        Frame {
            diagnostics: takt_llvm::Diagnostics::Ids,
            hardware: None,
            protect: None,
            prefix: Prefix::default(),
            stubs: false,
        }
    }
}

/// Wie [`build`], fuer ein Board mit seinen Angaben ([`Frame`]).
pub fn build_with(p: &Program, frame: Frame<'_>) -> McuHarness {
    let (diagnostics, hw, x) = (frame.diagnostics, frame.hardware, &frame.prefix);
    let layout = crate::layout::of(p);
    // 7.2: in Schrittordnung, wie der Interpreter und der Testrahmen.
    let driven: Vec<&takt_mir::machine::Machine> = takt_mir::analysis::schedule::order(p)
        .unwrap_or_else(|_| takt_mir::analysis::schedule::runnable(p))
        .into_iter()
        .map(|id| &p.machines[id.index()])
        .collect();

    let arena = takt_llvm::arena::of(p);
    let tick_at = frame.protect.map_or(arena.bytes, |pad| pad.max(arena.bytes));
    let mut t = Text::default();
    // Die Arena stellt der Wirt (12.11); jeder Einstieg bekommt sie.
    let _ = writeln!(
        t.code,
        "_Static_assert(offsetof(struct {x}_arena, tick) == {tick_at}, \"der Tick fuehrt die Runtime an\");\n"
    );
    // Der Tick zuerst: `tick_at` sagt, wo er steht.
    runtime_abi(&mut t, p, x);
    crate::parts::natives(&mut t.code, p);
    // Die Stroeme wie im Linux-Rahmen, ohne Stimulus; ihre Trace-Zeilen
    // gehen an das Board.
    crate::streams::emit(&mut t, p, crate::streams::Trace::Board, x);
    // 9.8: die geplanten Schreibvorgaenge, hinter dem Latch, weil
    // `apply_scheduled` ihn schreibt.
    crate::parts::scheduled(&mut t, p, &layout, hw, x);
    crate::parts::jitter(&mut t.code, p, hw, x);
    jobs(&mut t, p, x);
    declarations(&mut t.code, p, &driven, x);
    // 8.10, 12.6: die Treiber, stark gebunden; Stummel nur ausdruecklich.
    let drivers = crate::drivers::of(p, &layout);
    t.code.push_str(&crate::drivers::c_prototypes(&drivers, x));
    if frame.stubs {
        t.code.push_str(&crate::drivers::c_stubs(&drivers, x));
    }
    // 12.6: der Treiberrand vor dem Abtasten, das ihn speist.
    crate::edge::emit(&mut t, p, &layout, &driven, deliveries(p, &layout, x), crate::streams::Trace::Board, x);
    init(&mut t.code, p, &layout, &driven, x);
    tick(&mut t, p, &layout, &driven, x);
    telemetry(&mut t, p, &layout, &driven, diagnostics, x);
    // Fuer einen Wirt, der die Arena nicht in C anlegt (12.11, [`arena_layout`]).
    let _ = writeln!(t.code, "const uint64_t {x}_arena_bytes = sizeof(struct {x}_arena);");
    let _ = writeln!(t.code, "const uint64_t {x}_arena_align = _Alignof(struct {x}_arena);");
    // Die Version der Schnittstelle: Die Huelle eines Wirts liest das Symbol,
    // und passt sie nicht, bindet das Programm nicht (12.11).
    let abi = crate::embed::ABI;
    let _ = writeln!(t.code, "const uint8_t {x}_abi_{abi} = {abi};");

    let mut head = String::new();
    prologue(&mut head, p);
    let arena_types = t.header(x, &arena, frame.protect);
    let entries = entry_prototypes(x);
    let source = format!("{head}{arena_types}\n{entries}\n{}", t.code);
    let header = header(x, &arena_types, &entries, &crate::drivers::c_prototypes(&drivers, x));
    McuHarness { source, header, layout, tick_at }
}

/// Die oeffentlichen Einstiege des Rahmens (12.11). Sie stehen im Kopf fuer
/// den Wirt und im Rahmen selbst: So prueft der Uebersetzer jede Definition
/// gegen ihre Deklaration.
fn entry_prototypes(x: &Prefix) -> String {
    let a = format!("struct {x}_arena *a");
    let lines = [
        format!("int32_t {x}_init_with({a}, void *user, const void *persist, int32_t persist_len);"),
        format!("void {x}_init({a}, void *user);"),
        format!("void {x}_tick({a}, int64_t k);"),
        format!("void {x}_commit({a});"),
        format!("uint8_t {x}_idle({a});"),
        format!("int64_t {x}_deadline({a});"),
        format!("void {x}_advance({a}, int64_t n);"),
        format!("int32_t {x}_persist_snapshot({a}, void *out, int32_t cap);"),
        format!("int32_t {x}_persist_restore({a}, const void *in, int32_t len);"),
        format!("void {x}_dump({a}, int32_t all);"),
        format!("void {x}_pc({a});"),
        format!("int32_t {x}_next_run({a}, int64_t *delay);"),
        format!("void {x}_end({a});"),
        format!("int64_t {x}_output({a}, int32_t index);"),
        format!("void {x}_overrun({a});"),
        format!("void {x}_hardware({a});"),
        format!("void {x}_tolerance(int64_t *ns, uint32_t *runs);"),
        format!("int32_t {x}_job_dispatch({a});"),
        format!("void {x}_job_work({a});"),
        format!("int32_t {x}_jobs_busy({a});"),
        format!("uint8_t *{x}_job_stack(uint32_t *size);"),
        format!("extern const int32_t {x}_persist_entries;"),
        format!("extern const int32_t {x}_persist_bound;"),
        format!("extern const uint64_t {x}_arena_bytes;"),
        format!("extern const uint64_t {x}_arena_align;"),
        format!("extern const uint8_t {x}_abi_{};", crate::embed::ABI),
    ];
    let mut s = String::from("/* Die Einstiege (12.11): jeder bekommt die Arena des Wirts. */\n");
    for line in lines {
        let _ = writeln!(s, "{line}");
    }
    s
}

/// Der Kopf fuer einen Wirt in C (`P.h`, 12.11): die Arena mit ihren Typen,
/// die Einstiege, die Treiber, die der Wirt stellt, und die Leitung des
/// Boards. Jeder Name traegt das Praefix oder gehoert der geteilten
/// Bibliothek und steht hinter einem Guard; so binden mehrere Programme
/// ihre Koepfe in dieselbe Uebersetzungseinheit.
fn header(x: &Prefix, arena_types: &str, entries: &str, drivers: &str) -> String {
    let guard = format!("{}_TAKT_H", x.as_str().to_uppercase());
    let mut s = String::new();
    let _ = writeln!(s, "/* Der Kopf des Programms `{x}` (12.11); erzeugt von takt-frame. */");
    let _ = writeln!(s, "#ifndef {guard}");
    let _ = writeln!(s, "#define {guard}\n");
    let _ = writeln!(s, "#include <stddef.h>");
    let _ = writeln!(s, "#include <stdint.h>\n");
    s.push_str(arena_types);
    let _ = writeln!(s, "\n/* Die Leitung des Boards (12.5): stellt der Wirt, einmal fuer alle Programme. */");
    let _ = writeln!(s, "#ifndef TAKT_BOARD_TRACE");
    let _ = writeln!(s, "#define TAKT_BOARD_TRACE");
    s.push_str(BOARD_TRACE_C);
    let _ = writeln!(s, "#endif\n");
    s.push_str(entries);
    s.push('\n');
    s.push_str(drivers);
    let _ = writeln!(s, "\n#endif");
    s
}

/// Die Leitung des Boards: Der Rahmen schreibt den Trace ueber sie (12.5).
const BOARD_TRACE_C: &str = "void takt_board_trace(const char *line);
void takt_board_trace_i64(long long value);
void takt_board_trace_u64(unsigned long long value);
void takt_board_trace_f64(double value);
void takt_board_trace_hex8(unsigned char value);
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
         Arena(core::mem::MaybeUninit::uninit())\n    }}\n}}\n"
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
    // Das Board stellt sie bereit; der Rahmen ruft sie nur.
    s.push_str(BOARD_TRACE_C);
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
    let (params, args) = if params.is_empty() {
        (format!("struct {x}_arena *a"), "a".to_string())
    } else {
        (format!("struct {x}_arena *a, {params}"), format!("a, {args}"))
    };
    let call = format!("takt_{name}({args})");
    let body = if ret == "void" {
        format!("{call}; takt_fenv_leave(f);")
    } else {
        format!("{ret} r = {call}; takt_fenv_leave(f); return r;")
    };
    let _ = writeln!(s, "{ret} {x}_{name}({params}) {{ takt_fenv f = takt_fenv_enter(); {body} }}\n");
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
    // Der Zeiger, den der Wirt bei `init` uebergab; jeder Treiber bekommt ihn
    // als erstes Argument (8.10, 12.6).
    let _ = writeln!(t.fields, "    void *user;");
    crate::parts::scope_flags(t, p);
    crate::parts::fault_names(&mut t.code, p);
    crate::parts::raised(t, p, x);
    crate::parts::alert_table(t, p, x);
    let s = &mut t.code;

    // 3.3: `now` ist die Dauer seit dem Start — Tickzahl mal T0.
    let _ = writeln!(s, "long long {x}_now(struct {x}_arena *a) {{ return a->tick * {}LL; }}\n", p.config.tick);

    // Jede Beobachtungszeile traegt ihren Tick, wie beim Interpreter
    // (`grammar/trace.md`): Ohne ihn laesst sie sich keinem Tick zuordnen.
    // 5.6: nur die Flanken, mit dem Namen der Maschine wie im Interpreter.
    let _ =
        writeln!(s, "void {x}_alert(struct {x}_arena *a, int m, int slot, unsigned char on, unsigned char invalid) {{");
    let _ = writeln!(s, "    if (!takt_alert_edge(a, m, slot, on)) return;");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
    let _ = writeln!(s, "    takt_board_trace(\"alert \");");
    let _ = writeln!(s, "    takt_board_trace(takt_machine_name(m));");
    let _ = writeln!(s, "    takt_board_trace(on ? \" on\" : \" off\");");
    let _ = writeln!(s, "    if (invalid) takt_board_trace(\" invalid\");");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}");
    for (name, args, kind, flags) in [
        ("log", "int m, int site", "log", &[][..]),
        ("verify", "int m, int site, unsigned char ok", "verify", &["ok"]),
        ("verdict", "int m, int site, unsigned char pass", "verdict", &["pass"]),
    ] {
        let _ = writeln!(s, "void {x}_{name}(struct {x}_arena *a, {args}) {{");
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
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
    let _ = writeln!(s, "void {x}_abort(struct {x}_arena *a, int m, int site) {{");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
    let _ = writeln!(s, "    takt_board_trace(\"abort \");");
    let _ = writeln!(s, "    takt_board_trace_i64(m);");
    let _ = writeln!(s, "    takt_board_trace_i64(site);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "    memset(a->raised, 1, sizeof a->raised);");
    let _ = writeln!(s, "}}\n");

    // 5.3: der Fault-Uebergang mit Maschine und Art, wie der Interpreter
    // ihn schreibt.
    let _ = writeln!(s, "void {x}_fault(struct {x}_arena *a, int m, int from, int code) {{");
    let _ = writeln!(s, "    (void)from;");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
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
    let _ = writeln!(s, "void {x}_measure(struct {x}_arena *a, int m, int site, double v, unsigned char invalid) {{");
    let _ = writeln!(s, "    unsigned long long bits;");
    let _ = writeln!(s, "    __builtin_memcpy(&bits, &v, sizeof bits);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
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
    let _ = writeln!(s, "void {x}_property(struct {x}_arena *a, int i, long long at) {{");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->tick);");
    let _ = writeln!(s, "    takt_board_trace(\"property \");");
    let _ = writeln!(s, "    takt_board_trace_i64(i);");
    let _ = writeln!(s, "    takt_board_trace_i64(at);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
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
/// Job-Kontext gehoert ein Auftrag (`work_*`): Die Hauptschleife fuellt
/// ihn, solange der Kontext ruht (`P_job_dispatch`), der Kontext
/// rechnet ihn und meldet `work_finished`, die Hauptschleife holt das
/// Ergebnis ab. Ein Neustart oder Abbruch waehrend der Rechnung erhoeht
/// die Generation des Slots, und ein Ergebnis zur alten verfaellt.
///
/// Ohne Jobs bleiben die Einstiege, damit das Board sie ohne Unterschied
/// rufen kann.
///
/// **Unter dem Stack liegt der Waechter** (12.3): 32 Byte am unteren Ende,
/// zusaetzlich zu Vertrag und Reserve und an 32 Byte ausgerichtet, damit
/// eine MPU-Region oder ein NAPOT-Watchpoint ihn genau abdeckt.
fn jobs(t: &mut Text, p: &Program, x: &Prefix) {
    let s = &mut t.code;
    let _ = writeln!(s, "/* Jobs (4.5): Slots der Hauptschleife, ein Auftrag fuer den Job-Kontext. */");
    let Some((slots, out_max)) = crate::parts::job_tables(s, p, x) else {
        let _ = writeln!(s, "int32_t {x}_job_dispatch(struct {x}_arena *a) {{ (void)a; return 0; }}");
        let _ = writeln!(s, "void {x}_job_work(struct {x}_arena *a) {{ (void)a; }}");
        let _ = writeln!(s, "int32_t {x}_jobs_busy(struct {x}_arena *a) {{ (void)a; return 0; }}");
        let _ = writeln!(s, "uint8_t *{x}_job_stack(uint32_t *size) {{ *size = 0; return 0; }}\n");
        return;
    };
    // So gross wie der Puffer, den der erzeugte Code fuer die Argumente anlegt.
    let in_max = crate::parts::job_slots(p)
        .iter()
        .map(|(_, _, n)| {
            let params = &p.natives[n.index()].params;
            params.iter().map(|q| 4 + u64::from(takt_mir::bytes::max_size(p, q.ty).unwrap_or(0))).sum::<u64>()
        })
        .max()
        .unwrap_or(0)
        .max(4);
    let stack = crate::parts::job_slots(p).iter().map(|(_, _, n)| p.natives[n.index()].stack).max().unwrap_or(0);
    let names: Vec<String> = crate::parts::job_slots(p)
        .iter()
        .map(|(mi, j, _)| {
            let m = &p.machines[*mi];
            let handle = m.layout.job_slots[*j].handle;
            format!("\"{} {}\"", m.name, m.vars.get(handle.index()).map_or("?", |v| v.name.as_str()))
        })
        .collect();

    let _ = writeln!(t.code, "enum {{ TAKT_JOB_FREE, TAKT_JOB_WAITING, TAKT_JOB_RUNNING, TAKT_JOB_DONE }};");
    let _ = writeln!(
        t.types,
        "typedef struct {{ unsigned char state, gen; int native; long long due, order; int in_len, out_len; unsigned char in[{in_max}], out[{out_max}]; }} {x}_job;"
    );
    // Slots und Auftrag liegen in der Runtime hinter dem Programmbereich:
    // Der Job-Kontext schreibt den Auftrag ausserhalb des Schritts, wenn der
    // Schutz den Programmbereich sperrt (12.3); `volatile`, weil zwei Faeden
    // ihn teilen.
    let f = &mut t.fields;
    let _ = writeln!(f, "    {x}_job jobs[{slots}];");
    let _ = writeln!(f, "    long long job_order;");
    let _ = writeln!(f, "    volatile int work_slot, work_finished;");
    let _ = writeln!(f, "    unsigned char work_gen;");
    let _ = writeln!(f, "    int work_native, work_in_len, work_out_len;");
    let _ = writeln!(f, "    unsigned char work_in[{in_max}], work_out[{out_max}];");
    let s = &mut t.code;
    let _ = writeln!(s, "static const char *const takt_job_names[{slots}] = {{ {} }};", names.join(", "));
    let _ = writeln!(s, "#ifndef TAKT_JOB_STACK_RESERVE");
    let _ = writeln!(s, "#define TAKT_JOB_STACK_RESERVE {JOB_STACK_RESERVE}");
    let _ = writeln!(s, "#endif");
    let _ = writeln!(
        s,
        "static unsigned char takt_job_stack_mem[32 + {stack} + TAKT_JOB_STACK_RESERVE] __attribute__((aligned(32)));"
    );
    let _ = writeln!(s, "uint8_t *{x}_job_stack(uint32_t *size) {{");
    let _ = writeln!(s, "    *size = sizeof takt_job_stack_mem; return takt_job_stack_mem;");
    let _ = writeln!(s, "}}");

    let _ = writeln!(
        s,
        "void {x}_job_begin(struct {x}_arena *a, int m, int slot, int native, const unsigned char *args, int len) {{"
    );
    let _ = writeln!(s, "    int i = takt_job_base[m] + slot; {x}_job *j = &a->jobs[i];");
    let _ = writeln!(s, "    if (len > (int)sizeof j->in) len = (int)sizeof j->in;");
    let _ = writeln!(s, "    memcpy(j->in, args, (size_t)len); j->in_len = len; j->native = native;");
    let _ = writeln!(s, "    j->state = TAKT_JOB_WAITING; j->gen++; j->order = ++a->job_order;");
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
    let _ = writeln!(s, "    if (a->work_slot < 0 || !a->work_finished) return;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    j = &a->jobs[a->work_slot];");
    let _ = writeln!(s, "    if (j->state == TAKT_JOB_RUNNING && j->gen == a->work_gen) {{");
    // `-1` ist `Err(FAILED)` ohne Bytes; ein `size_t` daraus kopierte alles.
    let _ = writeln!(
        s,
        "        if (a->work_out_len > 0) memcpy(j->out, a->work_out, (size_t)a->work_out_len); j->out_len = a->work_out_len;"
    );
    let _ = writeln!(s, "        j->state = TAKT_JOB_DONE;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    a->work_finished = 0; a->work_slot = -1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "/* Hauptschleife: Der ruhende Kontext bekommt den aeltesten wartenden Job; wahr, wenn er zu rechnen hat. */"
    );
    let _ = writeln!(s, "int32_t {x}_job_dispatch(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i, next = -1;");
    let _ = writeln!(s, "    takt_jobs_collect(a);");
    let _ = writeln!(s, "    if (a->work_slot >= 0) return 1;");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++)");
    let _ = writeln!(
        s,
        "        if (a->jobs[i].state == TAKT_JOB_WAITING && (next < 0 || a->jobs[i].order < a->jobs[next].order)) next = i;"
    );
    let _ = writeln!(s, "    if (next < 0) return 0;");
    let _ = writeln!(s, "    memcpy(a->work_in, a->jobs[next].in, (size_t)a->jobs[next].in_len);");
    let _ = writeln!(s, "    a->work_in_len = a->jobs[next].in_len; a->work_native = a->jobs[next].native;");
    let _ = writeln!(s, "    a->work_gen = a->jobs[next].gen; a->jobs[next].state = TAKT_JOB_RUNNING;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    a->work_finished = 0; a->work_slot = next;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "/* Job-Kontext: rechnet den Auftrag, den die Hauptschleife gegeben hat. */");
    let _ = writeln!(s, "static void takt_job_work(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int native;");
    let _ = writeln!(s, "    if (a->work_slot < 0 || a->work_finished) return;");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    native = a->work_native;");
    let _ = writeln!(s, "    {{");
    crate::parts::job_call(s, p, "a->work_in", "a->work_in_len", "a->work_out", "a->work_out_len", "        ");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    __atomic_signal_fence(__ATOMIC_SEQ_CST);");
    let _ = writeln!(s, "    a->work_finished = 1;");
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "job_work", "", "");
    let _ = writeln!(s, "/* 12.1: zu Tickbeginn. Ein fertiges Ergebnis wird sichtbar, wenn seine Dauer um ist. */");
    let _ = writeln!(s, "static void takt_jobs_poll(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int i, b;");
    let _ = writeln!(s, "    takt_jobs_collect(a);");
    let _ = writeln!(s, "    for (i = 0; i < {slots}; i++) {{");
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
    let _ = writeln!(s, "        takt_board_trace(\"t=\"); takt_board_trace_i64(a->tick);");
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
    let _ = writeln!(s, "    a->work_slot = -1; a->work_finished = 0;");
    let _ = writeln!(s, "}}\n");
}

/// `P_init`: einmal vor dem ersten Tick.
fn init(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(s, "/* Einmal vor dem ersten Tick (12.1, Schritt 1). */");
    let _ = writeln!(s, "static int32_t takt_persist_restore(struct {x}_arena *a, const void *in, int32_t len);");
    let _ = writeln!(s, "static void takt_sample(struct {x}_arena *a);");
    let _ = writeln!(
        s,
        "static int32_t takt_init_with(struct {x}_arena *a, void *user, const void *persist, int32_t persist_len) {{"
    );
    // 12.11: `init` beschreibt die ganze Arena und verlaesst sich nicht auf
    // genullten Speicher.
    let _ = writeln!(s, "    memset(a, 0, sizeof *a);");
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
        let _ = writeln!(s, "    *({ct} *)(a->params + {}) = {value}; /* {} */", slot.offset, slot.name);
    }
    if !crate::parts::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_init(a);");
    }
    // 9.4: Der Lauf beginnt mit den Outputs auf `safe`, vor jedem Init —
    // wie `Sim::new` und der Wirtsrahmen. Danach bindet die Simulation,
    // damit Tick 0 die `safe`-Werte eines Modells liest (8.3).
    crate::parts::safe_outputs(s, p, layout);
    crate::parts::sim_bindings(s, p, "    ");

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
    crate::parts::enter_machines(s, p, layout, driven, "    ", x);
    // Was `enter` und das erste `loop:` im Tick 0 senden, wird hier
    // sichtbar (FB-269).
    crate::parts::commit_sequence(s, p, driven, "    ", "0");
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
        "void *user, const void *persist, int32_t persist_len",
        "user, persist, persist_len",
    );
}

/// `P_tick`: ein Tick, von der Schleife gerufen.
fn tick(t: &mut Text, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(t.fields, "    _Bool overrun, hardware;");
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
    // 7.1, 12.6 Zeile 7: die Toleranz der Tickquelle fuer die Schleife.
    let (ns, runs) = p.config.tolerance();
    let _ = writeln!(s, "void {x}_tolerance(int64_t *ns, uint32_t *runs) {{ *ns = {ns}LL; *runs = {runs}u; }}");
    // 12.6 Zeile 6: Was der Commit an Treiberfehlern gesehen hat, wirkt im
    // naechsten Tick, wie ein Ueberlauf.
    let driven_out = driver_outputs(p, layout);
    let _ = writeln!(s, "static void takt_tick(struct {x}_arena *a, int64_t k) {{");
    let _ = writeln!(s, "    a->tick = k;");
    let _ = writeln!(s, "    a->done = k;");
    let _ = writeln!(s, "    if (a->overrun) {{");
    let _ = writeln!(s, "        a->overrun = 0;");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(k);");
    let _ = writeln!(s, "        takt_board_trace(\"runtime Overrun\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(a, m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Overrun))
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    if (a->hardware) {{");
    let _ = writeln!(s, "        a->hardware = 0;");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(k);");
    let _ = writeln!(s, "        takt_board_trace(\"runtime Hardware\\n\");");
    let _ = writeln!(
        s,
        "        for (int m = 0; m < {}; m++) takt_pend(a, m, {});",
        p.machines.len(),
        takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Runtime(takt_mir::machine::RuntimeKind::Hardware))
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
        let _ = writeln!(s, "        takt_board_trace(\"t=\");");
        let _ = writeln!(s, "        takt_board_trace_i64(k);");
        let _ = writeln!(s, "        takt_board_trace(\"runtime Driver {}\\n\");", out.name);
        let _ = writeln!(s, "        {pend}");
        let _ = writeln!(s, "    }}");
    }
    crate::parts::aging(s, p, layout, "    ");
    // 4.5: Was fertig und faellig ist, wird zu Tickbeginn sichtbar, wie
    // `poll_jobs` im Interpreter und im Wirtsrahmen.
    if !crate::parts::job_slots(p).is_empty() {
        let _ = writeln!(s, "    takt_jobs_poll(a);");
    }
    let _ = writeln!(s, "    takt_sample(a);");
    crate::parts::steps(s, p, layout, driven, "    ", "k", x);
    crate::parts::abort_phase(s, p, driven, "    ", "k", x);
    crate::parts::idle_drops(s, p, driven, "    ", x);
    crate::parts::commit_sequence(s, p, driven, "    ", "k");
    for (i, _) in monitors(p) {
        let _ = writeln!(s, "    {x}_monitor_{i}(a, k);");
    }
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "tick", "int64_t k", "k");

    sleep(s, p.config.tick, layout, p, driven, x);
    platform(s, p, layout, x);
}

/// Wie `P_next_run` das Ende eines Laufs meldet: die Nummer ist die
/// Stelle plus eins, 0 heisst weiter. `takt-mcu-program` liest dieselbe
/// Folge.
const NEXT_RUN_CODES: [takt_mir::sys::NextRun; 4] = [
    takt_mir::sys::NextRun::Now,
    takt_mir::sys::NextRun::After,
    takt_mir::sys::NextRun::OnWake,
    takt_mir::sys::NextRun::OnStart,
];

/// `P_next_run` und `P_end`: das Ende des Laufs (12.7), wie im
/// Wirtsrahmen — die Zeile `end`, dann alle Ausgaenge auf `safe`. Was
/// zwischen zwei Laeufen geschieht, fuehrt das Board aus.
fn platform(s: &mut String, p: &Program, layout: &Layout, x: &Prefix) {
    let next = crate::parts::next_run_slot(p, layout);
    let _ = writeln!(s, "/* 12.7: 0 weiter, 1 NOW, 2 AFTER (`delay` in ns), 3 ON_WAKE, 4 ON_START. */");
    let _ = writeln!(s, "int32_t {x}_next_run(struct {x}_arena *a, int64_t *delay) {{");
    let _ = writeln!(s, "    *delay = -1;");
    if let Some(n) = &next {
        let _ = writeln!(s, "    switch (*({} *)(a->latch + {})) {{", n.ct, n.slot.offset);
        for (d, end) in &n.ends {
            let code = NEXT_RUN_CODES.iter().position(|c| c == end).unwrap_or(0) + 1;
            // `AFTER(delay)`: die Dauer im ersten Fach hinter der Diskriminante (11.2).
            let delay = match end {
                takt_mir::sys::NextRun::After => format!("*delay = *(long long *)(a->latch + {}); ", n.slot.offset + 8),
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
    let _ = writeln!(s, "void {x}_end(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    static const char *const words[] = {{ {} }};", words.join(", "));
    let _ = writeln!(s, "    int64_t delay;");
    let _ = writeln!(s, "    int c = {x}_next_run(a, &delay);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(a->done);");
    let _ = writeln!(s, "    if (c >= 1 && c <= {}) takt_board_trace(words[c - 1]);", NEXT_RUN_CODES.len());
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
fn sleep(s: &mut String, tick: i64, layout: &Layout, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
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
    let _ = writeln!(s, "    return {x}_now(a) + best * {tick}LL;");
    let _ = writeln!(s, "}}");
    guarded(s, x, "int64_t", "deadline", "", "");

    // 9.9: „fuer jede Maschine: time_in_state += n*T0". Ein
    // uebersprungener Tick ruft kein `_step`; ohne das feuerte jede
    // `after`-Frist um die geschlafenen Ticks zu spaet.
    let _ = writeln!(s, "void {x}_init(struct {x}_arena *a, void *user) {{ (void){x}_init_with(a, user, 0, 0); }}\n");
    let _ = writeln!(s, "static void takt_advance(struct {x}_arena *a, int64_t n) {{");
    let _ = writeln!(s, "    a->tick += n;");
    for m in driven {
        let _ = writeln!(s, "    {x}_{0}_advance(a, n);", m.name);
    }
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "advance", "int64_t n", "n");

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
    guarded(s, x, "int32_t", "persist_snapshot", "void *out, int32_t cap", "out, cap");
    let _ = writeln!(s, "static int32_t takt_persist_restore(struct {x}_arena *a, const void *in, int32_t len) {{");
    let _ = writeln!(s, "    int n = 0;");
    for m in &persisting {
        let _ = writeln!(s, "    n += {x}_{0}_persist_restore(a, in, len);", m.name);
    }
    let _ = writeln!(s, "    (void)in; (void)len;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}");
    guarded(s, x, "int32_t", "persist_restore", "const void *in, int32_t len", "in, len");
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
        program_counters(&mut t.code, p, driven, x);
        sample(t, p, layout, x);
        commit(&mut t.code, p, layout, x);
        outputs(&mut t.code, layout, x);
        return;
    }
    let _ = writeln!(t.fields, "    _Alignas(8) unsigned char shown[{}];", layout.latch.max(1));
    let s = &mut t.code;
    let _ = writeln!(s, "/* Die Ausgaenge als Trace-Zeilen (grammar/trace.md); ohne `all` nur die geaenderten. */");
    let _ = writeln!(s, "{}", crate::parts::DURATION_C);
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
    let _ = writeln!(s, "static int takt_same(const unsigned char *x, const unsigned char *y, unsigned n) {{");
    let _ = writeln!(s, "    while (n--) if (*x++ != *y++) return 0;");
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
    let _ = writeln!(s, "static void takt_dump(struct {x}_arena *a, int32_t all) {{");
    let _ = writeln!(s, "    for (unsigned i = 0; i < {n}; i++) {{");
    let _ = writeln!(s, "        const struct takt_out *o = &g_outs[i];");
    let _ = writeln!(s, "        const unsigned char *v = a->latch + o->off;");
    let _ = writeln!(s, "        if (!all && takt_same(v, a->shown + o->off, o->size)) continue;");
    let _ = writeln!(s, "        memcpy(a->shown + o->off, v, o->size);");
    let _ = writeln!(s, "        takt_board_trace(\"t=\");");
    let _ = writeln!(s, "        takt_board_trace_i64(a->done);");
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
    let _ = writeln!(s, "    takt_stream_report(a, a->done);");
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "dump", "int32_t all", "all");
    program_counters(s, p, driven, x);
    sample(t, p, layout, x);
    commit(&mut t.code, p, layout, x);
    outputs(&mut t.code, layout, x);
}

/// `P_pc`: wo jede Maschine steht (11.2, Instrumentierung
/// `statements`). Ohne sie bleibt die Funktion leer.
fn program_counters(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    let _ = writeln!(s, "/* Der Programmzaehler je Maschine (11.2). */");
    let _ = writeln!(s, "void {x}_pc(struct {x}_arena *a) {{");
    for m in driven {
        let Some(at) =
            takt_llvm::machine::state_struct(m, p).and_then(|st| st.byte_offset(takt_llvm::machine::Role::Pc, 0))
        else {
            continue;
        };
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(a->done);");
        let _ = writeln!(s, "    takt_board_trace(\"pc {} \");", m.name);
        let _ = writeln!(s, "    takt_board_trace_i64(*(int *)(a->{} + {at}));", takt_llvm::arena::state_name(&m.name));
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

/// `takt_sample`: die Treiber an das Abbild (12.1, Schritt 2).
///
/// **Das Gegenstueck zu [`commit`], und aus demselben Grund ein Symbol.**
/// Aus `hw("ui/button")` wird `P_in_ui_button(user, now, &value, &quality, &t)`; wer
/// den Kanal bindet und keinen Treiber stellt, bekommt einen Linkfehler
/// mit dem Namen darin. Eine Registrierung zur Laufzeit waere flexibler,
/// aber ein nicht eingetragener Eingang fiele still aus — und ein
/// Eingang, der still `Bad` bleibt, ist schlimmer als einer, der fehlt.
///
/// **Warum die Qualitaet mitkommt.** 12.6 laesst Eingaenge degradieren
/// statt zu faulten: Ein Treiber, der nichts Frisches hat, sagt `Stale`,
/// einer mit unplausiblem Wert `Suspect`. Ohne diesen Rueckweg koennte er
/// nur luegen oder schweigen. Liefert er nichts, bleibt der Eintrag, wie
/// `init` ihn gesetzt hat: `Bad` (3.5).
///
/// Eingaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf, und
/// ebenso keiner, den ein `sim`-Output derselben Adresse speist: Im
/// Sim-Build ist das Modell seine Quelle, und ein Treiber, der zu
/// Tickbeginn laese, ueberschriebe es (8.3).
fn sample(t: &mut Text, p: &Program, layout: &Layout, x: &Prefix) {
    let tick = p.config.tick;
    let scalars = bound_scalars(p, layout, x);
    let streams = bound_streams(p, x);
    // Ein Platz je Element, das ein Strom in einem Tick liefern darf, und
    // eines mehr, damit der Rand `MAXPT` pruefen kann (12.6 Zeile 2).
    let pool: u64 = streams.iter().map(|b| u64::from(b.polls) * u64::from(b.cap)).sum();
    if pool > 0 {
        let _ = writeln!(t.fields, "    _Alignas(8) unsigned char edge_pool[{pool}];");
    }
    record_fields(t, p);
    let s = &mut t.code;
    record(s, p, x);

    let _ = writeln!(s, "\n/* Schritt 2: die Lieferungen an den Rand, der Rand ins Abbild (12.1, 12.6). */");
    let _ = writeln!(s, "static void takt_sample(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    int64_t now = a->tick * {tick}LL;");
    for b in &scalars {
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        {} v = 0;", b.ct);
        let _ = writeln!(s, "        uint8_t q = 0;");
        let _ = writeln!(s, "        int64_t t = now;");
        let _ = writeln!(s, "        if ({}(a->user, now, &v, &q, &t))", b.function);
        // `Bad` kommt ohne Wert (12.6 Zeile 2); sein Grund ist der Treiber.
        let _ = writeln!(
            s,
            "            takt_edge_reading(a, {}, &v, (int)sizeof v, {}, q, q == 3 ? 3 : 0, q != 3, t, 0LL);",
            b.channel, b.number
        );
        let _ = writeln!(s, "    }}");
    }
    let mut off = 0u64;
    for b in &streams {
        let _ = writeln!(s, "    {{ /* {} */", b.name);
        let _ = writeln!(s, "        long long last = a->edge_tracks[{}].last_seq;", b.channel);
        let _ = writeln!(s, "        long long next = last == (-9223372036854775807LL - 1) ? 0 : last + 1;");
        let _ = writeln!(s, "        for (int i = 0; i < {}; i++) {{", b.polls);
        let _ = writeln!(s, "            unsigned char *buf = a->edge_pool + {off} + i * {};", b.cap);
        let _ = writeln!(s, "            int32_t len = 0;");
        let _ = writeln!(s, "            int64_t t = now, seq = next;");
        let _ = writeln!(s, "            if (!{}(a->user, now, buf, {}, &len, &t, &seq)) break;", b.function, b.cap);
        let _ = writeln!(s, "            if (len < 0) len = 0;");
        let _ = writeln!(s, "            if (len > {0}) len = {0};", b.cap);
        let _ = writeln!(s, "            next = seq + 1;");
        let _ = writeln!(s, "            takt_edge_element(a, {}, buf, len, t, seq);", b.channel);
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "    }}");
        off += u64::from(b.polls) * u64::from(b.cap);
    }
    if !p.recorded.is_empty() {
        let _ = writeln!(s, "    takt_record(a, now);");
    }
    let _ = writeln!(s, "    takt_edge_commit(a, a->tick);");
    let _ = writeln!(s, "}}\n");
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
        let _ = writeln!(s, "        uint8_t q = 0;");
        let _ = writeln!(s, "        int64_t t = now;");
        let _ = writeln!(
            s,
            "        if ({f}(a->user, now, &v, &q, &t) && (!a->rec_seen_{i} || memcmp(&v, &a->rec_last_{i}, sizeof v) != 0 || q != a->rec_last_q_{i} || t != now)) {{"
        );
        let _ = writeln!(s, "            a->rec_seen_{i} = 1; a->rec_last_{i} = v; a->rec_last_q_{i} = q;");
        let _ = writeln!(s, "            takt_board_trace(\"t=\");");
        let _ = writeln!(s, "            takt_board_trace_i64(a->tick);");
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
    let _ = writeln!(s, "            takt_board_trace(\"t=\");");
    let _ = writeln!(s, "            takt_board_trace_i64(a->tick);");
    let _ = writeln!(s, "            takt_board_trace(\"rec {} \");", r.name);
    if wire {
        // Zwei Ziffern je Byte und Aufruf: `takt_board_trace_hex8` schreibt
        // je Byte ein `0x`, und eine Zeile am Stueck kann laenger sein, als
        // `takt_board_trace` liest.
        let _ = writeln!(s, "            static const char digits[] = \"0123456789abcdef\";");
        let _ = writeln!(s, "            takt_board_trace(\"0x\");");
        let _ = writeln!(s, "            for (int b = 0; b < len; b++) {{");
        let _ =
            writeln!(s, "                char d[3] = {{ digits[a->rec_{i}[b] >> 4], digits[a->rec_{i}[b] & 15], 0 }};");
        let _ = writeln!(s, "                takt_board_trace(d);");
        let _ = writeln!(s, "            }}");
        let _ = writeln!(s, "            takt_board_trace(\" \");");
    } else {
        let _ = writeln!(s, "            takt_board_trace_i64((long long)a->rec_{i}[0]);");
    }
    let _ = writeln!(s, "            if (t != now) {{ takt_board_trace(\"t=\"); takt_board_trace_i64(t); }}");
    let _ =
        writeln!(s, "            if (seq != expected) {{ takt_board_trace(\"seq=\"); takt_board_trace_i64(seq); }}");
    let _ = writeln!(s, "            takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
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
fn bound_scalars(p: &Program, layout: &Layout, x: &Prefix) -> Vec<BoundScalar> {
    use takt_mir::types::Type;
    let fed = crate::parts::sim_fed_inputs(p);
    layout
        .inputs
        .iter()
        .filter_map(|slot| {
            let channel = p.channels.iter().position(|c| c.name == slot.name)?;
            if fed.contains(&channel) {
                return None;
            }
            let function = format!("{x}_in_{}", slot.address.as_ref()?.ident());
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
fn bound_streams(p: &Program, x: &Prefix) -> Vec<BoundStream> {
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
                function: format!("{x}_poll_{}", addr.ident()),
                cap: crate::streams::payload_cap(p, *elem).saturating_add(1),
                polls: takt_hal::edge::maxpt_of(c, p.config.tick).unwrap_or(1).saturating_add(1),
            })
        })
        .collect()
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

    let _ = writeln!(s, "_Bool takt_edge_output(_Bool confirmed, _Bool alive, int free, int capacity);");

    let _ = writeln!(
        s,
        "\n/* Schritt 10: der Latch geht an die Geraete (12.1); was scheitert, faultet im naechsten Tick. */"
    );
    let _ = writeln!(s, "static void takt_commit(struct {x}_arena *a) {{");
    // Die Grenze des gerechneten Ticks: Ein Schlaf (9.9) rueckt `tick` schon vor dem Commit vor.
    let _ = writeln!(s, "    int64_t now = a->done * {}LL;", p.config.tick);
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
            "    if (takt_edge_output({fname}(a->user, now, *({ct} *)(a->latch + {})), alive_{device}, -1, -1)) a->driver_fault[{i}] = 1;",
            slot.offset
        );
    }
    for (c, fname) in &streams {
        let Some(i) = index(&c.name) else { continue };
        let device = takt_mir::pattern::Address::simple(&outputs[i].device).ident();
        let cap = c.attrs.capacity_bytes.map_or(-1, i64::from);
        let _ = writeln!(
            s,
            "    if (takt_edge_output(1, alive_{device}, {fname}(a->user, now), {cap})) a->driver_fault[{i}] = 1;"
        );
    }
    let _ = writeln!(s, "}}");
    guarded(s, x, "void", "commit", "", "");
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
        let _ = writeln!(s, "    case {i}: return (long long)*({ct} *)(a->latch + {});", slot.offset);
    }
    // Ein unbekannter Index ist kein Absturz: Der Aufrufer bekommt eine
    // Null und der Lauf geht weiter. 4.1 verlangt Totalitaet, und ein
    // Treiber, der nach einem entfallenen Ausgang fragt, ist ein
    // Uebersetzungsfehler — keiner, der zur Laufzeit stehen bleiben darf.
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}
