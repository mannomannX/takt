//! `import channels from "…"` (8.2): typisierte Channels aus der
//! Hardware-Konfiguration.
//!
//! Je Kanal der Konfiguration entsteht eine gewoehnliche Deklaration, die
//! durch dieselbe Pruefung laeuft wie eine geschriebene: Der Name ist die
//! Adresse, wie der Treiber sie auch nennt (`daq1/ai0` wird `daq1_ai0`,
//! 8.10), der Typ kommt aus `unit` und `range` oder aus `raw`, der
//! `safe`-Wert eines Outputs aus `safe`. Gebunden wird nur, was die Quelle
//! nennt; eine geschriebene Deklaration derselben Adresse gewinnt.

use takt_diag::{Diagnostic, Span};
use takt_mir::hardware::HwChannel;
use takt_mir::program::{Binding, Direction};
use takt_syntax::ast;

use crate::lower::{Lowerer, SC2};

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
            let name = takt_mir::pattern::Address::simple(&c.address).ident();
            if self.imports_used.as_ref().is_some_and(|used| !used.contains(&name)) || self.bound(c) {
                continue;
            }
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
    fn an_enum_mapping_names_the_type() {
        let c = channel("direction = output\nraw = ValveCmd\nsafe = CLOSED\n");
        assert_eq!(declaration(&c, "v").as_deref(), Ok("output v : ValveCmd @ hw(\"daq1/ai0\") with safe = CLOSED\n"));
    }
}
