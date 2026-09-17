//! Die Stroeme des Testrahmens (8.6, 9.6).
//!
//! **Was fehlte.** `takt_stream_count` lieferte null, `takt_stream_at`
//! schrieb nichts. Der Handler-Dispatch im erzeugten Code war damit
//! gebaut, aber nie mit Daten ausgefuehrt — er lief ueber ein Fenster,
//! das immer leer war (FB-115).
//!
//! **Warum eine Tabelle und kein Ring.** Der Ring der Runtime steht in
//! `takt-rt-core::stream` und ist die eine Implementierung fuer echte
//! Ziele. Ihn hier zu nutzen hiesse, ihn ueber `extern "C"` mit einem
//! rohen `void *out` zu rufen — das erste `unsafe` in einem Workspace,
//! der es verbietet (13.4 nennt das Verbot einen Zertifizierungsgrund).
//! Ihn in C nachzubauen waere eine zweite Implementierung derselben
//! Regeln, also genau die Gefahr, gegen die `takt-match` gebaut wurde.
//!
//! Der dritte Weg nutzt aus, was den Testrahmen vom echten Ziel
//! unterscheidet: **Der Stimulus steht vor dem Lauf fest.** Ein Treiber
//! liefert, wann er will; der Rahmen weiss es vorher. Damit braucht die
//! C-Seite keinen Ring, sondern eine Tabelle und je Konsument einen
//! Cursor — die Verdraengung entfaellt, weil nichts nachrueckt.
//!
//! **Die Schranken bleiben, so weit der Rahmen sie sieht.** `capacity`
//! und `capacity_bytes` sind in 8.6 nicht nur Speicher, sondern
//! beobachtbar: Was nicht hineinpasst, erhoeht `s.overflowed` und
//! faultet die Konsumenten. Der Rahmen laesst aus, was die Schranke
//! reisst — gerechnet je Tick, unter der Annahme eines Konsumenten, der
//! sein Fenster leert. Das ist der Fall, den der erzeugte Code
//! herstellt. Ein Stimulus, der einen Puffer ueber mehrere Ticks
//! *fuellen* soll, ohne dass jemand liest, braeuchte den Cursor des
//! Konsumenten — und den kennt erst der Lauf. `limits.rs` fuehrt das
//! als Grenze der Abnahme.

use std::fmt::Write as _;

use takt_mir::TypeId;
use takt_mir::program::{Channel, Direction, Program};
use takt_mir::types::Type;

use crate::stimulus::Stimulus;

/// Ein Element, wie es der Rahmen ablegt.
struct Element {
    /// Tick, an dem der Treiber liefert.
    tick: u64,
    /// Der Inhalt als Bytes.
    bytes: Vec<u8>,
}

/// Ein Strom mit seinen Elementen und Schranken.
struct Stream {
    /// Nummer, wie der erzeugte Code sie uebergibt (`stream_id`).
    id: i64,
    /// Name des Kanals, fuer den Kommentar im C.
    name: String,
    /// Kapazitaet der Bytes eines Elements: `line<N>` → N, ein Record →
    /// seine kanonische Byteform.
    cap: u32,
    /// Die Elemente in Reihenfolge, nach der Schrankenpruefung.
    elements: Vec<Element>,
}

