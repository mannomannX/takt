//! Die Bausteine des Rahmens: der Tickschritt aus 12.1 als C.
//!
//! Maschinen deklarieren und betreten, Schritte und Abort-Phase, geplante
//! Ausgaben und Commit, Alterung, Alerts und Fault-Namen, Jobs und Natives.
//! Beide Rahmen setzen sich daraus zusammen: der MCU-Rahmen (`mcu`) und der
//! Wirtsrahmen des Differentials in `takt-conformance`.

use std::fmt::Write as _;
use takt_llvm::symbols::Prefix;

use takt_mir::program::Program;

use crate::layout::{Layout, c_type};
use crate::text::Text;

/// Eine Dauer in der groessten ganzzahligen Einheit, wie `takt_mir::dump::duration`
/// sie schreibt (T2); beide Rahmen nehmen dieselben Funktionen.
pub const DURATION_C: &str = "\
static const long long takt_dur_factor[7] = { 86400000000000LL, 3600000000000LL, 60000000000LL, 1000000000LL, \
1000000LL, 1000LL, 1LL };
static const char *const takt_dur_name[7] = { \"d\", \"h\", \"min\", \"s\", \"ms\", \"us\", \"ns\" };
static inline int takt_dur_index(long long ns) {
    int i = 0;
    if (ns == 0) return 6;
    while (i < 6 && ns % takt_dur_factor[i] != 0) i++;
    return i;
}
static inline long long takt_dur_value(long long ns) { return ns / takt_dur_factor[takt_dur_index(ns)]; }
static inline const char *takt_dur_unit(long long ns) { return takt_dur_name[takt_dur_index(ns)]; }";

/// Die nativen Funktionen (4.5) liegen in der Runtime: `takt-native-abi`
/// liefert ihre C-Einstiege, fuer den Wirtsrahmen als statische
/// Bibliothek ([`native_library`]), fuer ein Board ueber `takt-embed`
/// (FB-293). Der Rahmen deklariert nur, was er selbst
/// ruft — die Jobs; den erzeugten Code bindet der Linker direkt.
pub fn natives(s: &mut String, p: &Program) {
    for n in &p.natives {
        if let Some(f) = takt_native::Native::by_name(&n.name).filter(|_| n.from.is_none()) {
            let _ = writeln!(s, "{};", prototype(f));
        }
    }
}

/// Die C-Deklaration eines Einstiegs: ein `bytes<N>`, Record oder Feld
/// als Zeiger und Laenge, ein Ergebnis aus Bytes nach `out`. Ein `bytes<N>`
/// als Ergebnis gibt seine Laenge zurueck, `-1`, wenn es keines gibt
/// (`aes_gcm_decrypt` mit falschem Tag: `Err(FAILED)`, 4.5).
fn prototype(f: takt_native::Native) -> String {
    use takt_native::Kind;
    let sig = f.signature();
    let mut params: Vec<&str> = sig.params.iter().map(|_| "const unsigned char *, int").collect();
    let ret = match sig.ret {
        Kind::U32 => "unsigned int",
        Kind::U16 => "unsigned short",
        Kind::U8 => "unsigned char",
        Kind::Bool => "_Bool",
        Kind::Digest | Kind::Sha256Ctx | Kind::Fixed(_) | Kind::Floats256 => {
            params.push("unsigned char *");
            "void"
        }
        Kind::Bytes => {
            params.push("unsigned char *");
            "int"
        }
    };
    let params = if params.is_empty() { "void".to_string() } else { params.join(", ") };
    format!("{ret} takt_native_{}({params})", f.name())
}

/// Der Wert eines Parameters als C-Literal (8.4).
///
/// Nur Literale: Ein berechneter Default braeuchte den Interpreter, und
/// der Rahmen soll nichts auswerten, was der Vergleich pruefen soll.
/// Setzt jeden Output auf seinen `safe`-Wert (5.3, 9.4).
///
/// Der Interpreter tut das in `Sim::new`, bevor eine Maschine laeuft:
/// Ein Lauf beginnt im sicheren Zustand, nicht bei null. Der Unterschied
/// faellt auf, sobald ein Wert nicht zufaellig 0 ist — ein Modell mit
/// `safe = 22 degC` lieferte sonst 0, und jeder Vergleich daran haengt.
///
/// Nur Literale: Ein berechneter `safe`-Wert braeuchte den Interpreter,
/// und der Rahmen soll ohne ihn auskommen (dieselbe Grenze wie bei den
/// Parametern).
pub fn safe_outputs(s: &mut String, p: &Program, layout: &crate::layout::Layout) {
    for slot in &layout.outputs {
        let Some(i) = p.channels.iter().position(|c| c.name == slot.name) else { continue };
        // Ohne `safe` gilt der Vorgabewert des Typs (`Value::default_for`),
        // im Latch lauter Nullen — auch am Ende eines Laufs (12.7).
        let Some(safe) = &p.channels[i].attrs.safe else {
            let _ = writeln!(
                s,
                "    memset(a->latch + {}, 0, {}); /* {} ohne safe: Vorgabe */",
                slot.offset, slot.size, slot.name
            );
            continue;
        };
        if let Some(text) = safe_payload(p, slot, safe) {
            s.push_str(&text);
            continue;
        }
        // Ein Array elementweise; sein `safe` ist ein Array-Literal (3.6).
        if let (takt_llvm::ty::LlvmType::Array(elem, _), takt_mir::expr::ExprKind::Array(items)) =
            (&slot.ty, &safe.kind)
        {
            let Some(ct) = c_type(elem, slot.signed) else { continue };
            for (k, item) in items.iter().enumerate() {
                let Some(text) = literal(p, item) else { continue };
                let _ = writeln!(
                    s,
                    "    (({ct} *)(a->latch + {}))[{k}] = {text}; /* {}[{k}] auf safe */",
                    slot.offset, slot.name
                );
            }
            continue;
        }
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(text) = literal(p, safe) else { continue };
        let _ = writeln!(s, "    *({ct} *)(a->latch + {}) = {text}; /* {} auf safe (5.3) */", slot.offset, slot.name);
    }
}

/// Der `safe`-Wert eines Enums mit Nutzlast (11.2): die Diskriminante als
/// `int` vorn, die Felder in ihren 8-Byte-Faechern, ungenutzte Faecher null
/// — `==` vergleicht auch sie.
fn safe_payload(p: &Program, slot: &crate::layout::Slot, safe: &takt_mir::expr::Expr) -> Option<String> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::expr::ExprKind;
    let LlvmType::Struct(_) = &slot.ty else { return None };
    let ExprKind::Variant { enum_id, variant, fields } = &safe.kind else { return None };
    let def = p.enums.get(enum_id.index())?;
    let v = def.variants.get(*variant as usize)?;
    let mut s = format!("    memset(a->latch + {}, 0, {});\n", slot.offset, slot.size);
    let _ = writeln!(
        s,
        "    *(int *)(a->latch + {}) = {}; /* {} auf safe (5.3) */",
        slot.offset, v.discriminant, slot.name
    );
    for (k, field) in fields.iter().enumerate() {
        let at = slot.offset + 8 + 8 * k as u64;
        let ct = match takt_llvm::ty::lower(v.fields.get(k)?.ty, p)? {
            LlvmType::F32 => "float",
            LlvmType::F64 => "double",
            _ => "long long",
        };
        let _ = writeln!(s, "    *({ct} *)(a->latch + {at}) = {};", literal(p, field)?);
    }
    Some(s)
}

