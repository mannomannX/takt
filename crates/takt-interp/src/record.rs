//! Aufzeichnung und Wiedergabe (12.5, 11.3).
//!
//! **Eine Aufzeichnung ist ein Trace mit Kopf.** Das Format aus
//! `grammar/trace.md` ist zeilenorientiert, kanonisch und hat Vektoren;
//! ein zweites braechte einen zweiten Leser (plan/m4.md 6). Der Kopf
//! traegt, was 12.5 verlangt und eine Trace-Zeile nicht sagen kann:
//! welches Programm lief, mit welchen Parametern und welcher Toolchain.
//!
//! **Warum der Logik-Hash im Kopf steht.** 12.5 sagt: „Abweichung =
//! Fehler in Runtime oder Treiber, nie in der Logik (Satz 9.4.4)." Dieser
//! Satz gilt nur, wenn die Logik dieselbe ist — und genau das prueft der
//! Hash. Eine Aufzeichnung gegen ein geaendertes Programm abzuspielen
//! ergibt Abweichungen, die nichts bedeuten; `replay` lehnt das ab, statt
//! sie zu melden.
//!
//! **Was der Kopf nicht traegt: Zeitstempel und Pfade.** 11.3 verlangt
//! reproduzierbare Builds, und eine Aufzeichnung, die sich bei jedem Lauf
//! unterscheidet, waere schlecht zu vergleichen. Wann etwas lief, steht
//! in den Zeiten der Zeilen; *wo* es lief, gehoert nicht zur Semantik.

use std::fmt::Write as _;

use takt_mir::program::Program;

use crate::trace::Trace;

/// Formatversion der Aufzeichnung (11.3).
///
/// Leser akzeptieren aeltere Versionen ihres Formats, Schreiber schreiben
/// die neueste. Version 2: Die `param`-Zeilen tragen den Anfangsvektor
/// des Laufs — Defaults, Profil, Ueberlagerung (13.7) —, und `replay`
/// wendet ihn an; Version 1 nannte die Defaults.
pub const RECORDING_VERSION: u16 = 2;

/// Der Kopf einer Aufzeichnung (12.5, 11.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Formatversion dieser Datei.
    pub version: u16,
    /// Edition der Sprache (2.5); Teil des Logik-Hashs.
    pub edition: u32,
    /// Logik-Hash des Programms (11.3): die MIR ohne Bindungen.
    pub logic: String,
    /// Basis-Tick T0 in Nanosekunden.
    pub tick: i64,
    /// Wie viele Ticks der Lauf hatte.
    ///
    /// Sie gehoert in den Kopf, nicht in die Zeilen: Ein Lauf kann
    /// laenger sein als seine letzte Eingabe, und meistens ist er es.
    pub ticks: u64,
    /// Gewaehltes Profil (8.4), wenn eines gesetzt war.
    pub profile: Option<String>,
    /// Der Parametervektor zu Beginn des Laufs: Name und Wert je Parameter.
    pub params: Vec<(String, String)>,
    /// Laufzeitprofil (12.8).
    pub target: Option<String>,
    /// Was die Runtime ueber ihre Umgebung meldet (12.2, 11.3).
    ///
    /// Der Interpreter weiss davon nichts — er laeuft ueberall, das
    /// Laufzeitprofil nur auf seiner Plattform. Die Zeilen kommen darum
    /// von der Runtime (`takt-rt-linux::Guarantee::header_lines`), und
    /// der Kopf traegt sie unveraendert.
    ///
    /// Warum sie hierher gehoeren: Eine Zeitgarantie, die still
    /// ausfaellt, ist schlimmer als eine, die fehlt — eine Messung unter
    /// ihr sieht gueltig aus. Der Unterschied zwischen „lief unter
    /// `linux_rt`" und „lief unter `linux_rt` *mit* der Zusage" gehoert
    /// in die Aufzeichnung, nicht in die Erinnerung des Bedieners.
    pub runtime: Vec<String>,
    /// Native Funktionen, die das Programm benutzt (4.5).
    ///
    /// 4.5 verlangt ihre Nennung im Kopf, „damit die erweiterte TCB
    /// sichtbar bleibt". Bei der kuratierten Menge ist die Liste kurz;
    /// bei Projekt-Natives (v1.1) ist sie der Punkt.
    pub natives: Vec<String>,
    /// Irreversible Outputs (12.7): der Lauf-Header nennt sie alle.
    pub irreversible: Vec<String>,
}

impl Header {
    /// Der Kopf eines Laufs.
    pub fn of(p: &Program, profile: Option<&str>, params: &[(String, String)], ticks: u64) -> Header {
        Header {
            version: RECORDING_VERSION,
            edition: p.config.edition,
            ticks,
            logic: takt_mir::hash::logic_hash(p).to_string(),
            tick: p.config.tick,
            profile: profile.map(str::to_string),
            params: params.to_vec(),
            target: p.config.target.clone(),
            runtime: Vec::new(),
            natives: p.natives.iter().map(|n| n.name.clone()).collect(),
            irreversible: p.channels.iter().filter(|c| c.attrs.irreversible).map(|c| c.name.clone()).collect(),
        }
    }

