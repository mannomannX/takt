//! Das Messprogramm von `takt bench` als Baustein (13.8, plan/m11.md 2.12):
//! was `takt bench --emit embed` neben die Kerne legt.
//!
//! **Der Laeufer** (`takt_bench.c`) misst Schritt fuer Schritt: zuerst, was
//! die Messung selbst kostet (zwei Lesungen des Zaehlers), dann jeden Kern
//! der Suite ([`super::suite`]) mit seiner C-Referenz, die Mathematik am
//! Einstieg der Probe und an ihren Vektoren und den Subnormal-Vektor (4.2). Der Wirt stellt zwei
//! Haken, `takt_bench_cycles` und `takt_bench_put`, und ruft je Schritt
//! `takt_bench_measure` — unter seiner Interruptsperre, wenn er den Kern
//! allein messen will —, danach `takt_bench_report`, das schreibt. So
//! laeuft die Senke nie unter der Sperre, und keine Sperre dauert laenger
//! als ein Schritt.
//!
//! **Eine Arena fuer alle.** Die Kerne laufen nacheinander; ihre Arenen
//! liegen in einer `union`, deren Groesse und Ausrichtung der C-Uebersetzer
//! rechnet. Die C-Referenzen halten ihren Zustand in `static`, wie Code, der
//! wirklich so geschrieben wird; jeder Schritt laeuft darum einmal je Start.
//!
//! **Die Kerne schreiben keinen Trace** (7.2: `record_and_telemeter`
//! gehoert nicht zum Tick). Ihre Rahmen sind mit [`TRACE`] auf die Stummel
//! des Laeufers uebersetzt; die Leitung des Wirts bleibt fuer seine
//! Programme frei, und beide binden in dasselbe Abbild.

use std::fmt::Write as _;

use super::{Kernel, Role};
use crate::math::{FUNCTIONS, Vector};

/// Durchlaeufe vor der Messung: Die Initialisierung der Maschinen ist
/// vorbei, Caches und Vorabrufpuffer sind gefuellt — auch fuer die Haken.
pub const WARMUP: u32 = 8;

/// Die Namen der Leitung (12.5), wie die Rahmen der Kerne sie rufen, und
/// wie der Laeufer sie stellt: als `-D` beim Uebersetzen der Rahmen.
pub const TRACE: [(&str, &str); 5] = [
    ("takt_board_trace", "takt_bench_trace"),
    ("takt_board_trace_i64", "takt_bench_trace_i64"),
    ("takt_board_trace_u64", "takt_bench_trace_u64"),
    ("takt_board_trace_f64", "takt_bench_trace_f64"),
    ("takt_board_trace_hex8", "takt_bench_trace_hex8"),
];

/// Was jede C-Referenz zusaetzlich bekommt: Takt zieht nie stillschweigend
/// zu `fma` zusammen (4.2), und ohne `-fno-math-errno` wird
/// `__builtin_fmaf` zum Bibliotheksaufruf statt zum Befehl; Takt kennt kein
/// `errno` (FB-287).
pub const REFERENCE_FLAGS: [&str; 2] = ["-ffp-contract=off", "-fno-math-errno"];

/// Der Name der Bibliothek, ihres Kopfs, Moduls und Manifests.
pub const NAME: &str = "takt_bench";

/// Die Schritte des Laeufers: Messaufwand, die Kerne, Mathematik, Subnormale.
pub fn steps(kernels: &[Kernel]) -> usize {
    kernels.len() + 3
}

/// Das Praefix eines Kerns (12.11).
pub fn prefix(kernel: &Kernel) -> String {
    format!("bench_{}", kernel.name)
}

/// Wie eine C-Referenz beim Uebersetzen heisst: ihre beiden Einstiege mit
/// dem Namen des Kerns, als `-D`.
pub fn reference_names(kernel: &Kernel) -> [(String, String); 2] {
    let x = prefix(kernel);
    [
        ("takt_bench_reference".to_string(), format!("{x}_c")),
        ("takt_bench_reference_digest".to_string(), format!("{x}_c_digest")),
    ]
}

/// `-Dalt=neu` fuer jede Umbenennung.
pub fn defines<'a>(names: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    names.into_iter().map(|(from, to)| format!("-D{from}={to}")).collect()
}

