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
//! **Der Pruefstand.** Das Bauskript des Programms erzeugt die Treiber
//! (`bringup::drivers`): je Adresse das Geraet, das die Verdrahtung des
//! Wirts-Bring-ups oder des Treiber-Crates nennt (`takt-drivers.toml`,
//! 12.6). Mit Treiber-Crate hat jede Adresse genau ein Geraet, sonst
//! scheitert der Bau vor dem Start mit ihrem Namen — einen Vorgabewert, der
//! still an die Stelle eines Geraets tritt, gibt es nicht (12.11). Ohne
//! Treiber-Crate bekommt jede Adresse ausser denen des Bring-ups einen
//! Stummel: Ein Eingang liefert nichts, ein Ausgang gilt als bestaetigt.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use super::{
    ATTEMPTS, Bin, Board, END, Options, complete, hash_build_inputs, hash_program, hash_tree, publish_checked, root,
    run_bounded,
};
use crate::bringup::WIRING;

/// Wie lange ein Lauf auf dem Wirt hoechstens dauern darf.
const RUN: Duration = Duration::from_secs(120);

/// Ein Bau zur Zeit: Cargo sperrt das Zielverzeichnis zwar selbst, aber
/// zwischen dem Bau und dem Kopieren des Abbilds schriebe ein zweiter Bau
/// mit einem anderen Programm dasselbe Binary. Gegen einen zweiten Prozess
/// hilft die Sperre nicht; dagegen nennt jedes Abbild seinen Schluessel
/// (`--key`), und nur ein Abbild mit dem eigenen wird veroeffentlicht.
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

    /// Ein Treiber-Crate verdrahtet jede Adresse des Programms, die das
    /// Wirts-Bring-up nicht stellt (12.6, 12.11): Eine offene Adresse
    /// scheitert vor dem Bau mit ihrem Namen, statt einen Stummel zu
    /// bekommen, ebenso eine, die beide verdrahten, und ein Crate ohne
    /// Verdrahtung. Ohne Treiber-Crate liefert jeder Kanal nichts
    /// ([`Host::new`]).
    fn check_wiring(&self, program: &Path) -> Result<(), String> {
        let Some(driver) = &self.driver else { return Ok(()) };
        let own = driver.join(WIRING);
        if !own.is_file() {
            return Err(format!("{}: keine Verdrahtung `{WIRING}` (12.6)", driver.display()));
        }
        let wiring = crate::bringup::read_wiring(&[&root().join("crates/takt-bringup-host").join(WIRING), &own])?;
        let src = std::fs::read_to_string(program).map_err(|e| format!("{}: {e}", program.display()))?;
        let dir = program.parent().unwrap_or(Path::new("."));
        let mut options = takt_sema::Options { build: takt_sema::Build::Hw, ..Default::default() };
        for file in takt_sema::channel_imports(&src) {
            let text = std::fs::read_to_string(dir.join(&file)).map_err(|e| format!("{file}: {e}"))?;
            options.channel_imports.insert(file, text);
        }
        let out = takt_sema::compile(&src, &options);
        let Some(p) = out.program else {
            let errors: Vec<String> =
                out.diagnostics.iter().filter(|d| d.is_error()).map(ToString::to_string).collect();
            return Err(format!("{}: uebersetzt nicht:\n  {}", program.display(), errors.join("\n  ")));
        };
        let drivers = takt_frame::drivers::of(&p, &takt_frame::layout::of(&p));
        let open = takt_frame::drivers::wiring_errors(&drivers, &wiring);
        if open.is_empty() {
            return Ok(());
        }
        Err(format!("{}: Pruefstand ohne Stummel (12.6):\n  {}", program.display(), open.join("\n  ")))
    }

    /// Das Programm, das Wirts-Bring-up und Treiber-Crate bindet; liefert
    /// sein Manifest.
    fn wrapper(&self) -> Result<PathBuf, String> {
        let canonical = |p: &Path| std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()));
        let bringup = canonical(&root().join("crates/takt-bringup-host"))?;
        let mut wiring = vec![bringup.join(WIRING)];
        let (dir, dependency) = match &self.driver {
            Some(driver) => {
                let name = crate_name(driver)?;
                let path = canonical(driver)?;
                wiring.push(path.join(WIRING));
                let dependency = format!("{name} = {{ path = {} }}\n", toml_path(&path));
                (name, dependency)
            }
            None => ("ohne-treiber".to_string(), String::new()),
        };
        // Je Treiber-Crate ein eigener Paketname: Cargo legt Bauskript, sein
        // `OUT_DIR` und das Binary nach Name und Version ab, nicht nach dem
        // Verzeichnis, und zwei Programme gleichen Namens schrieben einander
        // die erzeugten Treiber um.
        let package = format!("takt-host-{dir}");
        let dir = crate::target_dir().join("takt-host").join(dir);
        std::fs::create_dir_all(dir.join("src")).map_err(|e| format!("{}: {e}", dir.display()))?;
        let krate = |name: &str| canonical(&root().join("crates").join(name)).map(|p| toml_path(&p));
        let manifest = format!(
            "# Erzeugt von `takt-conformance::board::host`.\n[package]\nname = \"{package}\"\nversion = \"0.0.0\"\n\
             edition = \"2024\"\npublish = false\n\n[workspace]\n\n[build-dependencies]\n\
             takt-conformance = {{ path = {}, default-features = false }}\n\n[dependencies]\n\
             takt-bringup-host = {{ path = {} }}\ntakt-embed = {{ path = {} }}\n{dependency}",
            krate("takt-conformance")?,
            toml_path(&bringup),
            krate("takt-embed")?
        );
        let files: String = wiring.iter().map(|p| format!("        Path::new({:?}),\n", plain(p))).collect();
        let build = format!(
            "// Erzeugt von `takt-conformance::board::host`.\n\nuse std::path::{{Path, PathBuf}};\n\n\
             use takt_conformance::bringup;\n\nfn main() {{\n    println!(\"cargo:rerun-if-env-changed=TAKT_PROGRAM\");\n    \
             println!(\"cargo:rerun-if-env-changed=TAKT_HOST_KEY\");\n    \
             println!(\"cargo:rustc-env=TAKT_HOST_KEY={{}}\", std::env::var(\"TAKT_HOST_KEY\").unwrap_or_default());\n    \
             let program = std::env::var(\"TAKT_PROGRAM\").expect(\"TAKT_PROGRAM\");\n    \
             let Some(p) = bringup::compile(&program) else {{ panic!(\"{{program}}: uebersetzt nicht\") }};\n    \
             let wiring = bringup::wiring(&[\n{files}    ]);\n    \
             let out = PathBuf::from(std::env::var(\"OUT_DIR\").expect(\"OUT_DIR\"));\n    \
             bringup::drivers(&p, &wiring, &out.join(\"takt_drivers.rs\"));\n}}\n"
        );
        let main = "// Erzeugt von `takt-conformance::board::host`.\n\n\
                    mod drivers {\n    include!(concat!(env!(\"OUT_DIR\"), \"/takt_drivers.rs\"));\n}\n\n\
                    fn main() -> std::process::ExitCode {\n    \
                    // Der Schluessel des Abbilds, fuer den Zwischenspeicher von `board::host`.\n    \
                    if std::env::args().nth(1).as_deref() == Some(\"--key\") {\n        \
                    println!(\"{}\", env!(\"TAKT_HOST_KEY\"));\n        \
                    return std::process::ExitCode::SUCCESS;\n    }\n    \
                    let mut rig = drivers::Rig::default();\n    \
                    // SAFETY: Der Kleber in `drivers` ist fuer `Rig` erzeugt, und `rig` lebt bis zum Ende des Laufs.\n    \
                    unsafe { takt_bringup_host::run(core::ptr::from_mut(&mut rig).cast()) }\n}\n";
        write_if_changed(&dir.join("Cargo.toml"), &manifest)?;
        write_if_changed(&dir.join("build.rs"), &build)?;
        write_if_changed(&dir.join("src/main.rs"), main)?;
        // Dieselben Versionen wie im Workspace, und kein Netz.
        let lock = dir.join("Cargo.lock");
        if !lock.exists() {
            std::fs::copy(root().join("Cargo.lock"), &lock).map_err(|e| format!("{}: {e}", lock.display()))?;
        }
        Ok(dir.join("Cargo.toml"))
    }

    /// Der Schluessel eines Abbilds: Profil, Programm samt importierten
    /// Konfigurationen, Hardware-Konfiguration, Treiber-Crate, die Quellen
    /// und Verdrahtungen aller Crates und was den Bau sonst bestimmt
    /// ([`hash_build_inputs`]).
    pub(super) fn key(&self, program: &Path, options: &Options) -> Result<u64, String> {
        let mut h = DefaultHasher::new();
        cfg!(debug_assertions).hash(&mut h);
        hash_program(program, &mut h)?;
        if let Some(hw) = &options.hardware {
            std::fs::read(hw).map_err(|e| format!("{}: {e}", hw.display()))?.hash(&mut h);
        }
        let crates = root().join("crates");
        let mut dirs: Vec<PathBuf> =
            std::fs::read_dir(&crates).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
        dirs.sort();
        let mut trees: Vec<PathBuf> = dirs.iter().flat_map(|d| [d.join("src"), d.join(WIRING)]).collect();
        if let Some(driver) = &self.driver {
            trees.extend([driver.join("src"), driver.join("Cargo.toml"), driver.join(WIRING)]);
        }
        for path in &trees {
            if path.is_dir() {
                hash_tree(path, &mut h)?;
            } else if path.is_file() {
                std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?.hash(&mut h);
            }
        }
        hash_build_inputs(&root(), &mut h);
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
        if options.bin != Bin::Takt || options.timed || options.rtos || options.hostile_fpu {
            return Err("der Wirt kennt nur den Konformitaetslauf in logischer Zeit".into());
        }
        self.check_wiring(program)?;
        let images = crate::target_dir().join("takt-host-images");
        let key = format!("{:016x}", self.key(program, options)?);
        let cached = images.join(format!("{key}{}", std::env::consts::EXE_SUFFIX));
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
            .env("TAKT_PROGRAM", &program)
            .env("TAKT_HOST_KEY", &key);
        match &options.hardware {
            Some(hw) => cargo.env("TAKT_HARDWARE", absolute(hw)?),
            None => cargo.env_remove("TAKT_HARDWARE"),
        };
        std::fs::create_dir_all(&images).map_err(|e| format!("{}: {e}", images.display()))?;
        let ours = |part: &Path| -> Result<bool, String> {
            let out = Command::new(part).arg("--key").output().map_err(|e| format!("{}: {e}", part.display()))?;
            Ok(String::from_utf8_lossy(&out.stdout).trim() == key)
        };
        for _ in 0..ATTEMPTS {
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
            if publish_checked(Path::new(&exe), &cached, ours)? {
                return Ok(cached);
            }
        }
        Err(format!("{key}: {ATTEMPTS}-mal gebaut, jedes Mal hatte ein anderer Prozess das Binary ueberschrieben"))
    }

    fn run(&mut self, exe: &Path, options: &Options) -> Result<String, String> {
        let program = exe.to_string_lossy();
        let text = run_bounded(&program, &[&options.ticks.to_string()], RUN).map_err(|e| format!("{program}: {e}"))?;
        if text.contains(END) { complete(text, options) } else { Err(format!("kein `{END}`; gelesen:\n{text}")) }
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

/// Ein Pfad ohne das Praefix `\\?\`, das `canonicalize` unter Windows setzt.
fn plain(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").to_string()
}

/// Ein Pfad als TOML-Zeichenkette, die Backslashes nicht deutet.
fn toml_path(path: &Path) -> String {
    format!("'{}'", plain(path))
}

/// Schreibt nur, wenn sich der Inhalt aendert: Sonst baute Cargo jedes Mal neu.
fn write_if_changed(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}
