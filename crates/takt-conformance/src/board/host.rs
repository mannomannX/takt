//! Der Wirt als Board (13.8): `takt-bringup-host`, wahlweise mit einem
//! Treiber-Crate.
//!
//! Derselbe MCU-Rahmen, dieselbe Tickschleife und derselbe Treiberrand wie
//! auf den Boards, in logischer Zeit und ohne Hardware. Ein Treiber-Crate
//! kommt dazu, indem ein kleines Programm beide bindet: Cargo kennt keine
//! Abhaengigkeit, die erst beim Aufruf feststeht. Das Programm liegt unter
//! `takt-host/<crate>` im Zielverzeichnis, die Abbilder unter
//! `takt-host-images` — wie bei den Boards je Stand der Quellen.
//!
//! Die Einstiege des Treiber-Crates ersetzen die schwachen Voreinstellungen
//! des Rahmens, wie die des Bring-ups auf einem Board; ein Kanal, den das
//! Crate nicht stellt, liefert nichts, und ein Ausgang ohne Treiber gilt als
//! bestaetigt (12.1).

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use super::{Bin, Board, END, Options, complete, hash_tree, root, run_bounded};

/// Wie lange ein Lauf auf dem Wirt hoechstens dauern darf.
const RUN: Duration = Duration::from_secs(120);

/// Ein Bau zur Zeit: Cargo sperrt das Zielverzeichnis zwar selbst, aber
/// zwischen dem Bau und dem Kopieren des Abbilds schriebe ein zweiter Bau
/// mit einem anderen Programm dasselbe Binary.
static BUILDING: Mutex<()> = Mutex::new(());

/// Der MCU-Rahmen auf dem Wirt.
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// Das Treiber-Crate, das der Lauf bindet.
    driver: Option<PathBuf>,
}

impl Host {
    /// Ohne Treiber-Crate: Jeder Kanal liefert nichts.
    pub fn new() -> Host {
        Host::default()
    }

    /// Mit dem Treiber-Crate im Verzeichnis `dir`.
    pub fn with_driver(dir: PathBuf) -> Host {
        Host { driver: Some(dir) }
    }

    /// Das Programm, das Wirts-Bring-up und Treiber-Crate bindet; liefert
    /// sein Manifest.
    fn wrapper(&self) -> Result<PathBuf, String> {
        let bringup = root().join("crates/takt-bringup-host");
        let (dir, dependency, import) = match &self.driver {
            Some(driver) => {
                let name = crate_name(driver)?;
                let path = std::fs::canonicalize(driver).map_err(|e| format!("{}: {e}", driver.display()))?;
                let dependency = format!("{name} = {{ path = {} }}\n", toml_path(&path));
                (name.clone(), dependency, format!("use {} as _;\n\n", name.replace('-', "_")))
            }
            None => ("ohne-treiber".to_string(), String::new(), String::new()),
        };
        let dir = crate::target_dir().join("takt-host").join(dir);
        std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("{}: {e}", dir.display()))?;
        let bringup = std::fs::canonicalize(&bringup).map_err(|e| format!("{}: {e}", bringup.display()))?;
        let manifest = format!(
            "# Erzeugt von `takt-conformance::board::host`.\n[package]\nname = \"takt-host-run\"\nversion = \"0.0.0\"\n\
             edition = \"2024\"\npublish = false\n\n[workspace]\n\n[dependencies]\ntakt-bringup-host = {{ path = {} }}\n\
             {dependency}",
            toml_path(&bringup)
        );
        let main = format!("{import}fn main() -> std::process::ExitCode {{\n    takt_bringup_host::run()\n}}\n");
        write_if_changed(&dir.join("Cargo.toml"), &manifest)?;
        write_if_changed(&dir.join("src/main.rs"), &main)?;
        // Dieselben Versionen wie im Workspace, und kein Netz.
        let lock = dir.join("Cargo.lock");
        if !lock.exists() {
            std::fs::copy(root().join("Cargo.lock"), &lock).map_err(|e| format!("{}: {e}", lock.display()))?;
        }
        Ok(dir.join("Cargo.toml"))
    }

    /// Der Schluessel eines Abbilds: Profil, Programm, Konfiguration,
    /// Treiber-Crate und die Quellen aller Crates.
    fn key(&self, program: &Path, options: &Options) -> Result<u64, String> {
        let mut h = DefaultHasher::new();
        cfg!(debug_assertions).hash(&mut h);
        std::fs::read(program).map_err(|e| format!("{}: {e}", program.display()))?.hash(&mut h);
        if let Some(hw) = &options.hardware {
            std::fs::read(hw).map_err(|e| format!("{}: {e}", hw.display()))?.hash(&mut h);
        }
        let crates = root().join("crates");
        let mut trees: Vec<PathBuf> =
            std::fs::read_dir(&crates).map_err(|e| e.to_string())?.flatten().map(|e| e.path().join("src")).collect();
        trees.sort();
        trees.push(root().join("crates/takt-bringup-host/build.rs"));
        if let Some(driver) = &self.driver {
            trees.extend([driver.join("src"), driver.join("Cargo.toml")]);
        }
        for path in &trees {
            if path.is_dir() {
                hash_tree(path, &mut h)?;
            } else if path.is_file() {
                std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?.hash(&mut h);
            }
        }
        Ok(h.finish())
    }
}

