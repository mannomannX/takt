//! Ein C-Wirt auf einem Board (12.11; M11 Schritt 18b): das Programm als
//! Baustein (`takt build --emit embed`), das SDK-Archiv fuer das Ziel
//! (`crates/takt-embed-c`), ein Port aus `crates/takt-embed-c/ports` und die
//! Board-Unterstuetzung des Wirts, mit clang zu einem Abbild gebunden — so,
//! wie ein fremdes C-Projekt Takt bindet. Geschrieben, gestartet und gelesen
//! wird das Abbild wie das eines Bring-ups in Rust
//! ([`super::stm32f401::Stm32f401C`]).
//!
//! **Jedes Programm unter einem Praefix** ([`PREFIX`]): Der Wirt bindet
//! `app.h` und `app_ops`, gleich welches Programm. Treiber stellt er keine;
//! ein Programm, das Treiber bindet, scheitert am Link, denn einen
//! Vorgabewert, der still an ihre Stelle tritt, gibt es nicht (12.11).
//!
//! **Zwischengespeichert wie die Bring-ups** (FB-270): Der Stand heisst
//! `c-<tripel>-<stand>` unter [`super::IMAGES`], neben dem des Bring-ups
//! desselben Tripels, und haengt an den Quellen des Wirts, den Ports, dem SDK
//! und allen Crates; der Schluessel eines Abbilds ist der der Bring-ups
//! ([`super::image_key`]). So baut `.github/workflows/boards.yml` die Abbilder
//! der C-Wirte mit, und `tools/board-run.ps1 -FromCi` findet sie.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    Bin, Fingerprint, Form, Options, enter_stand, hash_build_inputs, hash_crates, hash_tree, image_key, images, missed,
    publish_checked, root,
};

/// Das Praefix, unter dem ein C-Wirt jedes Programm bindet (`app.h`,
/// `app_ops`, `libapp.a`).
pub const PREFIX: &str = "app";

/// Wie ein Port in einer Form bindet (12.11).
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// Die Form.
    pub form: Form,
    /// Der Wirt der Form, relativ zum Verzeichnis des Wirts.
    pub main: &'static str,
    /// Die Quellen des Ports und seines Adapters, relativ zur Wurzel.
    pub sources: &'static [&'static str],
    /// Die Kopfpfade des Ports, relativ zur Wurzel.
    pub include: &'static [&'static str],
}

/// Ein C-Wirt eines Boards.
#[derive(Clone, Copy, Debug)]
pub struct CHost {
    /// Das Verzeichnis des Wirts, relativ zur Wurzel.
    pub dir: &'static str,
    /// Das Zieltripel.
    pub triple: &'static str,
    /// Das Ziel, wie `takt build --target` es nennt.
    pub target: &'static str,
    /// Die Hardware-Konfiguration des Boards, relativ zur Wurzel; ein Lauf
    /// mit eigener ([`Options::hardware`]) nimmt die.
    pub hardware: &'static str,
    /// Was clang ueber das Ziel wissen muss: Kern, FPU und ihre ABI.
    pub cflags: &'static [&'static str],
    /// Start und Board-Unterstuetzung, relativ zu `dir`: Jede Form bindet sie.
    pub board: &'static [&'static str],
    /// Das Linker-Skript, relativ zu `dir`.
    pub script: &'static str,
    /// Die Formen, in denen der Wirt binden kann.
    pub bindings: &'static [Binding],
}

/// Was jeder C-Wirt mit clang braucht: C11 freistehend ohne C-Bibliothek
/// (`memcpy` und die Laufzeit der Ganzzahl- und Gleitkommarechnung bringt das
/// SDK-Archiv), jede Funktion in ihrem Abschnitt fuer `--gc-sections`, kein
/// Abwickeln, gebunden mit LLD, und jede Warnung ein Fehler.
const CFLAGS: &[&str] = &[
    "-std=c11",
    "-O2",
    "-g",
    "-ffreestanding",
    "-nostdlib",
    "-fno-common",
    "-ffunction-sections",
    "-fdata-sections",
    "-fno-unwind-tables",
    "-fno-asynchronous-unwind-tables",
    "-fuse-ld=lld",
    "-Wall",
    "-Wextra",
    "-Werror",
];

/// Was ausser dem Wirt und den Quellen der Crates in den Stand eingeht: die
/// Ports und der Kopf des SDK, die ausserhalb von `src` liegen.
const TREES: &[&str] = &["crates/takt-embed-c/ports", "crates/takt-embed-c/include"];

/// Die Versionen, mit denen das SDK-Archiv baut; es ist ein eigener
/// Workspace.
const FILES: &[&str] = &["crates/takt-embed-c/Cargo.lock"];

