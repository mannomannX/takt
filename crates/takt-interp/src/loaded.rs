//! Geladenes Programm: die MIR plus abgeleitete Tabellen, die `eval` und
//! `exec` je Knoten brauchen (Breiten je Typ). Der Verifier (`validate`)
//! prueft die MIR einmal beim Laden; danach indiziert die Ausfuehrung frei.

use takt_diag::Diagnostic;
use takt_mir::program::Program;
use takt_mir::types::{FloatWidth, IntWidth, Type, UnitDef};
use takt_mir::{TypeId, UnitId};

use crate::validate;

/// Programm mit Tabellen fuer die Ausfuehrung.
pub struct Loaded<'p> {
    /// Die MIR.
    pub program: &'p Program,
    int_widths: Vec<Option<IntWidth>>,
    float_widths: Vec<Option<FloatWidth>>,
}

impl<'p> Loaded<'p> {
    /// Prueft die MIR (Kernform, Indizes, Stufenknoten) und legt die Tabellen an.
    pub fn load(program: &'p Program) -> Result<Self, Diagnostic> {
        validate::check(program)?;
        Ok(Self::borrow(program))
    }

    /// Tabellen ohne Pruefung (Konstantenauswertung waehrend des Lowerings).
    pub fn borrow(program: &'p Program) -> Self {
        let int_widths = program
            .types
            .list
            .iter()
            .map(|t| match t {
                Type::Int { width, .. } => Some(*width),
                _ => None,
            })
            .collect();
        let float_widths = program
            .types
            .list
            .iter()
            .map(|t| match t {
                Type::Float { width, .. } => Some(*width),
                Type::Mat { .. } => Some(program.config.float_width),
                _ => None,
            })
            .collect();
        Loaded { program, int_widths, float_widths }
    }

    /// Typ zu einem Index.
    pub fn ty(&self, id: TypeId) -> &Type {
        self.program.types.get(id)
    }

    /// Breite eines Integer-Typs.
    pub fn int_width(&self, id: TypeId) -> Option<IntWidth> {
        self.int_widths.get(id.index()).copied().flatten()
    }

    /// Breite eines Fliesskommatyps.
    pub fn float_width(&self, id: TypeId) -> Option<FloatWidth> {
        self.float_widths.get(id.index()).copied().flatten()
    }

    /// Breite von `float` im Programm (4.2).
    pub fn program_float(&self) -> FloatWidth {
        self.program.config.float_width
    }

    /// Einheit.
    pub fn unit(&self, id: UnitId) -> &UnitDef {
        &self.program.units[id.index()]
    }
}
