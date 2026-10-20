//! Der Speicherschutz neben fremdem Code (12.3, 12.11; plan/m11.md 2.11).
//!
//! Was die Arena vor fremdem Code schuetzt, haengt davon ab, wem die
//! Schutzeinheit gehoert. Der Fall steht im Lauf-Header (11.3) als Kopfzeile
//! `Schutz: …` und im Konformitaetsbericht (`[port <name>]`): Ein
//! schwaecherer Schutz ist sichtbar, nicht angenommen. Den Fall nennt, wer
//! ihn herstellt — der Speicherschutz des Boards (`takt_rt_baremetal::
//! Protection`), der Adapter eines RTOS aus dessen Konfiguration —, nicht der
//! Wirt aus freien Stuecken.

/// Wie die Arena neben fremdem Code geschuetzt ist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    /// Die Schutzeinheit gehoert Takt (eigener Kern, Interrupt- und
    /// Pollform, Planer ohne eigene Verwaltung der Schutzeinheit): Die Arena
    /// ist eine eigene Region, beschreibbar nur im Schritt (12.3). Eine
    /// fremde ISR, die den Schritt unterbricht, findet sie offen.
    Window,
    /// Sie gehoert dem RTOS: Die Arena liegt in einer Partition, die nur der
    /// Schrittkontext beschreiben darf; geschuetzt gegen Aufgaben ohne
    /// Privileg, nicht gegen privilegierten Code.
    Partition,
    /// Sie gehoert niemandem, oder keine Region ist frei: Fremder Code im
    /// selben Adressraum gehoert zur TCB.
    Unprotected,
}

/// Das Wort der Kopfzeile vor dem Fall.
pub const LABEL: &str = "Schutz: ";

impl Case {
    /// Jeder Fall.
    pub const ALL: [Case; 3] = [Case::Window, Case::Partition, Case::Unprotected];

    /// Der Fall, wie die Kopfzeile ihn nennt (12.11).
    pub const fn name(self) -> &'static str {
        match self {
            Case::Window => "Fenster",
            Case::Partition => "Partition",
            Case::Unprotected => "keiner",
        }
    }

    /// Die Kopfzeile ohne Zeilenende, eingerueckt wie die uebrigen Zeilen
    /// des Kopfs (`  Form …`).
    pub const fn line(self) -> &'static str {
        match self {
            Case::Window => "  Schutz: Fenster",
            Case::Partition => "  Schutz: Partition",
            Case::Unprotected => "  Schutz: keiner",
        }
    }

    /// Der Fall zu seinem Namen.
    pub fn from_name(name: &str) -> Option<Case> {
        Case::ALL.into_iter().find(|c| c.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Jede Kopfzeile ist das Wort und der Name ihres Falls**, und der
    /// Name fuehrt zurueck: Die C-Ports schreiben dieselben Zeilen
    /// (`ports/rtos/takt_rtos.h`, `ports/irq/takt_irq.h`), der Wirt liest sie.
    #[test]
    fn every_line_names_its_case() {
        for case in Case::ALL {
            assert_eq!(case.line().trim_start().strip_prefix(LABEL), Some(case.name()), "{case:?}");
            assert_eq!(Case::from_name(case.name()), Some(case));
        }
        assert_eq!(Case::from_name("fenster"), None, "der Name ist genau der der Referenz");
    }
}
