//! Trace-Format (`grammar/trace.md`): Stimulus, Golden-Trace und die lesbare
//! Form der Aufzeichnung (12.5). Werte stehen in Literalschreibweise (2.3),
//! die Zeilen eines Ticks in kanonischer Ordnung, damit ein Vergleich die
//! Ordnungsunabhaengigkeit (Satz 9.4.1) unmittelbar prueft.

use std::fmt::Write as _;

use takt_mir::TypeId;
use takt_mir::program::Program;
use takt_mir::types::Type;

use crate::value::{Quality, Reason, Sample, Value};

/// Eine Zeile des Traces.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceLine {
    /// Tick-Nummer.
    pub tick: u64,
    /// Art und Felder, wie in `grammar/trace.md` beschrieben.
    pub kind: LineKind,
}

/// Inhalt einer Zeile.
#[derive(Clone, Debug, PartialEq)]
#[allow(missing_docs)]
pub enum LineKind {
    /// `in <channel> <wert>` oder `in <channel> <qualitaet> …`
    Input { channel: String, sample: SampleText },
    /// `cmd <command>`
    Command { name: String },
    /// `tune <name> <wert>`: ein Tunable aendert sich an dieser Tick-Grenze
    /// (8.4); ein verworfener Wert traegt ` rejected` (Golden).
    Tune { name: String, value: String, accepted: bool },
    /// `abort`
    Abort,
    /// `out <channel> <wert>`
    Output { channel: String, value: String },
    /// `state <maschine> <pfad>`
    State { machine: String, path: String },
    /// `pub <maschine> <variable> <wert>`
    Published { machine: String, var: String, value: String },
    /// `signal <maschine> <name>`
    Signal { machine: String, name: String },
    /// `job <maschine> <handle> done`: Fertigstellung eines Jobs (4.5);
    /// als Stimulus verlegt sie den Tick des Modells.
    Job { machine: String, handle: String },
    /// `fault <maschine> <art> "<meldung>" -> <ziel>`
    Fault { machine: String, kind: String, message: String, target: String },
    /// `log <maschine> "<text>"`
    Log { machine: String, text: String },
    /// `alert <maschine> on|off "<text>"`
    Alert { machine: String, on: bool, text: String },
    /// `measure <maschine> <name> <wert>`
    Measure { machine: String, name: String, value: String },
    /// `verify <maschine> ok|fail "<text>"`
    Verify { machine: String, ok: bool, text: String },
    /// `verdict <maschine> pass|fail ["<text>"]`
    Verdict { machine: String, pass: bool, text: Option<String> },
    /// `property|assumption <name> violated <tick>`: eine Eigenschaft ist
    /// an Position `at` entschieden verletzt (13.3).
    Property { assumption: bool, name: String, at: u64 },
    /// `stream <name> dropped=<n> overflowed=<n> malformed=<n>` — die
    /// Zaehler eines Stroms, wenn sie sich aendern (8.6).
    Stream { name: String, dropped: u32, overflowed: u32, malformed: u32 },
    /// `verdict-final PASS|FAIL|INCONCLUSIVE`
    Final { verdict: String },
    /// `end restart|deep_sleep|boot_jump`: der Lauf endet hier (12.7).
    End { reason: String },
    /// `persist <hex>`: die kanonische Form aller `persist`-Variablen am
    /// Ende des Laufs (5.9). Beobachtung, keine Semantik — aber genau die
    /// Bytes, die der erzeugte Code liefern muss (Satz 9.4.4).
    Persist { hex: String },
}

/// Wert oder Qualitaet eines Inputs (3.5).
#[derive(Clone, Debug, PartialEq)]
pub struct SampleText {
    /// Wert in Literalschreibweise; fehlt bei reiner Qualitaetsangabe.
    pub value: Option<String>,
    /// `bad`, `stale`, `suspect`; `None` ist `Good`.
    pub quality: Option<String>,
    /// `reason=…`
    pub reason: Option<String>,
    /// `age=…`
    pub age: Option<String>,
}