/// Die geplanten Schreibvorgaenge (9.8), fuer beide Rahmen.
///
/// `takt_jitter` (7.5): der Jitter je Output aus `hw`, bei einem Output,
/// der nur zu Tickbeginn geschrieben wird, um den Tick mehr (`tick_granular`);
/// ohne Konfiguration null wie in der Simulation.
pub fn jitter(s: &mut String, p: &Program, hw: Option<&takt_mir::hardware::Hardware>, x: &Prefix) {
    let _ = writeln!(s, "long long {x}_jitter(struct {x}_arena *a, int o) {{");
    let _ = writeln!(s, "    switch (o) {{");
    for (i, c) in p.channels.iter().enumerate() {
        let (takt_mir::program::Binding::Hw(a), Some(hw)) = (&c.binding, hw) else { continue };
        let Some(entry) = hw.channel(&a.text()) else { continue };
        let granular = if entry.tick_granular == Some(true) { p.config.tick } else { 0 };
        let ns = entry.jitter_ns.unwrap_or(0).saturating_add(granular);
        if ns != 0 {
            let _ = writeln!(s, "    case {i}: return {ns}LL; /* {} */", c.name);
        }
    }
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// Die Plaetze je Warteschlange geplanter Ausgaben, `K_o` aus 7.5.
const K_O: usize = 4;

/// **Der Rahmen ist hier die Runtime.** 11.2 legt `sched` in den
/// Runtime-Anteil des Outputs, und 9.8 gibt die Regeln vor: sortiert
/// nach `T`, hoechstens `K_o` Eintraege je Output, gleiche `T`
/// ueberschreiben einander. Eine Warteschlange je Output aus
/// [`queued_outputs`] — so viele, wie `takt size` rechnet (11.5); ein
/// Programm ohne `at`, `pulse` und `cancel` bekommt keine.
///
/// `takt_schedule` liefert `false`, wenn der Zeitpunkt nicht weiter als
/// `guard` in der Zukunft liegt (`TimingFault`) oder die Warteschlange
/// voll ist (`ScheduleOverflow`) — beides Faults der Maschine, die der
/// erzeugte Code an seinem Fault-Pfad behandelt. `guard` ist die gemessene
/// Treiberlatenz des Outputs aus `hw` (7.5, 8.10), ohne Konfiguration
/// null wie in der Simulation. `takt_apply_scheduled` ruft
/// [`commit_sequence`].
pub fn scheduled(t: &mut Text, p: &Program, layout: &Layout, hw: Option<&takt_mir::hardware::Hardware>, x: &Prefix) {
    let queues = queued_outputs(p);
    if queues.is_empty() {
        return;
    }
    let n = queues.len();
    let _ = writeln!(t.code, "#define TAKT_K_O {K_O}");
    let _ = writeln!(t.types, "struct {x}_sched {{ long long t; long long v; }};");
    let _ = writeln!(t.fields, "    struct {x}_sched sched[{n}][{K_O}];");
    let _ = writeln!(t.fields, "    int sched_n[{n}];");
    let s = &mut t.code;
    let guards: Vec<String> = queues
        .iter()
        .map(|c| {
            let guard = match (&p.channels[c.index()].binding, hw) {
                (takt_mir::program::Binding::Hw(a), Some(hw)) => hw.channel(&a.text()).and_then(|e| e.guard_ns),
                _ => None,
            };
            format!("{}LL", guard.unwrap_or(0))
        })
        .collect();
    let _ = writeln!(s, "static const long long g_guard[{n}] = {{ {} }};", guards.join(", "));
    let _ = writeln!(s, "static int takt_sched_slot(int o) {{");
    let _ = writeln!(s, "    switch (o) {{");
    for (q, c) in queues.iter().enumerate() {
        let _ = writeln!(s, "    case {}: return {q}; /* {} */", c.index(), p.channels[c.index()].name);
    }
    let _ = writeln!(s, "    default: return -1;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
    // Das Ergebnis ist null oder die Art des Faults (`abi::fault_code`).
    let timing = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Timing);
    let overflow = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::ScheduleOverflow);
    let _ = writeln!(s, "int {x}_schedule(struct {x}_arena *a, int o, long long t, long long v) {{");
    let _ = writeln!(s, "    int q = takt_sched_slot(o);");
    let _ = writeln!(s, "    if (q < 0) return {overflow};");
    // 7.5, 9.8: `T <= now + guard(o)` ist ein `TimingFault`.
    let _ = writeln!(s, "    if (t <= a->tick * {}LL + g_guard[q]) return {timing};", p.config.tick);
    // Gleiche `T`: die spaetere Anweisung gewinnt (9.8).
    let _ = writeln!(s, "    for (int i = 0; i < a->sched_n[q]; i++)");
    let _ = writeln!(s, "        if (a->sched[q][i].t == t) {{ a->sched[q][i].v = v; return 0; }}");
    let _ = writeln!(s, "    if (a->sched_n[q] >= TAKT_K_O) return {overflow};");
    let _ = writeln!(s, "    a->sched[q][a->sched_n[q]].t = t;");
    let _ = writeln!(s, "    a->sched[q][a->sched_n[q]].v = v;");
    let _ = writeln!(s, "    a->sched_n[q]++;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    let _ = writeln!(
        s,
        "void {x}_cancel(struct {x}_arena *a, int o) {{ int q = takt_sched_slot(o); if (q >= 0) a->sched_n[q] = 0; }}"
    );
    // 9.9: Schlaf nur, wenn alle `sched[o]` leer sind.
    let _ = writeln!(s, "static _Bool takt_sched_pending(struct {x}_arena *a) {{");
    let _ = writeln!(s, "    for (int q = 0; q < {n}; q++) if (a->sched_n[q]) return 1;");
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");

    // `apply_scheduled(k)`: Was faellig ist, geht in den Latch. Sind
    // mehrere faellig, gewinnt der spaeteste Zeitpunkt (9.8).
    let _ = writeln!(s, "static void takt_apply_scheduled(struct {x}_arena *a, long long now) {{");
    let _ = writeln!(s, "    for (int q = 0; q < {n}; q++) {{");
    let _ = writeln!(s, "        long long best_t = -1; long long best_v = 0; int hit = 0;");
    let _ = writeln!(s, "        int k = 0;");
    let _ = writeln!(s, "        for (int i = 0; i < a->sched_n[q]; i++) {{");
    let _ = writeln!(s, "            if (a->sched[q][i].t <= now) {{");
    let _ = writeln!(s, "                if (!hit || a->sched[q][i].t > best_t) {{");
    let _ = writeln!(s, "                    best_t = a->sched[q][i].t; best_v = a->sched[q][i].v; hit = 1;");
    let _ = writeln!(s, "                }}");
    let _ = writeln!(s, "            }} else {{");
    let _ = writeln!(s, "                a->sched[q][k++] = a->sched[q][i];");
    let _ = writeln!(s, "            }}");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "        a->sched_n[q] = k;");
    let _ = writeln!(s, "        if (!hit) continue;");
    let _ = writeln!(s, "        switch (q) {{");
    for (q, c) in queues.iter().enumerate() {
        let name = &p.channels[c.index()].name;
        let Some(slot) = layout.outputs.iter().find(|slot| slot.name == *name) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        // Der Wert kam als `i64` an; im Latch steht er in seinem Typ.
        // Ein `double` traegt dieselben Bits, eine Ganzzahl wird
        // verengt — beides genau die Umkehrung von `at` im Codegen.
        let back = if slot.ty.is_float() {
            format!("*({ct} *)(a->latch + {}) = ({ct})(*(double *)&best_v);", slot.offset)
        } else {
            format!("*({ct} *)(a->latch + {}) = ({ct})best_v;", slot.offset)
        };
        let _ = writeln!(s, "        case {q}: {back} break; /* {name} */");
    }
    let _ = writeln!(s, "        default: break;");
    let _ = writeln!(s, "        }}");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}\n");
}

/// Die Outputs mit `sched`-Warteschlange in der Reihenfolge ihrer Plaetze:
/// je Maschine ausser Vorlagen, was `at`, `pulse` und `cancel` treffen
/// (`Layout::output_queues`).
pub fn queued_outputs(p: &Program) -> Vec<takt_mir::ChannelId> {
    p.machines
        .iter()
        .filter(|m| m.kind != takt_mir::machine::MachineKind::Template)
        .flat_map(|m| m.layout.output_queues.iter().copied())
        .collect()
}

/// `raised[m]` (5.4): ein Abort, den `abort` in diesem Tick erhoben hat;
/// `pending[m]` (5.4, 9.6): ein Fault von aussen — Operator-Abort,
/// Runtime-Fault —, zugestellt zu Beginn des naechsten Schritts oder, wenn
/// die Maschine nicht aktiv ist, in der Abort-Phase. Ein Abort verdraengt
/// einen Runtime-Fault, nicht umgekehrt.
pub fn raised(t: &mut Text, p: &Program, x: &Prefix) {
    let n = p.machines.len().max(1);
    let abort = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Abort);
    let _ = writeln!(t.fields, "    _Bool raised[{n}];");
    let _ = writeln!(t.fields, "    int pending[{n}];");
    let s = &mut t.code;
    let _ = writeln!(s, "static void takt_pend(struct {x}_arena *a, int m, int code) {{");
    let _ = writeln!(s, "    if (a->pending[m] != {abort}) a->pending[m] = code;");
    let _ = writeln!(s, "}}");
}

