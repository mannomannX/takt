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
struct Strom {
    /// Nummer, wie der erzeugte Code sie uebergibt (`stream_id`).
    id: i64,
    /// Name des Kanals, fuer den Kommentar im C.
    name: String,
    /// Kapazitaet des Elementtyps in Byte (`line<N>` → N).
    cap: u32,
    /// Die Elemente in Reihenfolge, nach der Schrankenpruefung.
    elemente: Vec<Element>,
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
fn sammeln(p: &Program, stimulus: &[Stimulus]) -> Vec<Strom> {
    let mut stroeme: Vec<Strom> = Vec::new();
    for (i, c) in p.channels.iter().enumerate() {
        if c.dir != Direction::Input {
            continue;
        }
        let Some(cap) = element_cap(c, p) else { continue };
        // Die Schranken wie im Interpreter (`Image::new`): ohne Angabe
        // 16 Elemente, und die Bytegrenze das 256-fache davon.
        let schranke = c.attrs.capacity.unwrap_or(16);
        let schranke_bytes = c.attrs.capacity_bytes.unwrap_or(schranke.saturating_mul(256));
        let mut elemente: Vec<Element> = Vec::new();
        let mut je_tick: (u64, u32, u32) = (u64::MAX, 0, 0);
        for s in stimulus {
            let Stimulus::Element { tick, channel, text } = s else { continue };
            if *channel != c.name {
                continue;
            }
            // 3.9: Der Rand begrenzt die Laenge; was darueber steht, ist
            // abgeschnitten und nicht verworfen.
            let mut bytes = text.as_bytes().to_vec();
            bytes.truncate(cap as usize);
            if je_tick.0 != *tick {
                je_tick = (*tick, 0, 0);
            }
            let n = bytes.len() as u32;
            // 8.6: Zwei Schranken, Elemente und Byte. Was nicht
            // hineinpasst, ist ein Ueberlauf und kein Element.
            if je_tick.1 + 1 > schranke || je_tick.2 + n > schranke_bytes {
                continue;
            }
            je_tick = (*tick, je_tick.1 + 1, je_tick.2 + n);
            elemente.push(Element { tick: *tick, bytes });
        }
        if elemente.is_empty() {
            continue;
        }
        stroeme.push(Strom { id: i64::from(i as u32), name: c.name.clone(), cap, elemente });
    }
    stroeme
}

/// Die Kapazitaet des Elementtyps in Byte, falls der Kanal ein Strom
/// mit textartigem Element ist (8.6, 3.9).
fn element_cap(c: &Channel, p: &Program) -> Option<u32> {
    let Some(Type::Stream(elem)) = p.types.list.get(c.ty.index()) else { return None };
    match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => Some(*cap),
        _ => None,
    }
}

/// Schreibt die drei Aufrufe aus `takt-llvm/src/stream.rs` als C.
///
/// Ohne Elemente bleibt es beim leeren Fenster — der Fall, den jedes
/// Programm aushalten muss, und der bis hierher der einzige war.
pub fn emit(s: &mut String, p: &Program, stimulus: &[Stimulus]) {
    let stroeme = sammeln(p, stimulus);
    let _ = writeln!(s, "/* Stroeme (8.6, 9.6); der Stimulus steht vor dem Lauf fest. */");
    if stroeme.is_empty() {
        let _ = writeln!(s, "int takt_stream_count(int s, long long cur) {{ (void)s; (void)cur; return 0; }}");
        let _ = writeln!(s, "long long takt_stream_at(int s, long long cur, int i, void *out) {{");
        let _ = writeln!(s, "    (void)s; (void)cur; (void)i; (void)out; return 0;");
        let _ = writeln!(s, "}}");
        let _ = writeln!(s, "void takt_stream_examined(int s, long long seq) {{ (void)s; (void)seq; }}\n");
        return;
    }

    // Die Elemente aller Stroeme in einer Tabelle; `seq` ist der Index
    // je Strom, wie die laufende Nummer in 9.6.
    let _ =
        writeln!(s, "struct takt_elem {{ int stream; long long tick; long long seq; int len; const char *bytes; }};");
    let _ = writeln!(s, "static const struct takt_elem g_elems[] = {{");
    for st in &stroeme {
        for (seq, e) in st.elemente.iter().enumerate() {
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

    // Die Kapazitaet je Strom: `takt_stream_at` schreibt `line<N>` als
    // `{{ i32 len, [N x i8], i1 truncated }}` (takt-llvm/src/ty.rs).
    let _ = writeln!(s, "static int takt_stream_cap(int s) {{");
    let _ = writeln!(s, "    switch (s) {{");
    for st in &stroeme {
        let _ = writeln!(s, "    case {}: return {}; /* {} */", st.id, st.cap, st.name);
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");

    // 9.6: Sichtbar ist, was geliefert *und* noch nicht untersucht ist.
    // `g_tick` ist der laufende Tick des Rahmens; ein Element wird im
    // Tick seiner Lieferung sichtbar (8.6: der Rand liefert sofort).
    let _ = writeln!(s, "int takt_stream_count(int s, long long cur) {{");
    let _ = writeln!(s, "    int n = 0;");
    let _ = writeln!(s, "    for (int i = 0; i < g_elem_count; i++)");
    let _ = writeln!(s, "        if (g_elems[i].stream == s && g_elems[i].tick <= g_tick && g_elems[i].seq >= cur)");
    let _ = writeln!(s, "            n++;");
    let _ = writeln!(s, "    return n;");
    let _ = writeln!(s, "}}\n");

    // Das `i`-te Element des Fensters an den uebergebenen Platz. Der
    // Aufbau ist der von `line<N>`/`str<N>`: Laenge, Bytes, und bei
    // `line` das Flag `truncated` (3.9).
    let _ = writeln!(s, "long long takt_stream_at(int s, long long cur, int i, void *out) {{");
    let _ = writeln!(s, "    int seen = 0;");
    let _ = writeln!(s, "    for (int k = 0; k < g_elem_count; k++) {{");
    let _ = writeln!(s, "        const struct takt_elem *e = &g_elems[k];");
    let _ = writeln!(s, "        if (e->stream != s || e->tick > g_tick || e->seq < cur) continue;");
    let _ = writeln!(s, "        if (seen++ != i) continue;");
    let _ = writeln!(s, "        int cap = takt_stream_cap(s);");
    let _ = writeln!(s, "        unsigned char *p = (unsigned char *)out;");
    let _ = writeln!(s, "        memset(p, 0, (size_t)cap + 8);");
    let _ = writeln!(s, "        *(int *)p = e->len;");
    let _ = writeln!(s, "        memcpy(p + 4, e->bytes, (size_t)e->len);");
    let _ = writeln!(s, "        return e->seq;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    (void)out;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}\n");

    // 9.6: `cur[s, m] = examined + 1`. Der Cursor gehoert der Maschine,
    // und der erzeugte Code fuehrt ihn in seinem Zustand — der Rahmen
    // muss ihn darum nicht halten.
    let _ = writeln!(s, "void takt_stream_examined(int s, long long seq) {{ (void)s; (void)seq; }}\n");
}
