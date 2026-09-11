//! Laufzeitprofile (12.8).
//!
//! Die Meilensteintabelle nennt fuer M4 `linux_rt`; die uebrigen drei sind
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
    /// Darf die Schleife schlafen (9.9)?
    ///
    /// `boot` darf nicht: Ein Startprogramm, das schlaeft, verzoegert den
    /// Start, und seine Laufzeit ist ohnehin kurz (12.8).
    pub may_sleep: bool,
}

/// Die vier Profile aus 12.8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Kind {
    LinuxRt,
    Baremetal,
    Rtos,
    Boot,
}

impl Profile {
    /// `linux_rt` (12.2): PREEMPT_RT-Box, Zeitgarantie empirisch.
    pub const LINUX_RT: Profile = Profile { kind: Kind::LinuxRt, may_sleep: true };

    /// `baremetal` (12.3): `no_std` auf einer MCU, Budget statisch.
    pub const BAREMETAL: Profile = Profile { kind: Kind::Baremetal, may_sleep: true };

    /// `rtos` (12.8): Takt als hoechstpriore Aufgabe.
    pub const RTOS: Profile = Profile { kind: Kind::Rtos, may_sleep: true };

    /// `boot` (12.8): Startprogramme, minimale Runtime, kein Schlaf.
    pub const BOOT: Profile = Profile { kind: Kind::Boot, may_sleep: false };

    /// Das Profil zu seinem Namen im `system:`-Block.
    pub fn by_name(name: &str) -> Option<Profile> {
        match name {
            "linux_rt" => Some(Profile::LINUX_RT),
            "baremetal" => Some(Profile::BAREMETAL),
            "rtos" => Some(Profile::RTOS),
            "boot" => Some(Profile::BOOT),
            _ => None,
        }
    }

    /// Der Name des Profils; er steht im Lauf-Header (11.3).
    pub fn name(self) -> &'static str {
        match self.kind {
            Kind::LinuxRt => "linux_rt",
            Kind::Baremetal => "baremetal",
            Kind::Rtos => "rtos",
            Kind::Boot => "boot",
        }
    }
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::LINUX_RT
    }
}