/// Ein Trace: Zeilen in Reihenfolge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    /// Die Zeilen.
    pub lines: Vec<TraceLine>,
}

impl Trace {
    /// Liest einen Trace; Kommentare und Leerzeilen entfallen.
    pub fn parse(text: &str) -> Result<Trace, String> {
        let mut lines = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parsed = parse_line(line).map_err(|e| format!("Zeile {}: {e}", i + 1))?;
            lines.push(parsed);
        }
        Ok(Trace { lines })
    }

    /// Schreibt den Trace.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            let _ = writeln!(out, "{}", render_line(line));
        }
        out
    }

    /// Zeilen eines Ticks.
    pub fn at(&self, tick: u64) -> impl Iterator<Item = &TraceLine> {
        self.lines.iter().filter(move |l| l.tick == tick)
    }

    /// Hoechste Tick-Nummer.
    pub fn last_tick(&self) -> u64 {
        self.lines.iter().map(|l| l.tick).max().unwrap_or(0)
    }
}

fn parse_line(line: &str) -> Result<TraceLine, String> {
    let mut it = line.splitn(2, ' ');
    let head = it.next().unwrap_or_default();
    let rest = it.next().unwrap_or_default().trim();
    let tick: u64 = head
        .strip_prefix("t=")
        .ok_or_else(|| format!("`t=<tick>` erwartet, `{head}` gefunden"))?
        .parse()
        .map_err(|_| format!("Tick-Nummer erwartet, `{head}` gefunden"))?;
    let (art, args) = split_first(rest);
    let kind = match art {
        "in" => {
            let (channel, value) = split_first(args);
            if channel.is_empty() {
                return Err("`in <channel> <wert>` erwartet".into());
            }
            LineKind::Input { channel: channel.to_string(), sample: parse_sample(value)? }
        }
        "cmd" => LineKind::Command { name: nonempty(args, "`cmd <command>`")?.to_string() },
        "tune" => {
            let (name, value) = split_first(args);
            let (value, accepted) = match value.strip_suffix(" rejected") {
                Some(v) => (v, false),
                None => (value, true),
            };
            LineKind::Tune {
                name: nonempty(name, "`tune <name> <wert>`")?.to_string(),
                value: nonempty(value, "`tune <name> <wert>`")?.to_string(),
                accepted,
            }
        }
        "abort" => LineKind::Abort,
        "out" => {
            let (channel, value) = split_first(args);
            LineKind::Output {
                channel: nonempty(channel, "`out <channel> <wert>`")?.to_string(),
                value: value.to_string(),
            }
        }
        "state" => {
            let (machine, path) = split_first(args);
            LineKind::State {
                machine: nonempty(machine, "`state <maschine> <pfad>`")?.to_string(),
                path: path.to_string(),
            }
        }
        "pub" => {
            let (machine, rest) = split_first(args);
            let (var, value) = split_first(rest);
            LineKind::Published {
                machine: nonempty(machine, "`pub <maschine> <variable> <wert>`")?.to_string(),
                var: nonempty(var, "`pub <maschine> <variable> <wert>`")?.to_string(),
                value: value.to_string(),
            }
        }
        "signal" => {
            let (machine, name) = split_first(args);
            LineKind::Signal {
                machine: nonempty(machine, "`signal <maschine> <name>`")?.to_string(),
                name: nonempty(name, "`signal <maschine> <name>`")?.to_string(),
            }
        }
        "job" => {
            let (machine, rest) = split_first(args);
            let (handle, done) = split_first(rest);
            if done.trim() != "done" {
                return Err("`job <maschine> <handle> done` erwartet".into());
            }
            LineKind::Job {
                machine: nonempty(machine, "`job <maschine> <handle> done`")?.to_string(),
                handle: nonempty(handle, "`job <maschine> <handle> done`")?.to_string(),
            }
        }
        "property" | "assumption" => {
            let (name, rest) = split_first(args);
            let (word, rest) = split_first(rest);
            let form = "`property <name> violated <tick>`";
            if word != "violated" {
                return Err(format!("{form} erwartet"));
            }
            let at = nonempty(rest.trim(), form)?.parse::<u64>().map_err(|e| format!("{form}: {e}"))?;
            LineKind::Property { assumption: art == "assumption", name: name.to_string(), at }
        }
        "fault" => {
            let (machine, rest) = split_first(args);
            let (kind, rest) = split_first(rest);
            let (message, rest) = parse_quoted(rest)?;
            let target = rest.trim().strip_prefix("->").map(str::trim).unwrap_or("").to_string();
            LineKind::Fault { machine: machine.to_string(), kind: kind.to_string(), message, target }
        }
        "log" => {
            let (machine, rest) = split_first(args);
            let (text, _) = parse_quoted(rest)?;
            LineKind::Log { machine: machine.to_string(), text }
        }
        "alert" => {
            let (machine, rest) = split_first(args);
            let (edge, rest) = split_first(rest);
            let (text, _) = parse_quoted(rest)?;
            LineKind::Alert { machine: machine.to_string(), on: edge == "on", text }
        }
        "measure" => {
            let (machine, rest) = split_first(args);
            let (name, value) = split_first(rest);
            LineKind::Measure { machine: machine.to_string(), name: name.to_string(), value: value.to_string() }
        }
        "verify" => {
            let (machine, rest) = split_first(args);
            let (verdict, rest) = split_first(rest);
            let (text, _) = parse_quoted(rest)?;
            LineKind::Verify { machine: machine.to_string(), ok: verdict == "ok", text }
        }
        "verdict" => {
            let (machine, rest) = split_first(args);
            let (verdict, rest) = split_first(rest);
            let text = if rest.trim().is_empty() { None } else { Some(parse_quoted(rest)?.0) };
            LineKind::Verdict { machine: machine.to_string(), pass: verdict == "pass", text }
        }
        "stream" => {
            let (name, rest) = split_first(args);
            let mut counters = [0u32; 3];
            for (i, key) in ["dropped", "overflowed", "malformed"].iter().enumerate() {
                counters[i] = match rest.split_whitespace().find_map(|f| f.strip_prefix(&format!("{key}="))) {
                    Some(v) => v.parse().map_err(|_| format!("`{key}=` erwartet eine Zahl"))?,
                    None => return Err(format!("`{key}=` fehlt")),
                };
            }
            LineKind::Stream {
                name: name.to_string(),
                dropped: counters[0],
                overflowed: counters[1],
                malformed: counters[2],
            }
        }
        "end" => LineKind::End { reason: nonempty(args, "`end restart|deep_sleep|boot_jump`")?.to_string() },
        "persist" => {
            let hex = args.trim();
            if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("`persist <hex>` erwartet gerade viele Hexziffern".into());
            }
            LineKind::Persist { hex: hex.to_ascii_lowercase() }
        }
        "verdict-final" => {
            LineKind::Final { verdict: nonempty(args, "`verdict-final PASS|FAIL|INCONCLUSIVE`")?.to_string() }
        }
        other => return Err(format!("unbekannte Art `{other}`")),
    };
    Ok(TraceLine { tick, kind })
}

