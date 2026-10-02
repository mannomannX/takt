//! Laufzeitprofile (12.8).
//!
//! Die Meilensteintabelle nennt fuer M4 `linux_rt`; die uebrigen zwei sind
//! Aufsatzpunkte, nicht Platzhalter (plan/m4.md 2.6). Der Unterschied ist
//! wichtig: Ein Platzhalter ist Code, der nichts tut, ein Aufsatzpunkt ist
//! eine Stelle, an der die Struktur schon stimmt. Was hier steht, sind die
//! Eigenschaften, die der Kern *braucht* — jede weitere gehoert in den
//! Aufsatz, nicht hierher.

/// Ein Laufzeitprofil (12.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Welches Profil.
    pub kind: Kind,
    /// Darf die Schleife schlafen (9.9)? Jedes Profil darf; ein Lauf, der
    /// mit einem Lauf ohne Schlaf verglichen wird (Satz 9.9.1), schaltet es ab.
    pub may_sleep: bool,
}

/// Die drei Profile aus 12.8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Kind {
    LinuxRt,
    Baremetal,
    Rtos,
}

impl Profile {
    /// `linux_rt` (12.2): PREEMPT_RT-Box, Zeitgarantie empirisch.
    pub const LINUX_RT: Profile = Profile { kind: Kind::LinuxRt, may_sleep: true };

    /// `baremetal` (12.3): `no_std` auf einer MCU, Budget statisch.
    pub const BAREMETAL: Profile = Profile { kind: Kind::Baremetal, may_sleep: true };

    /// `rtos` (12.8): Takt als hoechstpriore Aufgabe.
    pub const RTOS: Profile = Profile { kind: Kind::Rtos, may_sleep: true };

    /// Das Profil zu seinem Namen im `system:`-Block.
    pub fn by_name(name: &str) -> Option<Profile> {
        match name {
            "linux_rt" => Some(Profile::LINUX_RT),
            "baremetal" => Some(Profile::BAREMETAL),
            "rtos" => Some(Profile::RTOS),
            _ => None,
        }
    }

    /// Der Name des Profils; er steht im Lauf-Header (11.3).
    pub fn name(self) -> &'static str {
        match self.kind {
            Kind::LinuxRt => "linux_rt",
            Kind::Baremetal => "baremetal",
            Kind::Rtos => "rtos",
        }
    }
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::LINUX_RT
    }
}