impl Board for Host {
    fn name(&self) -> &'static str {
        "host"
    }

    fn target(&self) -> &'static str {
        if cfg!(windows) { "x86_64-windows" } else { "x86_64" }
    }

    fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        if options.bin != Bin::Takt || options.timed || options.rtos {
            return Err("der Wirt kennt nur den Konformitaetslauf in logischer Zeit".into());
        }
        let images = crate::target_dir().join("takt-host-images");
        let cached = images.join(format!("{:016x}{}", self.key(program, options)?, std::env::consts::EXE_SUFFIX));
        if cached.is_file() {
            return Ok(cached);
        }
        let _building = BUILDING.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let manifest = self.wrapper()?;
        // Das Bauskript laeuft im Verzeichnis seines Crates.
        let absolute = |p: &Path| std::path::absolute(p).map_err(|e| format!("{}: {e}", p.display()));
        let program = absolute(program)?;
        let mut cargo = Command::new("cargo");
        cargo.args(["build", "--offline", "--message-format=json-render-diagnostics"]);
        // Dasselbe Profil wie der Aufrufer: Das Bauskript sucht `takt` darin,
        // und eine Testsuite hat das Werkzeug ihres eigenen Profils gebaut.
        if !cfg!(debug_assertions) {
            cargo.arg("--release");
        }
        cargo
            .arg("--manifest-path")
            .arg(&manifest)
            .env("CARGO_TARGET_DIR", crate::target_dir())
            .env("TAKT_PROGRAM", &program);
        match &options.hardware {
            Some(hw) => cargo.env("TAKT_HARDWARE", absolute(hw)?),
            None => cargo.env_remove("TAKT_HARDWARE"),
        };
        let out = cargo.output().map_err(|e| format!("cargo: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        let exe = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let rest = &l[l.find("\"executable\":\"")? + "\"executable\":\"".len()..];
                Some(rest[..rest.find('"')?].replace("\\\\", "\\"))
            })
            .next_back()
            .ok_or_else(|| "cargo meldete kein Binary".to_string())?;
        std::fs::create_dir_all(&images)
            .and_then(|()| std::fs::copy(&exe, &cached))
            .map_err(|e| format!("{}: {e}", cached.display()))?;
        Ok(cached)
    }

    fn run(&mut self, exe: &Path, options: &Options) -> Result<String, String> {
        let program = exe.to_string_lossy();
        let text = run_bounded(&program, &[&options.ticks.to_string()], RUN).map_err(|e| format!("{program}: {e}"))?;
        if text.contains(END) { complete(text) } else { Err(format!("kein `{END}`; gelesen:\n{text}")) }
    }
}

/// Der Name des Crates aus `[package]` seines Manifests.
fn crate_name(dir: &Path) -> Result<String, String> {
    let manifest = dir.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let mut package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            package = line == "[package]";
        } else if package && let Some(value) = line.strip_prefix("name").and_then(|r| r.trim_start().strip_prefix('='))
        {
            return Ok(value.trim().trim_matches('"').to_string());
        }
    }
    Err(format!("{}: kein `name` unter `[package]`", manifest.display()))
}

/// Ein Pfad als TOML-Zeichenkette, die Backslashes nicht deutet.
fn toml_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().trim_start_matches(r"\\?\"))
}

/// Schreibt nur, wenn sich der Inhalt aendert: Sonst baute Cargo jedes Mal neu.
fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