fn nonempty<'a>(s: &'a str, what: &str) -> Result<&'a str, String> {
    if s.is_empty() { Err(format!("{what} erwartet")) } else { Ok(s) }
}

fn split_first(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], s[i + 1..].trim_start()),
        None => (s, ""),
    }
}

fn parse_quoted(s: &str) -> Result<(String, &str), String> {
    let s = s.trim_start();
    let rest = s.strip_prefix('"').ok_or_else(|| format!("Anfuehrungszeichen erwartet, `{s}` gefunden"))?;
    let end = rest.find('"').ok_or("schliessendes Anfuehrungszeichen fehlt")?;
    Ok((rest[..end].to_string(), &rest[end + 1..]))
}

fn parse_sample(s: &str) -> Result<SampleText, String> {
    let mut value: Option<String> = None;
    let mut quality = None;
    let mut reason = None;
    let mut age = None;
    let mut words: Vec<&str> = Vec::new();
    // `age=<dauer>` traegt eine Einheit als eigenes Wort (`age=120 ms`).
    let mut in_age = false;
    for word in s.split_whitespace() {
        if let Some(r) = word.strip_prefix("reason=") {
            reason = Some(r.to_string());
            in_age = false;
        } else if let Some(a) = word.strip_prefix("age=") {
            age = Some(a.to_string());
            in_age = true;
        } else if in_age
            && word.bytes().all(|b| b.is_ascii_alphabetic())
            && !matches!(word, "bad" | "stale" | "suspect")
        {
            if let Some(a) = &mut age {
                a.push(' ');
                a.push_str(word);
            }
            in_age = false;
        } else {
            in_age = false;
            if matches!(word, "bad" | "stale" | "suspect") && words.is_empty() {
                quality = Some(word.to_string());
            } else {
                words.push(word);
            }
        }
    }
    if !words.is_empty() {
        value = Some(words.join(" "));
    }
    if value.is_none() && quality.is_none() {
        return Err("Wert oder Qualitaet erwartet".into());
    }
    Ok(SampleText { value, quality, reason, age })
}