/// Die Abort-Phase (5.4, 9.4): Nach den Schritten nimmt jede Maschine mit
/// vorgemerktem Abort ihren Fault-Pfad, in statischer Reihenfolge und
/// unabhaengig davon, ob sie in diesem Tick aktiv war.
pub fn abort_phase(
    s: &mut String,
    p: &Program,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    tick: &str,
    x: &Prefix,
) {
    let abort = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::Abort);
    let overflow = takt_llvm::abi::fault_code(takt_mir::machine::FaultKind::StreamOverflow);
    let scoped = scoped_of(p);
    for m in driven {
        let Some(i) = p.machines.iter().position(|x| x.name == m.name) else { continue };
        let scope = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, n)) => format!(" && a->scope_{owner}_{n}"),
            None => String::new(),
        };
        // 9.3, 9.4: Eine Maschine in `FAULTED` nimmt keinen Fault; ein
        // vorgemerkter wartet, bis sie `FAULTED` verlaesst. Der Abort-Phase
        // gehoeren nur Abort und Runtime; ein `StreamOverflow` wirkt erst bei
        // der naechsten Aktivierung (8.6).
        let live = match in_faulted(p, m) {
            Some(test) => format!(" && !({test})"),
            None => String::new(),
        };
        let (condition, pending) = (
            format!("a->raised[{i}]{scope}{live}"),
            format!("a->pending[{i}] && a->pending[{i}] != {overflow}{scope}{live}"),
        );
        let active = match (m.period.max(1), m.phase) {
            (1, _) => "1".to_string(),
            (per, ph) => format!("{tick} % {per} == {ph}"),
        };
        let _ = writeln!(
            s,
            "{indent}if ({condition}) {{ {x}_{0}_deliver(a, {abort}, {active}); {x}_{0}_publish(a); }}",
            m.name
        );
        // Ein vorgemerkter Fault einer Maschine, die in diesem Tick nicht
        // schritt; ein Abort aus `raised` geht vor, der Fault wartet.
        let _ = writeln!(
            s,
            "{indent}else if ({pending}) {{ {x}_{0}_deliver(a, a->pending[{i}], {active}); a->pending[{i}] = 0; {x}_{0}_publish(a); }}",
            m.name
        );
    }
    let _ = writeln!(s, "{indent}memset(a->raised, 0, sizeof a->raised);");
}

/// Der Verwurf im `idle` (5.10, 9.6 `advance_cursors`) nach der
/// Abort-Phase, fuer jede Maschine, die etwas zu verwerfen hat. Eine
/// inaktive gescopte Instanz hoert ohnehin nicht, und ihr Zustand teilt
/// sich den Speicher mit ihren Geschwistern (11.2).
pub fn idle_drops(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str, x: &Prefix) {
    let scoped = scoped_of(p);
    for m in driven.iter().filter(|m| takt_llvm::step::drops(m, p)) {
        let scope = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, n)) => format!("if (a->scope_{owner}_{n}) "),
            None => String::new(),
        };
        let _ = writeln!(s, "{indent}{scope}{x}_{0}_drop(a);", m.name);
    }
}

/// Die Flankentabelle der Alerts (5.6: „die Runtime protokolliert
/// Flanken"): ein Platz je Maschine, Alert-Stelle und Durchlauf der
/// umgebenden Schleifen, wie `alert_edge` im Interpreter; geschrieben
/// wird nur, was sich aendert.
pub fn alert_table(t: &mut Text, p: &Program, x: &Prefix) {
    let mut bases = Vec::with_capacity(p.machines.len());
    let mut total = 0u32;
    for m in &p.machines {
        bases.push(total.to_string());
        if m.kind != takt_mir::machine::MachineKind::Template {
            total = total.saturating_add(takt_llvm::machine::counters(m, p).alert_slots());
        }
    }
    let _ = writeln!(t.fields, "    _Bool alert[{}];", total.max(1));
    let s = &mut t.code;
    let _ = writeln!(s, "static const int g_alert_base[{}] = {{ {} }};", bases.len().max(1), bases.join(", "));
    let _ = writeln!(s, "static _Bool takt_alert_edge(struct {x}_arena *a, int m, int slot, unsigned char on) {{");
    let _ = writeln!(s, "    _Bool *was = &a->alert[g_alert_base[m] + slot];");
    let _ = writeln!(s, "    if (*was == (on != 0)) return 0;");
    let _ = writeln!(s, "    *was = on != 0;");
    let _ = writeln!(s, "    return 1;");
    let _ = writeln!(s, "}}");
}

