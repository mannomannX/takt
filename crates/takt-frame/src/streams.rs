//! Die Stroeme des Testrahmens (8.6, 9.6).
//!
//! **Was fehlte.** `takt_stream_count` lieferte null, `takt_stream_at`
//! schrieb nichts. Der Handler-Dispatch im erzeugten Code war damit
//! gebaut, aber nie mit Daten ausgefuehrt — er lief ueber ein Fenster,
//! das immer leer war (FB-115).
//!
//! **Ein Ring je Strom, gespeist vom Treiberrand.** Jeder Eingabestrom
//! hat im Lauf einen Ring wie ein interner Strom: Byte-Ring mit
//! Deskriptoren, Cursor je Leser. Seine Elemente bringt der Treiberrand
//! (`edge.rs`, 12.6), auf dem Wirt aus dem Stimulus, auf dem Board von den
//! Treibern — erst nachdem der Vertrag des Treibers gehalten hat, und dann
//! sofort sichtbar (8.6). Fruehere Fassungen legten den Stimulus vorab in
//! eine Tabelle; das ging, solange jedes Element ankam. Seit der Rand im
//! Lauf urteilt, entscheidet sich erst im Lauf, was ankommt, und die
//! Schranken greifen wie im Interpreter — auch fuer einen Leser, der
//! zurueckfaellt.

use std::fmt::Write as _;
use takt_llvm::symbols::Prefix;

use takt_mir::TypeId;
use takt_mir::program::{Channel, Direction, Program};
use takt_mir::types::Type;

use crate::text::Text;

/// Wohin der Rahmen seine Trace-Zeilen schreibt: `printf` auf dem Wirt,
/// die `takt_board_trace*`-Aufrufe des Boards auf der MCU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trace {
    /// `stdio`, der Linux-Rahmen.
    Stdio,
    /// Das Board stellt `takt_board_trace`, `_i64` und `_hex8`.
    Board,
}

