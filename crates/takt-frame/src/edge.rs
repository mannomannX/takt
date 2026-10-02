//! Der Treiberrand im erzeugten Rahmen (12.6, FB-285).
//!
//! Der Rahmen sammelt die Lieferungen eines Ticks — auf dem Wirt aus dem
//! Stimulus, auf dem Board von den Treibern — und uebergibt sie dem Kern aus
//! `takt-hal` ueber `takt_edge_*` (`takt-native-abi`): Vertrag je Treiber
//! (Zeilen 1, 2), Tor je Wert (3, 4). Hier steht nur, was der Kern nicht
//! wissen kann: wo ein Kanal im Abbild liegt, welcher Ring einen Strom
//! traegt, wie der Trace geschrieben wird. Eine zweite Fassung der Regeln
//! in C gibt es nicht; Satz 9.4.4 gilt fuer die Randfaelle nur, wenn
//! derselbe Code urteilt.
//!
//! Die Schnittstelle fuer den Rahmen: `takt_edge_reading` je Abtastung,
//! `takt_edge_element` je Stromelement — die Skalare zuerst, wie im
//! Interpreter —, dann `takt_edge_commit(tick)`.

use std::fmt::Write as _;

use takt_mir::machine::Machine;
use takt_mir::program::{Direction, Overflow, Program};
use takt_mir::types::Type;

use crate::layout::{Layout, c_type};
use crate::streams::Trace;
use crate::text::Text;

/// Ein Wert ohne Range und Steigung (`bool`, Enum): Das Tor laesst ihn durch.
pub const KIND_NONE: u8 = 0;
/// Eine Ganzzahl oder Dauer, wie `Scalar::as_i64` sie liefert.
pub const KIND_INT: u8 = 1;
/// Eine Fliesskommazahl.
pub const KIND_FLOAT: u8 = 2;

/// Kein Zeitpunkt (`contract::NONE`).
const NONE: &str = "(-9223372036854775807LL - 1)";

/// Die Verletzungen in der Reihenfolge ihrer Zahl (`Contract::code`).
const WHAT: [&str; 6] = ["", "timestamp", "seq", "maxpt", "flags", "window"];

