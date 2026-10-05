//! `import channels from "…"` (8.2): typisierte Channels aus der
//! Hardware-Konfiguration.
//!
//! Je Kanal der Konfiguration entsteht eine gewoehnliche Deklaration, die
//! durch dieselbe Pruefung laeuft wie eine geschriebene: Der Name ist die
//! Adresse, wie der Treiber sie auch nennt (`daq1/ai0` wird `daq1_ai0`,
//! 8.10), der Typ kommt aus `unit` und `range` oder aus `raw`, der
//! `safe`-Wert eines Outputs aus `safe`. Ein Kanal mit `max_rate_hz` ist
//! ein Strom, und `raw` nennt seinen Elementtyp. Gebunden wird nur, was die
//! Quelle nennt; eine geschriebene Deklaration derselben Adresse gewinnt.
//! Die uebrigen Inputs zeichnet die Runtime auf (`Program::recorded`).

use takt_diag::{Diagnostic, Span};
use takt_mir::hardware::HwChannel;
use takt_mir::pattern::Address;
use takt_mir::program::{Binding, Direction, Recorded, RecordedStream, RecordedValue};
use takt_syntax::ast;

use crate::lower::{Lowerer, SC2};
use crate::symbols::Entity;

/// Rohtypen, die als Typ des Kanals taugen.
const SCALARS: [&str; 11] = ["bool", "float", "int", "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"];

impl Lowerer<'_> {
    /// Loest ein `import channels` auf; die Texte der Konfigurationen
    /// liefert der Aufrufer in `Options::channel_imports`.
    pub fn import_channels(&mut self, file: &str, span: Span) {
        let Some(text) = self.options.channel_imports.get(file) else {
            self.error_hint(SC2, span, format!("`{file}` nicht gelesen"), "die Datei neben das Programm legen (8.2)");
            return;
        };
        let hw = match takt_mir::hardware::parse(text) {
            Ok(hw) => hw,
            Err(e) => {
                self.diags.push(Diagnostic::error(SC2, span, format!("`{file}`, Zeile {}: {}", e.line, e.message)));
                return;
            }
        };
        for c in hw.channels.values() {
            let name = Address::simple(&c.address).ident();
            if self.bound(c) {
                continue;
            }
            if self.imports_used.as_ref().is_some_and(|used| !used.contains(&name)) {
                if let Some(r) = self.recorded(c, name) {
                    self.program.recorded.push(r);
                }
                continue;
            }
            // Ein abgelehnter Kanal meldet keinen Folgefehler (FB-407).
            let errors = self.error_count();
            let ident = ast::Ident { name: name.clone(), span };
            let declared = declaration(c, &name).and_then(|text| {
                let toks = takt_syntax::tokenize_in(&text, self.edition);
                match takt_syntax::parse_file(&toks) {
                    (f, errors) if errors.is_empty() && toks.errors.is_empty() => match f.items.into_iter().next() {
                        Some(ast::Item::Channel(decl)) => Ok(decl),
                        _ => Err(format!("`{text}` ist keine Deklaration")),
                    },
                    _ => Err(format!("keine gueltige Deklaration: `{}`", text.trim())),
                }
            });
            let mut decl = match declared {
                Ok(d) => d,
                Err(e) => {
                    self.diags.push(Diagnostic::error(SC2, span, format!("`{file}`, Kanal `{}`: {e}", c.address)));
                    self.reject_unless_declared(&ident, errors);
                    continue;
                }
            };
            decl.span = span;
            decl.name.span = span;
            let (before, channels) = (self.diags.len(), self.program.channels.len());
            self.channel_decl(&decl);
            // Was die Deklaration bemaengelt, gehoert an den Import.
            for d in &mut self.diags[before..] {
                d.message = format!("`{file}`, Kanal `{}`: {}", c.address, d.message);
                d.span = span;
                d.notes.clear();
            }
            if let Some(channel) = self.program.channels.get_mut(channels) {
                channel.span = span;
            }
            self.reject_unless_declared(&ident, errors);
        }
    }

    /// Ein Input, den die Quelle nicht nennt, mit der Art, in der sein
    /// Treiber den Wert liefert — derselben, die eine Deklaration aus ihm
    /// haette. Ein Output hat nichts aufzuzeichnen; einen Kanal, aus dem
    /// sich kein Typ bilden laesst, liest kein Treiber, und ein Fehler ist er
    /// erst, wenn die Quelle ihn nennt (8.2).
    fn recorded(&self, c: &HwChannel, name: String) -> Option<Recorded> {
        if c.direction != Some(Direction::Input) {
            return None;
        }
        let address = Address::simple(&c.address);
        let Some(max_rate_hz) = c.max_rate_hz else {
            let value = self.scalar(c.unit.is_some(), c.raw.as_deref())?;
            return Some(Recorded { name, address, value, unit: c.unit.clone(), stream: None });
        };
        // Ein Strom (8.2, 8.6): ein `u8` je Element als Zahl, jedes andere
        // Element in seiner Drahtform.
        let raw = c.raw.as_deref()?;
        let (value, bytes) = match raw {
            "u8" => (RecordedValue::Int(takt_mir::types::IntWidth::U8), 1),
            _ => (RecordedValue::Wire, self.wire_bytes(raw)?),
        };
        Some(Recorded { name, address, value, unit: None, stream: Some(RecordedStream { max_rate_hz, bytes }) })
    }

    /// Die Art eines Skalars aus `unit` und `raw`, wie eine Deklaration aus
    /// ihnen sie haette (8.2).
    fn scalar(&self, unit: bool, raw: Option<&str>) -> Option<RecordedValue> {
        Some(match (unit, raw) {
            (true, _) | (false, Some("float")) => RecordedValue::Float(self.program.config.float_width),
            (false, Some("bool")) => RecordedValue::Bool,
            (false, Some(raw)) => match (super::decl::int_width_named(raw), self.peek(raw)) {
                (Some(width), _) => RecordedValue::Int(width),
                (None, Some(Entity::Enum(id)))
                    if self.program.enums[id.index()].variants.iter().all(|v| v.fields.is_empty()) =>
                {
                    RecordedValue::Enum(*id)
                }
                _ => return None,
            },
            (false, None) => return None,
        })
    }

    /// Die Bytes eines Stromelements in seiner Drahtform hoechstens:
    /// `bytes<N>` und `line<N>` tragen bis zu N, ein Record (auch `Edge`)
    /// seine kanonische Form (5.9).
    fn wire_bytes(&self, raw: &str) -> Option<u32> {
        for kind in ["bytes<", "line<"] {
            if let Some(n) = raw.strip_prefix(kind).and_then(|rest| rest.strip_suffix('>')) {
                return n.trim().parse().ok();
            }
        }
        match self.peek(raw) {
            Some(Entity::Record(id)) => takt_mir::bytes::record_size(&self.program, *id).ok(),
            _ => None,
        }
    }

    /// Bindet schon eine geschriebene Deklaration diese Adresse?
    fn bound(&self, c: &HwChannel) -> bool {
        let Some(dir) = c.direction else { return false };
        self.program
            .channels
            .iter()
            .any(|x| matches!(&x.binding, Binding::Hw(a) if a.text() == c.address && x.dir == dir))
    }
}

