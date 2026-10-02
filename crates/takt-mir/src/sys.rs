//! Das eingebaute Geraet `sys` (12.7, 7.4): Anfang und Ende eines Laufs und
//! die Wanduhr. Pruefung 60 kennt seine Kanaele ohne Hardware-Konfiguration —
//! eine Konfiguration muss sie nicht wiederholen, und ein Tippfehler
//! (`sys/next_rnu`) faellt auf.
//!
//! Was eine Variante von `NextRun` verlangt, steht hier einmal: Interpreter
//! und Rahmen fragen dieselbe Tabelle, statt Variantennamen je selbst zu
//! vergleichen.

use crate::program::{Binding, Direction, Program};
use crate::types::Type;

/// Der Typ eines System-Channels, wie das Programm ihn deklarieren muss.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SysType {
    /// Ein vordefiniertes Enum.
    Enum(&'static str),
    /// `Duration`.
    Duration,
}

impl SysType {
    /// Der Typ, wie er im Programm steht.
    pub fn name(self) -> &'static str {
        match self {
            SysType::Enum(n) => n,
            SysType::Duration => "Duration",
        }
    }
}

/// Ein Kanal des Geraets `sys`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SysChannel {
    /// Die Adresse, `sys/…`.
    pub address: &'static str,
    /// Richtung aus Sicht des Programms.
    pub dir: Direction,
    /// Typ.
    pub ty: SysType,
}

const fn sys(address: &'static str, dir: Direction, ty: SysType) -> SysChannel {
    SysChannel { address, dir, ty }
}

/// Wie der vorige Lauf endete (12.7).
pub const PREVIOUS_RUN: &str = "sys/previous_run";

/// Wann der naechste Lauf beginnt (12.7); ein Wert ausser `NONE` beendet
/// den laufenden.
pub const NEXT_RUN: &str = "sys/next_run";

/// Die Kanaele des Geraets: Anfang und Ende eines Laufs (12.7) und die
/// Wanduhr (7.4).
pub const SYS: [SysChannel; 3] = [
    sys(PREVIOUS_RUN, Direction::Input, SysType::Enum("PreviousRun")),
    sys(NEXT_RUN, Direction::Output, SysType::Enum("NextRun")),
    sys("sys/clock", Direction::Input, SysType::Duration),
];

/// Der Kanal zu einer Adresse, wenn sie zum Geraet gehoert.
pub fn channel(address: &str) -> Option<&'static SysChannel> {
    SYS.iter().find(|c| c.address == address)
}

/// Gehoert die Adresse zum Geraet — auch als Tippfehler?
pub fn is_sys(address: &str) -> bool {
    address.starts_with("sys/")
}

/// Was eine Variante von `NextRun` ausser `NONE` verlangt (12.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextRun {
    /// `NOW`: der naechste Lauf beginnt sofort.
    Now,
    /// `AFTER(delay)`: nach der Dauer im ersten Feld, oder frueher, wenn
    /// eine Wake-Quelle weckt.
    After,
    /// `ON_WAKE`: wenn eine Wake-Quelle weckt.
    OnWake,
    /// `ON_START`: mit dem naechsten Start der Plattform.
    OnStart,
}

impl NextRun {
    /// Die Bedeutung einer Variante; `None` fuer `NONE` und fuer jede, die
    /// diese Fassung nicht kennt (das Enum ist offen, 2.5).
    pub fn of(variant: &str) -> Option<NextRun> {
        Some(match variant {
            "NOW" => NextRun::Now,
            "AFTER" => NextRun::After,
            "ON_WAKE" => NextRun::OnWake,
            "ON_START" => NextRun::OnStart,
            _ => return None,
        })
    }

    /// Das Wort der Zeile `end` (`grammar/trace.md`).
    pub fn word(self) -> &'static str {
        match self {
            NextRun::Now => "now",
            NextRun::After => "after",
            NextRun::OnWake => "on_wake",
            NextRun::OnStart => "on_start",
        }
    }
}

/// Je Variante von `NextRun`, in deren Reihenfolge, die Diskriminante und
/// was sie verlangt.
pub type Variants = Vec<(i64, Option<NextRun>)>;

/// Der Output an `sys/next_run`: sein Index unter den Channels und seine
/// Varianten. `None`, wenn das Programm ihn nicht bindet.
///
/// Nur `hw`: Ein `sim`-Output speist einen Input derselben Adresse (8.3),
/// und `sys/next_run` hat keinen. Ein anderes Enum an der Adresse lehnt
/// Pruefung 60 ab; hier zaehlt nur `NextRun`.
pub fn next_run(p: &Program) -> Option<(usize, Variants)> {
    let (index, channel) =
        p.channels.iter().enumerate().find(|(_, c)| {
            c.dir == Direction::Output && matches!(&c.binding, Binding::Hw(a) if a.text() == NEXT_RUN)
        })?;
    let Type::Enum(e) = p.types.list.get(channel.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    if def.name != "NextRun" {
        return None;
    }
    Some((index, def.variants.iter().map(|v| (v.discriminant, NextRun::of(&v.name))).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_but_none_ends_the_run() {
        for (variant, word) in [("NOW", "now"), ("AFTER", "after"), ("ON_WAKE", "on_wake"), ("ON_START", "on_start")] {
            assert_eq!(NextRun::of(variant).map(NextRun::word), Some(word));
        }
        assert_eq!(NextRun::of("NONE"), None);
    }

    #[test]
    fn the_device_knows_three_channels() {
        assert_eq!(channel(NEXT_RUN).map(|c| c.dir), Some(Direction::Output));
        assert_eq!(channel(PREVIOUS_RUN).map(|c| c.ty), Some(SysType::Enum("PreviousRun")));
        assert!(channel("sys/reboot").is_none() && is_sys("sys/reboot"));
    }
}
