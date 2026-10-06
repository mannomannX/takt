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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{env, format, println, vec};

/// Ein Programm, das der Bau einbindet.
#[derive(Clone, Debug)]
pub struct Program {
    source: PathBuf,
    hardware: Option<PathBuf>,
    drivers: Option<String>,
    form: String,
    prefix: Option<String>,
    tool: Option<PathBuf>,
    build: Option<String>,
    instrument: Option<String>,
    diagnostics: Option<String>,
}

impl Program {
    /// Das Programm in der Datei `source`, relativ zum Crate.
    pub fn new(source: impl Into<PathBuf>) -> Program {
        Program {
            source: source.into(),
            hardware: None,
            drivers: None,
            form: "logical".into(),
            prefix: None,
            tool: None,
            build: None,
            instrument: None,
            diagnostics: None,
        }
    }

    /// Das Werkzeug; ohne Angabe `TAKT`, sonst `takt` im `PATH`.
    pub fn tool(mut self, path: impl Into<PathBuf>) -> Program {
        self.tool = Some(path.into());
        self
    }

    /// Der Build (8.3): `hw` oder `sim`, in dem Plant-Modelle mitlaufen;
    /// ohne Angabe der Hardware-Bau der Lieferform.
    pub fn build_for(mut self, build: &str) -> Program {
        self.build = Some(build.into());
        self
    }

    /// Die Instrumentierung (11.2): `none`, `states` oder `statements`.
    pub fn instrument(mut self, mode: &str) -> Program {
        self.instrument = Some(mode.into());
        self
    }

    /// Die Diagnosestufe des erzeugten Codes (11.2).
    pub fn diagnostics(mut self, level: &str) -> Program {
        self.diagnostics = Some(level.into());
        self
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

    /// Was der Bau ruft und wohin er legt, fuer das Ausgabeverzeichnis `out`
    /// und das Tripel `triple`: ohne Umgebung, damit es sich pruefen laesst.
    pub fn invocation(&self, out: &Path, triple: &str) -> Invocation {
        let prefix = self.prefix.clone().unwrap_or_else(|| stem(&self.source));
        let dir = out.join("takt").join(&prefix);
        let mut args: Vec<OsString> = vec!["build".into(), self.source.clone().into_os_string()];
        for word in ["--emit", "embed", "--target", triple, "--form", &self.form, "--prefix", &prefix, "--out"] {
            args.push(word.into());
        }
        args.push(dir.clone().into_os_string());
        if let Some(hw) = &self.hardware {
            args.push("--hardware".into());
            args.push(hw.clone().into_os_string());
        }
        if let Some(ty) = &self.drivers {
            args.push("--drivers".into());
            args.push(ty.into());
        }
        for (flag, value) in
            [("--build", &self.build), ("--instrument", &self.instrument), ("--diagnostics", &self.diagnostics)]
        {
            if let Some(v) = value {
                args.push(flag.into());
                args.push(v.into());
            }
        }
        let module = dir.join(format!("{prefix}.rs"));
        let env = format!("TAKT_{}_RS", prefix.to_uppercase());
        Invocation { args, dir, module, prefix, env }
    }

    /// Baut die Lieferform nach `OUT_DIR/takt/P`, bindet die Bibliothek und
    /// nennt das Modul in `TAKT_<P>_RS`. Liefert, was entstand.
    ///
    /// # Panics
    ///
    /// Wenn der Bau scheitert: Die Meldungen von `takt build` stehen davor
    /// als Warnungen.
    pub fn build(self) -> Built {
        let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR: nur aus einem Bauskript"));
        let triple = env::var("TARGET").expect("TARGET: nur aus einem Bauskript");
        let call = self.invocation(&out, &triple);
        let tool = match &self.tool {
            Some(path) => path.clone(),
            None => {
                println!("cargo:rerun-if-env-changed=TAKT");
                tool()
            }
        };
        // Auch das Werkzeug ist eine Quelle: Ein neues `takt` uebersetzt
        // anders, und ohne diese Zeile bliebe das alte Modul stehen.
        if tool.exists() {
            println!("cargo:rerun-if-changed={}", tool.display());
        }
        // Die Konfigurationen aus `import channels` liegen neben dem Programm (8.2).
        if let Some(parent) = self.source.parent().filter(|p| !p.as_os_str().is_empty()) {
            println!("cargo:rerun-if-changed={}", parent.display());
        }
        println!("cargo:rerun-if-changed={}", self.source.display());
        if let Some(hw) = &self.hardware {
            println!("cargo:rerun-if-changed={}", hw.display());
        }
        if let Err(lines) = call.run(&tool) {
            for line in lines {
                println!("cargo:warning={line}");
            }
            panic!("takt build --emit embed scheiterte fuer {}", self.source.display());
        }
        println!("cargo:rustc-link-search=native={}", call.dir.display());
        println!("cargo:rustc-link-lib=static={}", call.prefix);
        println!("cargo:rustc-env={}={}", call.env, call.module.display());
        Built { manifest: call.dir.join(format!("{}.manifest", call.prefix)), module: call.module }
    }
}

/// Was ein Bau lieferte: das Modul und das Manifest (12.11).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// Das Modul `P.rs`.
    pub module: PathBuf,
    /// Das Manifest `P.manifest`.
    pub manifest: PathBuf,
}

