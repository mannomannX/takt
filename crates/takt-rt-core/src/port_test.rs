//! Die fremde Last der Portpruefung (12.11, 13.8; `takt port-test`).
//!
//! Die Portpruefung verlangt, dass fremder Code mit der FPU rechnet, waehrend
//! Takt es tut: Rechnet eine andere Aufgabe mit der FPU, muss der Port die
//! Register sichern, wenn der Schritt sie unterbricht (plan/m11.md 2.10).
//! Der Wirt ruft [`ForeignLoad::round`] in seiner fremden Aufgabe — der
//! Hauptschleife, der Idle-Aufgabe, einer Aufgabe niedriger Prioritaet —, so
//! oft er kann, und schreibt vor der Bilanz die Zeile [`LINE`] mit
//! [`ForeignLoad::rounds`] und [`ForeignLoad::errors`].
//!
//! **Warum eine Drehung.** Eine Runde dreht einen Vektor [`STEPS`] Mal um
//! denselben Winkel, ganz in Registern der FPU. Eine Drehung erhaelt jeden
//! Fehler: Ueberschreibt eine ISR ein Register mitten in der Runde, liegt
//! das Ergebnis daneben. Eine Kette, die auf einen Fixpunkt zulaeuft, haette
//! den Fehler bis zum Ende der Runde wieder vergessen. Das Soll rechnet der
//! Compiler vorab ([`EXPECTED`]); IEEE 754 legt jede Operation fest, also
//! gleicht es dem Ergebnis auf jedem Ziel, mit und ohne FPU.

/// Die Zeile vor der Bilanz: `takt fremd fpu runden N fehler M`.
pub const LINE: &str = "takt fremd fpu runden ";

/// Das Wort vor der Zahl der falschen Runden in [`LINE`].
pub const ERRORS: &str = " fehler ";

/// Drehungen je Runde.
pub const STEPS: u32 = 64;

/// Kosinus und Sinus des Drehwinkels (etwa 0,1).
const COS: f32 = 0.995_004_2;
const SIN: f32 = 0.099_833_42;

/// Der Vektor vor der ersten Drehung.
const START: (f32, f32) = (1.0, 0.0);

/// Das Ergebnis einer Runde, vom Compiler gerechnet.
pub const EXPECTED: (u32, u32) = {
    let (x, y) = rotate(START.0, START.1);
    (x.to_bits(), y.to_bits())
};

/// Dreht `(x, y)` [`STEPS`] Mal.
#[expect(clippy::arithmetic_side_effects, reason = "MISRA C:2025 D.4.1: i < STEPS, der Zaehler laeuft nicht ueber")]
const fn rotate(mut x: f32, mut y: f32) -> (f32, f32) {
    let mut i = 0;
    while i < STEPS {
        let turned = x * COS - y * SIN;
        y = x * SIN + y * COS;
        x = turned;
        i += 1;
    }
    (x, y)
}

/// Die Runden der fremden Aufgabe und wie viele davon falsch rechneten.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForeignLoad {
    rounds: u64,
    errors: u64,
}

impl ForeignLoad {
    /// Noch keine Runde.
    pub const fn new() -> ForeignLoad {
        ForeignLoad { rounds: 0, errors: 0 }
    }

    /// Eine Runde: dreht und vergleicht mit dem Soll. `black_box` haelt den
    /// Compiler davon ab, die Runde schon beim Bau zu rechnen.
    pub fn round(&mut self) {
        let (x, y) = core::hint::black_box(START);
        let (x, y) = rotate(x, y);
        self.rounds = self.rounds.wrapping_add(1);
        if (x.to_bits(), y.to_bits()) != EXPECTED {
            self.errors = self.errors.wrapping_add(1);
        }
    }

    /// Gerechnete Runden.
    pub const fn rounds(&self) -> u64 {
        self.rounds
    }

    /// Runden mit falschem Ergebnis.
    pub const fn errors(&self) -> u64 {
        self.errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Runde zur Laufzeit gleicht dem Soll des Compilers.
    #[test]
    fn a_round_matches_what_the_compiler_computed() {
        let mut load = ForeignLoad::new();
        for _ in 0..3 {
            load.round();
        }
        assert_eq!((load.rounds(), load.errors()), (3, 0));
    }

    /// Ein ueberschriebenes Register mitten in der Runde bleibt im Ergebnis:
    /// Die Drehung vergisst keinen Fehler.
    #[test]
    fn a_clobbered_register_shows_in_the_result() {
        let (mut x, mut y) = START;
        for i in 0..STEPS {
            if i == STEPS / 2 {
                x = 0.0;
            }
            let turned = x * COS - y * SIN;
            y = x * SIN + y * COS;
            x = turned;
        }
        assert_ne!((x.to_bits(), y.to_bits()), EXPECTED);
    }
}