pub(crate) fn render_line(line: &TraceLine) -> String {
    let t = line.tick;
    match &line.kind {
        LineKind::Input { channel, sample } => {
            let mut out = format!("t={t} in {channel}");
            if let Some(v) = &sample.value {
                let _ = write!(out, " {v}");
            }
            if let Some(q) = &sample.quality {
                let _ = write!(out, " {q}");
            }
            if let Some(r) = &sample.reason {
                let _ = write!(out, " reason={r}");
            }
            if let Some(a) = &sample.age {
                let _ = write!(out, " age={a}");
            }
            out
        }
        LineKind::Command { name } => format!("t={t} cmd {name}"),
        LineKind::Abort => format!("t={t} abort"),
        LineKind::Output { channel, value } => format!("t={t} out {channel} {value}"),
        LineKind::State { machine, path } => format!("t={t} state {machine} {path}"),
        LineKind::Published { machine, var, value } => format!("t={t} pub {machine} {var} {value}"),
        LineKind::Signal { machine, name } => format!("t={t} signal {machine} {name}"),
        LineKind::Tune { name, value, accepted } => {
            format!("t={t} tune {name} {value}{}", if *accepted { "" } else { " rejected" })
        }
        LineKind::Job { machine, handle } => format!("t={t} job {machine} {handle} done"),
        LineKind::Fault { machine, kind, message, target } => {
            format!("t={t} fault {machine} {kind} \"{message}\" -> {target}")
        }
        LineKind::Log { machine, text } => format!("t={t} log {machine} \"{text}\""),
        LineKind::Stream { name, dropped, overflowed, malformed } => {
            format!("t={t} stream {name} dropped={dropped} overflowed={overflowed} malformed={malformed}")
        }
        LineKind::Alert { machine, on, text } => {
            format!("t={t} alert {machine} {} \"{text}\"", if *on { "on" } else { "off" })
        }
        LineKind::Measure { machine, name, value } => format!("t={t} measure {machine} {name} {value}"),
        LineKind::Verify { machine, ok, text } => {
            format!("t={t} verify {machine} {} \"{text}\"", if *ok { "ok" } else { "fail" })
        }
        LineKind::Verdict { machine, pass, text } => {
            let verdict = if *pass { "pass" } else { "fail" };
            match text {
                Some(t2) => format!("t={t} verdict {machine} {verdict} \"{t2}\""),
                None => format!("t={t} verdict {machine} {verdict}"),
            }
        }
        LineKind::Property { assumption, name, at } => {
            format!("t={t} {} {name} violated {at}", if *assumption { "assumption" } else { "property" })
        }
        LineKind::Final { verdict } => format!("t={t} verdict-final {verdict}"),
        LineKind::End { reason } => format!("t={t} end {reason}"),
        LineKind::Persist { hex } => format!("t={t} persist {hex}"),
    }
}