/// Der Kopf `takt_bench.h`: die Haken, die der Wirt stellt, und die Einstiege.
pub fn header(kernels: &[Kernel], suite: &str) -> String {
    format!(
        "/* Das Messprogramm von `takt bench` (13.8) als Baustein; erzeugt von\n \
         * `takt bench --emit embed`. Nicht von Hand aendern.\n *\n \
         * Der Wirt stellt die beiden Haken und ruft je Schritt `takt_bench_measure`,\n \
         * unter seiner Interruptsperre, wenn er die Kerne allein messen will, und\n \
         * danach `takt_bench_report`; `takt bench --import` liest, was die Senke\n \
         * bekam. Jeder Schritt laeuft einmal je Start.\n */\n\
         #ifndef TAKT_BENCH_H\n#define TAKT_BENCH_H\n\n#include <stdint.h>\n\n\
         /* Die Schritte und die Kennung der Kerne, wie das Protokoll sie nennt. */\n\
         #define TAKT_BENCH_STEPS {}u\n#define TAKT_BENCH_SUITE \"{suite}\"\n\n\
         /* Die Haken des Wirts: der Zyklenzaehler des Kerns (laeuft frei, darf\n \
         * ueberlaufen) und ein Byte des Protokolls. */\n\
         uint32_t takt_bench_cycles(void *user);\nvoid takt_bench_put(void *user, uint8_t byte);\n\n\
         /* Die Einstiege; `user` geht unveraendert an die Haken. */\n\
         void takt_bench_begin(void *user, uint32_t core_hz);\n\
         void takt_bench_measure(void *user, uint32_t step, uint32_t runs);\n\
         void takt_bench_report(void *user, uint32_t step);\n\
         void takt_bench_end(void *user);\n\n#endif\n",
        steps(kernels)
    )
}

/// Das Manifest `takt_bench.manifest`: was der Bau des Wirts braucht, und
/// fuer `takt check-image` die Einstiege, die messen.
pub fn manifest(kernels: &[Kernel], suite: &str, target: &str, triple: &str, xip: bool) -> String {
    format!(
        "# takt-bench 1\n# Erzeugt von `takt bench --emit embed` (13.8).\nprefix = {NAME}\nsuite = {suite}\n\
         steps = {}\ntarget = {target}\ntriple = {triple}\nxip_flash = {xip}\ntick_path = {NAME}_measure\n",
        steps(kernels)
    )
}