/// Schreibt Zustand, Schnittstelle und Commit des Rands. `max` ist die
/// Zahl der Lieferungen, die ein Tick hoechstens bringt. `takt_edge_init`
/// setzt die Spuren auf ihren Anfang; `init` ruft es (12.11).
pub fn emit(t: &mut Text, p: &Program, layout: &Layout, driven: &[&Machine], max: usize, trace: Trace) {
    let n = p.channels.len().max(1);
    let mut names: Vec<String> = Vec::new();
    let device_of: Vec<usize> = p
        .channels
        .iter()
        .map(|c| {
            let name = takt_hal::edge::driver_of(c);
            names.iter().position(|d| *d == name).unwrap_or_else(|| {
                names.push(name);
                names.len() - 1
            })
        })
        .collect();
    let devices = names.len().max(1);
    let max = max.max(1);
    // Ein Eingang mit Eintrag im Abbild hat ein Tor (Zeilen 3, 4); ohne
    // Eingaenge liefert kein Treiber etwas ins Programm. Was keiner
    // braucht, steht nicht in der Arena (12.11).
    let gated = layout.inputs.iter().any(|slot| gate_of(p, &slot.name).is_some());
    let delivers = p.channels.iter().any(|c| c.dir == Direction::Input);

    let ty = &mut t.types;
    let _ = writeln!(ty, "/* Treiberrand (12.6): Zustand je Kanal und Treiber, das Urteil im Kern (`takt-hal`). */");
    let _ = writeln!(ty, "struct takt_track {{ long long last_t, last_seq, last_measured; unsigned maxpt, count; }};");
    let _ = writeln!(ty, "struct takt_device {{ _Bool degraded, delivered; unsigned char broken; }};");
    let _ = writeln!(
        ty,
        "struct takt_delivery {{ unsigned channel; _Bool element, bad_with_value; long long t, age, seq, at; }};"
    );
    let _ = writeln!(ty, "struct takt_window {{ long long lo, hi, tolerance; }};");
    let _ = writeln!(ty, "struct takt_event {{ unsigned char kind, what; unsigned index; }};");
    let _ = writeln!(ty, "struct takt_gate {{ double good; long long good_t; _Bool has_good; unsigned strikes; }};");
    let _ = writeln!(
        ty,
        "struct takt_bounds {{ _Bool has_range; double lo, hi; _Bool has_slew; double slew; unsigned debounce; }};"
    );
    // Die Lieferungen eines Ticks, je mit dem, was der Kern nicht braucht:
    // der Wert fuers Abbild, die Bytes fuer den Ring.
    let _ = writeln!(
        ty,
        "struct takt_edge_value {{ unsigned char value[8]; unsigned char kind, quality, reason; _Bool has_value; \
         long long i; double f; long long age; const unsigned char *bytes; int len; }};"
    );
    let f = &mut t.fields;
    let _ = writeln!(f, "    struct takt_track edge_tracks[{n}];");
    let _ = writeln!(f, "    struct takt_device edge_devices[{devices}];");
    if gated {
        let _ = writeln!(f, "    struct takt_gate edge_gates[{n}];");
    }
    let _ = writeln!(f, "    struct takt_delivery edge_d[{max}];");
    if delivers {
        let _ = writeln!(f, "    struct takt_edge_value edge_v[{max}];");
    }
    let _ = writeln!(f, "    unsigned edge_n;");
    let s = &mut t.code;
    let _ = writeln!(
        s,
        "unsigned takt_edge_settle(struct takt_track *, unsigned, struct takt_device *, unsigned, const unsigned *, \
         struct takt_delivery *, unsigned, struct takt_window, struct takt_event *, unsigned);"
    );
    let _ = writeln!(
        s,
        "unsigned takt_edge_gate(struct takt_gate *, const struct takt_bounds *, _Bool, long long, double, long long);"
    );
    let _ = writeln!(s, "void takt_edge_driver_bad(struct takt_gate *);");

    let tracks: Vec<String> = p
        .channels
        .iter()
        .map(|c| {
            format!("{{ {NONE}, {NONE}, {NONE}, {}, 0 }}", takt_hal::edge::maxpt_of(c, p.config.tick).unwrap_or(0))
        })
        .collect();
    let _ = writeln!(s, "static const struct takt_track g_edge_track0[{n}] = {{ {} }};", or_zero(tracks));
    let _ = writeln!(s, "static void takt_edge_init(struct takt_arena *a) {{");
    let _ = writeln!(s, "    memcpy(a->edge_tracks, g_edge_track0, sizeof a->edge_tracks);");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static const unsigned g_edge_device_of[{n}] = {{ {} }};",
        or_zero(device_of.iter().map(usize::to_string).collect())
    );
    let bounds: Vec<String> = p
        .channels
        .iter()
        .map(|c| {
            let l = takt_interp::image::limits_of(c, p);
            let (lo, hi) = l.range.unwrap_or((0.0, 0.0));
            format!(
                "{{ {}, {}, {}, {}, {}, {} }}",
                u8::from(l.range.is_some()),
                double(lo),
                double(hi),
                u8::from(l.max_slew.is_some()),
                double(l.max_slew.unwrap_or(0.0)),
                l.debounce
            )
        })
        .collect();
    let _ = writeln!(s, "static const struct takt_bounds g_edge_bounds[{n}] = {{ {} }};", or_zero(bounds));
    let quoted = |v: &[String]| v.iter().map(|x| format!("\"{x}\"")).collect::<Vec<_>>();
    let _ =
        writeln!(s, "static const char *const g_edge_devices_named[{devices}] = {{ {} }};", or_zero(quoted(&names)));
    let channel_names: Vec<String> = p.channels.iter().map(|c| c.name.clone()).collect();
    let _ =
        writeln!(s, "static const char *const g_edge_channels_named[{n}] = {{ {} }};", or_zero(quoted(&channel_names)));
    let what: Vec<String> = WHAT.iter().map(|w| format!("\"{w}\"")).collect();
    let _ = writeln!(s, "static const char *const g_edge_what[{}] = {{ {} }};", WHAT.len(), what.join(", "));

    if delivers {
        deliveries(s, max);
    }
    report(s, trace);
    degrade(s, p, layout);
    apply(s, p, layout, driven, delivers);

    let tick = p.config.tick;
    let _ = writeln!(s, "static void takt_edge_commit(struct takt_arena *a, long long tick) {{");
    let _ = writeln!(s, "    struct takt_window w = {{ tick * {tick}LL - {tick}LL, tick * {tick}LL, {tick}LL }};");
    let _ = writeln!(s, "    struct takt_event ev[{}];", max + devices);
    let _ = writeln!(
        s,
        "    unsigned k = takt_edge_settle(a->edge_tracks, {n}, a->edge_devices, {devices}, g_edge_device_of, a->edge_d, a->edge_n, w, ev, {});",
        max + devices
    );
    let _ = writeln!(s, "    for (unsigned i = 0; i < k; i++) takt_edge_report(a, tick, &ev[i]);");
    let _ = writeln!(s, "    takt_edge_degrade(a);");
    let _ = writeln!(s, "    for (unsigned j = 0; j < a->edge_n; j++)");
    let _ = writeln!(s, "        if (a->edge_d[j].at != {NONE}) takt_edge_apply(a, j);");
    let _ = writeln!(s, "    a->edge_n = 0;");
    let _ = writeln!(s, "}}\n");
}