/// Die Namen, mit denen die Rahmen einen Fault schreiben: die Maschine
/// und die Art wie im Trace des Interpreters (`FaultKind::name`), die Art
/// nach ihrer Zahl in der ABI (`abi::fault_code`).
pub fn fault_names(s: &mut String, p: &Program) {
    let names: Vec<String> = p.machines.iter().map(|m| format!("\"{}\"", m.name)).collect();
    let _ = writeln!(s, "static const char *const takt_machine_names[{}] = {{ {} }};", names.len().max(1), {
        if names.is_empty() { "\"?\"".to_string() } else { names.join(", ") }
    });
    let _ = writeln!(s, "static const char *takt_machine_name(int m) {{");
    let _ = writeln!(s, "    return m >= 0 && m < {} ? takt_machine_names[m] : \"?\";", names.len());
    let _ = writeln!(s, "}}");
    let _ = writeln!(s, "static const char *takt_fault_name(int code) {{");
    let _ = writeln!(s, "    switch (code) {{");
    for kind in takt_mir::machine::FaultKind::all() {
        let _ = writeln!(s, "    case {}: return \"{}\";", takt_llvm::abi::fault_code(kind), kind.name());
    }
    let _ = writeln!(s, "    default: return \"?\";");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// Speist die `sim`-Outputs in die `hw`-Inputs derselben Adresse (8.3).
///
/// Ein Plant-Modell schreibt `output p_sim : float[bar] @ sim("daq1/ai0")`,
/// das Programm liest `input p : float[bar] @ hw("daq1/ai0")`. Die
/// Adresse ist die Naht, und der Interpreter zieht sie in
/// `Image::apply_sim_bindings`. Hier steht dieselbe Naht in C.
///
/// Der Wert geht aus dem Latch in den Wertteil des Abbild-Eintrags, und
/// die Qualitaet wird `Good` (0): Der Eingang hat eine Quelle, also ist
/// er nicht mehr `Bad` (3.5). Ohne das bliebe er `Bad`, und jeder
/// Lesezugriff faultete.
pub fn sim_bindings(s: &mut String, p: &Program, indent: &str) {
    use takt_mir::program::{Binding, Build, Direction};
    // Im Hardware-Build speisen die Treiber die Eingaenge (8.3).
    if p.config.build == Build::Hw {
        return;
    }
    let adresse = |b: &Binding| match b {
        Binding::Hw(a) | Binding::Sim(a) => Some(a.clone()),
        Binding::None => None,
    };
    for (i, out) in p.channels.iter().enumerate() {
        if out.dir != Direction::Output {
            continue;
        }
        let Binding::Sim(_) = &out.binding else { continue };
        let Some(addr) = adresse(&out.binding) else { continue };
        // Der Eingang an derselben Adresse; ein Strom wird gesendet, nicht
        // gestellt (8.8) und bleibt hier aussen vor.
        let Some((j, inp)) = p
            .channels
            .iter()
            .enumerate()
            .find(|(_, c)| c.dir == Direction::Input && matches!(&c.binding, Binding::Hw(a) if *a == addr))
        else {
            continue;
        };
        if matches!(p.types.list.get(inp.ty.index()), Some(takt_mir::types::Type::Stream(_))) {
            continue;
        }
        let from = takt_mir::ChannelId(i as u32);
        let to = takt_mir::ChannelId(j as u32);
        let (Some(src), Some(dst)) = (takt_llvm::image::latch_offset(from, p), takt_llvm::image::offset_of(to, p))
        else {
            continue;
        };
        let Some(size) = takt_llvm::ty::lower(inp.ty, p).map(|t| t.size()) else { continue };
        let _ = writeln!(
            s,
            "{indent}memcpy(a->image + {dst}, a->latch + {src}, {size}); /* {} -> {} */",
            out.name, inp.name
        );
        // Qualitaet `Good` (3.5): Der Eingang hat jetzt eine Quelle — sofern
        // der Wert in der deklarierten Range liegt; sonst `Bad` (12.6). Der
        // Interpreter prueft am Rand auch `max_slew` und `debounce`; das
        // bleibt hier aussen vor (LIMITS).
        let Some(q) = quality_offset(p, &inp.name) else { continue };
        if let Some(age) = age_offset(p, &inp.name) {
            let _ = writeln!(s, "{indent}*(long long *)(a->image + {age}) = 0;");
        }
        match range_check(p, inp.ty) {
            Some((ct, lo, hi)) => {
                let _ = writeln!(
                    s,
                    "{indent}{{ {ct} v = *({ct} *)(a->image + {dst}); a->image[{q}] = (v < {lo} || v > {hi}) ? 3 : 0; }}"
                );
            }
            None => {
                let _ = writeln!(s, "{indent}a->image[{q}] = 0;");
            }
        }
    }
}

/// C-Typ und Grenzen eines Skalars mit deklarierter Range (3.4).
fn range_check(p: &Program, ty: takt_mir::TypeId) -> Option<(&'static str, String, String)> {
    use takt_llvm::ty::LlvmType;
    use takt_mir::types::{Const, FloatWidth, Type};
    let literal = |c: &Const| match c {
        Const::Int(i) | Const::Duration(i) => format!("{i}LL"),
        Const::Float(f) => format!("{f:?}"),
        Const::Bool(b) => u8::from(*b).to_string(),
    };
    let (ct, r) = match p.types.list.get(ty.index())? {
        Type::Int { width, range: Some(r), .. } => {
            (crate::layout::c_type(&LlvmType::Int(width.bits()), width.signed())?, r)
        }
        Type::Float { width, range: Some(r), .. } => {
            let t = if *width == FloatWidth::F32 { LlvmType::F32 } else { LlvmType::F64 };
            (crate::layout::c_type(&t, true)?, r)
        }
        _ => return None,
    };
    Some((ct, literal(&r.lo), literal(&r.hi)))
}

/// Die `hw`-Eingaenge, die ein `sim`-Output derselben Adresse speist (8.3):
/// Im Sim-Build ist das Modell ihre Quelle, kein Treiber; im Hardware-Build
/// keiner, dort hat jeder Eingang seinen Treiber.
pub fn sim_fed_inputs(p: &Program) -> Vec<usize> {
    use takt_mir::program::{Binding, Build, Direction};
    if p.config.build == Build::Hw {
        return Vec::new();
    }
    p.channels
        .iter()
        .enumerate()
        .filter(|(_, c)| c.dir == Direction::Input)
        .filter(|(_, c)| {
            let Binding::Hw(addr) = &c.binding else { return false };
            p.channels.iter().any(|o| o.dir == Direction::Output && matches!(&o.binding, Binding::Sim(a) if a == addr))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Die Einstiege des erzeugten Codes je Maschine (12.11), fuer beide
/// Rahmen: aus derselben Liste, aus der der Codegen sie schreibt
/// ([`takt_llvm::arena::entries`]).
pub fn machine_declarations(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], x: &Prefix) {
    for m in driven {
        for (suffix, shape) in takt_llvm::arena::entries(m, p) {
            let arena = format!("struct {x}_arena *a");
            let params: Vec<&str> =
                std::iter::once(arena.as_str()).chain(shape.extra.iter().map(|t| c_of(t))).collect();
            let symbol = takt_llvm::arena::entry_symbol(x, &m.name, &suffix);
            let _ = writeln!(s, "{} {symbol}({});", c_of(shape.ret), params.join(", "));
        }
    }
}

/// Der C-Typ zu einem LLVM-Typ der Einstiege.
fn c_of(llvm: &str) -> &'static str {
    match llvm {
        "i1" => "_Bool",
        "i32" => "int",
        "i64" => "long long",
        "ptr" => "void *",
        "ptr readonly" => "const void *",
        _ => "void",
    }
}

/// 5.11: je gescopter Instanz, ob sie zu Beginn des vorigen Ticks aktiv
/// war — der Vergleich liefert Ein- und Austritt.
pub fn scope_flags(t: &mut Text, p: &Program) {
    let scoped = scoped_of(p);
    for (owner, _, i) in &scoped {
        let _ = writeln!(t.fields, "    _Bool scope_{owner}_{i};");
    }
    // 5.11: Der Tick (plus eins) des letzten Fault-Uebergangs je Maschine;
    // ein Besitzer, der seinen Zustand so verliess, fuehrt die `exit:`-Bloecke
    // seiner Instanzen nicht aus (`note_fault`, `scoped_lifecycle`).
    if !scoped.is_empty() {
        let _ = writeln!(t.fields, "    long long fault_tick[{}];", p.machines.len().max(1));
    }
}

/// Merkt im Fault-Rueckruf eines Rahmens (`P_fault`) den Tick des
/// Fault-Uebergangs der Maschine `m` (5.11): `scoped_lifecycle` laesst
/// danach die `exit:`-Bloecke ihrer Instanzen aus, wie der Interpreter
/// (`leave_scoped` mit `faulted`). Ohne gescopte Instanzen nichts.
pub fn note_fault(s: &mut String, p: &Program, indent: &str) {
    if !scoped_of(p).is_empty() {
        let _ = writeln!(s, "{indent}a->fault_tick[m] = a->tick + 1;");
    }
}

/// Die Eintritte vor dem ersten Tick, in Schrittordnung und nach jedem
/// `publish`, damit Follower schon im Tick 0 frisch lesen (7.2, 9.4).
/// Eine gescopte Instanz betritt nichts, solange ihr Scope nicht steht;
/// `scoped_lifecycle` nach dem `enter` des Besitzers holt sie herein
/// (5.11).
pub fn enter_machines(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    x: &Prefix,
) {
    let scoped: Vec<String> = scoped_of(p).into_iter().map(|(_, inst, _)| inst).collect();
    for m in driven.iter().filter(|m| !scoped.contains(&m.name)) {
        let _ = writeln!(s, "{indent}{x}_{0}_enter(a);", m.name);
        let _ = writeln!(s, "{indent}{x}_{0}_publish(a);", m.name);
    }
    scoped_lifecycle(s, p, layout, indent, x);
}

/// Die Schritte eines Ticks, fuer beide Rahmen: erst die Trigger-Phase
/// (7.5, wie im Interpreter zwischen Zustellung und Schritt), dann jede
/// Maschine in Schrittordnung — in jedem `period`-ten Tick mit ihrer
/// Phase (7.2), eine gescopte Instanz nur, solange ihr Scope zu
/// Tick-Beginn steht —, zuletzt der Lebenszyklus der gescopten Instanzen
/// (5.11). `tick` ist der Ausdruck der Tickzahl.
pub fn steps(
    s: &mut String,
    p: &Program,
    layout: &Layout,
    driven: &[&takt_mir::machine::Machine],
    indent: &str,
    tick: &str,
    x: &Prefix,
) {
    // 9.6 `deliver(D_k)`: Was der vorige Tick an Stroeme mit `drop_oldest`
    // gesendet hat, wird vor der Trigger-Phase sichtbar (7.5).
    if crate::streams::stages(p) {
        let _ = writeln!(s, "{indent}takt_int_deliver_sent(a);");
    }
    for m in driven {
        if !m.layout.trigger_flags.is_empty() {
            let _ = writeln!(s, "{indent}{x}_{0}_triggers(a);", m.name);
        }
    }
    let scoped = scoped_of(p);
    for m in driven {
        let condition = match (m.period.max(1), m.phase) {
            (1, _) => String::new(),
            (per, 0) => format!("if ({tick} % {per} == 0) "),
            (per, ph) => format!("if ({tick} % {per} == {ph}) "),
        };
        let condition = match scoped.iter().find(|(_, inst, _)| *inst == m.name) {
            Some((owner, _, i)) if condition.is_empty() => format!("if (a->scope_{owner}_{i}) "),
            Some((owner, _, i)) => format!("{} if (a->scope_{owner}_{i}) ", condition.trim_end()),
            None => condition,
        };
        // 9.6: Ein vorgemerkter Fault geht zu Beginn des Schritts in den
        // Zustand; der Schritt nimmt ihn statt seines Rumpfs. In `FAULTED`
        // wartet er, bis die Maschine es verlaesst (9.3).
        let i = p.machines.iter().position(|x| x.name == m.name).unwrap_or(0);
        let live = match in_faulted(p, m) {
            Some(test) => format!(" && !({test})"),
            None => String::new(),
        };
        let _ = writeln!(
            s,
            "{indent}{condition}{{ if (a->pending[{i}]{live}) {{ {x}_{0}_pend(a, a->pending[{i}]); a->pending[{i}] = 0; }} {x}_{0}_step(a); {x}_{0}_publish(a); }}",
            m.name
        );
    }
    scoped_lifecycle(s, p, layout, indent, x);
}

/// Die gescopten Instanzen mit ihrem Besitzer und der Nummer, unter der
/// der Codegen das Praedikat `<besitzer>_scope_<n>` erzeugt (5.11).
pub fn scoped_of(p: &Program) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    for owner in &p.machines {
        for (i, si) in owner.states.iter().flat_map(|s| s.instances.iter()).enumerate() {
            out.push((owner.name.clone(), p.machines[si.machine.index()].name.clone(), i));
        }
    }
    out
}

/// Der Lebenszyklus der gescopten Instanzen nach dem Schritt des
/// Besitzers (5.11): Eintritt initialisiert frisch und betritt `initial`,
/// Austritt setzt die Outputs auf `safe` und verwirft den Zustand.
///
/// Das Aktivitaetsbit steht im Rahmen, nicht im Zustands-Struct: Es ist
/// eine Aussage ueber den *Besitzer*, und der Rahmen fragt sie ohnehin
/// vor jedem Schritt ab (`<besitzer>_scope_<n>`).
pub fn scoped_lifecycle(s: &mut String, p: &Program, layout: &Layout, indent: &str, x: &Prefix) {
    for (owner, inst, i) in scoped_of(p) {
        let state = takt_llvm::arena::state_name(&inst);
        let by_fault = left_by_fault(p, &owner);
        let _ = writeln!(s, "{indent}{{ _Bool now = {x}_{owner}_scope_{i}(a);");
        let _ = writeln!(s, "{indent}  if (now && !a->scope_{owner}_{i}) {{");
        let _ = writeln!(s, "{indent}    memset(a->{state}, 0, sizeof a->{state});");
        let _ = writeln!(s, "{indent}    {x}_{inst}_init_vars(a);");
        let _ = writeln!(s, "{indent}    {x}_{inst}_enter(a);");
        let _ = writeln!(s, "{indent}    {x}_{inst}_publish(a);");
        let _ = writeln!(s, "{indent}  }} else if (!now && a->scope_{owner}_{i}) {{");
        // 5.11: erst die `exit:`-Bloecke von innen nach aussen, dann
        // gehen die Outputs auf `safe` — sie ueberschreiben, was ein
        // `exit` an ihnen tat, genau wie im Interpreter. Verliess der
        // Besitzer den Zustand ueber einen Fault, laufen keine `exit:`.
        let _ = writeln!(s, "{indent}    if (!({by_fault})) {x}_{inst}_exit_all(a);");
        safe_outputs_of(s, p, layout, &inst, &format!("{indent}    "));
        let _ = writeln!(s, "{indent}    memset(a->{state}, 0, sizeof a->{state});");
        let _ = writeln!(s, "{indent}    {x}_{inst}_publish(a);");
        let _ = writeln!(s, "{indent}  }}");
        let _ = writeln!(s, "{indent}  a->scope_{owner}_{i} = now; }}");
    }
}

/// Hat der Besitzer seinen Zustand ueber einen Fault verlassen (5.11)? In
/// diesem Tick mit einem Fault-Uebergang (`note_fault`), oder er steht in
/// `FAULTED` — hinter dem letzten Blatt (5.3). Dieselbe Frage wie
/// `faulted || last_fault.tick == tick` im Interpreter.
fn left_by_fault(p: &Program, owner: &str) -> String {
    let Some((i, m)) = p.machines.iter().enumerate().find(|(_, m)| m.name == owner) else { return "0".into() };
    let mut test = format!("a->fault_tick[{i}] == a->tick + 1");
    if let Some(faulted) = in_faulted(p, m) {
        let _ = write!(test, " || {faulted}");
    }
    test
}

/// Steht die Maschine in `FAULTED` (5.3)? Das Blatt steht dann hinter dem
/// letzten (`conf[0]`, wie der Schritt es liest); `None` ohne Zustand.
fn in_faulted(p: &Program, m: &takt_mir::machine::Machine) -> Option<String> {
    let at = takt_llvm::machine::state_struct(m, p)?.byte_offset(takt_llvm::machine::Role::Conf, 0)?;
    let leaves = takt_llvm::machine::leaves(m).len();
    Some(format!("a->{}[{at}] >= {leaves}", takt_llvm::arena::state_name(&m.name)))
}

/// Die Outputs einer Maschine auf ihren `safe`-Wert (5.11, 5.2 Regel 5).
fn safe_outputs_of(s: &mut String, p: &Program, layout: &Layout, machine: &str, indent: &str) {
    let Some(id) = p.machines.iter().position(|m| m.name == machine).map(|i| takt_mir::MachineId(i as u32)) else {
        return;
    };
    for slot in &layout.outputs {
        let Some(c) = p.channels.iter().find(|c| c.name == slot.name) else { continue };
        if c.owner != Some(id) {
            continue;
        }
        let Some(safe) = c.attrs.safe.as_ref().and_then(|e| literal(p, e)) else { continue };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ =
            writeln!(s, "{indent}*({ct} *)(a->latch + {}) = {safe}; /* {} auf safe (5.11) */", slot.offset, slot.name);
    }
}

/// Ψ_{k+1} wird Ψ_k (9.4): die zweite Bank in die erste kopieren, dann
/// `fresh` und die Signale loeschen — ein Signal ist einen Tick sichtbar
/// (5.8), und ohne `fresh` liest ein Follower wieder Ψ_k (7.2).
/// Der Commit eines Ticks, fuer beide Rahmen und fuer Tick 0 dieselbe
/// Folge (12.1): erst die geplanten Ausgaben, dann Ψ, dann die `sim`-Outputs an ihre `hw`-Inputs (8.3,
/// Unit-Delay wie bei Ψ), dann die Sendepuffer (8.8: gesendet wird beim
/// Commit, und der Treiber holt seine Rate ab, bevor der Latch
/// ausgeschrieben wird) und die internen Ringe (8.6). Zwei Fassungen
/// dieser Folge wichen einmal voneinander ab (FB-269).
pub fn commit_sequence(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str, tick: &str) {
    // 9.8, 12.1: Was in diesem Tick faellig wird, geht nach den Schritten
    // in den Latch, vor dem Commit — ein geplanter Wert gewinnt gegen eine
    // Zuweisung desselben Ticks, und die `sim`-Bindung sieht ihn. Vor Ψ:
    // Eine andere Maschine liest den Ausgang im naechsten Tick so, wie er
    // committet wurde (`committed_output` im Interpreter), samt Plan.
    if !queued_outputs(p).is_empty() {
        let _ = writeln!(s, "{indent}takt_apply_scheduled(a, {tick} * {}LL);", p.config.tick);
    }
    psi_commit(s, p, driven, indent);
    sim_bindings(s, p, indent);
    let _ = writeln!(s, "{indent}takt_tx_commit(a, {tick});");
    let _ = writeln!(s, "{indent}takt_int_commit(a);");
}

/// Der Ψ-Tausch nach dem Commit (8.3, 11.2): Was eine Maschine ausgab,
/// steht ab dem naechsten Tick in der Bank, aus der die anderen lesen;
/// `fresh` und die Signale der getriebenen Maschinen beginnen leer.
pub fn psi_commit(s: &mut String, p: &Program, driven: &[&takt_mir::machine::Machine], indent: &str) {
    use takt_llvm::psi::{Field, bank_size, field_offset, region_offset};
    let Some(first) = region_offset(takt_mir::MachineId(0), false, p) else { return };
    let Some(next) = region_offset(takt_mir::MachineId(0), true, p) else { return };
    // 8.3: Was eine Maschine ausgibt, lesen die anderen im naechsten Tick
    // aus ihrer Ψ-Bank.
    for (i, c) in p.channels.iter().enumerate() {
        let id = takt_mir::ChannelId(i as u32);
        let (Some(owner), Some(src)) = (c.owner, takt_llvm::image::latch_offset(id, p)) else { continue };
        let (Some(bank), Some(off), Some(ty)) =
            (region_offset(owner, true, p), field_offset(owner, Field::Output(id), p), takt_llvm::ty::lower(c.ty, p))
        else {
            continue;
        };
        let dst = bank + off;
        // Ein Skalar als Zuweisung: `memcpy` unbekannter Ausrichtung wird auf RV32 ein Aufruf je Byte.
        let _ = match c_type(&ty, false) {
            Some(ct) => {
                writeln!(
                    s,
                    "{indent}*({ct} *)(a->image + {dst}) = *(const {ct} *)(a->latch + {src}); /* Psi {} */",
                    c.name
                )
            }
            None => {
                writeln!(s, "{indent}memcpy(a->image + {dst}, a->latch + {src}, {}); /* Psi {} */", ty.size(), c.name)
            }
        };
    }
    // Bank und Regionen sind 8-ausgerichtet (psi.rs); die Byte-Schleife kostete auf RV32 rund 10 us je Tick.
    let _ = writeln!(
        s,
        "{indent}for (unsigned i = 0; i < {}; i++) ((unsigned long long *)(a->image + {first}))[i] = ((const unsigned long long *)(a->image + {next}))[i]; /* Psi */",
        bank_size(p) / 8
    );
    for m in driven {
        let Some(id) = p.machines.iter().position(|x| x.name == m.name) else { continue };
        let id = takt_mir::MachineId(id as u32);
        let Some(base) = region_offset(id, true, p) else { continue };
        let _ = writeln!(s, "{indent}a->image[{base}] = 0; /* fresh {} */", m.name);
        for i in 0..m.signals.len() {
            if let Some(off) = field_offset(id, Field::Signal(takt_mir::SignalId(i as u32)), p) {
                let _ =
                    writeln!(s, "{indent}a->image[{}] = 0; /* Signal {}.{} */", base + off, m.name, m.signals[i].name);
            }
        }
    }
}

/// Der Vorgabewert des Parameters `index` als C-Literal.
pub fn param_literal(p: &Program, index: usize) -> Option<String> {
    // 8.4: Das beim Bau gewaehlte Profil gilt, sonst der Default.
    let chosen = p.config.params_profile.as_deref().and_then(|name| p.profiles.iter().find(|pr| pr.name == name));
    let assigned = chosen.and_then(|pr| pr.assignments.iter().find(|(id, _)| id.index() == index)).map(|(_, e)| e);
    literal(p, assigned.unwrap_or(&p.params.get(index)?.default))
}

/// Ein Tunable, wie der Rahmen es nimmt und in den Trace schreibt (8.4).
struct Tunable<'a> {
    /// Der Index in `Program::params`, der Parameter der Schleife.
    index: usize,
    name: &'a str,
    /// Der C-Typ im Parameterspeicher und sein Versatz.
    ct: &'static str,
    offset: u64,
    /// Die Laenge der kanonischen Byteform (5.9).
    bytes: u64,
    kind: TuneKind,
}

