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
use crate::stimulus::Stimulus;
use crate::streams::Trace;

/// Ein Wert ohne Range und Steigung (`bool`, Enum): Das Tor laesst ihn durch.
pub(crate) const KIND_NONE: u8 = 0;
/// Eine Ganzzahl oder Dauer, wie `Scalar::as_i64` sie liefert.
pub(crate) const KIND_INT: u8 = 1;
/// Eine Fliesskommazahl.
pub(crate) const KIND_FLOAT: u8 = 2;

/// Kein Zeitpunkt (`contract::NONE`).
const NONE: &str = "(-9223372036854775807LL - 1)";

/// Die Verletzungen in der Reihenfolge ihrer Zahl (`Contract::code`).
const WHAT: [&str; 6] = ["", "timestamp", "seq", "maxpt", "flags", "window"];

/// Schreibt Zustand, Schnittstelle und Commit des Rands. `max` ist die
/// Zahl der Lieferungen, die ein Tick hoechstens bringt.
pub(crate) fn emit(s: &mut String, p: &Program, layout: &Layout, driven: &[&Machine], max: usize, trace: Trace) {
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

    let _ = writeln!(s, "/* Treiberrand (12.6): Zustand je Kanal und Treiber, das Urteil im Kern (`takt-hal`). */");
    let _ = writeln!(s, "struct takt_track {{ long long last_t, last_seq, last_measured; unsigned maxpt, count; }};");
    let _ = writeln!(s, "struct takt_device {{ _Bool degraded, delivered; unsigned char broken; }};");
    let _ = writeln!(
        s,
        "struct takt_delivery {{ unsigned channel; _Bool element, bad_with_value; long long t, age, seq, at; }};"
    );
    let _ = writeln!(s, "struct takt_window {{ long long lo, hi, tolerance; }};");
    let _ = writeln!(s, "struct takt_event {{ unsigned char kind, what; unsigned index; }};");
    let _ = writeln!(s, "struct takt_gate {{ double good; long long good_t; _Bool has_good; unsigned strikes; }};");
    let _ = writeln!(
        s,
        "struct takt_bounds {{ _Bool has_range; double lo, hi; _Bool has_slew; double slew; unsigned debounce; }};"
    );
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
    let _ = writeln!(s, "static struct takt_track g_edge_tracks[{n}] = {{ {} }};", or_zero(tracks));
    let _ = writeln!(s, "static struct takt_device g_edge_devices[{devices}];");
    let _ = writeln!(
        s,
        "static const unsigned g_edge_device_of[{n}] = {{ {} }};",
        or_zero(device_of.iter().map(usize::to_string).collect())
    );
    let _ = writeln!(s, "static struct takt_gate g_edge_gates[{n}];");
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

    // Die Lieferungen eines Ticks, je mit dem, was der Kern nicht braucht:
    // der Wert fuers Abbild, die Bytes fuer den Ring.
    let _ = writeln!(
        s,
        "struct takt_edge_value {{ unsigned char value[8]; unsigned char kind, quality, reason; _Bool has_value; \
         long long i; double f; long long age; const unsigned char *bytes; int len; _Bool malformed; }};"
    );
    let _ = writeln!(s, "static struct takt_delivery g_edge_d[{max}];");
    let _ = writeln!(s, "static struct takt_edge_value g_edge_v[{max}];");
    let _ = writeln!(s, "static unsigned g_edge_n;");
    let _ = writeln!(
        s,
        "static void takt_edge_reading(unsigned c, const void *value, int size, unsigned char kind, long long i, double f, \
         unsigned char quality, unsigned char reason, _Bool has_value, long long t, long long age) {{"
    );
    let _ = writeln!(s, "    if (g_edge_n >= {max}) return;");
    let _ = writeln!(s, "    struct takt_delivery *d = &g_edge_d[g_edge_n];");
    let _ = writeln!(s, "    struct takt_edge_value *v = &g_edge_v[g_edge_n++];");
    let _ = writeln!(s, "    memset(d, 0, sizeof *d);");
    let _ = writeln!(s, "    memset(v, 0, sizeof *v);");
    let _ = writeln!(s, "    d->channel = c; d->bad_with_value = quality == 3 && has_value; d->t = t; d->age = age;");
    let _ = writeln!(s, "    if (has_value && size > 0 && size <= 8) memcpy(v->value, value, (size_t)size);");
    let _ = writeln!(s, "    v->kind = kind; v->i = i; v->f = f; v->quality = quality; v->reason = reason;");
    let _ = writeln!(s, "    v->has_value = has_value; v->age = age;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "static void takt_edge_element(unsigned c, const unsigned char *bytes, int len, _Bool malformed, long long t, long long seq) {{"
    );
    let _ = writeln!(s, "    if (g_edge_n >= {max}) return;");
    let _ = writeln!(s, "    struct takt_delivery *d = &g_edge_d[g_edge_n];");
    let _ = writeln!(s, "    struct takt_edge_value *v = &g_edge_v[g_edge_n++];");
    let _ = writeln!(s, "    memset(d, 0, sizeof *d);");
    let _ = writeln!(s, "    memset(v, 0, sizeof *v);");
    let _ = writeln!(s, "    d->channel = c; d->element = 1; d->t = t; d->seq = seq;");
    let _ = writeln!(s, "    v->bytes = bytes; v->len = len; v->malformed = malformed;");
    let _ = writeln!(s, "}}");

    report(s, trace);
    degrade(s, p, layout);
    apply(s, p, layout, driven);

    let tick = p.config.tick;
    let _ = writeln!(s, "static void takt_edge_commit(long long tick) {{");
    let _ = writeln!(s, "    struct takt_window w = {{ tick * {tick}LL - {tick}LL, tick * {tick}LL, {tick}LL }};");
    let _ = writeln!(s, "    struct takt_event ev[{}];", max + devices);
    let _ = writeln!(
        s,
        "    unsigned k = takt_edge_settle(g_edge_tracks, {n}, g_edge_devices, {devices}, g_edge_device_of, g_edge_d, g_edge_n, w, ev, {});",
        max + devices
    );
    let _ = writeln!(s, "    for (unsigned i = 0; i < k; i++) takt_edge_report(tick, &ev[i]);");
    let _ = writeln!(s, "    takt_edge_degrade();");
    let _ = writeln!(s, "    for (unsigned j = 0; j < g_edge_n; j++)");
    let _ = writeln!(s, "        if (g_edge_d[j].at != {NONE}) takt_edge_apply(j);");
    let _ = writeln!(s, "    g_edge_n = 0;");
    let _ = writeln!(s, "}}\n");
}

/// Eine Meldung des Rands als Trace-Zeile `driver` (`grammar/trace.md`).
fn report(s: &mut String, trace: Trace) {
    let _ = writeln!(s, "static void takt_edge_report(long long tick, const struct takt_event *e) {{");
    let _ = writeln!(s, "    const char *device, *rest, *word;");
    let _ = writeln!(s, "    switch (e->kind) {{");
    let _ = writeln!(
        s,
        "    case 1: device = g_edge_devices_named[e->index]; word = \"degraded\"; rest = e->what < 6 ? g_edge_what[e->what] : \"\"; break;"
    );
    let _ = writeln!(s, "    case 2: device = g_edge_devices_named[e->index]; word = \"recovered\"; rest = 0; break;");
    let _ = writeln!(s, "    case 3: {{");
    let _ = writeln!(s, "        unsigned c = g_edge_d[e->index].channel;");
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
            let _ = writeln!(s, "    takt_board_trace(\" driver \");");
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
    let _ = writeln!(s, "static void takt_edge_degrade(void) {{");
    for slot in &layout.inputs {
        let Some(c) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let Some(e) = entry(p, &slot.name) else { continue };
        let _ = writeln!(
            s,
            "    if (g_edge_devices[g_edge_device_of[{c}]].degraded) {{ takt_edge_driver_bad(&g_edge_gates[{c}]); \
             image[{}] = 3; image[{}] = 3; *(long long *)(image + {}) = 0; }} /* {} */",
            e.quality, e.reason, e.age, slot.name
        );
    }
    let _ = writeln!(s, "}}");
}

/// Eine Lieferung eines vertragstreuen Treibers ins Abbild oder in den Ring:
/// das Tor fuer den Wert (Zeilen 3, 4), wie `Image::through_edge`; ein
/// Element, das sich nicht decodieren liess, verworfen (Zeile 5).
fn apply(s: &mut String, p: &Program, layout: &Layout, driven: &[&Machine]) {
    let _ = writeln!(s, "static void takt_edge_apply(unsigned j) {{");
    let _ = writeln!(s, "    const struct takt_delivery *d = &g_edge_d[j];");
    let _ = writeln!(s, "    const struct takt_edge_value *v = &g_edge_v[j];");
    let _ = writeln!(s, "    switch (d->channel) {{");
    for slot in &layout.inputs {
        let Some(c) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        let (Some(e), Some(_)) = (entry(p, &slot.name), c_type(&slot.ty, slot.signed)) else { continue };
        let _ = writeln!(s, "    case {c}: {{ /* {} */", slot.name);
        let _ = writeln!(s, "        unsigned verdict = 0;");
        let _ = writeln!(s, "        if (v->has_value && v->kind != 0)");
        let _ = writeln!(
            s,
            "            verdict = takt_edge_gate(&g_edge_gates[{c}], &g_edge_bounds[{c}], v->kind == 1, v->i, v->f, d->at);"
        );
        let _ = writeln!(s, "        if (!v->has_value || (verdict & 0xff) == 0) {{");
        let _ = writeln!(s, "            if (v->has_value) memcpy(image + {}, v->value, {});", slot.offset, slot.size);
        let _ = writeln!(s, "            image[{}] = v->quality; image[{}] = v->reason;", e.quality, e.reason);
        let _ = writeln!(s, "        }} else {{");
        // `Suspect` haelt den letzten guten Wert, der schon im Abbild steht;
        // `Bad` hat keinen.
        let _ = writeln!(
            s,
            "            image[{}] = (verdict & 0xff) == 1 ? 1 : 3; image[{}] = (unsigned char)(verdict >> 8);",
            e.quality, e.reason
        );
        let _ = writeln!(s, "        }}");
        let _ = writeln!(s, "        *(long long *)(image + {}) = v->age;", e.age);
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
        let _ = writeln!(s, "        if (v->malformed) break;");
        let _ = writeln!(
            s,
            "        int r = takt_int_deliver(takt_int_slot({c}), v->bytes, v->len, d->at, {drop_oldest});"
        );
        // 9.6, `Sim::overflow_channel`: Der Ueberlauf faultet jeden Leser,
        // der nicht schlaeft (oder den der Strom weckt) und noch keinen
        // Fault vorgemerkt hat.
        let readers: Vec<String> = driven
            .iter()
            .filter(|m| m.layout.cursors.contains(&takt_mir::expr::StreamRef::Channel(takt_mir::ChannelId(c as u32))))
            .filter_map(|m| {
                let i = p.machines.iter().position(|x| x.name == m.name)?;
                let awake = if ch.attrs.wake { "1".to_string() } else { format!("!{0}_idle(state_{0})", m.name) };
                Some(format!("if ({awake} && g_pending[{i}] == 0) g_pending[{i}] = {code};"))
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

/// Die Versaetze eines Eintrags im Abbild.
struct Entry {
    quality: u64,
    reason: u64,
    age: u64,
}

fn entry(p: &Program, name: &str) -> Option<Entry> {
    Some(Entry {
        quality: crate::harness::quality_offset(p, name)?,
        reason: crate::harness::reason_offset(p, name)?,
        age: crate::harness::age_offset(p, name)?,
    })
}

/// Eine Liste fuer einen C-Initialisierer; leer ist `0`, denn C kennt
/// keine leeren Initialisierer.
fn or_zero(items: Vec<String>) -> String {
    if items.is_empty() { "0".to_string() } else { items.join(", ") }
}

/// Eine Zahl als C-Literal vom Typ `double`.
pub(crate) fn double(f: f64) -> String {
    if f.is_nan() {
        "__builtin_nan(\"\")".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "__builtin_inf()".to_string() } else { "(-__builtin_inf())".to_string() }
    } else {
        format!("{f:?}")
    }
}

/// Die Lieferungen des Stimulus (`in`-Zeilen) an den Rand, je Tick: die
/// Skalare zuerst, dann die Elemente, je in ihrer Reihenfolge — wie
/// `deliver` im Interpreter. Ohne `t=` gilt die Tickgrenze, ohne `seq=`
/// die naechste Nummer der lueckenlosen Folge; beides steht schon vor dem
/// Lauf fest, denn der Rand gleicht nach jeder Lieferung ab. Ein
/// Record-Element darf als Bytes stehen; misslingt `decode`, ist es
/// `malformed` (Zeile 5).
///
/// Das Ergebnis ist die Zahl der Lieferungen im vollsten Tick.
pub(crate) fn stimulus(s: &mut String, p: &Program, layout: &Layout, inputs: &[Stimulus]) -> usize {
    use std::collections::BTreeMap;
    let tick_ns = p.config.tick;
    let mut ticks: BTreeMap<u64, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let mut last_seq: BTreeMap<usize, i64> = BTreeMap::new();
    for stim in inputs {
        let Stimulus::Input { tick, channel, sample } = stim else { continue };
        let Some(c) = p.channels.iter().position(|ch| ch.name == *channel && ch.dir == Direction::Input) else {
            continue;
        };
        let t = sample.t.unwrap_or_else(|| i64::try_from(*tick).unwrap_or(i64::MAX).saturating_mul(tick_ns));
        let ty = p.channels[c].ty;
        let entry = ticks.entry(*tick).or_default();
        if let Some(Type::Stream(elem)) = p.types.list.get(ty.index()) {
            let seq = sample.seq.unwrap_or_else(|| last_seq.get(&c).map_or(0, |s| s.saturating_add(1)));
            last_seq.insert(c, seq);
            let Some((bytes, malformed)) = element_bytes(p, *elem, sample.value.as_deref().unwrap_or_default()) else {
                continue;
            };
            let text: String = bytes.iter().map(|b| format!("\\x{b:02x}")).collect();
            entry.1.push(format!(
                "takt_edge_element({c}, (const unsigned char *)\"{text}\", {}, {}, {t}LL, {seq}LL); /* {channel} */",
                bytes.len(),
                u8::from(malformed)
            ));
            continue;
        }
        let Some(slot) = layout.inputs.iter().find(|sl| sl.name == *channel) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Ok(sample) = takt_interp::trace::sample_from_text(sample, ty, p) else { continue };
        let quality = match sample.quality {
            takt_interp::value::Quality::Good => 0,
            takt_interp::value::Quality::Suspect => 1,
            takt_interp::value::Quality::Stale => 2,
            takt_interp::value::Quality::Bad => 3,
        };
        let reason = match sample.reason {
            Some(takt_interp::value::Reason::OutOfRange) => 1,
            Some(takt_interp::value::Reason::Implausible) => 2,
            Some(takt_interp::value::Reason::Driver) => 3,
            Some(takt_interp::value::Reason::Node) => 4,
            Some(takt_interp::value::Reason::Stale) | None => 0,
        };
        let call = match sample.value.as_ref().and_then(|v| number(p, ty, v)) {
            Some((literal, kind, i, f)) => format!(
                "takt_edge_reading({c}, &({ct}){{ {literal} }}, (int)sizeof({ct}), {kind}, {i}LL, {}, {quality}, {reason}, 1, {t}LL, {}LL);",
                double(f),
                sample.age
            ),
            None => {
                format!("takt_edge_reading({c}, 0, 0, 0, 0LL, 0.0, {quality}, {reason}, 0, {t}LL, {}LL);", sample.age)
            }
        };
        entry.0.push(format!("{call} /* {channel} */"));
    }
    let _ = writeln!(s, "/* Die Lieferungen des Stimulus an den Treiberrand (12.6). */");
    let _ = writeln!(s, "static void takt_edge_stimulus(long long tick) {{");
    let _ = writeln!(s, "    switch (tick) {{");
    for (tick, (readings, elements)) in &ticks {
        let _ = writeln!(s, "    case {tick}:");
        for line in readings.iter().chain(elements) {
            let _ = writeln!(s, "        {line}");
        }
        let _ = writeln!(s, "        break;");
    }
    let _ = writeln!(s, "    default: break;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_edge_commit(tick);");
    let _ = writeln!(s, "}}\n");
    ticks.values().map(|(r, e)| r.len() + e.len()).max().unwrap_or(0)
}

/// Die Bytes eines Stromelements im Ring und ob sein `decode` misslingt.
fn element_bytes(p: &Program, elem: takt_mir::TypeId, text: &str) -> Option<(Vec<u8>, bool)> {
    let cap = match p.types.list.get(elem.index()) {
        Some(Type::Line { cap } | Type::Str { cap } | Type::Bytes { cap }) => Some(*cap as usize),
        _ => None,
    };
    if let Some(cap) = cap {
        // 3.9: Der Rand begrenzt die Laenge; was darueber steht, ist
        // abgeschnitten und nicht verworfen.
        let mut bytes = text.as_bytes().to_vec();
        bytes.truncate(cap);
        return Some((bytes, false));
    }
    if let (Some(Type::Record(_)), Some(hex)) = (p.types.list.get(elem.index()), text.trim().strip_prefix("0x")) {
        let bytes: Vec<u8> =
            (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()).collect();
        let malformed = takt_interp::bytes::decode(p, &bytes, elem).is_err();
        return Some((bytes, malformed));
    }
    let value = takt_interp::trace::parse_value(text, elem, p).ok()?;
    Some((takt_interp::bytes::encode(p, &value, elem).ok()?, false))
}

/// Ein Wert als C-Literal, dazu seine Form fuer das Tor (`Scalar`):
/// Art, `as_i64`, `as_f64`.
fn number(p: &Program, ty: takt_mir::TypeId, v: &takt_interp::Value) -> Option<(String, u8, i64, f64)> {
    use takt_interp::Value;
    Some(match v {
        Value::Int(n) => (format!("{n}LL"), KIND_INT, *n, *n as f64),
        Value::Duration(n) => (format!("{n}LL"), KIND_INT, *n, *n as f64),
        Value::UInt(u) => match i64::try_from(*u) {
            Ok(n) => (format!("{u}ULL"), KIND_INT, n, *u as f64),
            Err(_) => (format!("{u}ULL"), KIND_FLOAT, 0, *u as f64),
        },
        Value::F64(f) => (double(*f), KIND_FLOAT, 0, *f),
        Value::F32(f) => (format!("(float){}", double(f64::from(*f))), KIND_FLOAT, 0, f64::from(*f)),
        Value::Bool(b) => (u8::from(*b).to_string(), KIND_NONE, 0, 0.0),
        Value::Enum { variant, .. } => {
            let Type::Enum(e) = p.types.get(ty) else { return None };
            let d = p.enums.get(e.index())?.variants.get(*variant as usize)?.discriminant;
            (d.to_string(), KIND_NONE, 0, 0.0)
        }
        _ => return None,
    })
}