impl Built {
    /// Ein Wert des Manifests, etwa `arena_bytes` oder `tick_at`.
    ///
    /// # Panics
    ///
    /// Wenn das Manifest fehlt oder den Schluessel nicht nennt: Der Bau
    /// ginge sonst mit einer Annahme weiter.
    pub fn value(&self, key: &str) -> String {
        let text =
            std::fs::read_to_string(&self.manifest).unwrap_or_else(|e| panic!("{}: {e}", self.manifest.display()));
        value(&text, key).unwrap_or_else(|| panic!("{}: kein `{key}`", self.manifest.display()))
    }
}

/// Der Wert zu `key` in einem Manifest (`key = wert`).
fn value(manifest: &str, key: &str) -> Option<String> {
    manifest.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

/// Ein Aufruf von `takt build --emit embed`: Argumente, Ziel und was der Bau
/// dem Crate nennt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// Die Argumente nach dem Werkzeug.
    pub args: Vec<OsString>,
    /// Wohin die Lieferform geht.
    pub dir: PathBuf,
    /// Das Modul `P.rs`.
    pub module: PathBuf,
    /// Das Praefix der Einstiege und der Bibliothek.
    pub prefix: String,
    /// Die Umgebungsvariable, die das Modul nennt (`TAKT_<P>_RS`).
    pub env: String,
}