/// Der Laeufer `takt_bench.c`.
pub fn runner(kernels: &[Kernel], vectors: &[Vector]) -> String {
    let n = steps(kernels);
    let (math, subnormal) = (kernels.len() + 1, kernels.len() + 2);
    let mut s = String::new();
    s.push_str(
        "/* Das Messprogramm von `takt bench` (13.8): Schritte, Messung, Protokoll.\n \
         * Erzeugt von `takt bench --emit embed`; nicht von Hand aendern. */\n\
         #include <stdint.h>\n\n#include \"takt_bench.h\"\n",
    );
    for k in kernels {
        let _ = writeln!(s, "#include \"{}.h\"", prefix(k));
    }
    let _ = write!(s, "\n#define WARMUP {WARMUP}u\n#define VECTORS {}u\n\n", vectors.len());

    s.push_str("/* Die C-Referenzen (13.8), beim Uebersetzen umbenannt. */\n");
    for k in kernels.iter().filter(|k| matches!(k.role, Role::Reference { .. })) {
        let [(_, run), (_, digest)] = reference_names(k);
        let _ = writeln!(s, "void {run}(void);\nunsigned long long {digest}(void);");
    }
    s.push_str("\n/* Die korrekt gerundete Mathematik (4.2): die Einstiege, die der erzeugte Code ruft. */\n");
    for (wide, t) in [(true, "double"), (false, "float")] {
        for fun in FUNCTIONS {
            let w = if wide { "f64" } else { "f32" };
            let params = if binary(fun) { format!("{t} x, {t} y") } else { format!("{t} x") };
            let _ = writeln!(s, "{t} takt_m_{fun}_{w}({params});");
        }
    }
    s.push_str(
        "\n/* Die Leitung der Kerne: Ein Messkern schreibt keinen Trace (7.2). */\n\
         void takt_bench_trace(const char *line) { (void)line; }\n\
         void takt_bench_trace_i64(long long value) { (void)value; }\n\
         void takt_bench_trace_u64(unsigned long long value) { (void)value; }\n\
         void takt_bench_trace_f64(double value) { (void)value; }\n\
         void takt_bench_trace_hex8(unsigned char value) { (void)value; }\n\n\
         /* Eine Messreihe in Zyklen; `digest` ist, was der Kern danach ergab. */\n\
         struct series {\n    uint32_t min;\n    uint32_t max;\n    uint64_t total;\n    uint32_t n;\n    \
         uint64_t digest;\n};\n\n\
         /* Die Kerne laufen nacheinander und teilen sich eine Arena. */\nstatic union {\n",
    );
    for (i, k) in kernels.iter().enumerate() {
        let _ = writeln!(s, "    struct {}_arena k{};", prefix(k), i + 1);
    }
    let _ = write!(
        s,
        "}} arena;\n\nstatic struct series takt_series[{n}];\nstatic struct series c_series[{n}];\n\
         static struct series entry_series[2];\nstatic uint64_t math_result[VECTORS];\n\
         static uint32_t math_cycles[VECTORS];\nstatic uint32_t subnormal;\n\n"
    );
    s.push_str("/* Die Namen der Schritte im Protokoll; eine C-Referenz hat nur ein Referenzkern. */\n");
    let _ = writeln!(s, "static const char *const names[{n}] = {{");
    s.push_str("    \"overhead\",\n");
    for k in kernels {
        let _ = writeln!(s, "    \"{}\",", k.name);
    }
    s.push_str("    \"math\",\n    \"subnormal\",\n};\n");
    let _ = writeln!(s, "static const uint8_t has_c[{n}] = {{");
    let flags: Vec<&str> = std::iter::once("0u")
        .chain(kernels.iter().map(|k| if matches!(k.role, Role::Reference { .. }) { "1u" } else { "0u" }))
        .chain(["0u", "0u"])
        .collect();
    let _ = writeln!(s, "    {}\n}};\n", flags.join(", "));
    s.push_str(SERIES_C);

    for (i, k) in kernels.iter().enumerate() {
        kernel_step(&mut s, i + 1, k);
    }
    math_step(&mut s, vectors);
    s.push_str(SUBNORMAL_C);
    s.push_str(REPORT_C);

    s.push_str("void takt_bench_measure(void *user, uint32_t step, uint32_t runs) {\n    switch (step) {\n");
    s.push_str("    case 0u:\n        measure_overhead(user, runs);\n        break;\n");
    for i in 1..=kernels.len() {
        let _ = writeln!(s, "    case {i}u:\n        measure_{i}(user, runs);\n        break;");
    }
    let _ = writeln!(s, "    case {math}u:\n        measure_math(user, runs);\n        break;");
    let _ = writeln!(s, "    case {subnormal}u:\n        measure_subnormal();\n        break;");
    s.push_str("    default:\n        break;\n    }\n}\n\n");

    let _ = write!(
        s,
        "void takt_bench_report(void *user, uint32_t step) {{\n    \
         if (step == 0u) {{\n        put_series(user, \"overhead\", (const char *)0, &takt_series[0]);\n    \
         }} else if (step < {math}u) {{\n        put_series(user, \"takt\", names[step], &takt_series[step]);\n        \
         if (has_c[step] != 0u) {{\n            put_series(user, \"c\", names[step], &c_series[step]);\n        }}\n    \
         }} else if (step == {math}u) {{\n        put_series(user, \"entry\", \"f32\", &entry_series[0]);\n        \
         put_series(user, \"entry\", \"f64\", &entry_series[1]);\n        put_math(user);\n    \
         }} else if (step == {subnormal}u) {{\n        put(user, \"bench subnormal \");\n        \
         put_u64(user, subnormal);\n        put(user, \"\\n\");\n    }} else {{\n    }}\n}}\n"
    );
    s
}

/// `atan2` und `pow` nehmen zwei Argumente.
fn binary(fun: &str) -> bool {
    matches!(fun, "atan2" | "pow")
}