/// Schreibt die Aufrufe aus `takt-llvm/src/stream.rs` als C: jeder Strom,
/// interner wie Eingabestrom, ueber seinen Ring.
///
/// **Was ein Programm nicht hat, bekommt es nicht.** Ohne Ringe stehen
/// weder ihre Felder in der Arena noch ihr Code im Rahmen; es bleiben
/// Commit und Bericht, die der Tick ruft, als leere Funktionen. Einzelne
/// `static`-Puffer hatte der Compiler entfernt, wenn niemand sie las —
/// Felder der Arena bleiben stehen (12.11).
pub fn emit(t: &mut Text, p: &Program, trace: Trace, x: &Prefix) {
    let dyns = dynamic_streams(p);
    if dyns.is_empty() {
        let _ = writeln!(t.code, "static void takt_int_commit(struct {x}_arena *a) {{ (void)a; }}");
        let _ = writeln!(
            t.code,
            "static void takt_stream_report(struct {x}_arena *a, long long t) {{ (void)a; (void)t; }}\n"
        );
        emit_send(t, p, false, trace, x);
        return;
    }
    emit_internal(t, p, &dyns, x);
    let s = &mut t.code;
    let _ = writeln!(s, "/* Stroeme (8.6, 9.6): jeder ueber seinen Ring. */");
    let _ = writeln!(s, "int {x}_stream_count(struct {x}_arena *a, int s, long long cur) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    return k >= 0 ? takt_int_count(a, k, cur) : 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "long long {x}_stream_bind(struct {x}_arena *a, int s, long long cur, int i, void *data, long long *t) {{"
    );
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    return k >= 0 ? takt_int_bind(a, k, cur, i, data, t) : 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "long long {x}_stream_at(struct {x}_arena *a, int s, long long cur, int i, void *out) {{");
    let _ = writeln!(s, "    return {x}_stream_bind(a, s, cur, i, (unsigned char *)out + 8, (long long *)out);");
    let _ = writeln!(s, "}}");
    // 9.6: `cur[s, m] = examined + 1`. Der Cursor gehoert der Maschine,
    // und der erzeugte Code fuehrt ihn in seinem Zustand; der Ring gibt
    // frei, was jeder Leser untersucht hat.
    let _ = writeln!(s, "void {x}_stream_examined(struct {x}_arena *a, int s, int m, long long seq) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    if (k >= 0) takt_int_examined(a, k, m, seq);");
    let _ = writeln!(s, "}}");
    // 8.6: `s.dropped` (0), `s.overflowed` (1), `s.malformed` (2) am Ring.
    let _ = writeln!(s, "int {x}_stream_counter(struct {x}_arena *a, int s, int which) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    if (k < 0) return 0;");
    let _ = writeln!(
        s,
        "    return (int)(which == 0 ? a->int_dropped[k] : which == 1 ? a->int_overflowed[k] : a->int_malformed[k]);"
    );
    let _ = writeln!(s, "}}\n");
    report(t, &dyns, trace, x);
    emit_send(t, p, true, trace, x);
}

/// `takt_stream_report(t)`: die Zeile `stream <name> dropped=… overflowed=…
/// malformed=…` je Strom, dessen Zaehler sich geaendert haben (8.6,
/// `grammar/trace.md`) — erst die Eingabestroeme, dann die internen, wie
/// der Interpreter. Der erste Aufruf nach Tick 0 merkt den Stand nur: In
/// Tick 0 sind die Zaehler keine Beobachtung (`Recorder::stream_counters`).
fn report(t: &mut Text, dyns: &[Dynamic], trace: Trace, x: &Prefix) {
    let order: Vec<usize> =
        (0..dyns.len()).filter(|&k| dyns[k].input).chain((0..dyns.len()).filter(|&k| !dyns[k].input)).collect();
    let rows = dyns.len();
    let names: Vec<String> = dyns.iter().map(|d| format!("\"{}\"", d.name)).collect();
    let _ = writeln!(t.fields, "    unsigned int_shown[{rows}][3];");
    let _ = writeln!(t.fields, "    _Bool int_based;");
    let s = &mut t.code;
    let _ = writeln!(s, "static const char *const g_int_name[{rows}] = {{ {} }};", names.join(", "));
    let _ = writeln!(s, "static void takt_stream_report(struct {x}_arena *a, long long t) {{");
    {
        let list: Vec<String> = order.iter().map(usize::to_string).collect();
        let _ = writeln!(s, "    static const int order[] = {{ {} }};", list.join(", "));
        let _ = writeln!(s, "    for (unsigned j = 0; j < sizeof order / sizeof order[0]; j++) {{");
        let _ = writeln!(s, "        int k = order[j];");
        let _ = writeln!(
            s,
            "        unsigned now[3] = {{ a->int_dropped[k], a->int_overflowed[k], a->int_malformed[k] }};"
        );
        let _ = writeln!(s, "        if (a->int_based && memcmp(now, a->int_shown[k], sizeof now) == 0) continue;");
        let _ = writeln!(s, "        memcpy(a->int_shown[k], now, sizeof now);");
        let _ = writeln!(s, "        if (!a->int_based) continue;");
        match trace {
            Trace::Stdio => {
                let _ = writeln!(
                    s,
                    "        printf(\"t=%lld stream %s dropped=%u overflowed=%u malformed=%u\\n\", t, g_int_name[k], now[0], now[1], now[2]);"
                );
            }
            Trace::Board => {
                let _ = writeln!(s, "        takt_board_trace(\"t=\");");
                let _ = writeln!(s, "        takt_board_trace_i64(t);");
                let _ = writeln!(s, "        takt_board_trace(\"stream \");");
                let _ = writeln!(s, "        takt_board_trace(g_int_name[k]);");
                let _ = writeln!(s, "        takt_board_trace(\" dropped=\");");
                let _ = writeln!(s, "        takt_board_trace_u64(now[0]);");
                let _ = writeln!(s, "        takt_board_trace(\"overflowed=\");");
                let _ = writeln!(s, "        takt_board_trace_u64(now[1]);");
                let _ = writeln!(s, "        takt_board_trace(\"malformed=\");");
                let _ = writeln!(s, "        takt_board_trace_u64(now[2]);");
                let _ = writeln!(s, "        takt_board_trace(\"\\n\");");
            }
        }
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    a->int_based = 1;");
    let _ = writeln!(s, "}}\n");
}

/// Die Gestalt eines Elementtyps, dessen Elemente in kanonischer Form
/// kommen und ein `decode` brauchen (12.6, Zeile 5): alles ausser Text,
/// Bytes und `u8` — dieselbe Wahl wie `elements_of` im Interpreter.
pub(crate) fn element_shape(p: &Program, elem: TypeId) -> Option<Vec<u8>> {
    match p.types.list.get(elem.index())? {
        Type::Line { .. } | Type::Str { .. } | Type::Bytes { .. } => None,
        Type::Int { width: takt_mir::types::IntWidth::U8, .. } => None,
        _ => takt_mir::bytes::shape(p, elem).ok(),
    }
}

/// Die Gestalten der Elementtypen aller Eingabestroeme, die ein `decode`
/// brauchen, als `g_shape_<kanal>`, und der Leser der TCB dazu.
fn shapes(s: &mut String, p: &Program) {
    let _ = writeln!(s, "{}", crate::edge::DECODES);
    for (c, ch) in p.channels.iter().enumerate() {
        let Some(Type::Stream(elem)) = p.types.list.get(ch.ty.index()) else { continue };
        if ch.dir != Direction::Input {
            continue;
        }
        let Some(shape) = element_shape(p, *elem) else { continue };
        let bytes: Vec<String> = shape.iter().map(u8::to_string).collect();
        let _ = writeln!(s, "static const unsigned char g_shape_{c}[] = {{ {} }}; /* {} */", bytes.join(", "), ch.name);
    }
}

/// Die Kapazitaet der Bytes eines Elements (8.6, 3.9): `N` bei Text,
/// sonst die kanonische Byteform — dieselbe Rechnung wie
/// `takt_llvm::stream::scratch`.
pub(crate) fn payload_cap(p: &Program, elem: TypeId) -> u32 {
    match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => *cap,
        _ => takt_mir::bytes::max_size(p, elem).unwrap_or(1),
    }
}

/// Interne Stroeme (8.6, 9.6): Elemente entstehen im Lauf durch `send`,
/// werden im naechsten Tick sichtbar (Unit-Delay) und verschwinden, sobald
/// jeder Leser sie untersucht hat — den Stand je Leser meldet
/// `takt_stream_examined(s, m, seq)`, die Leser eines Stroms kennt die
/// Sema (`Stream::readers`). Ein voller Ring weist das Element ab, und der
/// Sender faultet (8.6); mit `overflow = drop` verwirft er es still.
/// `drop_oldest` fuehrt `limits.rs` als ungeprueft.
/// Ein Ring im Lauf: ein interner Strom oder ein Eingabestrom.
struct Dynamic {
    /// Nummer, wie der erzeugte Code sie uebergibt.
    id: i64,
    name: String,
    capacity: u32,
    /// `capacity_bytes` (8.6); die Sema setzt den Default.
    capacity_bytes: u32,
    readers: Vec<u32>,
    /// Ein Eingabestrom: `stream`-Zeilen kommen vor denen der internen.
    input: bool,
    /// Zaehlt ein abgewiesenes `send` als `overflowed`? Mit `overflow =
    /// drop` nicht, wie `System::send` im Interpreter.
    counts_overflow: bool,
}

/// Die Ringe des Laufs: erst die internen Stroeme, dann die
/// Eingabestroeme in Kanalreihenfolge. Einen Eingabestrom speist der
/// Treiberrand (`edge.rs`), einen gekoppelten der `sim`-Ausgabestrom
/// derselben Adresse (8.3), den Schreibstrom eines Registerports auf dem
/// Wirt der Rahmen (`ports.rs`, 12.10).
fn dynamic_streams(p: &Program) -> Vec<Dynamic> {
    let mut out: Vec<Dynamic> = p
        .streams
        .iter()
        .enumerate()
        .map(|(i, st)| Dynamic {
            id: -1 - i as i64,
            name: st.name.clone(),
            capacity: st.capacity,
            capacity_bytes: st.capacity_bytes.unwrap_or(st.capacity.saturating_mul(payload_cap(p, st.elem))),
            readers: st.readers.iter().map(|m| m.0).collect(),
            input: false,
            counts_overflow: !matches!(st.overflow, takt_mir::program::Overflow::Drop),
        })
        .collect();
    let inputs = p.channels.iter().enumerate().filter_map(|(i, c)| match p.types.list.get(c.ty.index()) {
        Some(Type::Stream(elem)) if c.dir == Direction::Input => Some((i, *elem)),
        _ => None,
    });
    for (in_id, elem) in inputs {
        let c = &p.channels[in_id];
        let readers = p
            .machines
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.layout.cursors.contains(&takt_mir::expr::StreamRef::Channel(takt_mir::ChannelId(in_id as u32)))
            })
            .map(|(i, _)| i as u32)
            .collect();
        let capacity = c.attrs.capacity.unwrap_or(16);
        out.push(Dynamic {
            id: in_id as i64,
            name: c.name.clone(),
            capacity,
            capacity_bytes: c.attrs.capacity_bytes.unwrap_or(capacity.saturating_mul(payload_cap(p, elem))),
            readers,
            input: true,
            counts_overflow: true,
        });
    }
    out
}