impl Invocation {
    /// Ruft `tool` mit den Argumenten.
    ///
    /// # Errors
    ///
    /// Die Meldungen des Werkzeugs, wenn es scheitert, oder eine, wenn es
    /// sich nicht aufrufen laesst.
    pub fn run(&self, tool: &Path) -> Result<(), Vec<String>> {
        let output = Command::new(tool)
            .args(&self.args)
            .output()
            .map_err(|e| vec![format!("`{}` nicht aufrufbar ({e}); `TAKT` nennt das Werkzeug", tool.display())])?;
        if output.status.success() {
            return Ok(());
        }
        Err(String::from_utf8_lossy(&output.stderr).lines().map(ToString::to_string).collect())
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

    fn words(call: &Invocation) -> Vec<String> {
        call.args.iter().map(|a| a.to_string_lossy().replace('\\', "/")).collect()
    }

    /// **Die Befehlszeile ohne Angaben**: logische Zeit (12.11), das
    /// Praefix aus dem Dateinamen, das Ziel nach `OUT_DIR/takt/P`, das Modul
    /// in `TAKT_<P>_RS` in Grossbuchstaben.
    #[test]
    fn the_default_invocation_builds_logical_time_into_out_dir() {
        let call = Program::new("takt/valve.takt").invocation(Path::new("out"), "x86_64-pc-windows-msvc");
        assert_eq!(
            words(&call),
            [
                "build",
                "takt/valve.takt",
                "--emit",
                "embed",
                "--target",
                "x86_64-pc-windows-msvc",
                "--form",
                "logical",
                "--prefix",
                "valve",
                "--out",
                "out/takt/valve"
            ]
        );
        assert_eq!((call.prefix.as_str(), call.env.as_str()), ("valve", "TAKT_VALVE_RS"));
        assert_eq!(call.module, Path::new("out").join("takt").join("valve").join("valve.rs"));
    }

    /// Form, Praefix, Treiber, Hardware, Build, Instrumentierung und
    /// Diagnosestufe gehen in die Befehlszeile; ohne Angabe fehlen die
    /// letzten drei, und das Werkzeug waehlt.
    #[test]
    fn every_option_reaches_the_command_line() {
        let call = Program::new("p/kessel.takt")
            .form("rtos")
            .prefix("boiler")
            .drivers("crate::io::Board")
            .hardware("p/board.hw")
            .build_for("sim")
            .instrument("statements")
            .diagnostics("none")
            .invocation(Path::new("o"), "thumbv7em-none-eabihf");
        let w = words(&call);
        let after = |flag: &str| w.iter().position(|x| x == flag).and_then(|i| w.get(i + 1)).cloned();
        assert_eq!(after("--form").as_deref(), Some("rtos"));
        assert_eq!(after("--prefix").as_deref(), Some("boiler"));
        assert_eq!(after("--drivers").as_deref(), Some("crate::io::Board"));
        assert_eq!(after("--hardware").as_deref(), Some("p/board.hw"));
        assert_eq!(after("--out").as_deref(), Some("o/takt/boiler"));
        assert_eq!(after("--build").as_deref(), Some("sim"));
        assert_eq!(after("--instrument").as_deref(), Some("statements"));
        assert_eq!(after("--diagnostics").as_deref(), Some("none"));
        assert_eq!(call.env, "TAKT_BOILER_RS");
        let plain = words(&Program::new("p/kessel.takt").invocation(Path::new("o"), "x"));
        assert!(!plain.iter().any(|w| ["--build", "--instrument", "--diagnostics"].contains(&w.as_str())), "{plain:?}");
    }

    /// Ein Wert des Manifests nach seinem Schluessel, ohne Leerraum; ein
    /// Schluessel, der nur Praefix eines anderen ist, trifft nicht.
    #[test]
    fn a_manifest_value_is_found_by_its_key() {
        let text = "# takt-manifest 1\narena_bytes = 4096\narena_align = 8\ntick_at = 160\n";
        assert_eq!(value(text, "tick_at").as_deref(), Some("160"));
        assert_eq!(value(text, "arena").as_deref(), None);
        assert_eq!(value(text, "arena_align").as_deref(), Some("8"));
    }

    /// Ein Dateiname, der mit einer Ziffer beginnt, ergibt kein gueltiges
    /// Praefix (12.11); der Bauhelfer reicht ihn weiter, und das Werkzeug
    /// lehnt ihn ab — ein Name aus Versehen wird nicht still umgebogen.
    #[test]
    fn a_file_name_that_is_no_prefix_is_passed_on_unchanged() {
        let call = Program::new("takt/01_valve.takt").invocation(Path::new("o"), "x");
        assert_eq!((call.prefix.as_str(), call.env.as_str()), ("01_valve", "TAKT_01_VALVE_RS"));
    }

    /// **Der Fehlerpfad**: Ein Werkzeug, das scheitert, liefert seine
    /// Meldungen; eines, das es nicht gibt, sagt, dass `TAKT` es nennt.
    #[test]
    fn a_failing_or_missing_tool_reports_why() {
        let call = Program::new("gibt/es/nicht.takt").invocation(Path::new("o"), "x");
        let failed = call.run(Path::new("rustc")).expect_err("rustc kennt `build` nicht");
        assert!(!failed.is_empty(), "die Meldungen des Werkzeugs");
        let missing = call.run(Path::new("kein-solches-werkzeug")).expect_err("fehlt");
        assert!(missing[0].contains("nicht aufrufbar") && missing[0].contains("TAKT"), "{missing:?}");
    }
}