/// Die Lieferungen eines Treibers an den Rand: ein Wert (`takt_edge_reading`)
/// oder ein Element eines Stroms (`takt_edge_element`), hoechstens `max` je
/// Tick.
fn deliveries(s: &mut String, max: usize) {
    let _ = writeln!(
        s,
        "static void takt_edge_reading(struct takt_arena *a, unsigned c, const void *value, int size, unsigned char kind, long long i, double f, \
         unsigned char quality, unsigned char reason, _Bool has_value, long long t, long long age) {{"
    );
    let _ = writeln!(s, "    if (a->edge_n >= {max}) return;");
    let _ = writeln!(s, "    struct takt_delivery *d = &a->edge_d[a->edge_n];");
    let _ = writeln!(s, "    struct takt_edge_value *v = &a->edge_v[a->edge_n++];");
    let _ = writeln!(s, "    memset(d, 0, sizeof *d);");
    let _ = writeln!(s, "    memset(v, 0, sizeof *v);");
    let _ = writeln!(s, "    d->channel = c; d->bad_with_value = quality == 3 && has_value; d->t = t; d->age = age;");
    let _ = writeln!(s, "    if (has_value && size > 0 && size <= 8) memcpy(v->value, value, (size_t)size);");
    let _ = writeln!(s, "    v->kind = kind; v->i = i; v->f = f; v->quality = quality; v->reason = reason;");
    let _ = writeln!(s, "    v->has_value = has_value; v->age = age;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_edge_element(struct takt_arena *a, unsigned c, const unsigned char *bytes, int len, long long t, long long seq) {{"
    );
    let _ = writeln!(s, "    if (a->edge_n >= {max}) return;");
    let _ = writeln!(s, "    struct takt_delivery *d = &a->edge_d[a->edge_n];");
    let _ = writeln!(s, "    struct takt_edge_value *v = &a->edge_v[a->edge_n++];");
    let _ = writeln!(s, "    memset(d, 0, sizeof *d);");
    let _ = writeln!(s, "    memset(v, 0, sizeof *v);");
    let _ = writeln!(s, "    d->channel = c; d->element = 1; d->t = t; d->seq = seq;");
    let _ = writeln!(s, "    v->bytes = bytes; v->len = len;");
    let _ = writeln!(s, "}}");
}