/// Wert in Literalschreibweise (2.3, T2): mit Einheit, Dauer in der groessten
/// ganzzahligen Einheit, Fliesskomma stets mit Dezimalpunkt.
pub fn value_text(v: &Value, ty: TypeId, p: &Program) -> String {
    let unit_suffix = match p.types.get(ty) {
        Type::Float { unit: Some(u), .. } | Type::Int { unit: Some(u), .. } => format!(" {}", p.units[u.index()].name),
        _ => String::new(),
    };
    match v {
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => format!("{i}{unit_suffix}"),
        Value::UInt(u) => format!("{u}{unit_suffix}"),
        Value::F32(f) => format!("{}{unit_suffix}", float32_text(*f)),
        Value::F64(f) => format!("{}{unit_suffix}", float_text(*f)),
        Value::Duration(d) => takt_mir::dump::duration(*d),
        Value::Str(s) => format!("{s:?}"),
        Value::Line { text, .. } => format!("{text:?}"),
        Value::Optional(None) => "none".to_string(),
        Value::Optional(Some(inner)) => {
            let elem = match p.types.get(ty) {
                Type::Optional(t) => *t,
                _ => ty,
            };
            value_text(inner, elem, p)
        }
        Value::Enum { variant, fields } => {
            let name = match p.types.get(ty) {
                Type::Enum(e) => p.enums[e.index()].variants.get(*variant as usize).map(|v| v.name.clone()),
                _ => None,
            };
            let name = name.unwrap_or_else(|| format!("V{variant}"));
            if fields.is_empty() {
                name
            } else {
                let tys: Vec<TypeId> = match p.types.get(ty) {
                    Type::Enum(e) => {
                        p.enums[e.index()].variants[*variant as usize].fields.iter().map(|f| f.ty).collect()
                    }
                    _ => Vec::new(),
                };
                let parts: Vec<String> = fields
                    .iter()
                    .enumerate()
                    .map(|(i, f)| value_text(f, tys.get(i).copied().unwrap_or(ty), p))
                    .collect();
                format!("{name}({})", parts.join(", "))
            }
        }
        Value::Record(fields) => {
            let (name, tys) = match p.types.get(ty) {
                Type::Record(r) => {
                    let def = &p.records[r.index()];
                    (def.name.clone(), def.fields.iter().map(|f| f.ty).collect::<Vec<_>>())
                }
                _ => ("record".to_string(), Vec::new()),
            };
            let parts: Vec<String> =
                fields.iter().enumerate().map(|(i, f)| value_text(f, tys.get(i).copied().unwrap_or(ty), p)).collect();
            format!("{name}({})", parts.join(", "))
        }
        Value::Array(items) | Value::Vec(items) | Value::Samples(items) => {
            let elem = match p.types.get(ty) {
                Type::Array { elem, .. } | Type::Vec { elem, .. } | Type::Samples { elem, .. } => *elem,
                _ => ty,
            };
            let parts: Vec<String> = items.iter().map(|x| value_text(x, elem, p)).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Bytes(b) => format!("[{}]", b.iter().map(|x| format!("{x:#04x}")).collect::<Vec<_>>().join(", ")),
        Value::Result(Ok(inner)) => {
            let elem = match p.types.get(ty) {
                Type::Result { ok, .. } => *ok,
                _ => ty,
            };
            format!("OK({})", value_text(inner, elem, p))
        }
        Value::Result(Err(inner)) => format!("ERR({})", value_text(inner, ty, p)),
        Value::Mat { data, .. } => {
            format!("[{}]", data.iter().map(|x| value_text(x, ty, p)).collect::<Vec<_>>().join(", "))
        }
        Value::Table(points) => format!(
            "[{}]",
            points
                .iter()
                .map(|(a, b)| format!("({}, {})", value_text(a, ty, p), value_text(b, ty, p)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map(slots) => format!(
            "[{}]",
            slots
                .iter()
                .flatten()
                .map(|(a, b)| format!("({}, {})", value_text(a, ty, p), value_text(b, ty, p)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Block(_) => "<block>".to_string(),
        Value::Handle => "<handle>".to_string(),
    }
}

/// Fliesskomma stets mit Dezimalpunkt (T2).
pub fn float_text(x: f64) -> String {
    if x == x.trunc() && x.abs() < 1e15 { format!("{x:.1}") } else { format!("{x}") }
}

/// Ein `f32` als Text. Die Erweiterung nach f64 vor dem Drucken zeigte die
/// Ziffern der f64-Darstellung (`0.1f32` wurde `0.10000000149011612`); Rust
/// druckt einen `f32` als kuerzeste Ziffernfolge, die ihn eindeutig
/// bestimmt, und genau die gehoert in Trace und Ausgabe (4.1).
pub fn float32_text(x: f32) -> String {
    if x == x.trunc() && x.abs() < 1e15 { format!("{x:.1}") } else { format!("{x}") }
}

/// Abtastung aus der Textform (T2).
pub fn sample_from_text(text: &SampleText, ty: TypeId, p: &Program) -> Result<Sample, String> {
    let value = match &text.value {
        Some(v) => Some(parse_value(v, ty, p)?),
        None => None,
    };
    let quality = match text.quality.as_deref() {
        None => Quality::Good,
        Some("bad") => Quality::Bad,
        Some("stale") => Quality::Stale,
        Some("suspect") => Quality::Suspect,
        Some(other) => return Err(format!("unbekannte Qualitaet `{other}`")),
    };
    let reason = match text.reason.as_deref() {
        None => (quality == Quality::Bad).then_some(Reason::Driver),
        Some("Stale") => Some(Reason::Stale),
        Some("OutOfRange") => Some(Reason::OutOfRange),
        Some("Implausible") => Some(Reason::Implausible),
        Some("Driver") => Some(Reason::Driver),
        Some("Node") => Some(Reason::Node),
        Some(other) => return Err(format!("unbekannter Grund `{other}`")),
    };
    let age = match &text.age {
        Some(a) => parse_duration(a)?,
        None => 0,
    };
    Ok(Sample { value, quality, age, reason })
}

/// Wert aus der Literalschreibweise (T2).
pub fn parse_value(text: &str, ty: TypeId, p: &Program) -> Result<Value, String> {
    let text = text.trim();
    match p.types.get(ty) {
        Type::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("`true` oder `false` erwartet, `{text}` gefunden")),
        },
        Type::Int { width, .. } => {
            let number = text.split_whitespace().next().unwrap_or(text);
            let v: i128 =
                number.replace('_', "").parse().map_err(|_| format!("Ganzzahl erwartet, `{text}` gefunden"))?;
            Ok(Value::int(*width, v))
        }
        Type::Float { width, .. } => {
            let number = text.split_whitespace().next().unwrap_or(text);
            let v: f64 = number.parse().map_err(|_| format!("Zahl erwartet, `{text}` gefunden"))?;
            Ok(Value::float(*width, v))
        }
        Type::Duration { .. } => Ok(Value::Duration(parse_duration(text)?)),
        Type::Enum(e) => {
            let (name, rest) = match text.find('(') {
                Some(i) => (&text[..i], Some(text[i + 1..].trim_end_matches(')'))),
                None => (text, None),
            };
            let def = &p.enums[e.index()];
            let variant = def
                .variants
                .iter()
                .position(|v| v.name == name.trim())
                .ok_or_else(|| format!("`{}` hat keine Variante `{}`", def.name, name.trim()))?;
            let field_tys: Vec<TypeId> = def.variants[variant].fields.iter().map(|f| f.ty).collect();
            let fields = match rest {
                None => Vec::new(),
                Some(args) if args.trim().is_empty() => Vec::new(),
                Some(args) => args
                    .split(',')
                    .zip(&field_tys)
                    .map(|(a, t)| parse_value(a.trim(), *t, p))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Ok(Value::Enum { variant: variant as u32, fields })
        }
        Type::Str { .. } | Type::Line { .. } => {
            let s = text.trim_matches('"').to_string();
            Ok(match p.types.get(ty) {
                Type::Line { .. } => Value::Line { text: s, truncated: false },
                _ => Value::Str(s),
            })
        }
        Type::Optional(inner) => {
            if text == "none" {
                Ok(Value::Optional(None))
            } else {
                Ok(Value::Optional(Some(Box::new(parse_value(text, *inner, p)?))))
            }
        }
        Type::Array { elem, len } => {
            let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
            let parts: Vec<&str> = if inner.trim().is_empty() { Vec::new() } else { split_top(inner) };
            if parts.len() != *len as usize {
                return Err(format!("{len} Elemente erwartet, {} gefunden", parts.len()));
            }
            Ok(Value::Array(parts.iter().map(|x| parse_value(x, *elem, p)).collect::<Result<Vec<_>, _>>()?))
        }
        // 8.9: ein Tick-Array eines oversampelten Kanals. Es darf kuerzer als
        // `N` sein — fehlende Samples sind der Normalfall und geben dem Wert
        // die Qualitaet `Stale`, nicht einen Lesefehler.
        Type::Samples { elem, len } => {
            let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
            let parts: Vec<&str> = if inner.trim().is_empty() { Vec::new() } else { split_top(inner) };
            if parts.len() > *len as usize {
                return Err(format!("hoechstens {len} Samples, {} gefunden", parts.len()));
            }
            Ok(Value::Samples(parts.iter().map(|x| parse_value(x, *elem, p)).collect::<Result<Vec<_>, _>>()?))
        }
        // Wie `value_text` ihn schreibt: `Name(feld, feld, ...)`.
        Type::Record(r) => {
            let def = &p.records[r.index()];
            let inner = text
                .strip_prefix(def.name.as_str())
                .map(str::trim)
                .and_then(|s| s.strip_prefix('('))
                .and_then(|s| s.strip_suffix(')'))
                .ok_or_else(|| format!("`{}(...)` erwartet, `{text}` gefunden", def.name))?;
            let parts: Vec<&str> = if inner.trim().is_empty() { Vec::new() } else { split_top(inner) };
            if parts.len() != def.fields.len() {
                return Err(format!("{} Felder erwartet, {} gefunden", def.fields.len(), parts.len()));
            }
            let fields = parts.iter().zip(&def.fields).map(|(x, f)| parse_value(x, f.ty, p));
            Ok(Value::Record(fields.collect::<Result<Vec<_>, _>>()?))
        }
        other => Err(format!("Wert vom Typ {other:?} kann der Trace nicht lesen")),
    }
}

/// Zerlegt eine Liste auf oberster Klammerebene.
fn split_top(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        out.push(last);
    }
    out
}

/// Dauer aus der Literalschreibweise (3.3).
pub fn parse_duration(text: &str) -> Result<i64, String> {
    let text = text.trim();
    let (number, unit) = match text.find(|c: char| c.is_ascii_alphabetic()) {
        Some(i) => (text[..i].trim(), text[i..].trim()),
        None => (text, "ns"),
    };
    let factor: i64 = match unit {
        "ns" | "" => 1,
        "us" => 1_000,
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "min" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        other => return Err(format!("unbekannte Zeiteinheit `{other}`")),
    };
    let value: f64 = number.replace('_', "").parse().map_err(|_| format!("Dauer erwartet, `{text}` gefunden"))?;
    let ns = value * factor as f64;
    if ns.fract() != 0.0 {
        return Err(format!("`{text}` ist nicht ganzzahlig in Nanosekunden (3.3)"));
    }
    Ok(ns as i64)
}