/// Was ein Tunable ist: wie der Rahmen den Wert prueft und schreibt.
enum TuneKind {
    Bool,
    /// Eine Ganzzahl mit ihrer Range; `wide` ist ein `u64` jenseits von `i64`.
    Int {
        check: String,
        unit: Option<String>,
        wide: bool,
    },
    /// Eine endliche Zahl in ihrer Range.
    Float {
        check: String,
        unit: Option<String>,
    },
    Duration {
        check: String,
    },
    /// Ein Enum ohne Felder: die Diskriminante als `i64` (5.9).
    Enum(Vec<(i64, String)>),
}

/// Die Tunables des Programms, die der Rahmen nehmen kann.
fn tunables<'a>(p: &'a Program, layout: &Layout) -> Vec<Tunable<'a>> {
    use takt_mir::types::{Const, FloatWidth, Type};
    let bound = |c: &Const| match c {
        Const::Int(i) | Const::Duration(i) => format!("{i}LL"),
        Const::Float(f) => format!("{f:?}"),
        Const::Bool(b) => u8::from(*b).to_string(),
    };
    let range = |r: &Option<takt_mir::types::Range>| {
        r.as_ref().map_or_else(|| "1".into(), |r| format!("x >= {} && x <= {}", bound(&r.lo), bound(&r.hi)))
    };
    let unit = |u: &Option<takt_mir::UnitId>| u.map(|u| p.units[u.index()].name.clone());
    let mut out = Vec::new();
    for (index, param) in p.params.iter().enumerate() {
        let Some(slot) = layout.parameters.iter().find(|sl| sl.name == param.name).filter(|_| param.tunable) else {
            continue;
        };
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let (bytes, kind) = match p.types.list.get(param.ty.index()) {
            Some(Type::Bool) => (1, TuneKind::Bool),
            Some(Type::Int { width, range: r, unit: u }) => (
                u64::from(width.bits() / 8),
                TuneKind::Int { check: range(r), unit: unit(u), wide: !width.signed() && width.bits() == 64 },
            ),
            Some(Type::Float { width, range: r, unit: u }) => {
                let finite = "x == x && x - x == 0";
                let check = match r {
                    Some(r) => format!("{finite} && x >= {} && x <= {}", bound(&r.lo), bound(&r.hi)),
                    None => finite.to_string(),
                };
                (if *width == FloatWidth::F32 { 4 } else { 8 }, TuneKind::Float { check, unit: unit(u) })
            }
            Some(Type::Duration { range: r }) => (8, TuneKind::Duration { check: range(r) }),
            Some(Type::Enum(e)) => {
                let Some(def) = p.enums.get(e.index()).filter(|d| d.variants.iter().all(|v| v.fields.is_empty()))
                else {
                    continue;
                };
                (8, TuneKind::Enum(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect()))
            }
            _ => continue,
        };
        out.push(Tunable { index, name: &param.name, ct, offset: slot.offset, bytes, kind });
    }
    out
}

