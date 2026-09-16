//! Tunables aus einer Datei (8.4, 12.5): Zeilen `t=<k> tune <name> <wert>`
//! in der Form der Aufzeichnung (`grammar/trace.md`), damit ein
//! aufgezeichneter Lauf live wiederholt werden kann. Die Datei wird beim
//! Oeffnen gelesen — vor dem ersten Tick, wo es keine Deadline gibt.
//!
//! Name und Wert loest der Aufsatz auf: Er kennt das Programm, die Runtime
//! nicht. `resolve` liefert Parameterindex und Wert in kanonischer
//! Byteform oder `None` fuer eine Zeile, die verworfen wird.

use std::path::Path;

use takt_rt_core::loopcore::Tunables;

/// Loest Name und Wert einer Zeile in Parameterindex und Byteform auf.
pub type Resolve<'a> = &'a dyn Fn(&str, &str) -> Option<(u32, Vec<u8>)>;

/// Eine Aenderung, aufgeloest und noch nicht ausgeliefert.
struct Change {
    tick: u64,
    param: u32,
    value: Vec<u8>,
}

/// Die `tune`-Zeilen einer Datei.
pub struct FileTunables {
    changes: Vec<Change>,
    next: usize,
}

impl FileTunables {
    /// Liest die Datei; `resolve(name, wert)` gibt Index und Byteform.
    pub fn open(path: &Path, resolve: Resolve<'_>) -> std::io::Result<FileTunables> {
        let text = std::fs::read_to_string(path)?;
        Ok(FileTunables::parse(&text, resolve))
    }

    /// Liest die Zeilen eines Textes; andere Zeilenarten werden ueberlesen.
    pub fn parse(text: &str, resolve: Resolve<'_>) -> FileTunables {
        let mut changes = Vec::new();
        for line in text.lines() {
            let mut words = line.trim().splitn(4, ' ');
            let Some(tick) = words.next().and_then(|w| w.strip_prefix("t=")).and_then(|t| t.parse::<u64>().ok()) else {
                continue;
            };
            if words.next() != Some("tune") {
                continue;
            }
            let (Some(name), Some(value)) = (words.next(), words.next()) else { continue };
            if let Some((param, value)) = resolve(name, value.trim()) {
                changes.push(Change { tick, param, value });
            }
        }
        // Stabil nach Tick: Zeilen desselben Ticks bleiben in ihrer Reihenfolge.
        changes.sort_by_key(|c| c.tick);
        FileTunables { changes, next: 0 }
    }

    /// Wie viele Aenderungen noch ausstehen.
    pub fn pending(&self) -> usize {
        self.changes.len() - self.next
    }
}

impl Tunables for FileTunables {
    fn poll(&mut self, k: u64, apply: &mut dyn FnMut(u32, &[u8])) {
        // Alles bis einschliesslich `k`: Ein verpasster Tick (Schlaf, 9.9)
        // holt seinen Satz nach, statt ihn zu verlieren.
        while let Some(c) = self.changes.get(self.next) {
            if c.tick > k {
                break;
            }
            apply(c.param, &c.value);
            self.next += 1;
        }
    }
}