/// Eine Meldung des Rands als Trace-Zeile `driver` (`grammar/trace.md`).
fn report(s: &mut String, trace: Trace) {
    let _ = writeln!(
        s,
        "static void takt_edge_report(struct takt_arena *a, long long tick, const struct takt_event *e) {{"
    );
    let _ = writeln!(s, "    const char *device, *rest, *word;");
    let _ = writeln!(s, "    switch (e->kind) {{");
    let _ = writeln!(
        s,
        "    case 1: device = g_edge_devices_named[e->index]; word = \"degraded\"; rest = e->what < 6 ? g_edge_what[e->what] : \"\"; break;"
    );
    let _ = writeln!(s, "    case 2: device = g_edge_devices_named[e->index]; word = \"recovered\"; rest = 0; break;");
    let _ = writeln!(s, "    case 3: {{");
    let _ = writeln!(s, "        unsigned c = a->edge_d[e->index].channel;");
    let _ = writeln!(
        s,
        "        device = g_edge_devices_named[g_edge_device_of[c]]; word = \"warped\"; rest = g_edge_channels_named[c]; break;"
    );
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    default: return;");
    let _ = writeln!(s, "    }}");
    match trace {
        Trace::Stdio => {
            let _ = writeln!(s, "    if (rest) printf(\"t=%lld driver %s %s %s\\n\", tick, device, word, rest);");
            let _ = writeln!(s, "    else printf(\"t=%lld driver %s %s\\n\", tick, device, word);");
        }
        Trace::Board => {
            let _ = writeln!(s, "    takt_board_trace(\"t=\");");
            let _ = writeln!(s, "    takt_board_trace_i64(tick);");
            let _ = writeln!(s, "    takt_board_trace(\"driver \");");
            let _ = writeln!(s, "    takt_board_trace(device);");
            let _ = writeln!(s, "    takt_board_trace(\" \");");
            let _ = writeln!(s, "    takt_board_trace(word);");
            let _ = writeln!(s, "    if (rest) {{ takt_board_trace(\" \"); takt_board_trace(rest); }}");
            let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
        }
    }
    let _ = writeln!(s, "}}");
}

/// Zeile 2: Die Inputs eines degradierten Treibers sind `Bad` mit Grund
/// `Driver`, der Bezugspunkt faellt weg (`Image::degrade`).
fn degrade(s: &mut String, p: &Program, layout: &Layout) {
    let _ = writeln!(s, "static void takt_edge_degrade(struct takt_arena *a) {{");
    for slot in &layout.inputs {
        let Some((c, e)) = gate_of(p, &slot.name) else { continue };
        let _ = writeln!(
            s,
            "    if (a->edge_devices[g_edge_device_of[{c}]].degraded) {{ takt_edge_driver_bad(&a->edge_gates[{c}]); \
             a->image[{}] = 3; a->image[{}] = 3; *(long long *)(a->image + {}) = 0; }} /* {} */",
            e.quality, e.reason, e.age, slot.name
        );
    }
    let _ = writeln!(s, "}}");
}

