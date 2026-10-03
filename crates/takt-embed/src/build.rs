//! Der Bauhelfer fuer `build.rs` (12.11): ein Takt-Programm als Baustein
//! eines Rust-Projekts.
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     takt_embed::build::Program::new("takt/valve.takt").drivers("crate::Tank").build();
//! }
//! // src/main.rs
//! mod valve {
//!     include!(env!("TAKT_VALVE_RS"));
//! }
//! ```
//!
//! **Er ruft das Werkzeug, nicht die Bibliothek** (FB-138): Ein Bauskript,
//! das anders uebersetzte als `takt build`, waere eine zweite Quelle. Das
//! Werkzeug nennt `TAKT`, sonst steht es im `PATH`. Das Tripel ist das von
//! Cargo (`TARGET`); passt seine Float-ABI nicht zur Zielklasse, bricht der
//! Bau mit beiden Namen ab.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::{String, ToString};
use std::{env, format, println};

/// Ein Programm, das der Bau einbindet.
#[derive(Clone, Debug)]
pub struct Program {
    source: PathBuf,
    hardware: Option<PathBuf>,
    drivers: Option<String>,
    form: String,
    prefix: Option<String>,
}

impl Program {
    /// Das Programm in der Datei `source`, relativ zum Crate.
    pub fn new(source: impl Into<PathBuf>) -> Program {
        Program { source: source.into(), hardware: None, drivers: None, form: "logical".into(), prefix: None }
    }

    /// Die Hardware-Konfiguration (8.10): Zielklasse, Kalibrierung, Kanaele.
    pub fn hardware(mut self, path: impl Into<PathBuf>) -> Program {
        self.hardware = Some(path.into());
        self
    }

    /// Der Typ, der den Trait `Drivers` des Programms erfuellt, als Pfad im
    /// Crate (`crate::io::Board`); der erzeugte Kleber ruft ihn (12.6).
    pub fn drivers(mut self, ty: &str) -> Program {
        self.drivers = Some(ty.into());
        self
    }

    /// Die Form (12.11): `own`, `interrupt`, `poll`, `rtos`, `linux` oder
    /// `logical`. Ohne Angabe die logische Zeit, die Form eines Tests auf
    /// dem Wirt.
    pub fn form(mut self, form: &str) -> Program {
        self.form = form.into();
        self
    }

    /// Das Praefix der Einstiege (12.11); ohne Angabe der Dateiname.
    pub fn prefix(mut self, prefix: &str) -> Program {
        self.prefix = Some(prefix.into());
        self
    }

    /// Baut die Lieferform nach `OUT_DIR/takt/P`, bindet die Bibliothek und
    /// nennt das Modul in `TAKT_<P>_RS`. Liefert den Pfad des Moduls.
    ///
    /// # Panics
    ///
    /// Wenn der Bau scheitert: Die Meldungen von `takt build` stehen davor
    /// als Warnungen.
    pub fn build(self) -> PathBuf {
        let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR: nur aus einem Bauskript"));
        let triple = env::var("TARGET").expect("TARGET: nur aus einem Bauskript");
        let prefix = self.prefix.clone().unwrap_or_else(|| stem(&self.source));
        let dir = out.join("takt").join(&prefix);
        println!("cargo:rerun-if-env-changed=TAKT");
        // Die Konfigurationen aus `import channels` liegen neben dem Programm (8.2).
        if let Some(parent) = self.source.parent().filter(|p| !p.as_os_str().is_empty()) {
            println!("cargo:rerun-if-changed={}", parent.display());
        }
        println!("cargo:rerun-if-changed={}", self.source.display());
        let mut cmd = Command::new(tool());
        cmd.arg("build")
            .arg(&self.source)
            .args(["--emit", "embed", "--target", &triple, "--form", &self.form, "--prefix", &prefix])
            .arg("--out")
            .arg(&dir);
        if let Some(hw) = &self.hardware {
            println!("cargo:rerun-if-changed={}", hw.display());
            cmd.arg("--hardware").arg(hw);
        }
        if let Some(ty) = &self.drivers {
            cmd.args(["--drivers", ty]);
        }
        let output = cmd.output().unwrap_or_else(|e| panic!("`takt` nicht aufrufbar ({e}); `TAKT` nennt es"));
        if !output.status.success() {
            for line in String::from_utf8_lossy(&output.stderr).lines() {
                println!("cargo:warning={line}");
            }
            panic!("takt build --emit embed scheiterte fuer {}", self.source.display());
        }
        println!("cargo:rustc-link-search=native={}", dir.display());
        println!("cargo:rustc-link-lib=static={prefix}");
        let module = dir.join(format!("{prefix}.rs"));
        println!("cargo:rustc-env=TAKT_{}_RS={}", prefix.to_uppercase(), module.display());
        module
    }
}

/// Das Werkzeug: `TAKT`, sonst `takt` im `PATH`.
fn tool() -> PathBuf {
    env::var_os("TAKT").map_or_else(|| PathBuf::from("takt"), PathBuf::from)
}

/// Der Dateiname ohne Endung, das Praefix ohne Angabe.
fn stem(source: &Path) -> String {
    source.file_stem().map_or_else(|| "app".to_string(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_defaults_to_the_file_name() {
        assert_eq!(stem(Path::new("takt/valve.takt")), "valve");
    }
}