impl CHost {
    /// Die Art des Stands im Zwischenspeicher: `c-<tripel>`, neben dem
    /// Bring-up desselben Tripels.
    fn kind(&self) -> String {
        format!("c-{}", self.triple)
    }

    /// Das Verzeichnis des Stands im Zwischenspeicher.
    fn stand(&self) -> Result<PathBuf, String> {
        Ok(images().join(format!("{}-{:016x}", self.kind(), self.sources()?)))
    }

    /// Der Stand, gegen den gebaut wird: der Wirt, die Ports, das SDK, die
    /// Hardware-Konfiguration des Boards und alle Crates, aus denen Werkzeug
    /// und Archiv entstehen.
    fn sources(&self) -> Result<u64, String> {
        let root = root();
        let mut f = Fingerprint::new();
        for tree in std::iter::once(self.dir).chain(TREES.iter().copied()) {
            f.str(tree);
            hash_tree(&root.join(tree), &mut f)?;
        }
        for file in std::iter::once(self.hardware).chain(FILES.iter().copied()) {
            f.str(file);
            f.file_if_any(&root.join(file));
        }
        hash_crates(&mut f)?;
        hash_build_inputs(&root.join(self.dir), &mut f)?;
        Ok(f.finish())
    }

    /// Das Abbild fuer `program`, aus dem Zwischenspeicher oder frisch gebaut
    /// mit dem `takt` aus `target/release`, wie die Bring-ups
    /// ([`super::Bringup::build`]). Gebaut wird in einem Verzeichnis je
    /// Prozess; veroeffentlicht wird nur ein ELF, das seinen Schluessel traegt.
    pub fn build(&self, program: &Path, options: &Options) -> Result<PathBuf, String> {
        let stand = self.stand()?;
        let key = format!("{:016x}", image_key(self.triple, program, options)?);
        let cached = stand.join(format!("{key}.elf"));
        if cached.is_file() {
            return Ok(cached);
        }
        missed(&stand, &cached, &program.display().to_string())?;
        let work = crate::target_dir().join("takt-c-build").join(format!("{key}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        let tool = crate::target_dir().join("release").join(if cfg!(windows) { "takt.exe" } else { "takt" });
        let ours = |part: &Path| -> Result<bool, String> {
            let elf = std::fs::read(part).map_err(|e| format!("{}: {e}", part.display()))?;
            Ok(takt_board_support::image_key::carries(&elf, &key))
        };
        let published = self
            .link(&tool, program, options, &work, &key)
            .and_then(|elf| enter_stand(&self.kind(), &stand).and_then(|()| publish_checked(&elf, &cached, ours)));
        let _ = std::fs::remove_dir_all(&work);
        if published? { Ok(cached) } else { Err(format!("{}: ohne den Schluessel {key}", cached.display())) }
    }

    /// Baut das Abbild fuer `program` in `out` mit dem Werkzeug `tool`: die
    /// Lieferform in der Form der Optionen, das SDK-Archiv fuer das Tripel,
    /// dann Wirt, Port und beide Bibliotheken mit clang. `key` geht als Symbol
    /// ins ELF (`takt_board_support::image_key`). Liefert das ELF.
    pub fn link(
        &self,
        tool: &Path,
        program: &Path,
        options: &Options,
        out: &Path,
        key: &str,
    ) -> Result<PathBuf, String> {
        supported(options)?;
        let binding = self
            .bindings
            .iter()
            .find(|b| b.form == options.form)
            .ok_or_else(|| format!("der C-Wirt `{}` bindet nicht in der Form {:?}", self.dir, options.form))?;
        let root = root();
        let embed = out.join("embed");
        let hardware = options.hardware.clone().unwrap_or_else(|| root.join(self.hardware));
        let delivered = Command::new(tool)
            .current_dir(&root)
            .arg("build")
            .arg(program)
            .args(["--emit", "embed", "--target", self.target, "--form", form_name(options.form), "--prefix", PREFIX])
            .args(["--build", if options.build == takt_sema::Build::Hw { "hw" } else { "sim" }])
            .arg("--hardware")
            .arg(&hardware)
            .arg("--out")
            .arg(&embed)
            .output()
            .map_err(|e| format!("{}: {e}", tool.display()))?;
        if !delivered.status.success() {
            return Err(format!("takt build {}: {}", program.display(), String::from_utf8_lossy(&delivered.stderr)));
        }
        let manifest_path = embed.join(format!("{PREFIX}.manifest"));
        let manifest =
            std::fs::read_to_string(&manifest_path).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        let number = |name: &str| manifest_number(&manifest, name);
        let (program_stack, tick_stack, job_stack) =
            (number("tick_stack_program")?, number("tick_stack_bytes")?, number("job_stack_bytes")?);
        let sdk = crate::harness::sdk_library_for(Some(self.triple))?;
        let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
            return Err("clang fehlt; ohne ihn entsteht kein C-Wirt".to_string());
        };
        let dir = root.join(self.dir);
        let elf = out.join(format!("{PREFIX}.elf"));
        let mut cc = Command::new(&clang);
        cc.env("SOURCE_DATE_EPOCH", "0")
            .arg(format!("--target={}", self.triple))
            .args(self.cflags)
            .args(CFLAGS)
            .arg(format!("-DTAKT_TICKS={}u", options.ticks))
            .arg(format!("-DTAKT_LOGICAL={}", u8::from(!options.timed)))
            .arg(format!("-DTAKT_FRESH_JOURNAL={}", u8::from(options.fresh)))
            .arg(format!("-DTAKT_TICK_STACK_PROGRAM={program_stack}u"))
            .arg("-I")
            .arg(&embed)
            .arg("-I")
            .arg(&dir);
        for include in binding.include {
            cc.arg("-I").arg(root.join(include));
        }
        cc.args(self.board.iter().map(|file| dir.join(file)))
            .arg(dir.join(binding.main))
            .args(binding.sources.iter().map(|file| root.join(file)))
            .arg(embed.join(format!("lib{PREFIX}.a")))
            .arg(&sdk)
            .arg(format!("-Wl,-T,{}", dir.join(self.script).display()))
            .arg("-Wl,--gc-sections")
            // Schritt und Jobs rechnen auf dem Stack der Interrupts (12.3).
            .arg(format!("-Wl,--defsym=board_stack_bytes={}", tick_stack + job_stack))
            .arg(format!("-Wl,--defsym={}{key}=0", takt_board_support::image_key::SYMBOL))
            .arg("-o")
            .arg(&elf);
        let linked = cc.output().map_err(|e| format!("{}: {e}", clang.display()))?;
        if !linked.status.success() {
            return Err(format!("{}: {}", clang.display(), String::from_utf8_lossy(&linked.stderr)));
        }
        Ok(elf)
    }
}

/// Was ein C-Wirt nicht kann, scheitert beim Bau, statt still zu fehlen:
/// Er faehrt nur das Programm selbst, ohne absichtlich falschen Port, ohne
/// verstellte FPU und ohne Tunes von der Konsole.
fn supported(options: &Options) -> Result<(), String> {
    let missing = [
        (options.bin != Bin::Takt, "nur das Programm selbst, kein Mess- oder Native-Programm"),
        (options.flaw.is_some(), "keinen absichtlich falschen Port"),
        (options.hostile_fpu, "keine verstellte FPU"),
        (!options.console.is_empty(), "keine Tunes von der Konsole"),
    ];
    match missing.iter().find(|(asked, _)| *asked) {
        Some((_, what)) => Err(format!("ein C-Wirt kennt {what}")),
        None => Ok(()),
    }
}

/// Die Form, wie `takt build --form` sie nennt; Embassy ist die Form `rtos`.
fn form_name(form: Form) -> &'static str {
    match form {
        Form::Own => "own",
        Form::Interrupt => "interrupt",
        Form::Poll => "poll",
        Form::Rtos | Form::Embassy => "rtos",
    }
}