/// Eine Lieferung eines vertragstreuen Treibers ins Abbild oder in den Ring:
/// das Tor fuer den Wert (Zeilen 3, 4), wie `Image::through_edge`; ein
/// Element, das sich nicht decodieren liess, verworfen (Zeile 5). Ohne
/// Eingaenge gibt es keine Lieferung.
fn apply(s: &mut String, p: &Program, layout: &Layout, driven: &[&Machine], delivers: bool) {
    if !delivers {
        let _ = writeln!(s, "static void takt_edge_apply(struct takt_arena *a, unsigned j) {{ (void)a; (void)j; }}");
        return;
    }
    let _ = writeln!(s, "static void takt_edge_apply(struct takt_arena *a, unsigned j) {{");
    let _ = writeln!(s, "    const struct takt_delivery *d = &a->edge_d[j];");
    let _ = writeln!(s, "    const struct takt_edge_value *v = &a->edge_v[j];");
    let _ = writeln!(s, "    switch (d->channel) {{");
    for slot in &layout.inputs {
        let (Some((c, e)), Some(_)) = (gate_of(p, &slot.name), c_type(&slot.ty, slot.signed)) else { continue };
        let _ = writeln!(s, "    case {c}: {{ /* {} */", slot.name);
        let _ = writeln!(s, "        unsigned verdict = 0;");
        let _ = writeln!(s, "        if (v->has_value && v->kind != 0)");
        let _ = writeln!(
            s,
            "            verdict = takt_edge_gate(&a->edge_gates[{c}], &g_edge_bounds[{c}], v->kind == 1, v->i, v->f, d->at);"
        );
        let _ = writeln!(s, "        if (!v->has_value || (verdict & 0xff) == 0) {{");
        let _ =
            writeln!(s, "            if (v->has_value) memcpy(a->image + {}, v->value, {});", slot.offset, slot.size);
        let _ = writeln!(s, "            a->image[{}] = v->quality; a->image[{}] = v->reason;", e.quality, e.reason);
        let _ = writeln!(s, "        }} else {{");
        // `Suspect` haelt den letzten guten Wert, der schon im Abbild steht;
        // `Bad` hat keinen.
        let _ = writeln!(
            s,
            "            a->image[{}] = (verdict & 0xff) == 1 ? 1 : 3; a->image[{}] = (unsigned char)(verdict >> 8);",
            e.quality, e.reason
        );
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "        *(long long *)(a->image + {}) = v->age;", e.age);
        let _ = writeln!(s, "        break;");
        let _ = writeln!(s, "    }}");
    }
    let code = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::StreamOverflow);
    for (c, ch) in p.channels.iter().enumerate() {
        if ch.dir != Direction::Input || !matches!(p.types.list.get(ch.ty.index()), Some(Type::Stream(_))) {
            continue;
        }
        let drop_oldest = u8::from(matches!(ch.attrs.overflow, Some(Overflow::DropOldest)));
        let _ = writeln!(s, "    case {c}: {{ /* {} */", ch.name);
        // Zeile 5: Ein Element, dessen `decode` misslingt, wird verworfen.
        if let Some(Type::Stream(elem)) = p.types.list.get(ch.ty.index())
            && crate::streams::element_shape(p, *elem).is_some()
        {
            let _ = writeln!(
                s,
                "        if (!takt_edge_decodes(g_shape_{c}, (unsigned)sizeof g_shape_{c}, v->bytes, (unsigned)v->len, 1)) {{ a->int_malformed[takt_int_slot({c})]++; break; }}"
            );
        }
        let _ = writeln!(
            s,
            "        int r = takt_int_deliver(a, takt_int_slot({c}), v->bytes, v->len, d->at, {drop_oldest});"
        );
        // 9.6, `Sim::overflow_channel`: Der Ueberlauf faultet jeden Leser,
        // der nicht schlaeft (oder den der Strom weckt) und noch keinen
        // Fault vorgemerkt hat.
        let readers: Vec<String> = driven
            .iter()
            .filter(|m| m.layout.cursors.contains(&takt_mir::expr::StreamRef::Channel(takt_mir::ChannelId(c as u32))))
            .filter_map(|m| {
                let i = p.machines.iter().position(|x| x.name == m.name)?;
                let awake = if ch.attrs.wake { "1".to_string() } else { format!("!takt_{0}_idle(a)", m.name) };
                Some(format!("if ({awake} && a->pending[{i}] == 0) a->pending[{i}] = {code};"))
            })
            .collect();
        if !readers.is_empty() {
            let _ = writeln!(s, "        if (r == 2) {{ {} }}", readers.join(" "));
        }
        let _ = writeln!(s, "        (void)r;");
        let _ = writeln!(s, "        break;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    default: break;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// Der Kanal eines Eingangs mit Eintrag im Abbild: einer, dessen Wert durch
/// ein Tor geht (`edge_gates`).
fn gate_of(p: &Program, name: &str) -> Option<(usize, Entry)> {
    Some((p.channels.iter().position(|c| c.name == name)?, entry(p, name)?))
}

/// Die Versaetze eines Eintrags im Abbild.
struct Entry {
    quality: u64,
    reason: u64,
    age: u64,
}

fn entry(p: &Program, name: &str) -> Option<Entry> {
    Some(Entry {
        quality: crate::parts::quality_offset(p, name)?,
        reason: crate::parts::reason_offset(p, name)?,
        age: crate::parts::age_offset(p, name)?,
    })
}

/// Eine Liste fuer einen C-Initialisierer; leer ist `0`, denn C kennt
/// keine leeren Initialisierer.
fn or_zero(items: Vec<String>) -> String {
    if items.is_empty() { "0".to_string() } else { items.join(", ") }
}

/// Eine Zahl als C-Literal vom Typ `double`.
pub fn double(f: f64) -> String {
    if f.is_nan() {
        "__builtin_nan(\"\")".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "__builtin_inf()".to_string() } else { "(-__builtin_inf())".to_string() }
    } else {
        format!("{f:?}")
    }
}
