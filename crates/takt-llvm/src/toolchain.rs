//! Die LLVM-Werkzeugkette finden (plan/m4.md 4.1).
//!
//! Der Compiler braucht LLVM **nicht**, um zu bauen oder seine IR zu
//! pruefen — das ist der Gewinn der Textausgabe. Gebraucht wird es fuer
//! die Abnahme (Schritt 8): Die erzeugte IR muss uebersetzt und
//! *ausgefuehrt* werden, sonst prueft das differentielle Testen nur, dass
//! zwei Textdateien gleich aussehen.
//!
//! **Warum `clang` und nicht `llc`.** Die gaengige Windows-Distribution
//! (winget `LLVM.LLVM`) liefert nur die Clang-Werkzeuge; `llc`, `opt` und
//! `llvm-as` sind nicht dabei. `clang` kann alles, was die Abnahme
//! braucht: `-c` assembliert wie `llc`, `-O2` optimiert wie `opt`, und es
//! linkt und erzeugt ein lauffaehiges Programm. Ein Werkzeug, das
//! ueberall vorhanden ist, ist einem vorzuziehen, das man nachinstallieren
//! muss.
//!
//! **Warum die Suche und nicht nur `PATH`.** Ein frisch installiertes
//! LLVM steht erst nach einer neuen Sitzung im `PATH` laufender Prozesse.
//! Die Suche findet es trotzdem, und wer es woanders hat, setzt
//! `TAKT_CLANG`.

use std::path::PathBuf;
use std::process::Command;

/// Wo `clang` steckt, oder warum nicht.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clang {
    /// Gefunden.
    At(PathBuf),
    /// Nicht gefunden; die Abnahme laeuft dann nicht.
    Missing,
}

/// Die Stellen, an denen `clang` ueblicherweise steht.
///
/// `TAKT_CLANG` gewinnt, dann der `PATH`, dann die bekannten Orte. Die
/// Reihenfolge ist die uebliche: Was jemand ausdruecklich setzt, gilt.
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("TAKT_CLANG") {
        out.push(PathBuf::from(p));
    }
    out.push(PathBuf::from("clang"));
    for p in [
        r"C:\Program Files\LLVM\bin\clang.exe",
        r"C:\Program Files (x86)\LLVM\bin\clang.exe",
        "/usr/bin/clang",
        "/usr/local/bin/clang",
        "/opt/homebrew/opt/llvm/bin/clang",
    ] {
        out.push(PathBuf::from(p));
    }
    out
}

/// Sucht `clang`.
pub fn find() -> Clang {
    for path in candidates() {
        if Command::new(&path).arg("--version").output().is_ok_and(|o| o.status.success()) {
            return Clang::At(path);
        }
    }
    Clang::Missing
}

impl Clang {
    /// Was jeder Aufruf mitbekommt, damit das Ergebnis reproduzierbar
    /// ist (11.3).
    ///
    /// **`SOURCE_DATE_EPOCH` ist der Standard dafuer**, und clang wie
    /// LLD lesen ihn: Er ersetzt jeden Zeitstempel im Ergebnis durch
    /// einen festen Wert. Ohne ihn traegt ein PE-Binary den Build-
    /// Zeitpunkt in seinem Kopf (`TimeDateStamp`), und zwei
    /// Uebersetzungen derselben Quelle unterscheiden sich in genau
    /// diesem einen Feld — gemessen an `23_patterns`: ein Byte von
    /// 151552.
    ///
    /// Der Wert ist null, nicht die aktuelle Zeit: 11.3 verlangt, dass
    /// „gleiche Quelle plus gleiche Toolchain-Version" genuegt, und eine
    /// Zeit, die der Compiler selbst waehlt, waere eine dritte Eingabe.
    ///
    /// Die Flags stehen hier und nicht an den Aufrufstellen, weil sie
    /// eine Zusage sind und keine Vorliebe: Wer clang ruft, ruft ihn so.
    pub fn deterministic(cmd: &mut Command) -> &mut Command {
        cmd.env("SOURCE_DATE_EPOCH", "0")
    }

    /// Der Pfad, wenn gefunden.
    pub fn path(&self) -> Option<&PathBuf> {
        match self {
            Clang::At(p) => Some(p),
            Clang::Missing => None,
        }
    }

    /// Prueft eine IR-Datei, ohne sie auszufuehren: Sie muss sich
    /// assemblieren lassen.
    ///
    /// `-Wno-override-module` unterdrueckt eine Warnung, die kein Fehler
    /// ist: clang ersetzt das Target-Triple der Datei durch das eigene,
    /// vollstaendigere. Fuer die Abnahme ist das richtig — geprueft wird
    /// die IR, nicht die Schreibweise des Triples.
    pub fn assembles(&self, ir: &str, dir: &std::path::Path) -> Result<(), String> {
        let path = self.path().ok_or("clang nicht gefunden")?;
        let ll = dir.join("modul.ll");
        let obj = dir.join("modul.o");
        std::fs::write(&ll, ir).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(path);
        let out = Clang::deterministic(&mut cmd)
            .args(["-c", "-Wno-override-module"])
            .arg(&ll)
            .arg("-o")
            .arg(&obj)
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            return Ok(());
        }
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }

    /// Uebersetzt ein Modul mit `main` und fuehrt es aus; liefert seine
    /// Ausgabe. Das ist der Weg der Abnahme (Schritt 8).
    pub fn run(&self, ir: &str, dir: &std::path::Path) -> Result<String, String> {
        let path = self.path().ok_or("clang nicht gefunden")?;
        let ll = dir.join("lauf.ll");
        let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
        std::fs::write(&ll, ir).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(path);
        let build = Clang::deterministic(&mut cmd)
            .args(["-Wno-override-module", "-O2"])
            .arg(&ll)
            .arg("-o")
            .arg(&exe)
            .output()
            .map_err(|e| e.to_string())?;
        if !build.status.success() {
            return Err(String::from_utf8_lossy(&build.stderr).to_string());
        }
        let out = Command::new(&exe).output().map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
}