/// Der C-Typ ohne Vorzeichen, in dem `bytes` kanonische Bytes stehen.
fn unsigned_of(bytes: u64) -> &'static str {
    match bytes {
        1 => "uint8_t",
        2 => "uint16_t",
        4 => "uint32_t",
        _ => "uint64_t",
    }
}

/// `takt_tune_value(a, param, value, len)`: ein Tunable aendert sich (8.4),
/// fuer beide Rahmen. `value` ist die kanonische Byteform (5.9); angenommen
/// wird nur ein `tunable param` mit passender Laenge, endlicher Zahl,
/// bekannter Variante und Wert in seiner Range, wie `run.rs` im Interpreter
/// — sonst bleibt der Wert, und die Rueckgabe ist 0 (`rejected`). Der Wert
/// gilt ab dem naechsten Schritt; die Schleife ruft vor ihm
/// (`Runtime::service_with`).
pub fn tune(s: &mut String, p: &Program, layout: &Layout, x: &Prefix) {
    let _ = writeln!(
        s,
        "static int32_t takt_tune_value(struct {x}_arena *a, uint32_t param, const unsigned char *v, int32_t len) {{"
    );
    let _ = writeln!(s, "    unsigned long long raw = 0;");
    let _ = writeln!(s, "    if (len < 1 || len > 8) return 0;");
    let _ = writeln!(s, "    for (int i = 0; i < len; i++) raw |= (unsigned long long)v[i] << (8 * i);");
    let _ = writeln!(s, "    switch (param) {{");
    for t in tunables(p, layout) {
        let (ct, off) = (t.ct, t.offset);
        let _ = writeln!(s, "    case {}: {{ /* {} */", t.index, t.name);
        let check = match &t.kind {
            TuneKind::Enum(variants) => {
                let known: Vec<String> = variants.iter().map(|(d, _)| format!("d == {d}LL")).collect();
                let _ = writeln!(s, "        long long d;");
                let _ = writeln!(s, "        if (len != 8) return 0;");
                let _ = writeln!(s, "        memcpy(&d, &raw, 8);");
                let _ = writeln!(s, "        if (!({})) return 0;", known.join(" || "));
                let _ = writeln!(s, "        *({ct} *)(a->params + {off}) = ({ct})d;");
                let _ = writeln!(s, "        return 1;");
                let _ = writeln!(s, "    }}");
                continue;
            }
            TuneKind::Bool => "x <= 1",
            TuneKind::Int { check, .. } | TuneKind::Float { check, .. } | TuneKind::Duration { check } => check,
        };
        let (unsigned, bytes) = (unsigned_of(t.bytes), t.bytes);
        let _ = writeln!(s, "        {unsigned} u = ({unsigned})raw;");
        let _ = writeln!(s, "        {ct} x;");
        let _ = writeln!(s, "        if (len != {bytes}) return 0;");
        let _ = writeln!(s, "        memcpy(&x, &u, {bytes});");
        let _ = writeln!(s, "        if (!({check})) return 0;");
        let _ = writeln!(s, "        *({ct} *)(a->params + {off}) = x;");
        let _ = writeln!(s, "        return 1;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    default: (void)a; (void)raw; return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}

/// `takt_tune_trace(k, param, value, len, accepted)`: die Zeile `t=<k> tune
/// <name> <wert>` des Golden-Trace (`grammar/trace.md`), eine verworfene
/// mit ` rejected` — in der Form, die der Interpreter als Stimulus liest
/// (12.5). Ein Wert, der keinem Tunable gehoert oder nicht seine Laenge
/// hat, ist keine Tune-Zeile und steht nicht im Trace. Jedes Stueck endet
/// wie `takt_board_trace_i64` mit einem Leerzeichen; `DURATION_C` steht davor.
pub fn tune_trace(s: &mut String, p: &Program, layout: &Layout) {
    let _ = writeln!(
        s,
        "static void takt_tune_trace(int64_t k, uint32_t param, const unsigned char *v, int32_t len, int32_t accepted) {{"
    );
    let _ = writeln!(s, "    unsigned long long raw = 0;");
    let _ = writeln!(s, "    if (len < 1 || len > 8) return;");
    let _ = writeln!(s, "    for (int i = 0; i < len; i++) raw |= (unsigned long long)v[i] << (8 * i);");
    let _ = writeln!(s, "    switch (param) {{");
    for t in tunables(p, layout) {
        let (ct, bytes) = (t.ct, t.bytes);
        let head =
            format!("takt_board_trace(\"t=\"); takt_board_trace_i64(k); takt_board_trace(\"tune {} \");", t.name);
        let _ = writeln!(s, "    case {}: {{", t.index);
        if let TuneKind::Enum(variants) = &t.kind {
            let _ = writeln!(s, "        long long d;");
            let _ = writeln!(s, "        if (len != 8) return;");
            let _ = writeln!(s, "        memcpy(&d, &raw, 8);");
            let _ = writeln!(s, "        {head}");
            let _ = writeln!(s, "        switch (d) {{");
            for (d, name) in variants {
                let _ = writeln!(s, "        case {d}LL: takt_board_trace(\"{name} \"); break;");
            }
            let _ = writeln!(s, "        default: takt_board_trace_i64(d); break;");
            let _ = writeln!(s, "        }}");
            let _ = writeln!(s, "        break;");
            let _ = writeln!(s, "    }}");
            continue;
        }
        let unsigned = unsigned_of(bytes);
        let _ = writeln!(s, "        {unsigned} u = ({unsigned})raw;");
        let _ = writeln!(s, "        {ct} x;");
        let _ = writeln!(s, "        if (len != {bytes}) return;");
        let _ = writeln!(s, "        memcpy(&x, &u, {bytes});");
        let _ = writeln!(s, "        {head}");
        let (value, unit) = match &t.kind {
            TuneKind::Bool => {
                ("if (x <= 1) takt_board_trace(x ? \"true \" : \"false \"); else takt_board_trace_i64(x);", None)
            }
            TuneKind::Int { unit, wide: true, .. } => ("takt_board_trace_u64((unsigned long long)x);", unit.as_deref()),
            TuneKind::Int { unit, .. } => ("takt_board_trace_i64((long long)x);", unit.as_deref()),
            TuneKind::Float { unit, .. } => ("takt_board_trace_f64((double)x);", unit.as_deref()),
            TuneKind::Duration { .. } => (
                "takt_board_trace_i64(takt_dur_value(x)); takt_board_trace(takt_dur_unit(x)); takt_board_trace(\" \");",
                None,
            ),
            TuneKind::Enum(_) => continue,
        };
        let _ = writeln!(s, "        {value}");
        if let Some(unit) = unit {
            let _ = writeln!(s, "        takt_board_trace(\"{unit} \");");
        }
        let _ = writeln!(s, "        break;");
        let _ = writeln!(s, "    }}");
    }
    let _ = writeln!(s, "    default: (void)k; (void)raw; return;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    takt_board_trace(accepted ? \"\\n\" : \"rejected\\n\");");
    let _ = writeln!(s, "}}");
}

/// Ein Literal als C-Text; alles andere braeuchte den Interpreter.
fn literal(p: &Program, e: &takt_mir::expr::Expr) -> Option<String> {
    match &e.kind {
        takt_mir::expr::ExprKind::Int(n) => Some(n.to_string()),
        takt_mir::expr::ExprKind::Duration(d) => Some(d.to_string()),
        takt_mir::expr::ExprKind::Bool(b) => Some(u8::from(*b).to_string()),
        takt_mir::expr::ExprKind::Float(f) => Some(format!("{f:?}")),
        // Eine feldlose Variante ist ihre Diskriminante — die kann
        // explizit gesetzt sein und von der Nummer abweichen.
        takt_mir::expr::ExprKind::Variant { enum_id, variant, fields } if fields.is_empty() => {
            Some(p.enums.get(enum_id.index())?.variants.get(*variant as usize)?.discriminant.to_string())
        }
        _ => None,
    }
}

/// Der Versatz eines Feldes im Eintrag eines Channels: Wert, Qualitaet,
/// Grund, `age` (`image`).
fn entry_field(p: &Program, name: &str, field: takt_llvm::image::Slot) -> Option<u64> {
    let index = p.channels.iter().position(|c| c.name == name)?;
    let id = takt_mir::ChannelId(index as u32);
    let base = takt_llvm::image::offset_of(id, p)?;
    Some(base + takt_llvm::image::entry_type(id, p)?.field_offset(field as usize))
}

/// Der Versatz des Qualitaetsbytes eines Inputs im Abbild (3.5).
pub fn quality_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Quality)
}

/// Der Versatz des Grundes (`reason`) im Eintrag eines Channels (3.5).
pub fn reason_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Reason)
}