/// Ein Schritt je Kern: `init`, Aufwaermen und `runs` Ticks, jeder zwischen
/// zwei Lesungen des Zaehlers; ein Referenzkern danach seine C-Referenz.
///
/// Gemessen wird ein Tick, wie die Schleife ihn faehrt: Schritt und Commit an
/// die Treiber (7.2: `T_IO` enthaelt den Commit). Die Treiber sind Stummel;
/// was ein echter Treiber kostet, gehoert in sein Budget.
///
/// Der Kern zaehlt ab 1, wie die Huelle der Lieferform (Tick 0 ist der
/// Start). Seine C-Referenz rechnet Tick 0 vorweg nach, sonst laege der Kern
/// einen Tick vorn und die Digests liessen sich nicht vergleichen (FB-287).
fn kernel_step(s: &mut String, step: usize, k: &Kernel) {
    let x = prefix(k);
    let a = format!("&arena.k{step}");
    let _ = write!(
        s,
        "static void measure_{step}(void *user, uint32_t runs) {{\n    \
         struct series *s = &takt_series[{step}];\n    int64_t k = 1;\n    reset(s);\n    \
         {x}_init({a}, (void *)0);\n    \
         for (uint32_t i = 0u; i < WARMUP + runs; i++) {{\n        \
         uint32_t start = takt_bench_cycles(user);\n        {x}_tick({a}, k);\n        {x}_commit({a});\n        \
         uint32_t spent = takt_bench_cycles(user) - start;\n        k++;\n        \
         if (i >= WARMUP) {{\n            add(s, spent);\n        }}\n    }}\n    \
         s->digest = (uint64_t){x}_output({a}, 0);\n"
    );
    if matches!(k.role, Role::Reference { .. }) {
        let [(_, run), (_, digest)] = reference_names(k);
        let _ = write!(
            s,
            "    struct series *c = &c_series[{step}];\n    reset(c);\n    {run}();\n    \
             for (uint32_t i = 0u; i < WARMUP + runs; i++) {{\n        \
             uint32_t start = takt_bench_cycles(user);\n        {run}();\n        \
             uint32_t spent = takt_bench_cycles(user) - start;\n        \
             if (i >= WARMUP) {{\n            add(c, spent);\n        }}\n    }}\n    \
             c->digest = (uint64_t){digest}();\n"
        );
    }
    s.push_str("}\n\n");
}

/// Die Vektoren der Mathematik aus `grammar/libtaktm.md` und ihr Schritt:
/// der Einstieg der Probe `math` an ihren Argumenten ([`super::MATH_ARGS`]),
/// `runs`-mal je Breite, dann je Vektor ein Aufruf zwischen zwei Lesungen
/// des Zaehlers, mit Ergebnis.
fn math_step(s: &mut String, vectors: &[Vector]) {
    s.push_str(
        "/* Die Vektoren der Stufe 2 (4.2): Funktion nach ihrer Stellung, `f64`, Argumente als Bits. */\n\
         struct vector {\n    uint8_t fun;\n    uint8_t wide;\n    uint64_t x;\n    uint64_t y;\n};\n\n\
         static const struct vector vectors[VECTORS] = {\n",
    );
    for v in vectors {
        let fun = FUNCTIONS.iter().position(|f| *f == v.fun).unwrap_or(0);
        let _ = writeln!(s, "    {{{fun}u, {}u, {:#018x}ULL, {:#018x}ULL}},", u8::from(v.wide), v.args[0], v.args[1]);
    }
    s.push_str("};\n\n");
    s.push_str(BITS_C);
    s.push_str("/* Ein Vektor, gerechnet ueber den Einstieg seiner Funktion. */\nstatic uint64_t math_call(const struct vector *v) {\n");
    for (wide, t, of, bits, w) in
        [(true, "double", "f64_of", "bits_f64", "f64"), (false, "float", "f32_of", "bits_f32", "f32")]
    {
        let _ = write!(
            s,
            "    {}{{\n        {t} x = {of}(v->x);\n        {t} y = {of}(v->y);\n        switch (v->fun) {{\n",
            if wide { "if (v->wide != 0u) " } else { "" }
        );
        for (i, fun) in FUNCTIONS.iter().enumerate() {
            let args = if binary(fun) { "x, y" } else { "x" };
            let _ = writeln!(s, "        case {i}u:\n            return {bits}(takt_m_{fun}_{w}({args}));");
        }
        s.push_str("        default:\n            (void)y;\n            return 0u;\n        }\n    }\n");
    }
    let pow = FUNCTIONS.iter().position(|f| *f == "pow").unwrap_or(0);
    let [x64, y64] = super::math_probe_bits(true);
    let [x32, y32] = super::math_probe_bits(false);
    let _ = write!(
        s,
        "}}\n\n/* Die Argumente der Probe `math`, wie der Kern sie rechnet. */\n\
         static const struct vector probes[2] = {{\n    {{{pow}u, 0u, {x32:#018x}ULL, {y32:#018x}ULL}},\n    \
         {{{pow}u, 1u, {x64:#018x}ULL, {y64:#018x}ULL}},\n}};\n\n\
         static void measure_math(void *user, uint32_t runs) {{\n    \
         for (uint32_t w = 0u; w < 2u; w++) {{\n        struct series *s = &entry_series[w];\n        \
         reset(s);\n        for (uint32_t i = 0u; i < WARMUP + runs; i++) {{\n            \
         uint32_t start = takt_bench_cycles(user);\n            uint64_t result = math_call(&probes[w]);\n            \
         uint32_t spent = takt_bench_cycles(user) - start;\n            s->digest = result;\n            \
         if (i >= WARMUP) {{\n                add(s, spent);\n            }}\n        }}\n    }}\n    \
         for (uint32_t i = 0u; i < VECTORS; i++) {{\n        \
         uint32_t start = takt_bench_cycles(user);\n        uint64_t result = math_call(&vectors[i]);\n        \
         math_cycles[i] = takt_bench_cycles(user) - start;\n        math_result[i] = result;\n    }}\n}}\n\n"
    );
}