/// Sammelt die Stroeme mit ihren Elementen (8.6).
///
/// Ein Element, das die Schranken reisst, steht nicht in der Liste: Der
/// Interpreter verwirft es ebenso (`Buffer::push`), und der Vergleich
/// soll dieselbe Folge sehen.
///
/// **Was der Rahmen dabei annimmt.** Er rechnet mit einem Konsumenten,
/// der in jedem Tick alles untersucht — dann steht im Puffer nie mehr
/// als das, was ein Tick liefert, und die Schranke greift genau dort.
/// Das ist der Fall, den der erzeugte Code herstellt (der Dispatch
/// laeuft ueber das ganze Fenster), und `grenzen()` in `limits.rs` nennt
/// den Rest.
fn collect_streams(p: &Program, stimulus: &[Stimulus]) -> Vec<Stream> {
    let mut streams: Vec<Stream> = Vec::new();
    for (i, c) in p.channels.iter().enumerate() {
        if c.dir != Direction::Input {
            continue;
        }
        let Some((elem, cap)) = element_cap(c, p) else { continue };
        // Die Schranken wie im Interpreter (`Image::new`): ohne Angabe
        // 16 Elemente, und die Bytegrenze das 256-fache davon.
        let bound = c.attrs.capacity.unwrap_or(16);
        let bound_bytes = c.attrs.capacity_bytes.unwrap_or(bound.saturating_mul(256));
        let mut elements: Vec<Element> = Vec::new();
        let mut per_tick: (u64, u32, u32) = (u64::MAX, 0, 0);
        for s in stimulus {
            let Stimulus::Element { tick, channel, text } = s else { continue };
            if *channel != c.name {
                continue;
            }
            // 3.9: Der Rand begrenzt die Laenge; was darueber steht, ist
            // abgeschnitten und nicht verworfen. Ein Record steht im
            // Stimulus wie im Trace und geht als kanonische Byteform in
            // den Ring (plan/m6.md 2.2).
            let mut bytes = if textual(p, elem) {
                text.as_bytes().to_vec()
            } else {
                let value = takt_interp::trace::parse_value(text, elem, p);
                match value.map(|v| takt_interp::bytes::encode(p, &v, elem)) {
                    Ok(Ok(b)) => b,
                    _ => continue,
                }
            };
            bytes.truncate(cap as usize);
            if per_tick.0 != *tick {
                per_tick = (*tick, 0, 0);
            }
            let n = bytes.len() as u32;
            // 8.6: Zwei Schranken, Elemente und Byte. Was nicht
            // hineinpasst, ist ein Ueberlauf und kein Element.
            if per_tick.1 + 1 > bound || per_tick.2 + n > bound_bytes {
                continue;
            }
            per_tick = (*tick, per_tick.1 + 1, per_tick.2 + n);
            elements.push(Element { tick: *tick, bytes });
        }
        if elements.is_empty() {
            continue;
        }
        streams.push(Stream { id: i64::from(i as u32), name: c.name.clone(), cap, elements });
    }
    streams
}

/// Elementtyp und Kapazitaet seiner Bytes, falls der Kanal ein Strom ist.
fn element_cap(c: &Channel, p: &Program) -> Option<(TypeId, u32)> {
    let Some(Type::Stream(elem)) = p.types.list.get(c.ty.index()) else { return None };
    Some((*elem, payload_cap(p, *elem)))
}

/// Text kommt als Bytes, wie er im Stimulus steht.
fn textual(p: &Program, elem: TypeId) -> bool {
    matches!(p.types.list.get(elem.index()), Some(Type::Line { .. } | Type::Str { .. } | Type::Bytes { .. }))
}