    /// Der Kopf als Text; jede Zeile beginnt mit `#!`.
    ///
    /// Das Praefix trennt ihn von Kommentaren (`#`) und von Trace-Zeilen,
    /// ohne dass ein Leser den Zustand wechseln muss: Eine Zeile gehoert
    /// zum Kopf, wenn sie so beginnt.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "#! takt-aufzeichnung {}", self.version);
        let _ = writeln!(out, "#! edition {}", self.edition);
        let _ = writeln!(out, "#! logik {}", self.logic);
        let _ = writeln!(out, "#! tick {}", self.tick);
        let _ = writeln!(out, "#! ticks {}", self.ticks);
        if let Some(p) = &self.profile {
            let _ = writeln!(out, "#! profil {p}");
        }
        if let Some(t) = &self.target {
            let _ = writeln!(out, "#! target {t}");
        }
        for (name, value) in &self.params {
            let _ = writeln!(out, "#! param {name} {value}");
        }
        for line in &self.runtime {
            let _ = writeln!(out, "#! runtime {line}");
        }
        for n in &self.natives {
            let _ = writeln!(out, "#! native {n}");
        }
        for o in &self.irreversible {
            let _ = writeln!(out, "#! irreversibel {o}");
        }
        out
    }

    /// Der Parametervektor, den `replay` anwendet (12.5): ab Version 2
    /// steht er im Kopf; Version 1 nannte die Defaults, die ohnehin gelten.
    pub fn overrides(&self) -> Vec<(String, String)> {
        if self.version >= 2 { self.params.clone() } else { Vec::new() }
    }

    /// Liest einen Kopf; `None`, wenn keine Kopfzeile dasteht.
    pub fn parse(text: &str) -> Option<Header> {
        let mut h = Header {
            version: 0,
            edition: 0,
            logic: String::new(),
            tick: 0,
            ticks: 0,
            runtime: Vec::new(),
            profile: None,
            params: Vec::new(),
            target: None,
            natives: Vec::new(),
            irreversible: Vec::new(),
        };
        let mut seen = false;
        for line in text.lines() {
            let Some(rest) = line.trim_start().strip_prefix("#!") else { continue };
            let mut w = rest.split_whitespace();
            let Some(key) = w.next() else { continue };
            let value = w.next().unwrap_or("");
            match key {
                "takt-aufzeichnung" => {
                    h.version = value.parse().unwrap_or(0);
                    seen = true;
                }
                "edition" => h.edition = value.parse().unwrap_or(0),
                "logik" => h.logic = value.to_string(),
                "tick" => h.tick = value.parse().unwrap_or(0),
                "ticks" => h.ticks = value.parse().unwrap_or(0),
                "profil" => h.profile = Some(value.to_string()),
                "target" => h.target = Some(value.to_string()),
                "irreversibel" => h.irreversible.push(value.to_string()),
                "param" => h.params.push((value.to_string(), w.collect::<Vec<_>>().join(" "))),
                "runtime" => {
                    let rest: Vec<&str> = w.collect();
                    h.runtime.push(if rest.is_empty() {
                        value.to_string()
                    } else {
                        format!("{value} {}", rest.join(" "))
                    });
                }
                "native" => h.natives.push(value.to_string()),
                _ => {}
            }
        }
        seen.then_some(h)
    }
}

/// Eine Aufzeichnung: Kopf und Eingaben (12.5).
///
/// Aufgezeichnet werden die *Eingaben*, nicht die Ausgaben. Das ist die
/// Konstruktion aus Satz 9.4.1: Der Trace ist eine Funktion von (s0, I_k),
/// also genuegt I_k, um ihn zu reproduzieren. Die Ausgaben mit
/// aufzuzeichnen waere das, was man vergleichen will — und eine Datei,
/// die schon enthaelt, was sie belegen soll, belegt nichts.
#[derive(Clone, Debug)]
pub struct Recording {
    /// Der Kopf.
    pub header: Header,
    /// Die Eingaben als Trace-Zeilen.
    pub inputs: Trace,
}

impl Recording {
    /// Die Aufzeichnung als Text.
    pub fn render(&self) -> String {
        format!("{}{}", self.header.render(), self.inputs.render())
    }

    /// Liest eine Aufzeichnung.
    pub fn parse(text: &str) -> Result<Recording, String> {
        let header = Header::parse(text).ok_or("kein Kopf: erwartet `#! takt-aufzeichnung <version>`")?;
        if header.version > RECORDING_VERSION {
            return Err(format!(
                "Aufzeichnung der Version {} ist neuer als diese Fassung ({RECORDING_VERSION}); \
                 Leser akzeptieren nur aeltere Versionen (11.3)",
                header.version
            ));
        }
        let inputs = Trace::parse(text)?;
        Ok(Recording { header, inputs })
    }

    /// Prueft, ob diese Aufzeichnung zu einem Programm gehoert (12.5).
    ///
    /// Satz 9.4.4 gilt nur, wenn die Logik dieselbe ist. Eine
    /// Aufzeichnung gegen ein geaendertes Programm abzuspielen ergibt
    /// Abweichungen, die nichts bedeuten — die Ablehnung ist die
    /// ehrlichere Antwort.
    pub fn matches(&self, p: &Program) -> Result<(), String> {
        let want = takt_mir::hash::logic_hash(p).to_string();
        if self.header.logic != want {
            return Err(format!(
                "die Aufzeichnung gehoert zu einem anderen Programm:\n  \
                 aufgezeichnet {}\n  geladen       {want}\n  \
                 (12.5: eine Abweichung waere dann kein Befund, sondern ein anderes Programm)",
                self.header.logic
            ));
        }
        if self.header.edition != p.config.edition {
            return Err(format!(
                "die Aufzeichnung nennt Edition {}, das Programm {} (2.5)",
                self.header.edition, p.config.edition
            ));
        }
        Ok(())
    }
}