/// Die Zahl hinter `name = ` im Manifest der Lieferform.
fn manifest_number(manifest: &str, name: &str) -> Result<u64, String> {
    manifest
        .lines()
        .filter_map(|line| line.split_once(" = "))
        .find(|(key, _)| *key == name)
        .and_then(|(_, value)| value.trim().parse().ok())
        .ok_or_else(|| format!("das Manifest nennt `{name}` nicht als Zahl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_the_stacks() {
        let manifest = "tick_stack_bytes = 3492\ntick_stack_program = 128\ntick_stack_symbol = app_tick_stack\n";
        assert_eq!(manifest_number(manifest, "tick_stack_bytes"), Ok(3492));
        assert_eq!(manifest_number(manifest, "tick_stack_program"), Ok(128));
        assert!(manifest_number(manifest, "tick_stack_symbol").is_err());
        assert!(manifest_number(manifest, "job_stack_bytes").is_err());
    }

    /// Was der C-Wirt nicht kann, nennt der Bau, bevor er etwas uebersetzt.
    #[test]
    fn a_c_host_refuses_what_it_cannot_do() {
        assert_eq!(supported(&Options::fresh(10)), Ok(()));
        assert!(supported(&Options::natives()).is_err());
        assert!(supported(&Options::fresh(10).with_hostile_fpu()).is_err());
        assert!(supported(&Options::fresh(10).with_flaw(super::super::Flaw::UnsavedFpu)).is_err());
        let tuned = Options::fresh(10).with_console(std::time::Duration::from_millis(5), "tune 0 01\n");
        assert!(supported(&tuned).is_err());
    }
}