/// Der Versatz von `age` im Eintrag eines Channels (3.5).
pub fn age_offset(p: &Program, name: &str) -> Option<u64> {
    entry_field(p, name, takt_llvm::image::Slot::Age)
}

/// Die Abtastungen altern um einen Tick; ueber `max_age` werden sie
/// `Stale` (3.5), wie `age_inputs` im Interpreter — vor der Lieferung des
/// Ticks, die das Alter zuruecksetzt.
pub fn aging(s: &mut String, p: &Program, layout: &Layout, indent: &str) {
    age_by(s, p, layout, indent, "1", &[]);
}

/// Die Abtastungen altern ueber `n` geschlafene Ticks wie in leeren
/// Schritten (9.9, `Run::skip`): Ein Eingang ohne Lieferung ist danach so
/// alt, wie er ohne Schlaf waere. Ein Eingang, den ein `sim`-Output speist,
/// altert nicht: Der Commit jedes geschlafenen Ticks haette ihn frisch
/// gestellt (8.3).
pub fn aging_slept(s: &mut String, p: &Program, layout: &Layout, indent: &str) {
    age_by(s, p, layout, indent, "n", &sim_fed_inputs(p));
}

/// Das Altern um `ticks` Ticks, einen C-Ausdruck, ohne die Kanaele in `fed`.
fn age_by(s: &mut String, p: &Program, layout: &Layout, indent: &str, ticks: &str, fed: &[usize]) {
    let tick = p.config.tick;
    for slot in &layout.inputs {
        if p.channels.iter().position(|c| c.name == slot.name).is_some_and(|i| fed.contains(&i)) {
            continue;
        }
        let (Some(q), Some(age), Some(reason)) = (
            quality_offset(p, &slot.name),
            age_offset(p, &slot.name),
            entry_field(p, &slot.name, takt_llvm::image::Slot::Reason),
        ) else {
            continue;
        };
        let stale = match p.channels.iter().find(|c| c.name == slot.name).and_then(|c| c.attrs.max_age) {
            Some(max) => {
                format!(" if (*at > {max}LL && a->image[{q}] != 3) {{ a->image[{q}] = 2; a->image[{reason}] = 0; }}")
            }
            None => String::new(),
        };
        let _ = writeln!(
            s,
            "{indent}{{ long long *at = (long long *)(a->image + {age}); long long d = ({ticks}) > {}LL ? {max}LL : \
             ({ticks}) * {tick}LL; *at = *at > {max}LL - d ? {max}LL : *at + d;{stale} }}",
            i64::MAX / tick.max(1),
            max = i64::MAX
        );
    }
}

/// Wo `sys/next_run` im Latch steht und was jede Diskriminante verlangt
/// (12.7). Die Bedeutungen kommen aus `takt_mir::sys`, derselben Tabelle,
/// die der Interpreter fragt.
pub struct NextRunSlot<'a> {
    /// Der Platz des Kanals im Latch.
    pub slot: &'a crate::layout::Slot,
    /// Der C-Typ der Diskriminante: `int` vorn im Struct, weil `NextRun`
    /// mit `AFTER(delay)` Felder hat (11.2).
    pub ct: &'static str,
    /// Diskriminante und Bedeutung jeder Variante, die den Lauf beendet.
    pub ends: Vec<(i64, takt_mir::sys::NextRun)>,
}

/// Der Latch-Platz von `sys/next_run`, wenn das Programm den Kanal bindet.
pub fn next_run_slot<'a>(p: &Program, layout: &'a Layout) -> Option<NextRunSlot<'a>> {
    let (output, variants) = takt_mir::sys::next_run(p)?;
    let slot = layout.outputs.iter().find(|s| s.name == p.channels[output].name)?;
    let ct = match &slot.ty {
        takt_llvm::ty::LlvmType::Struct(_) => "int",
        t => c_type(t, slot.signed)?,
    };
    let ends = variants.into_iter().filter_map(|(d, next)| Some((d, next?))).collect();
    Some(NextRunSlot { slot, ct, ends })
}