/// Messreihen und der Schritt, der misst, was die Messung selbst kostet.
const SERIES_C: &str = "static void reset(struct series *s) {
    s->min = UINT32_MAX;
    s->max = 0u;
    s->total = 0u;
    s->n = 0u;
    s->digest = 0u;
}

static void add(struct series *s, uint32_t cycles) {
    if (cycles < s->min) {
        s->min = cycles;
    }
    if (cycles > s->max) {
        s->max = cycles;
    }
    s->total += cycles;
    s->n++;
}

/* Schritt 0: zwei Lesungen des Zaehlers ohne etwas dazwischen. Ihr Minimum
 * zieht `takt bench --import` von jeder Messung ab. */
static void measure_overhead(void *user, uint32_t runs) {
    struct series *s = &takt_series[0];
    reset(s);
    for (uint32_t i = 0u; i < WARMUP + runs; i++) {
        uint32_t start = takt_bench_cycles(user);
        uint32_t spent = takt_bench_cycles(user) - start;
        if (i >= WARMUP) {
            add(s, spent);
        }
    }
}

";

/// Bitmuster und Zahlen, ohne Typumdeutung ueber Zeiger.
const BITS_C: &str = "static double f64_of(uint64_t bits) {
    double d;
    __builtin_memcpy(&d, &bits, sizeof d);
    return d;
}

static float f32_of(uint64_t bits) {
    uint32_t low = (uint32_t)bits;
    float f;
    __builtin_memcpy(&f, &low, sizeof f);
    return f;
}

static uint64_t bits_f64(double d) {
    uint64_t bits;
    __builtin_memcpy(&bits, &d, sizeof bits);
    return bits;
}

static uint64_t bits_f32(float f) {
    uint32_t bits;
    __builtin_memcpy(&bits, &f, sizeof bits);
    return (uint64_t)bits;
}

";

/// Der Subnormal-Vektor (4.2, 13.8): die Faelle, deren Ergebnis nicht das
/// IEEE-754-Bitmuster ist. Ein Kern mit Flush-to-Zero liefert statt der
/// kleinen Werte null und faellt in jedem Fall auf; `volatile` haelt den
/// Uebersetzer davon ab, die Ergebnisse vorauszurechnen.
const SUBNORMAL_C: &str = "/* Der Subnormal-Vektor (4.2): kleinste Normale halbiert, kleinste Subnormale
 * verdreifacht, zwei Subnormale ergeben eine Normale, die Differenz zweier
 * Normaler ist subnormal. */