/// Die Deklaration eines Kanals als Quelltext.
fn declaration(c: &HwChannel, name: &str) -> Result<String, String> {
    let dir = match c.direction {
        Some(Direction::Input) => "input",
        Some(Direction::Output) => "output",
        None => return Err("ohne `direction`".to_string()),
    };
    // Nur Stroeme haben eine Hoechstrate (8.6); `capacity` bleibt beim
    // Default, denn sie haengt an den Lesern, nicht an der Hardware.
    if let Some(hz) = c.max_rate_hz {
        let elem = c.raw.as_deref().ok_or("Strom ohne `raw` (Elementtyp)")?;
        let framing = c.framing.as_deref().map(|f| format!(", framing = {f}")).unwrap_or_default();
        return Ok(format!("{dir} {name} : stream<{elem}> @ hw(\"{}\") with max_rate = {hz} Hz{framing}\n", c.address));
    }
    let range = |unit: &str| c.range.map(|(lo, hi)| format!(" in {lo:?}..{hi:?}{unit}")).unwrap_or_default();
    let ty = match (&c.unit, c.raw.as_deref()) {
        (Some(unit), _) => format!("float[{unit}]{}", range(&format!(" {unit}"))),
        (None, Some("bool")) => "bool".to_string(),
        (None, Some("float")) => format!("float{}", range("")),
        (None, Some(raw)) if SCALARS.contains(&raw) => {
            let whole = c.range.map(|(lo, hi)| format!(" in {}..{}", lo as i64, hi as i64)).unwrap_or_default();
            format!("{raw}{whole}")
        }
        // Eine Enum-Abbildung nennt den Typ, den das Programm deklariert (8.1).
        (None, Some(raw)) if raw.starts_with(|ch: char| ch.is_ascii_uppercase()) => raw.to_string(),
        (None, raw) => return Err(format!("kein Typ aus `raw = {}` ohne `unit`", raw.unwrap_or("?"))),
    };
    let safe = match (dir, &c.safe) {
        ("output", Some(v)) => format!(" with safe = {v}"),
        ("output", None) => return Err("Output ohne `safe`".to_string()),
        _ => String::new(),
    };
    Ok(format!("{dir} {name} : {ty} @ hw(\"{}\"){safe}\n", c.address))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(text: &str) -> HwChannel {
        let hw = takt_mir::hardware::parse(&format!("# takt-hw 9\n[channel daq1/ai0]\n{text}")).expect("lesbar");
        hw.channel("daq1/ai0").expect("Kanal").clone()
    }

    #[test]
    fn a_unit_makes_a_float_with_its_range() {
        let c = channel("direction = input\nraw = i16\nunit = bar\nrange = 0..250\n");
        assert_eq!(
            declaration(&c, "daq1_ai0").as_deref(),
            Ok("input daq1_ai0 : float[bar] in 0.0..250.0 bar @ hw(\"daq1/ai0\")\n")
        );
    }

    #[test]
    fn an_output_takes_its_safe_value() {
        let c = channel("direction = output\nraw = u8\nrange = 0..100\nsafe = 0\n");
        assert_eq!(declaration(&c, "x").as_deref(), Ok("output x : u8 in 0..100 @ hw(\"daq1/ai0\") with safe = 0\n"));
        let c = channel("direction = output\nraw = bool\n");
        assert!(declaration(&c, "x").is_err_and(|e| e.contains("safe")));
    }

    #[test]
    fn a_channel_with_a_maximum_rate_is_a_stream() {
        let c = channel("direction = input\nraw = line<80>\nmax_rate_hz = 200\nframing = lines\n");
        assert_eq!(
            declaration(&c, "log").as_deref(),
            Ok("input log : stream<line<80>> @ hw(\"daq1/ai0\") with max_rate = 200 Hz, framing = lines\n")
        );
        let c = channel("direction = input\nmax_rate_hz = 200\n");
        assert!(declaration(&c, "log").is_err_and(|e| e.contains("Elementtyp")));
    }

    #[test]
    fn negative_bounds_and_a_unit_without_range_carry_over() {
        let c = channel("direction = input\nraw = i16\nrange = -40..125\n");
        assert_eq!(declaration(&c, "t").as_deref(), Ok("input t : i16 in -40..125 @ hw(\"daq1/ai0\")\n"));
        let c = channel("direction = input\nraw = i16\nunit = K\nrange = -40..125\n");
        assert_eq!(declaration(&c, "t").as_deref(), Ok("input t : float[K] in -40.0..125.0 K @ hw(\"daq1/ai0\")\n"));
        let c = channel("direction = input\nraw = i16\nunit = bar\n");
        assert_eq!(declaration(&c, "p").as_deref(), Ok("input p : float[bar] @ hw(\"daq1/ai0\")\n"));
    }

    #[test]
    fn an_output_stream_needs_no_safe_value() {
        // 8.8: Ein Ausgabestrom hat keinen Latch und darum keinen `safe`.
        let c = channel("direction = output\nraw = u8\nmax_rate_hz = 1000\n");
        assert_eq!(
            declaration(&c, "tx").as_deref(),
            Ok("output tx : stream<u8> @ hw(\"daq1/ai0\") with max_rate = 1000 Hz\n")
        );
    }

    #[test]
    fn a_channel_without_direction_has_no_declaration() {
        let c = channel("raw = bool\n");
        assert_eq!(declaration(&c, "x"), Err("ohne `direction`".to_string()));
    }

    #[test]
    fn an_enum_mapping_names_the_type() {
        let c = channel("direction = output\nraw = ValveCmd\nsafe = CLOSED\n");
        assert_eq!(declaration(&c, "v").as_deref(), Ok("output v : ValveCmd @ hw(\"daq1/ai0\") with safe = CLOSED\n"));
    }
}