/// Speist ein `sim`-Ausgabestrom den Eingabestrom `channel` (8.3)? Dann
/// liefert ihn das Modell, kein Treiber.
pub(crate) fn coupled_input(p: &Program, channel: usize) -> bool {
    coupled(p).iter().any(|(i, _, _)| *i == channel)
}

/// `(Eingang, Ausgang, Elementtyp)` je Kopplung: ein `hw`-Eingabestrom und
/// der `sim`-Ausgabestrom derselben Adresse (8.3).
fn coupled(p: &Program) -> Vec<(usize, usize, TypeId)> {
    use takt_mir::program::Binding;
    let mut out = Vec::new();
    for (o, out_c) in p.channels.iter().enumerate() {
        let Binding::Sim(addr) = &out_c.binding else { continue };
        if out_c.dir != Direction::Output {
            continue;
        }
        let Some((i, in_c)) = p
            .channels
            .iter()
            .enumerate()
            .find(|(_, c)| c.dir == Direction::Input && matches!(&c.binding, Binding::Hw(a) if a == addr))
        else {
            continue;
        };
        let Some(Type::Stream(elem)) = p.types.list.get(in_c.ty.index()) else { continue };
        out.push((i, o, *elem));
    }
    out
}

fn emit_internal(t: &mut Text, p: &Program, dyns: &[Dynamic], x: &Prefix) {
    let n = dyns.len();
    let rows = n;
    let readers = dyns.iter().map(|d| d.readers.len()).max().unwrap_or(1).max(1);
    // Byte-Ring mit Deskriptorring je Strom (8.6): Ein Element belegt,
    // was es lang ist, nicht seine Kapazitaet — `stream<bytes<1024>>` mit
    // `capacity = 8` und `capacity_bytes = 1024` sind 1,2 KB statt 8,4 KB
    // in Slots. Feste Elemente fuellen ihren Ring genau (`CAPB = CAP * N`,
    // 9.6), also gilt ein Weg fuer beide. Der Deskriptor traegt `off`
    // und `len`; die Freigabe ist FIFO, darum reicht ein Lesezeiger, und
    // die Nummer folgt aus der naechsten und der Zahl der Elemente.
    let mut doff = Vec::with_capacity(n);
    let mut boff = Vec::with_capacity(n);
    let (mut descs, mut bytes) = (0usize, 0usize);
    for d in dyns {
        doff.push(descs);
        boff.push(bytes);
        descs += d.capacity.max(1) as usize;
        bytes += d.capacity_bytes.max(1) as usize;
    }
    let list = |v: Vec<usize>| v.iter().map(usize::to_string).collect::<Vec<_>>().join(", ");
    let _ = writeln!(t.code, "#define TAKT_INT_STREAMS {n}");
    // 12 statt 16 Byte je Element: der Zeitstempel in zwei Haelften (kein
    // 8-Byte-Feld, also Ausrichtung 4), Versatz und Laenge als u16, wo
    // die Bytekapazitaet es zulaesst. Ein interner Strom stempelt mit der
    // Tickgrenze, der Rand mit dem Zeitstempel der Lieferung (12.6).
    let narrow = dyns.iter().all(|d| d.capacity_bytes <= 65_535);
    let field = if narrow { "unsigned short" } else { "unsigned" };
    let _ = writeln!(t.types, "struct {x}_idesc {{ unsigned t_lo, t_hi; {field} off, len; }};");
    let machines = p.machines.len().max(1);
    let f = &mut t.fields;
    let _ = writeln!(f, "    struct {x}_idesc int_desc[{descs}];");
    let _ = writeln!(f, "    _Alignas(8) unsigned char int_pool[{}];", bytes.max(8));
    let _ = writeln!(f, "    int int_head[{rows}], int_n[{rows}], int_new[{rows}];");
    let _ = writeln!(f, "    int int_bhead[{rows}], int_bused[{rows}];");
    let _ = writeln!(f, "    long long int_seq[{rows}];");
    // 8.6: `s.dropped`, `s.overflowed` und `s.malformed` am Ring, wie
    // `Buffer` im Interpreter.
    let _ = writeln!(f, "    unsigned int_dropped[{rows}], int_overflowed[{rows}], int_malformed[{rows}];");
    let _ = writeln!(f, "    long long int_ex[{rows}][{machines}]; /* examined + 1 je Leser */");
    let s = &mut t.code;
    let _ = writeln!(s, "/* Ringe im Lauf (8.6, 8.3): Byte-Ring mit Deskriptoren, Unit-Delay, Cursor je Leser. */");
    let _ = writeln!(s, "static const int g_int_doff[{rows}] = {{ {} }};", list(doff));
    let _ = writeln!(s, "static const int g_int_boff[{rows}] = {{ {} }};", list(boff));
    let _ = writeln!(
        s,
        "static const int g_int_cap[{rows}] = {{ {} }};",
        list(dyns.iter().map(|d| d.capacity.max(1) as usize).collect())
    );
    let _ = writeln!(
        s,
        "static const int g_int_capb[{rows}] = {{ {} }};",
        list(dyns.iter().map(|d| d.capacity_bytes.max(1) as usize).collect())
    );
    let _ = writeln!(
        s,
        "static const _Bool g_int_counts_overflow[{rows}] = {{ {} }};",
        list(dyns.iter().map(|d| usize::from(d.counts_overflow)).collect())
    );
    let _ = writeln!(s, "static const int g_int_readers[{rows}][{readers}] = {{");
    for d in dyns {
        let list: Vec<String> = d.readers.iter().map(|m| m.to_string()).collect();
        let _ = writeln!(
            s,
            "    {{ {} }}, /* {} */",
            if list.is_empty() { "0".to_string() } else { list.join(", ") },
            d.name
        );
    }
    let _ = writeln!(s, "}};");
    let counts: Vec<String> = dyns.iter().map(|d| d.readers.len().to_string()).collect();
    let _ = writeln!(s, "static const int g_int_reader_n[{rows}] = {{ {} }};", counts.join(", "));
    // Die Nummer des erzeugten Codes auf den Ring: negativ die internen
    // Stroeme, sonst ein gekoppelter Eingang.
    let _ = writeln!(s, "static int takt_int_slot(int s) {{");
    let _ = writeln!(s, "    switch (s) {{");
    for (k, d) in dyns.iter().enumerate() {
        let _ = writeln!(s, "    case {}: return {k}; /* {} */", d.id, d.name);
    }
    let _ = writeln!(s, "    default: return -1;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static struct {x}_idesc *takt_int_desc(struct {x}_arena *a, int k, int i) {{");
    let _ = writeln!(s, "    int j = a->int_head[k] + i;");
    let _ = writeln!(s, "    if (j >= g_int_cap[k]) j -= g_int_cap[k];");
    let _ = writeln!(s, "    return &a->int_desc[g_int_doff[k] + j];");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static long long takt_int_seq_at(struct {x}_arena *a, int k, int i) {{");
    let _ = writeln!(s, "    return a->int_seq[k] - a->int_n[k] + i;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "/* Der Byte-Ring laeuft um; ein Element darf ueber das Ende reichen. */");
    let _ = writeln!(s, "static void takt_int_read(struct {x}_arena *a, int k, int off, unsigned char *dst, int n) {{");
    let _ = writeln!(s, "    const unsigned char *b = a->int_pool + g_int_boff[k];");
    let _ = writeln!(s, "    int first = g_int_capb[k] - off;");
    let _ = writeln!(s, "    if (first > n) first = n;");
    let _ = writeln!(s, "    memcpy(dst, b + off, (size_t)first);");
    let _ = writeln!(s, "    if (n > first) memcpy(dst + first, b, (size_t)(n - first));");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_int_write(struct {x}_arena *a, int k, int off, const unsigned char *src, int n) {{"
    );
    let _ = writeln!(s, "    unsigned char *b = a->int_pool + g_int_boff[k];");
    let _ = writeln!(s, "    int first = g_int_capb[k] - off;");
    let _ = writeln!(s, "    if (first > n) first = n;");
    let _ = writeln!(s, "    memcpy(b + off, src, (size_t)first);");
    let _ = writeln!(s, "    if (n > first) memcpy(b, src + first, (size_t)(n - first));");
    let _ = writeln!(s, "}}");
    // 9.6: Sichtbar ist, was ab `cur` liegt und in einem frueheren Tick
    // kam. Die Nummern steigen mit der Lage, also ist der Anfang eine
    // Differenz und das Ende die Zahl der Elemente dieses Ticks.
    let _ = writeln!(s, "static int takt_int_first(struct {x}_arena *a, int k, long long cur) {{");
    let _ = writeln!(s, "    if (a->int_n[k] == 0) return 0;");
    let _ = writeln!(s, "    long long d = cur - takt_int_seq_at(a, k, 0);");
    let _ = writeln!(s, "    if (d <= 0) return 0;");
    let _ = writeln!(s, "    return d < a->int_n[k] ? (int)d : a->int_n[k];");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static int takt_int_count(struct {x}_arena *a, int k, long long cur) {{");
    let _ = writeln!(s, "    int n = a->int_n[k] - a->int_new[k] - takt_int_first(a, k, cur);");
    let _ = writeln!(s, "    return n > 0 ? n : 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static long long takt_int_bind(struct {x}_arena *a, int k, long long cur, int i, void *data, long long *t) {{"
    );
    let _ = writeln!(s, "    int first = takt_int_first(a, k, cur);");
    let _ = writeln!(s, "    if (i < 0 || i >= a->int_n[k] - a->int_new[k] - first) return 0;");
    let _ = writeln!(s, "    const struct {x}_idesc *e = takt_int_desc(a, k, first + i);");
    let _ = writeln!(s, "    unsigned char *p = (unsigned char *)data;");
    let _ = writeln!(s, "    long long when = (long long)(((unsigned long long)e->t_hi << 32) | e->t_lo);");
    let _ = writeln!(s, "    int len = e->len;");
    let _ = writeln!(s, "    memcpy(t, &when, sizeof when);");
    let _ = writeln!(s, "    memcpy(p, &len, sizeof len);");
    let _ = writeln!(s, "    takt_int_read(a, k, e->off, p + 4, len);");
    let _ = writeln!(s, "    return takt_int_seq_at(a, k, first + i);");
    let _ = writeln!(s, "}}");
    // 8.6: Zwei Schranken, Elemente und Bytes — wie `Buffer::push`. Ob
    // ein volles `send` faultet oder verwirft, entscheidet der erzeugte
    // Code an der Politik des Stroms.
    let _ =
        writeln!(s, "static _Bool takt_int_push(struct {x}_arena *a, int k, const char *b, int n, long long at) {{");
    let _ = writeln!(s, "    if (a->int_n[k] >= g_int_cap[k] || a->int_bused[k] + n > g_int_capb[k]) return 0;");
    let _ = writeln!(s, "    struct {x}_idesc *e = takt_int_desc(a, k, a->int_n[k]);");
    let _ = writeln!(s, "    e->t_lo = (unsigned)(unsigned long long)at;");
    let _ = writeln!(s, "    e->t_hi = (unsigned)((unsigned long long)at >> 32);");
    let _ = writeln!(s, "    int off = a->int_bhead[k] + a->int_bused[k];");
    let _ = writeln!(s, "    if (off >= g_int_capb[k]) off -= g_int_capb[k];");
    let _ = writeln!(s, "    e->off = off;");
    let _ = writeln!(s, "    e->len = n;");
    let _ = writeln!(s, "    takt_int_write(a, k, e->off, (const unsigned char *)b, n);");
    let _ = writeln!(s, "    a->int_bused[k] += n;");
    let _ = writeln!(s, "    a->int_n[k]++;");
    let _ = writeln!(s, "    a->int_new[k]++;");
    let _ = writeln!(s, "    a->int_seq[k]++;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static _Bool takt_int_send(struct {x}_arena *a, int k, const char *b, int n) {{");
    let _ = writeln!(s, "    return takt_int_push(a, k, b, n, (long long)a->tick * {}LL);", p.config.tick);
    let _ = writeln!(s, "}}");
    // 8.6, `Buffer::push`: Ein Element vom Rand ist sofort sichtbar. Passt
    // es nicht, verdraengt es mit `drop_oldest` die aeltesten (1), sonst
    // ist es ein Ueberlauf (2) — ebenso, wenn es allein die Byteschranke
    // sprengt.
    let _ = writeln!(
        s,
        "static int takt_int_deliver(struct {x}_arena *a, int k, const unsigned char *b, int n, long long at, _Bool drop_oldest) {{"
    );
    let _ = writeln!(s, "    int dropped = 0;");
    let _ = writeln!(
        s,
        "    while (drop_oldest && a->int_n[k] > 0 && (a->int_n[k] >= g_int_cap[k] || a->int_bused[k] + n > g_int_capb[k])) {{"
    );
    let _ = writeln!(s, "        int len = takt_int_desc(a, k, 0)->len;");
    let _ = writeln!(s, "        a->int_bhead[k] += len;");
    let _ = writeln!(s, "        if (a->int_bhead[k] >= g_int_capb[k]) a->int_bhead[k] -= g_int_capb[k];");
    let _ = writeln!(s, "        a->int_bused[k] -= len;");
    let _ = writeln!(s, "        if (++a->int_head[k] == g_int_cap[k]) a->int_head[k] = 0;");
    let _ = writeln!(s, "        a->int_n[k]--;");
    let _ = writeln!(s, "        a->int_dropped[k]++;");
    let _ = writeln!(s, "        dropped = 1;");
    let _ = writeln!(s, "    }}");
    let _ =
        writeln!(s, "    if (!takt_int_push(a, k, (const char *)b, n, at)) {{ a->int_overflowed[k]++; return 2; }}");
    let _ = writeln!(s, "    a->int_new[k]--;");
    let _ = writeln!(s, "    return dropped;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_int_examined(struct {x}_arena *a, int k, int m, long long seq) {{");
    let _ = writeln!(s, "    if (m < 0 || m >= {machines}) return;");
    let _ = writeln!(s, "    if (seq + 1 > a->int_ex[k][m]) a->int_ex[k][m] = seq + 1;");
    let _ = writeln!(s, "}}");
    // 9.6, `advance_cursors()`: frei wird, was unter dem kleinsten Cursor
    // aller Leser liegt, von vorn; ein Strom ohne Leser behaelt nichts.
    let _ = writeln!(s, "static void takt_int_commit(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    for (int k = 0; k < TAKT_INT_STREAMS; k++) {{");
    let _ = writeln!(s, "        a->int_new[k] = 0;");
    let _ = writeln!(s, "        if (g_int_reader_n[k] == 0) {{ a->int_n[k] = 0; a->int_bused[k] = 0; continue; }}");
    let _ = writeln!(s, "        long long min = a->int_ex[k][g_int_readers[k][0]];");
    let _ = writeln!(s, "        for (int r = 1; r < g_int_reader_n[k]; r++) {{");
    let _ = writeln!(s, "            long long c = a->int_ex[k][g_int_readers[k][r]];");
    let _ = writeln!(s, "            if (c < min) min = c;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        while (a->int_n[k] > 0 && takt_int_seq_at(a, k, 0) < min) {{");
    let _ = writeln!(s, "            int len = takt_int_desc(a, k, 0)->len;");
    let _ = writeln!(s, "            a->int_bhead[k] += len;");
    let _ = writeln!(s, "            if (a->int_bhead[k] >= g_int_capb[k]) a->int_bhead[k] -= g_int_capb[k];");
    let _ = writeln!(s, "            a->int_bused[k] -= len;");
    let _ = writeln!(s, "            if (++a->int_head[k] == g_int_cap[k]) a->int_head[k] = 0;");
    let _ = writeln!(s, "            a->int_n[k]--;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
}

/// `takt_stream_send` (8.8): der Sendepuffer eines Ausgabestroms.
///
/// **Der Treiber leert ihn mit `max_rate`.** 8.8: „die Simulation leert
/// exakt `max_rate * T0` Bytes pro Tick". Ein `send` legt also in eine
/// Warteschlange, und je Tick geht daraus ein Stueck hinaus — bei
/// 1000 Hz und 10 ms Tick sind das zehn Byte. Der Interpreter rechnet
/// ebenso (`TxBuffer::per_tick`), und was er abholt, schreibt er als
/// `out <stream> [bytes]` in den Trace.
///
/// Ohne diese Rate stuende im nativen Trace der ganze Text in einem
/// Tick, im interpretierten haeppchenweise — und der Vergleich saehe
/// einen Unterschied, den es in der Sache nicht gibt.
fn emit_send(t: &mut Text, p: &Program, rings: bool, trace: Trace, x: &Prefix) {
    let streams: Vec<(usize, &Channel)> = p
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
        .collect();
    // Je Strom so viel Platz wie seine Kapazitaet (8.8, Default 256), und
    // fuer das Abgeholte so viel, wie ein Tick hoechstens abholt; ohne Rate
    // der ganze Puffer.
    let cap = |c: &Channel| c.attrs.capacity.unwrap_or(256);
    let per_tick = |c: &Channel| match rate_hz(c) {
        Some(hz) => u32::try_from((hz.saturating_mul(p.config.tick as u64) / 1_000_000_000).max(1))
            .map_or(cap(c), |n| n.min(cap(c))),
        None => cap(c),
    };
    if rings {
        shapes(&mut t.code, p);
    }
    if streams.is_empty() {
        let _ =
            writeln!(t.code, "static void takt_tx_commit(struct {x}_arena *a, long long t) {{ (void)a; (void)t; }}");
        if rings {
            stream_send(&mut t.code, true, false, x);
        }
        return;
    }
    let n = streams.len();
    let tx_max = streams.iter().map(|(_, c)| cap(c)).max().unwrap_or(1);
    let sent_max = streams.iter().map(|(_, c)| cap(c).min(per_tick(c))).max().unwrap_or(1);
    let _ = writeln!(t.code, "#define TAKT_TX_MAX {tx_max}");
    let _ = writeln!(t.fields, "    _Alignas(8) unsigned char tx[{n}][{tx_max}];");
    let _ = writeln!(t.fields, "    int tx_n[{n}];");
    // 8.8, FB-132: was der letzte Commit abgeholt hat, liest `o.sent` im
    // naechsten Tick — der Unit-Delay eines Outputs.
    let _ = writeln!(t.fields, "    _Alignas(8) unsigned char tx_sent[{n}][{sent_max}];");
    let _ = writeln!(t.fields, "    int tx_sent_n[{n}];");
    let s = &mut t.code;
    let _ = writeln!(s, "static int takt_tx_slot(int s) {{");
    let _ = writeln!(s, "    switch (s) {{");
    for (slot, (i, c)) in streams.iter().enumerate() {
        let _ = writeln!(s, "    case {i}: return {slot}; /* {} */", c.name);
    }
    let _ = writeln!(s, "    default: return -1;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    // Die Kapazitaet je Strom (8.8, Default 256): Was nicht hineinpasst,
    // ist ein `StreamOverflow`.
    let _ = writeln!(s, "static int takt_tx_cap(int s) {{");
    let _ = writeln!(s, "    switch (s) {{");
    for (i, c) in &streams {
        let _ = writeln!(s, "    case {i}: return {};", c.attrs.capacity.unwrap_or(256));
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    if !coupled(p).is_empty() {
        couple(s, x);
    }
    stream_send(s, rings, true, x);
    // Der Commit: Der Treiber holt `per_tick` Bytes ab und meldet sie als
    // `out <stream> [0x.., ..]` — dieselbe Schreibweise wie im
    // Interpreter (`value_text` fuer `Value::Bytes`).
    let _ = writeln!(s, "static void takt_tx_commit(struct {x}_arena *a, long long t) {{");
    for (slot, (i, c)) in streams.iter().enumerate() {
        let per_tick = per_tick(c);
        let _ = writeln!(s, "    if (a->tx_n[{slot}] > 0) {{");
        let _ = writeln!(s, "        int n = a->tx_n[{slot}] < {per_tick} ? a->tx_n[{slot}] : {per_tick};");
        match trace {
            Trace::Stdio => {
                let _ = writeln!(s, "        printf(\"t=%lld out {} [\", t);", c.name);
                let _ = writeln!(s, "        for (int i = 0; i < n; i++)");
                let _ = writeln!(s, "            printf(i ? \", 0x%02x\" : \"0x%02x\", a->tx[{slot}][i]);");
                let _ = writeln!(s, "        printf(\"]\\n\");");
            }
            Trace::Board => {
                let _ = writeln!(s, "        takt_board_trace(\"t=\");");
                let _ = writeln!(s, "        takt_board_trace_i64(t);");
                let _ = writeln!(s, "        takt_board_trace(\"out {} [\");", c.name);
                let _ = writeln!(s, "        for (int i = 0; i < n; i++) {{");
                let _ = writeln!(s, "            if (i) takt_board_trace(\", \");");
                let _ = writeln!(s, "            takt_board_trace_hex8(a->tx[{slot}][i]);");
                let _ = writeln!(s, "        }}");
                let _ = writeln!(s, "        takt_board_trace(\"]\\n\");");
            }
        }
        let _ = writeln!(s, "        memcpy(a->tx_sent[{slot}], a->tx[{slot}], (size_t)n);");
        let _ = writeln!(s, "        a->tx_sent_n[{slot}] = n;");
        // 8.3: ein `sim`-Ausgabestrom speist den `hw`-Eingang derselben
        // Adresse — je Byte ein Element eines `stream<u8>`, Text als ein
        // Element, sonst so viele Elemente fester Byteform, wie hineinpassen.
        if let Some((in_id, _, elem)) = coupled(p).into_iter().find(|(_, o, _)| *o == *i) {
            let width = match p.types.list.get(elem.index()) {
                Some(Type::Int { width, .. }) if width.bits() == 8 => 1,
                Some(Type::Line { .. } | Type::Str { .. } | Type::Bytes { .. }) => 0,
                _ => takt_mir::bytes::max_size(p, elem).unwrap_or(0),
            };
            let shape = if element_shape(p, elem).is_some() {
                format!("g_shape_{in_id}, (unsigned)sizeof g_shape_{in_id}")
            } else {
                "0, 0".to_string()
            };
            let _ =
                writeln!(s, "        takt_couple(a, takt_int_slot({in_id}), a->tx_sent[{slot}], n, {width}, {shape});");
        }
        let _ = writeln!(s, "        memmove(a->tx[{slot}], a->tx[{slot}] + n, (size_t)(a->tx_n[{slot}] - n));");
        let _ = writeln!(s, "        a->tx_n[{slot}] -= n;");
        let _ = writeln!(s, "    }} else {{");
        let _ = writeln!(s, "        a->tx_sent_n[{slot}] = 0;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}\n");
    // `o.sent` (8.8): `{ i32 len, [CAP x i8] }` an die uebergebene Stelle.
    let _ = writeln!(s, "int {x}_stream_sent(struct {x}_arena *a, int s, void *out) {{");
    let _ = writeln!(s, "    int k = takt_tx_slot(s);");
    let _ = writeln!(s, "    unsigned char *o = (unsigned char *)out;");
    let _ = writeln!(s, "    int n = k < 0 ? 0 : a->tx_sent_n[k];");
    let _ = writeln!(
        s,
        "    o[0] = (unsigned char)n; o[1] = (unsigned char)(n >> 8); o[2] = (unsigned char)(n >> 16); o[3] = (unsigned char)(n >> 24);"
    );
    let _ = writeln!(s, "    if (n > 0) memcpy(o + 4, a->tx_sent[k], (size_t)n);");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");
}

/// `takt_couple`: ein Element des `sim`-Ausgabestroms in den gekoppelten
/// Eingabestrom (8.3). 12.6 Zeile 5: Ein Slot, dessen `decode` misslingt,
/// wird verworfen, wie `elements_of` im Interpreter.
fn couple(s: &mut String, x: &Prefix) {
    let _ = writeln!(
        s,
        "static void takt_couple(struct {x}_arena *a, int k, const unsigned char *b, int n, int w, const unsigned char *shape, unsigned shape_len) {{"
    );
    let _ = writeln!(s, "    if (k < 0) return;");
    let _ = writeln!(
        s,
        "    if (w == 0) {{ if (!takt_int_send(a, k, (const char *)b, n)) a->int_overflowed[k]++; return; }}"
    );
    let _ = writeln!(s, "    for (int off = 0; off + w <= n; off += w) {{");
    let _ = writeln!(
        s,
        "        if (shape && !takt_edge_decodes(shape, shape_len, b + off, (unsigned)w, 0)) a->int_malformed[k]++;"
    );
    let _ = writeln!(s, "        else if (!takt_int_send(a, k, (const char *)b + off, w)) a->int_overflowed[k]++;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// `takt_stream_send`: in den Ring eines internen Stroms (`rings`) oder in
/// den Sendepuffer eines Ausgabestroms (`sends`).
fn stream_send(s: &mut String, rings: bool, sends: bool, x: &Prefix) {
    // Ein abgewiesenes `send` auf einen internen Strom zaehlt, ausser mit
    // `overflow = drop` (8.6, `System::send`).
    let _ = writeln!(s, "_Bool {x}_stream_send(struct {x}_arena *a, int s, const char *b, int n) {{");
    if rings {
        let _ = writeln!(s, "    int r = takt_int_slot(s);");
        let _ = writeln!(s, "    if (r >= 0) {{");
        let _ = writeln!(s, "        _Bool ok = takt_int_send(a, r, b, n);");
        let _ = writeln!(s, "        if (!ok && g_int_counts_overflow[r]) a->int_overflowed[r]++;");
        let _ = writeln!(s, "        return ok;");
        let _ = writeln!(s, "    }}");
    }
    if !sends {
        let _ = writeln!(s, "    (void)b; (void)n;");
        let _ = writeln!(s, "    return 0;");
        let _ = writeln!(s, "}}");
        return;
    }
    let _ = writeln!(s, "    int k = takt_tx_slot(s);");
    let _ = writeln!(s, "    if (k < 0) return 0;");
    // 8.8: `len > tx.free` ist ein `StreamOverflow`.
    let _ = writeln!(s, "    if (a->tx_n[k] + n > takt_tx_cap(s)) return 0;");
    let _ = writeln!(s, "    memcpy(a->tx[k] + a->tx_n[k], b, (size_t)n);");
    let _ = writeln!(s, "    a->tx_n[k] += n;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
}

/// `max_rate` eines Stroms in Hz; nur ein Literal, wie im Sema (8.6).
///
/// Dieselbe Rechnung wie `Image::new` im Interpreter — eine zweite
/// Auslegung derselben Angabe waere ein Unterschied, den 9.4.4 nicht
/// zulaesst.
fn rate_hz(c: &Channel) -> Option<u64> {
    match &c.attrs.max_rate.as_ref()?.kind {
        takt_mir::expr::ExprKind::Int(n) => u64::try_from(*n).ok(),
        takt_mir::expr::ExprKind::Float(f) if *f >= 0.0 => Some(*f as u64),
        _ => None,
    }
}