static void measure_subnormal(void) {
    static const uint64_t a64[4] = {0x0010000000000000ULL, 0x0000000000000001ULL, 0x0008000000000000ULL,
                                    0x0010000000000001ULL};
    static const uint64_t b64[4] = {0x4000000000000000ULL, 0x4008000000000000ULL, 0x0008000000000000ULL,
                                    0x0010000000000000ULL};
    static const uint64_t want64[4] = {0x0008000000000000ULL, 0x0000000000000003ULL, 0x0010000000000000ULL,
                                       0x0000000000000001ULL};
    static const uint32_t a32[4] = {0x00800000u, 0x00000001u, 0x00400000u, 0x00800001u};
    static const uint32_t b32[4] = {0x40000000u, 0x40400000u, 0x00400000u, 0x00800000u};
    static const uint32_t want32[4] = {0x00400000u, 0x00000003u, 0x00800000u, 0x00000001u};
    uint32_t failures = 0u;
    for (uint32_t i = 0u; i < 4u; i++) {
        volatile double a = f64_of(a64[i]);
        volatile double b = f64_of(b64[i]);
        volatile float c = f32_of(a32[i]);
        volatile float d = f32_of(b32[i]);
        double wide;
        float narrow;
        switch (i) {
        case 0u:
            wide = a / b;
            narrow = c / d;
            break;
        case 1u:
            wide = a * b;
            narrow = c * d;
            break;
        case 2u:
            wide = a + b;
            narrow = c + d;
            break;
        default:
            wide = a - b;
            narrow = c - d;
            break;
        }
        failures += (bits_f64(wide) != want64[i]) ? 1u : 0u;
        failures += (bits_f32(narrow) != (uint64_t)want32[i]) ? 1u : 0u;
    }
    subnormal = failures;
}

";

/// Das Protokoll: Zeilen ueber den Haken `takt_bench_put`.
const REPORT_C: &str = "static void put(void *user, const char *text) {
    for (const char *p = text; *p != '\\0'; p++) {
        takt_bench_put(user, (uint8_t)*p);
    }
}

static void put_u64(void *user, uint64_t value) {
    char digits[20];
    uint32_t n = 0u;
    do {
        digits[n] = (char)('0' + (char)(value % 10u));
        n++;
        value /= 10u;
    } while (value != 0u);
    while (n > 0u) {
        n--;
        takt_bench_put(user, (uint8_t)digits[n]);
    }
}

static void put_hex(void *user, uint64_t value) {
    static const char hex[] = \"0123456789abcdef\";
    for (uint32_t shift = 64u; shift > 0u; shift -= 4u) {
        takt_bench_put(user, (uint8_t)hex[(value >> (shift - 4u)) & 15u]);
    }
}