/// Schreibt die drei Aufrufe aus `takt-llvm/src/stream.rs` als C.
///
/// Ohne Elemente bleibt es beim leeren Fenster — der Fall, den jedes
/// Programm aushalten muss, und der bis hierher der einzige war.
pub fn emit(s: &mut String, p: &Program, stimulus: &[Stimulus]) {
    let streams = collect_streams(p, stimulus);
    emit_internal(s, p);
    let _ = writeln!(s, "/* Stroeme (8.6, 9.6); der Stimulus steht vor dem Lauf fest. */");
    if streams.is_empty() {
        let _ = writeln!(s, "int takt_stream_count(int s, long long cur) {{");
        let _ = writeln!(s, "    int k = takt_int_slot(s);");
        let _ = writeln!(s, "    return k >= 0 ? takt_int_count(k, cur) : 0;");
        let _ = writeln!(s, "}}");
        let _ = writeln!(s, "long long takt_stream_at(int s, long long cur, int i, void *out) {{");
        let _ = writeln!(s, "    int k = takt_int_slot(s);");
        let _ = writeln!(s, "    return k >= 0 ? takt_int_at(k, cur, i, out) : 0;");
        let _ = writeln!(s, "}}");
        let _ = writeln!(s, "void takt_stream_examined(int s, int m, long long seq) {{");
        let _ = writeln!(s, "    int k = takt_int_slot(s);");
        let _ = writeln!(s, "    if (k >= 0) takt_int_examined(k, m, seq);");
        let _ = writeln!(s, "}}\n");
        emit_send(s, p);
        return;
    }

    // Die Elemente aller Stroeme in einer Tabelle; `seq` ist der Index
    // je Strom, wie die laufende Nummer in 9.6.
    let _ =
        writeln!(s, "struct takt_elem {{ int stream; long long tick; long long seq; int len; const char *bytes; }};");
    let _ = writeln!(s, "static const struct takt_elem g_elems[] = {{");
    for st in &streams {
        for (seq, e) in st.elements.iter().enumerate() {
            let text: String = e.bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
            let _ = writeln!(
                s,
                "    {{ {}, {}, {}, {}, \"{}\" }}, /* {} */",
                st.id,
                e.tick,
                seq,
                e.bytes.len(),
                text,
                st.name
            );
        }
    }
    let _ = writeln!(s, "}};");
    let _ = writeln!(s, "static const int g_elem_count = (int)(sizeof g_elems / sizeof g_elems[0]);\n");

    // Die Kapazitaet je Strom: so viele Bytes schreibt `takt_stream_at`
    // hinter `t` und die Laenge.
    let _ = writeln!(s, "static int takt_stream_cap(int s) {{");
    let _ = writeln!(s, "    switch (s) {{");
    for st in &streams {
        let _ = writeln!(s, "    case {}: return {}; /* {} */", st.id, st.cap, st.name);
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");

    // 9.6: Sichtbar ist, was geliefert *und* noch nicht untersucht ist.
    // `g_tick` ist der laufende Tick des Rahmens; ein Element wird im
    // Tick seiner Lieferung sichtbar (8.6: der Rand liefert sofort).
    let _ = writeln!(s, "int takt_stream_count(int s, long long cur) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    if (k >= 0) return takt_int_count(k, cur);");
    let _ = writeln!(s, "    int n = 0;");
    let _ = writeln!(s, "    for (int i = 0; i < g_elem_count; i++)");
    let _ = writeln!(s, "        if (g_elems[i].stream == s && g_elems[i].tick <= g_tick && g_elems[i].seq >= cur)");
    let _ = writeln!(s, "            n++;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");

    // Das `i`-te Element des Fensters an den uebergebenen Platz, im
    // Aufbau von `takt_llvm::stream::Streams::AT`: `t` in Nanosekunden
    // (i64), die Laenge (i32 bei 8), die Bytes ab 12.
    let _ = writeln!(s, "long long takt_stream_at(int s, long long cur, int i, void *out) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    if (k >= 0) return takt_int_at(k, cur, i, out);");
    let _ = writeln!(s, "    int seen = 0;");
    let _ = writeln!(s, "    for (int j = 0; j < g_elem_count; j++) {{");
    let _ = writeln!(s, "        const struct takt_elem *e = &g_elems[j];");
    let _ = writeln!(s, "        if (e->stream != s || e->tick > g_tick || e->seq < cur) continue;");
    let _ = writeln!(s, "        if (seen++ != i) continue;");
    let _ = writeln!(s, "        int cap = takt_stream_cap(s);");
    let _ = writeln!(s, "        unsigned char *p = (unsigned char *)out;");
    let _ = writeln!(s, "        long long t = e->tick * {}LL;", p.config.tick);
    let _ = writeln!(s, "        memset(p, 0, (size_t)cap + 12);");
    let _ = writeln!(s, "        memcpy(p, &t, sizeof t);");
    let _ = writeln!(s, "        memcpy(p + 8, &e->len, sizeof e->len);");
    let _ = writeln!(s, "        memcpy(p + 12, e->bytes, (size_t)e->len);");
    let _ = writeln!(s, "        return e->seq;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    (void)out;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}\n");

    // 9.6: `cur[s, m] = examined + 1`. Der Cursor gehoert der Maschine,
    // und der erzeugte Code fuehrt ihn in seinem Zustand; fuer einen
    // Eingabestrom muss der Rahmen darum nichts halten. Ein interner
    // Strom gibt frei, was jeder Leser untersucht hat.
    let _ = writeln!(s, "void takt_stream_examined(int s, int m, long long seq) {{");
    let _ = writeln!(s, "    int k = takt_int_slot(s);");
    let _ = writeln!(s, "    if (k >= 0) takt_int_examined(k, m, seq);");
    let _ = writeln!(s, "}}\n");
    emit_send(s, p);
}

/// Die Kapazitaet der Bytes eines Elements (8.6, 3.9): `N` bei Text,
/// sonst die kanonische Byteform — dieselbe Rechnung wie
/// `takt_llvm::stream::scratch`.
fn payload_cap(p: &Program, elem: TypeId) -> u32 {
    match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => *cap,
        _ => takt_mir::bytes::max_size(p, elem).unwrap_or(1),
    }
}

