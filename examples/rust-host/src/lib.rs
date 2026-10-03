//! Ein Fuellventil als Takt-Programm in einem Rust-Projekt (12.11).
//!
//! `build.rs` uebersetzt `takt/valve.takt` fuer das Ziel des Baus; das
//! Modul [`valve`] bringt Arena, Konstanten, den Trait `Drivers` und die
//! Huelle `Program`. Dieses Crate stellt nur die Geraete: [`Tank`].

/// Das Programm (erzeugt): `Arena`, `TICK_NS`, `Drivers`, `Program`.
#[allow(missing_docs, reason = "erzeugter Code; die Dokumentation steht im Programm")]
pub mod valve {
    include!(env!("TAKT_VALVE_RS"));
}

/// Das Ventil am Tank: stellt, was das Programm verlangt, und zaehlt, wie
/// oft es sich geoeffnet hat.
#[derive(Debug, Default)]
pub struct Tank {
    /// Ist das Ventil offen?
    pub open: bool,
    /// Wie oft es sich geoeffnet hat.
    pub openings: u32,
}

impl valve::Drivers for Tank {
    fn out_tank_valve(&mut self, value: bool, _now: i64) -> bool {
        self.openings += u32::from(value && !self.open);
        self.open = value;
        true
    }

    fn alive_tank(&mut self, _now: i64) -> bool {
        true
    }
}