/* `bench <art> [<name>] min .. mean .. max .. n .. digest ..` */
static void put_series(void *user, const char *kind, const char *name, const struct series *s) {
    put(user, \"bench \");
    put(user, kind);
    if (name != (const char *)0) {
        put(user, \" \");
        put(user, name);
    }
    put(user, \" min \");
    put_u64(user, (s->n == 0u) ? 0u : s->min);
    put(user, \" mean \");
    put_u64(user, (s->n == 0u) ? 0u : (s->total / s->n));
    put(user, \" max \");
    put_u64(user, s->max);
    put(user, \" n \");
    put_u64(user, s->n);
    put(user, \" digest \");
    put_u64(user, s->digest);
    put(user, \"\\n\");
}

/* `math <i> <ergebnis> cycles <zyklen>`, wie `takt_conformance::math` sie liest. */
static void put_math(void *user) {
    for (uint32_t i = 0u; i < VECTORS; i++) {
        put(user, \"math \");
        put_u64(user, i);
        put(user, \" \");
        put_hex(user, math_result[i]);
        put(user, \" cycles \");
        put_u64(user, math_cycles[i]);
        put(user, \"\\n\");
    }
}

void takt_bench_begin(void *user, uint32_t core_hz) {
    put(user, \"\\ntakt bench \" TAKT_BENCH_SUITE \"\\nbench core_hz \");
    put_u64(user, core_hz);
    put(user, \"\\n\");
}

void takt_bench_end(void *user) {
    put(user, \"takt end\\n\");
}

";

/// Das Modul `takt_bench.rs` fuer einen Wirt in Rust: die Haken als Trait,
/// die Einstiege als Funktionen und ein Lauf ueber alle Schritte.
pub fn rust_module(kernels: &[Kernel], suite: &str) -> String {
    format!(
        "// Das Messprogramm von `takt bench` (13.8) fuer einen Wirt in Rust; erzeugt von\n\
         // `takt bench --emit embed`. Nicht von Hand aendern.\n\n\
         /// Die Schritte des Messprogramms; jeder misst und schreibt fuer sich.\n\
         pub const STEPS: u32 = {steps};\n\n\
         /// Die Kennung der Kerne, wie das Protokoll sie nennt.\n\
         pub const SUITE: &str = \"{suite}\";\n\n\
         /// Was der Wirt dem Messprogramm stellt (13.8).\n\
         pub trait Host {{\n    \
         /// Der Zyklenzaehler des Kerns: laeuft frei und darf ueberlaufen.\n    \
         fn cycles(&mut self) -> u32;\n\n    \
         /// Ein Byte des Protokolls; es geht an `takt bench --import`.\n    \
         fn put(&mut self, byte: u8);\n}}\n\n\
         mod ffi {{\n    use core::ffi::c_void;\n\n    unsafe extern \"C\" {{\n        \
         pub fn takt_bench_begin(user: *mut c_void, core_hz: u32);\n        \
         pub fn takt_bench_measure(user: *mut c_void, step: u32, runs: u32);\n        \
         pub fn takt_bench_report(user: *mut c_void, step: u32);\n        \
         pub fn takt_bench_end(user: *mut c_void);\n    }}\n}}\n\n\
         /// Der Wirt, wie die Haken ihn sehen: Ein duenner Zeiger zeigt auf diesen Griff.\n\
         type Hooks<'a> = &'a mut dyn Host;\n\n\
         #[unsafe(no_mangle)]\n\
         extern \"C\" fn takt_bench_cycles(user: *mut core::ffi::c_void) -> u32 {{\n    \
         // SAFETY: `user` zeigt fuer die Dauer eines Einstiegs auf den Griff aus `with`.\n    \
         unsafe {{ (*user.cast::<Hooks<'_>>()).cycles() }}\n}}\n\n\
         #[unsafe(no_mangle)]\n\
         extern \"C\" fn takt_bench_put(user: *mut core::ffi::c_void, byte: u8) {{\n    \
         // SAFETY: wie in `takt_bench_cycles`.\n    \
         unsafe {{ (*user.cast::<Hooks<'_>>()).put(byte) }}\n}}\n\n\
         /// Ruft einen Einstieg mit dem Wirt als `user`.\n\
         fn with(host: &mut dyn Host, entry: impl FnOnce(*mut core::ffi::c_void)) {{\n    \
         let mut hooks: Hooks<'_> = host;\n    entry((&raw mut hooks).cast());\n}}\n\n\
         /// Schreibt den Kopf des Protokolls mit dem Kerntakt in Hertz.\n\
         pub fn begin(host: &mut dyn Host, core_hz: u32) {{\n    \
         // SAFETY: Der Einstieg ruft die Haken nur mit diesem `user`.\n    \
         with(host, |user| unsafe {{ ffi::takt_bench_begin(user, core_hz) }});\n}}\n\n\
         /// Misst Schritt `step` mit `runs` Messungen je Reihe; schreibt nichts.\n\
         pub fn measure(host: &mut dyn Host, step: u32, runs: u32) {{\n    \
         // SAFETY: wie in `begin`.\n    \
         with(host, |user| unsafe {{ ffi::takt_bench_measure(user, step, runs) }});\n}}\n\n\
         /// Schreibt, was Schritt `step` gemessen hat.\n\
         pub fn report(host: &mut dyn Host, step: u32) {{\n    \
         // SAFETY: wie in `begin`.\n    \
         with(host, |user| unsafe {{ ffi::takt_bench_report(user, step) }});\n}}\n\n\
         /// Schreibt das Ende des Protokolls.\n\
         pub fn end(host: &mut dyn Host) {{\n    \
         // SAFETY: wie in `begin`.\n    \
         with(host, |user| unsafe {{ ffi::takt_bench_end(user) }});\n}}\n\n\
         /// Alle Schritte nach `begin` und vor `end`: `locked` faehrt eine Messung\n\
         /// ohne Unterbrechung, etwa unter der Interruptsperre des Wirts; die Senke\n\
         /// laeuft nie darunter.\n\
         pub fn steps(host: &mut dyn Host, runs: u32, mut locked: impl FnMut(&mut dyn FnMut())) {{\n    \
         for step in 0..STEPS {{\n        locked(&mut || measure(host, step, runs));\n        \
         report(host, step);\n    }}\n}}\n",
        steps = steps(kernels)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernels() -> Vec<Kernel> {
        let source = super::super::frame_kernel();
        vec![
            Kernel { name: "frame".into(), source: source.clone(), role: Role::Frame },
            Kernel { name: "pid".into(), source, role: Role::Reference { c: String::new() } },
        ]
    }

    fn vectors() -> Vec<Vector> {
        vec![Vector { line: 1, fun: "pow", wide: true, args: [0x4000_0000_0000_0000, 0x3ff0_0000_0000_0000], want: 0 }]
    }

    /// Jeder Kern hat seinen Schritt und seinen Platz in der Arena; ein
    /// Referenzkern misst dazu seine C-Referenz, umbenannt auf sein Praefix.
    #[test]
    fn every_kernel_has_its_step() {
        let c = runner(&kernels(), &vectors());
        assert!(c.contains("#include \"bench_frame.h\"\n#include \"bench_pid.h\""), "{c}");
        assert!(c.contains("struct bench_frame_arena k1;\n    struct bench_pid_arena k2;"), "{c}");
        assert!(c.contains("case 1u:\n        measure_1(user, runs);"), "{c}");
        assert!(c.contains("case 3u:\n        measure_math(user, runs);"), "{c}");
        assert!(
            c.contains(&format!("{{9u, 1u, {:#018x}ULL, {:#018x}ULL}},", 0.7f64.to_bits(), 1.3f64.to_bits())),
            "{c}"
        );
        assert!(c.contains("case 4u:\n        measure_subnormal();"), "{c}");
        assert!(c.contains("bench_pid_c();\n    for"), "Tick 0 der Referenz vorweg: {c}");
        assert!(c.contains("bench_frame_tick(&arena.k1, k);\n        bench_frame_commit(&arena.k1);"), "{c}");
        assert!(c.contains("c->digest = (uint64_t)bench_pid_c_digest();"), "{c}");
        assert!(!c.contains("bench_frame_c_digest"), "der leere Kern hat keine Referenz: {c}");
        assert!(c.contains("{9u, 1u, 0x4000000000000000ULL, 0x3ff0000000000000ULL},"), "{c}");
        assert!(c.contains("case 9u:\n            return bits_f64(takt_m_pow_f64(x, y));"), "{c}");
        assert!(c.contains("static const uint8_t has_c[5] = {\n    0u, 0u, 1u, 0u, 0u\n};"), "{c}");
    }

    /// Kopf und Modul nennen dieselbe Zahl der Schritte und dieselbe Kennung.
    #[test]
    fn header_and_module_agree() {
        let h = header(&kernels(), "0123456789abcdef");
        let r = rust_module(&kernels(), "0123456789abcdef");
        assert!(h.contains("#define TAKT_BENCH_STEPS 5u\n#define TAKT_BENCH_SUITE \"0123456789abcdef\""), "{h}");
        assert!(r.contains("pub const STEPS: u32 = 5;") && r.contains("pub const SUITE: &str = \"0123456789abcdef\";"));
        for entry in ["takt_bench_begin", "takt_bench_measure", "takt_bench_report", "takt_bench_end"] {
            assert!(
                h.contains(&format!("void {entry}(void *user")) && r.contains(&format!("pub fn {entry}(")),
                "{entry}"
            );
        }
        assert!(
            h.contains("uint32_t takt_bench_cycles(void *user);") && r.contains("extern \"C\" fn takt_bench_cycles(")
        );
    }

    #[test]
    fn the_renames_are_defines() {
        let k = &kernels()[1];
        let names = reference_names(k);
        let d = defines(names.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        assert_eq!(d, ["-Dtakt_bench_reference=bench_pid_c", "-Dtakt_bench_reference_digest=bench_pid_c_digest"]);
        assert_eq!(defines(TRACE)[0], "-Dtakt_board_trace=takt_bench_trace");
    }
}