/// Interne Stroeme (8.6, 9.6): Elemente entstehen im Lauf durch `send`,
/// werden im naechsten Tick sichtbar (Unit-Delay) und verschwinden, sobald
/// jeder Leser sie untersucht hat — den Stand je Leser meldet
/// `takt_stream_examined(s, m, seq)`, die Leser eines Stroms kennt die
/// Sema (`Stream::readers`). Ein voller Ring weist das Element ab; der
/// Sender faultet, wo der Interpreter die Leser faultet (8.6) — beides
/// ein Ueberlauf, den `limits.rs` als ungeprueft fuehrt.
/// Ein Ring im Lauf: ein interner Strom oder ein `hw`-Eingabestrom, den
/// ein `sim`-Ausgabestrom derselben Adresse speist (8.3).
struct Dynamic {
    /// Nummer, wie der erzeugte Code sie uebergibt.
    id: i64,
    name: String,
    elem: TypeId,
    capacity: u32,
    readers: Vec<u32>,
}

/// Die Ringe des Laufs: erst die internen Stroeme, dann die gekoppelten
/// Eingaenge.
fn dynamic_streams(p: &Program) -> Vec<Dynamic> {
    let mut out: Vec<Dynamic> = p
        .streams
        .iter()
        .enumerate()
        .map(|(i, st)| Dynamic {
            id: -1 - i as i64,
            name: st.name.clone(),
            elem: st.elem,
            capacity: st.capacity,
            readers: st.readers.iter().map(|m| m.0).collect(),
        })
        .collect();
    for (in_id, _, elem) in coupled(p) {
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
        out.push(Dynamic {
            id: in_id as i64,
            name: c.name.clone(),
            elem,
            capacity: c.attrs.capacity.unwrap_or(16),
            readers,
        });
    }
    out
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

fn emit_internal(s: &mut String, p: &Program) {
    let dyns = dynamic_streams(p);
    let n = dyns.len();
    let rows = n.max(1);
    let cap = dyns.iter().map(|d| d.capacity).max().unwrap_or(1).max(1);
    let bytes = dyns.iter().map(|d| payload_cap(p, d.elem)).max().unwrap_or(1).max(1);
    let machines = p.machines.len().max(1);
    let readers = dyns.iter().map(|d| d.readers.len()).max().unwrap_or(1).max(1);
    let _ = writeln!(s, "/* Ringe im Lauf (8.6, 8.3): Unit-Delay, Cursor je Leser, Freigabe unter dem kleinsten. */");
    let _ = writeln!(s, "#define TAKT_INT_STREAMS {n}");
    let _ =
        writeln!(s, "struct takt_ielem {{ long long tick; long long seq; int len; unsigned char bytes[{bytes}]; }};");
    let _ = writeln!(s, "static struct takt_ielem g_int[{rows}][{cap}];");
    let _ = writeln!(s, "static int g_int_n[{rows}];");
    let _ = writeln!(s, "static long long g_int_seq[{rows}];");
    let _ = writeln!(s, "static long long g_int_ex[{rows}][{machines}]; /* examined + 1 je Leser */");
    let _ = writeln!(s, "static const int g_int_readers[{rows}][{readers}] = {{");
    for d in &dyns {
        let list: Vec<String> = d.readers.iter().map(|m| m.to_string()).collect();
        let _ = writeln!(
            s,
            "    {{ {} }}, /* {} */",
            if list.is_empty() { "0".to_string() } else { list.join(", ") },
            d.name
        );
    }
    if n == 0 {
        let _ = writeln!(s, "    {{ 0 }}");
    }
    let _ = writeln!(s, "}};");
    let counts: Vec<String> = dyns.iter().map(|d| d.readers.len().to_string()).collect();
    let _ = writeln!(
        s,
        "static const int g_int_reader_n[{rows}] = {{ {} }};",
        if counts.is_empty() { "0".to_string() } else { counts.join(", ") }
    );
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
    let _ = writeln!(s, "static int takt_int_cap(int k) {{");
    let _ = writeln!(s, "    switch (k) {{");
    for (i, d) in dyns.iter().enumerate() {
        let _ = writeln!(s, "    case {i}: return {}; /* {} */", d.capacity, d.name);
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static int takt_int_bytes(int k) {{");
    let _ = writeln!(s, "    switch (k) {{");
    for (i, d) in dyns.iter().enumerate() {
        let _ = writeln!(s, "    case {i}: return {};", payload_cap(p, d.elem));
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    // 9.6: sichtbar ist, was in einem frueheren Tick gesendet wurde.
    let _ = writeln!(s, "static int takt_int_count(int k, long long cur) {{");
    let _ = writeln!(s, "    int n = 0;");
    let _ = writeln!(s, "    for (int i = 0; i < g_int_n[k]; i++)");
    let _ = writeln!(s, "        if (g_int[k][i].tick < g_tick && g_int[k][i].seq >= cur) n++;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static long long takt_int_at(int k, long long cur, int i, void *out) {{");
    let _ = writeln!(s, "    int seen = 0;");
    let _ = writeln!(s, "    for (int j = 0; j < g_int_n[k]; j++) {{");
    let _ = writeln!(s, "        const struct takt_ielem *e = &g_int[k][j];");
    let _ = writeln!(s, "        if (e->tick >= g_tick || e->seq < cur) continue;");
    let _ = writeln!(s, "        if (seen++ != i) continue;");
    let _ = writeln!(s, "        unsigned char *p = (unsigned char *)out;");
    let _ = writeln!(s, "        long long t = e->tick * {}LL;", p.config.tick);
    let _ = writeln!(s, "        memset(p, 0, (size_t)takt_int_bytes(k) + 12);");
    let _ = writeln!(s, "        memcpy(p, &t, sizeof t);");
    let _ = writeln!(s, "        memcpy(p + 8, &e->len, sizeof e->len);");
    let _ = writeln!(s, "        memcpy(p + 12, e->bytes, (size_t)e->len);");
    let _ = writeln!(s, "        return e->seq;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static _Bool takt_int_send(int k, const char *b, int n) {{");
    let _ = writeln!(s, "    if (n > takt_int_bytes(k) || g_int_n[k] >= takt_int_cap(k)) return 0;");
    let _ = writeln!(s, "    struct takt_ielem *e = &g_int[k][g_int_n[k]++];");
    let _ = writeln!(s, "    e->tick = g_tick;");
    let _ = writeln!(s, "    e->seq = g_int_seq[k]++;");
    let _ = writeln!(s, "    e->len = n;");
    let _ = writeln!(s, "    memcpy(e->bytes, b, (size_t)n);");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static void takt_int_examined(int k, int m, long long seq) {{");
    let _ = writeln!(s, "    if (m < 0 || m >= {machines}) return;");
    let _ = writeln!(s, "    if (seq + 1 > g_int_ex[k][m]) g_int_ex[k][m] = seq + 1;");
    let _ = writeln!(s, "}}");
    // 9.6, `advance_cursors()`: frei wird, was unter dem kleinsten Cursor
    // aller Leser liegt; ein Strom ohne Leser behaelt nichts.
    let _ = writeln!(s, "static void takt_int_commit(void) {{");
    let _ = writeln!(s, "    for (int k = 0; k < TAKT_INT_STREAMS; k++) {{");
    let _ = writeln!(s, "        if (g_int_reader_n[k] == 0) {{ g_int_n[k] = 0; continue; }}");
    let _ = writeln!(s, "        long long min = g_int_ex[k][g_int_readers[k][0]];");
    let _ = writeln!(s, "        for (int r = 1; r < g_int_reader_n[k]; r++) {{");
    let _ = writeln!(s, "            long long c = g_int_ex[k][g_int_readers[k][r]];");
    let _ = writeln!(s, "            if (c < min) min = c;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        int w = 0;");
    let _ = writeln!(s, "        for (int i = 0; i < g_int_n[k]; i++)");
    let _ = writeln!(s, "            if (g_int[k][i].seq >= min) g_int[k][w++] = g_int[k][i];");
    let _ = writeln!(s, "        g_int_n[k] = w;");
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
fn emit_send(s: &mut String, p: &Program) {
    let streams: Vec<(usize, &Channel)> = p
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dir == Direction::Output && matches!(p.types.list.get(c.ty.index()), Some(Type::Stream(_))))
        .collect();
    let _ = writeln!(s, "#define TAKT_TX_MAX 4096");
    let _ = writeln!(s, "static unsigned char g_tx[{}][TAKT_TX_MAX];", streams.len().max(1));
    let _ = writeln!(s, "static int g_tx_n[{}];", streams.len().max(1));
    // 8.8, FB-132: was der letzte Commit abgeholt hat, liest `o.sent` im
    // naechsten Tick — der Unit-Delay eines Outputs.
    let _ = writeln!(s, "static unsigned char g_tx_sent[{}][TAKT_TX_MAX];", streams.len().max(1));
    let _ = writeln!(s, "static int g_tx_sent_n[{}];", streams.len().max(1));
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
    let _ = writeln!(s, "static void takt_couple(int k, const unsigned char *b, int n, int w) {{");
    let _ = writeln!(s, "    if (k < 0) return;");
    let _ = writeln!(s, "    if (w == 0) {{ takt_int_send(k, (const char *)b, n); return; }}");
    let _ = writeln!(s, "    for (int off = 0; off + w <= n; off += w) takt_int_send(k, (const char *)b + off, w);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "_Bool takt_stream_send(int s, const char *b, int n) {{");
    let _ = writeln!(s, "    if (takt_int_slot(s) >= 0) return takt_int_send(takt_int_slot(s), b, n);");
    let _ = writeln!(s, "    int k = takt_tx_slot(s);");
    let _ = writeln!(s, "    if (k < 0) return 0;");
    // 8.8: `len > tx.free` ist ein `StreamOverflow`.
    let _ = writeln!(s, "    if (g_tx_n[k] + n > takt_tx_cap(s) || g_tx_n[k] + n > TAKT_TX_MAX) return 0;");
    let _ = writeln!(s, "    memcpy(g_tx[k] + g_tx_n[k], b, (size_t)n);");
    let _ = writeln!(s, "    g_tx_n[k] += n;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
    // Der Commit: Der Treiber holt `per_tick` Bytes ab und meldet sie als
    // `out <stream> [0x.., ..]` — dieselbe Schreibweise wie im
    // Interpreter (`value_text` fuer `Value::Bytes`).
    let _ = writeln!(s, "static void takt_tx_commit(long long t) {{");
    for (slot, (i, c)) in streams.iter().enumerate() {
        let per_tick = match rate_hz(c) {
            Some(hz) => {
                let bytes = hz.saturating_mul(p.config.tick as u64) / 1_000_000_000;
                u32::try_from(bytes.max(1)).unwrap_or(u32::MAX)
            }
            None => u32::MAX,
        };
        let _ = writeln!(s, "    if (g_tx_n[{slot}] > 0) {{");
        let _ = writeln!(s, "        int n = g_tx_n[{slot}] < {per_tick} ? g_tx_n[{slot}] : {per_tick};");
        let _ = writeln!(s, "        printf(\"t=%lld out {} [\", t);", c.name);
        let _ = writeln!(s, "        for (int i = 0; i < n; i++)");
        let _ = writeln!(s, "            printf(i ? \", 0x%02x\" : \"0x%02x\", g_tx[{slot}][i]);");
        let _ = writeln!(s, "        printf(\"]\\n\");");
        let _ = writeln!(s, "        memcpy(g_tx_sent[{slot}], g_tx[{slot}], (size_t)n);");
        let _ = writeln!(s, "        g_tx_sent_n[{slot}] = n;");
        // 8.3: ein `sim`-Ausgabestrom speist den `hw`-Eingang derselben
        // Adresse — je Byte ein Element eines `stream<u8>`, Text als ein
        // Element, sonst so viele Elemente fester Byteform, wie hineinpassen.
        if let Some((in_id, _, elem)) = coupled(p).into_iter().find(|(_, o, _)| *o == *i) {
            let width = match p.types.list.get(elem.index()) {
                Some(Type::Int { width, .. }) if width.bits() == 8 => 1,
                Some(Type::Line { .. } | Type::Str { .. } | Type::Bytes { .. }) => 0,
                _ => takt_mir::bytes::max_size(p, elem).unwrap_or(0),
            };
            let _ = writeln!(s, "        takt_couple(takt_int_slot({in_id}), g_tx_sent[{slot}], n, {width});");
        }
        let _ = writeln!(s, "        memmove(g_tx[{slot}], g_tx[{slot}] + n, (size_t)(g_tx_n[{slot}] - n));");
        let _ = writeln!(s, "        g_tx_n[{slot}] -= n;");
        let _ = writeln!(s, "    }} else {{");
        let _ = writeln!(s, "        g_tx_sent_n[{slot}] = 0;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "}}\n");
    // `o.sent` (8.8): `{ i32 len, [CAP x i8] }` an die uebergebene Stelle.
    let _ = writeln!(s, "int takt_stream_sent(int s, void *out) {{");
    let _ = writeln!(s, "    int k = takt_tx_slot(s);");
    let _ = writeln!(s, "    unsigned char *o = (unsigned char *)out;");
    let _ = writeln!(s, "    int n = k < 0 ? 0 : g_tx_sent_n[k];");
    let _ = writeln!(
        s,
        "    o[0] = (unsigned char)n; o[1] = (unsigned char)(n >> 8); o[2] = (unsigned char)(n >> 16); o[3] = (unsigned char)(n >> 24);"
    );
    let _ = writeln!(s, "    if (n > 0) memcpy(o + 4, g_tx_sent[k], (size_t)n);");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");
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