/// Die Job-Slots eines Programms, flach ueber die Maschinen wie im Abbild:
/// Maschine, Slot in ihr, native Funktion.
pub fn job_slots(p: &Program) -> Vec<(usize, usize, takt_mir::NativeId)> {
    p.machines
        .iter()
        .enumerate()
        .flat_map(|(mi, m)| m.layout.job_slots.iter().enumerate().map(move |(j, s)| (mi, j, s.native)))
        .collect()
}

/// Wie gross der Eingang eines Job-Slots ist: die kanonischen Bloecke der
/// Argumente (je `u32` Laenge, dann die Bytes) des groessten Jobs.
pub fn job_in_max(p: &Program) -> u64 {
    job_slots(p)
        .iter()
        .map(|(_, _, n)| {
            let params = &p.natives[n.index()].params;
            params.iter().map(|q| 4 + u64::from(takt_mir::bytes::max_size(p, q.ty).unwrap_or(0))).sum::<u64>()
        })
        .max()
        .unwrap_or(0)
        .max(4)
}

/// Was beide Rahmen ueber ihre Jobs wissen (4.5): Versatz jedes Slots im
/// Abbild, seine Dauer in Ticks, der erste Slot je Maschine und die Helfer
/// aus `JOBS_C`. Gibt die Zahl der Slots und die groesste Ergebnislaenge
/// zurueck; `None`, wenn das Programm keine Jobs startet.
pub fn job_tables(s: &mut String, p: &Program, x: &Prefix) -> Option<(usize, u64)> {
    use takt_mir::fns::NativeKind;
    let slots = job_slots(p);
    if slots.is_empty() || !p.natives.iter().any(|n| n.kind == NativeKind::Job) {
        return None;
    }
    let t0 = p.config.tick.max(1);
    let out_max = slots
        .iter()
        .filter_map(|(_, _, n)| takt_llvm::image::job_entry_size(*n, p))
        .map(|e| e.saturating_sub(8))
        .max()
        .unwrap_or(8)
        .max(8);
    let at: Vec<String> = slots
        .iter()
        .map(|(mi, j, _)| takt_llvm::image::job_offset(takt_mir::MachineId(*mi as u32), *j, p).unwrap_or(0).to_string())
        .collect();
    let ticks: Vec<String> = slots
        .iter()
        .map(|(_, _, n)| {
            let d = p.natives[n.index()].duration.unwrap_or(0).max(0);
            (d.saturating_add(t0 - 1) / t0).to_string()
        })
        .collect();
    let mut base = Vec::new();
    let mut acc = 0usize;
    for m in &p.machines {
        base.push(acc.to_string());
        acc += m.layout.job_slots.len();
    }
    let _ = writeln!(s, "static const long long takt_job_at[{}] = {{ {} }};", slots.len(), at.join(", "));
    let _ = writeln!(s, "static const int takt_job_ticks[{}] = {{ {} }};", slots.len(), ticks.join(", "));
    let _ = writeln!(s, "static const int takt_job_base[{}] = {{ {} }};", base.len().max(1), base.join(", "));
    s.push_str(JOBS_C);
    let _ = writeln!(s, "static void takt_job_image(struct {x}_arena *a, int i, int done, int ok, int err) {{");
    let _ = writeln!(s, "    unsigned char *e = a->image + takt_job_at[i];");
    let _ = writeln!(
        s,
        "    e[0] = (unsigned char)done; e[1] = (unsigned char)ok; takt_job_put32(e + 4, (unsigned int)err);"
    );
    let _ = writeln!(s, "}}");
    Some((slots.len(), out_max))
}

/// Ruft die native Funktion eines Jobs (4.5): `native` waehlt sie, `args`
/// und `len` sind die Folge kanonischer Bloecke (`u32` Laenge, Bytes), ein
/// `bytes<N>` darin als Laenge und Daten (5.9); das Ergebnis geht in
/// kanonischer Form nach `out`, seine Laenge nach `out_len`.
pub fn job_call(s: &mut String, p: &Program, args: &str, len: &str, out: &str, out_len: &str, indent: &str) {
    let _ = writeln!(s, "{indent}const unsigned char *arg[8]; int n[8]; int k = 0, p = 0;");
    let _ = writeln!(s, "{indent}for (k = 0; k < 8; k++) {{ arg[k] = {args}; n[k] = 0; }}");
    // Ein Block, dessen Laenge ueber das Ende reicht, gilt nicht: Keine
    // Native liest hinter die Argumente (4.5).
    let _ = writeln!(
        s,
        "{indent}for (k = 0; k < 8 && p + 4 <= {len}; k++) {{ n[k] = (int)takt_job_le32({args} + p); if (n[k] < 0 || n[k] > {len} - p - 4) {{ n[k] = 0; break; }} arg[k] = {args} + p + 4; p += 4 + n[k]; }}"
    );
    let _ = writeln!(s, "{indent}{out_len} = 0;");
    let _ = writeln!(s, "{indent}switch (native) {{");
    for (idx, n) in p.natives.iter().enumerate() {
        if n.kind != takt_mir::fns::NativeKind::Job {
            continue;
        }
        if let Some(case) = job_case(n, out, out_len) {
            let _ = writeln!(s, "{indent}case {idx}: {{ {case} }} break;");
        }
    }
    let _ = writeln!(s, "{indent}default: break;");
    let _ = writeln!(s, "{indent}}}");
}

/// Der Aufruf einer `native job` im Rahmen: Argumente nach Art, das
/// Ergebnis in kanonischer Form nach `out`, seine Laenge nach `out_len`;
/// `-1` dort heisst `Err(FAILED)`.
fn job_case(n: &takt_mir::fns::Native, out: &str, out_len: &str) -> Option<String> {
    use takt_native::Kind;
    let sig = takt_native::Native::by_name(&n.name)?.signature();
    let args: Vec<String> = sig
        .params
        .iter()
        .enumerate()
        .map(|(k, kind)| match kind {
            Kind::Bytes | Kind::Digest | Kind::Fixed(_) => format!("arg[{k}] + 4, (int)takt_job_le32(arg[{k}])"),
            _ => format!("arg[{k}], n[{k}]"),
        })
        .collect();
    let call = format!("takt_native_{}({})", n.name, args.join(", "));
    Some(match sig.ret {
        Kind::U32 => format!("unsigned int v = {call}; takt_job_put32({out}, v); {out_len} = 4;"),
        Kind::U16 => format!(
            "unsigned short v = {call}; {out}[0] = (unsigned char)v; {out}[1] = (unsigned char)(v >> 8); {out_len} = 2;"
        ),
        Kind::U8 | Kind::Bool => format!("{out}[0] = {call}; {out_len} = 1;"),
        Kind::Digest => format!("takt_native_{}({}, {out}); {out_len} = 36;", n.name, args.join(", ")),
        Kind::Sha256Ctx => format!(
            "takt_native_{}({}, {out}); {out_len} = 44 + (int)takt_job_le32({out} + 32);",
            n.name,
            args.join(", ")
        ),
        // Ein Feld traegt keine Laenge; es ist so lang wie die Eingabe.
        Kind::Floats256 => format!("takt_native_{}({}, {out}); {out_len} = n[0];", n.name, args.join(", ")),
        Kind::Fixed(_) => return None,
        Kind::Bytes => format!(
            "int r = takt_native_{}({}, {out} + 4); if (r < 0) {{ {out_len} = -1; }} else {{ takt_job_put32({out}, (unsigned int)r); {out_len} = 4 + r; }}",
            n.name,
            args.join(", ")
        ),
    })
}

const JOBS_C: &str = r#"
static unsigned int takt_job_le32(const unsigned char *b) {
    return (unsigned int)b[0] | ((unsigned int)b[1] << 8) | ((unsigned int)b[2] << 16) | ((unsigned int)b[3] << 24);
}
static void takt_job_put32(unsigned char *b, unsigned int v) {
    b[0] = (unsigned char)v; b[1] = (unsigned char)(v >> 8); b[2] = (unsigned char)(v >> 16); b[3] = (unsigned char)(v >> 24);
}
/* `takt_job_image(a, i, done, ok, err)` folgt mit der Arena des Programms:
   err ist die Diskriminante von JobErr (CANCELLED 0, FAILED 1, PENDING 2). */
"#;
