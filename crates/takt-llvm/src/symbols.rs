//! Die externen Namen einer Programmbibliothek (12.11).
//!
//! Jedes externe Symbol eines Programms traegt sein Praefix, damit mehrere
//! Programme in einem Abbild liegen: die Einstiege `P_<maschine>_<einstieg>`
//! ([`crate::arena::entry_symbol`]), die Aufrufe in den Rahmen (`P_now`,
//! `P_stream_count`, …), die Einstiege des Rahmens (`P_tick`, …) und die
//! Treiber (`P_in_<adresse>`, …). Was alle Programme teilen — `takt_m_*`,
//! `takt_native_*`, `takt_edge_*` —, liegt einmal im Abbild unter `takt_`;
//! ein Praefix darf darum weder `takt` heissen noch so beginnen.

use std::fmt;

/// Das Praefix eines Programms: ein C-Bezeichner aus Kleinbuchstaben,
/// Ziffern und `_` ausser `takt` und `takt_…`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Prefix(String);

impl Prefix {
    /// Prueft `name` als Praefix.
    ///
    /// Ein fuehrender Unterstrich ist ausgeschlossen, weil C solche Namen
    /// auf Dateiebene der Implementierung vorbehaelt. Grossbuchstaben sind
    /// es, damit sich zwei Praefixe nie nur in der Schreibweise
    /// unterscheiden: `valve` und `VALVE` gaeben denselben Kopfwaechter
    /// `TAKT_VALVE_H` und unter Windows dieselbe Datei (12.11, GEN-019).
    pub fn new(name: &str) -> Result<Prefix, String> {
        let mut chars = name.chars();
        let Some(first) = chars.next() else { return Err("das Praefix ist leer".into()) };
        if !first.is_ascii_lowercase() || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(format!(
                "`{name}` ist kein Praefix: verlangt ist ein C-Bezeichner aus Kleinbuchstaben, Ziffern und `_`, \
                 der mit einem Buchstaben beginnt (12.11)"
            ));
        }
        if name == "takt" || name.starts_with("takt_") {
            return Err(format!("`{name}` ist kein Praefix: `takt` gehoert den geteilten Bibliotheken (12.11)"));
        }
        Ok(Prefix(name.to_string()))
    }

    /// Das Praefix selbst.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Der externe Name `P_<name>`.
    pub fn name(&self, name: &str) -> String {
        format!("{}_{name}", self.0)
    }
}

/// `app`: das Praefix der eigenen Bring-ups und der Testrahmen.
impl Default for Prefix {
    fn default() -> Prefix {
        Prefix("app".to_string())
    }
}

impl fmt::Display for Prefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Prefix;

    #[test]
    fn a_prefix_is_a_c_identifier_outside_takt() {
        assert_eq!(Prefix::new("valve").map(|p| p.name("init")), Ok("valve_init".to_string()));
        assert!(Prefix::new("pump_2").is_ok());
        for bad in ["", "01_minimal", "_valve", "valve-2", "takt", "takt_m", "ventil\u{e4}", "VALVE", "Valve", "vALVE"]
        {
            assert!(Prefix::new(bad).is_err(), "`{bad}` gilt als Praefix");
        }
    }

    /// GEN-019: Ein C-Schluesselwort ist ein Praefix, denn es steht nie
    /// allein — jeder Name traegt dahinter `_` (`int_tick`), und der
    /// Kopfwaechter beginnt mit `TAKT_`. Ein langes Praefix bleibt ein
    /// Praefix: 12.11 setzt keine Laenge.
    #[test]
    fn a_keyword_or_a_long_name_is_a_prefix() {
        assert_eq!(Prefix::new("int").map(|p| p.name("tick")), Ok("int_tick".to_string()));
        assert!(Prefix::new("static").is_ok());
        let long = "a".repeat(40);
        assert_eq!(Prefix::new(&long).map(|p| p.name("init")), Ok(format!("{long}_init")));
    }
}
