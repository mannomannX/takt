//! `takt build DATEI --emit embed` (12.11, M11 Schritt 8): die Lieferform
//! eines Programms fuer einen Wirt.
//!
//! **Ein Aufruf liefert alles**, in ein Verzeichnis: die Bibliothek mit
//! erzeugtem Code und Rahmen (`libP.a`, `P.lib` fuer MSVC), den Kopf fuer C
//! (`P.h`), das Modul fuer Rust (`P.rs`) und das Manifest (`P.manifest`).
//! Das Tripel kommt vom Wirt (Cargo `TARGET`, CMake); die Form sagt, wer
//! `service` ruft, und legt das Profil fest (12.11).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_llvm::Target;
use takt_llvm::symbols::Prefix;
use takt_mir::program::RuntimeProfile;

/// Wer `service` ruft (12.11, Tabelle der Formen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    /// Die Runtime wartet selbst auf die Frist (12.3).
    Own,
    /// Eine Timer-ISR; fremder Code laeuft darunter.
    Interrupt,
    /// Die Hauptschleife des Wirts, sobald die Frist erreicht ist.
    Poll,
    /// Eine Aufgabe unter einem RTOS.
    Rtos,
    /// Ein Faden unter Linux (12.2).
    Linux,
    /// Ein Test, der `service` selbst ruft, so schnell er will.
    Logical,
}

impl Form {
    const ALL: [Form; 6] = [Form::Own, Form::Interrupt, Form::Poll, Form::Rtos, Form::Linux, Form::Logical];

    fn name(self) -> &'static str {
        match self {
            Form::Own => "own",
            Form::Interrupt => "interrupt",
            Form::Poll => "poll",
            Form::Rtos => "rtos",
            Form::Linux => "linux",
            Form::Logical => "logical",
        }
    }

    pub(crate) fn parse(name: &str) -> Result<Form, String> {
        Form::ALL.into_iter().find(|f| f.name() == name).ok_or_else(|| {
            let known: Vec<&str> = Form::ALL.iter().map(|f| f.name()).collect();
            format!("--form: `{name}` unbekannt; bekannt: {}", known.join(", "))
        })
    }

    /// Das Profil der Form (12.8, 12.11); die logische Zeit hat keines.
    pub(crate) fn profile(self) -> Option<RuntimeProfile> {
        match self {
            Form::Own => Some(RuntimeProfile::Baremetal),
            Form::Interrupt | Form::Poll | Form::Rtos => Some(RuntimeProfile::Shared),
            Form::Linux => Some(RuntimeProfile::LinuxRt),
            Form::Logical => None,
        }
    }
}

/// Was die Lieferform braucht; `takt build` hat es schon berechnet.
pub(crate) struct Embed<'a> {
    pub(crate) program: &'a takt_mir::Program,
    pub(crate) ir: &'a str,
    pub(crate) target: Target,
    pub(crate) triple: &'a str,
    pub(crate) prefix: &'a Prefix,
    pub(crate) diagnostics: takt_llvm::Diagnostics,
    pub(crate) hardware: Option<&'a takt_mir::hardware::Hardware>,
    pub(crate) form: Form,
    pub(crate) drivers_type: Option<&'a str>,
    /// So lange haelt ein Journal-Vorgang den Kern hoechstens (12.3); 0 heisst nie.
    pub(crate) nvm_blocking_ns: i64,
    /// Die Konstanten als Rust (`--emit consts-rs`).
    pub(crate) consts_rs: String,
    pub(crate) out: PathBuf,
}

/// Schreibt die Lieferform nach `e.out`.
pub(crate) fn embed(e: &Embed<'_>) -> Result<(), String> {
    check_form(e.program, e.form)?;
    let x = e.prefix;
    std::fs::create_dir_all(&e.out).map_err(|err| format!("{}: {err}", e.out.display()))?;
    let frame = takt_frame::mcu::build_with(
        e.program,
        takt_frame::mcu::Frame {
            diagnostics: e.diagnostics,
            hardware: e.hardware,
            protect: None,
            prefix: x.clone(),
            stubs: false,
        },
    );
    let flags: Vec<String> =
        if e.target.march.is_empty() { Vec::new() } else { vec![format!("-march={}", e.target.march)] };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let (bytes, align) = takt_frame::mcu::arena_layout(&frame, x, e.target.triple, &flags)?;

    let (ll, c) = (e.out.join(format!("{x}.ll")), e.out.join(format!("{x}_frame.c")));
    write(&ll, e.ir)?;
    write(&c, &frame.source)?;
    let (obj, obj_frame) = (e.out.join(format!("{x}.o")), e.out.join(format!("{x}_frame.o")));
    compile(&ll, &obj, e.target, &flags)?;
    compile(&c, &obj_frame, e.target, &flags)?;
    let lib = e.out.join(if e.triple.ends_with("-msvc") { format!("{x}.lib") } else { format!("lib{x}.a") });
    archive(&lib, &[&obj, &obj_frame])?;

    write(&e.out.join(format!("{x}.h")), &header(&frame.header, e.program, x, e.nvm_blocking_ns))?;
    let layout = takt_frame::layout::of(e.program);
    let drivers = takt_frame::drivers::of(e.program, &layout);
    let module = takt_frame::embed::Module {
        prefix: x,
        consts: &e.consts_rs,
        arena: (bytes, align),
        drivers: &drivers,
        drivers_type: e.drivers_type,
    };
    write(&e.out.join(format!("{x}.rs")), &takt_frame::embed::rust_module(&module))?;
    write(&e.out.join(format!("{x}.manifest")), &manifest(e, &drivers, bytes, align))?;
    println!(
        "{}: `{x}` fuer {} als {}, Arena {bytes} Byte (Ausrichtung {align}), {} Treiber",
        e.out.display(),
        e.triple,
        e.form.name(),
        drivers.len()
    );
    Ok(())
}

/// Das Profil im Programm darf der Form nicht widersprechen (12.8, 12.11):
/// Die Form legt es fest, und ein anderes im Programm ist ein Fehler mit
/// beiden Namen.
fn check_form(p: &takt_mir::Program, form: Form) -> Result<(), String> {
    let (Some(declared), Some(wanted)) = (p.config.runtime_profile(), form.profile()) else { return Ok(()) };
    if p.config.target.is_some() && declared != wanted {
        return Err(format!(
            "das Programm nennt `system: target = {}`, die Form `{}` verlangt `{}`; `system: target` weglassen, die \
             Form legt das Profil fest (12.11)",
            declared.name(),
            form.name(),
            wanted.name()
        ));
    }
    Ok(())
}

/// Der Kopf `P.h`: der Kopf des Rahmens und die Konstanten des Programms,
/// dieselben wie im Rust-Modul, mit dem Praefix.
fn header(frame_header: &str, p: &takt_mir::Program, x: &Prefix, nvm_blocking_ns: i64) -> String {
    let upper = x.as_str().to_uppercase();
    let hash = takt_mir::hash::logic_hash(p);
    let key = u64::from_le_bytes(hash.0[..8].try_into().unwrap_or_default());
    let mut consts = String::new();
    let _ = writeln!(consts, "/* Die Konstanten des Programms (5.9, 7.1, 7.3, 12.3). */");
    let _ = writeln!(consts, "#define {upper}_TICK_NS {}LL", p.config.tick);
    let _ = writeln!(consts, "#define {upper}_NVM_BLOCKING_NS {nvm_blocking_ns}LL");
    let alert = u8::from(p.config.overrun == takt_mir::program::OverrunPolicy::Alert);
    let _ = writeln!(consts, "#define {upper}_OVERRUN_ALERT {alert}");
    let _ = writeln!(consts, "#define {upper}_LOGIC_HASH {key:#018x}ULL");
    let _ = writeln!(consts, "#define {upper}_PERSIST_BOUND {}u", takt_mir::persist::max_payload(p).unwrap_or(0));
    let min = takt_mir::persist::min_interval_ns(p).unwrap_or(0);
    let _ = writeln!(consts, "#define {upper}_PERSIST_MIN_INTERVAL_NS {min}LL\n");
    let _ = writeln!(consts, "/* Die Ausgaenge in der Reihenfolge, die `{x}_output` erwartet. */");
    let outputs = crate::output_names(p);
    for (index, name) in outputs.iter().enumerate() {
        let _ = writeln!(consts, "#define {upper}_OUT_{name} {index}");
    }
    let _ = writeln!(consts, "#define {upper}_OUTPUTS {}\n", outputs.len());
    let body = frame_header.trim_end().strip_suffix("#endif").unwrap_or(frame_header);
    format!("{body}{consts}#endif\n")
}

/// Das Manifest `P.manifest` (12.11): was der Wirt in seinen Speicher- und
/// Bauplan uebernimmt.
fn manifest(e: &Embed<'_>, drivers: &[takt_frame::drivers::Driver], bytes: u64, align: u64) -> String {
    let p = e.program;
    let x = e.prefix;
    let hash = takt_mir::hash::logic_hash(p);
    let mut s = String::new();
    let _ = writeln!(s, "# takt-manifest 1");
    let _ = writeln!(s, "# Erzeugt von `takt build --emit embed` (12.11).");
    let _ = writeln!(s, "prefix = {x}");
    let _ = writeln!(s, "abi = {}", takt_frame::embed::ABI);
    let _ = writeln!(s, "logic_hash = {}", hash.0.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let _ = writeln!(s, "target = {}", e.target.name);
    let _ = writeln!(s, "triple = {}", e.triple);
    let _ = writeln!(s, "form = {}", e.form.name());
    let _ = writeln!(s, "profile = {}", e.form.profile().map_or("none", RuntimeProfile::name));
    let _ = writeln!(s, "tick_ns = {}", p.config.tick);
    let _ = writeln!(s, "arena_bytes = {bytes}");
    let _ = writeln!(s, "arena_align = {align}");
    let _ = writeln!(s, "persist_bytes = {}", takt_mir::persist::max_payload(p).unwrap_or(0));
    let jobs = !takt_frame::parts::job_slots(p).is_empty();
    let _ = writeln!(s, "jobs = {jobs}");
    let mut services = Vec::new();
    if takt_mir::persist::max_payload(p).is_some_and(|n| n > 0) {
        services.push("journal");
    }
    if jobs {
        services.push("job_context");
    }
    if takt_mir::sys::next_run(p).is_some() {
        services.push("next_run");
    }
    let _ = writeln!(s, "services = {}", services.join(", "));
    let symbols: Vec<String> = drivers.iter().map(|d| d.symbol(x)).collect();
    let _ = writeln!(s, "drivers = {}", symbols.join(", "));
    s
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Uebersetzt eine Quelle (IR oder C) fuer das Ziel, mit den Flags, die
/// auch der erzeugte Code bekommt.
fn compile(src: &Path, obj: &Path, target: Target, flags: &[&str]) -> Result<(), String> {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        return Err("clang fehlt; ohne ihn entsteht keine Bibliothek".into());
    };
    let mut cmd = Command::new(&clang);
    let cmd = takt_llvm::toolchain::Clang::deterministic(&mut cmd)
        .args(["-c", "-Wno-override-module", "-ffreestanding"])
        .args(takt_llvm::toolchain::object_flags(target.triple))
        .arg(format!("--target={}", target.triple))
        .args(flags);
    if target.is_bare_metal() {
        cmd.arg("-nostdlib");
    }
    let out = cmd.arg(src).arg("-o").arg(obj).output().map_err(|e| format!("{}: {e}", clang.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{}: {}", src.display(), String::from_utf8_lossy(&out.stderr)))
    }
}

/// Bindet die Objekte zur Bibliothek, mit dem `llvm-ar` neben clang.
fn archive(lib: &Path, objs: &[&Path]) -> Result<(), String> {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        return Err("clang fehlt".into());
    };
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let _ = std::fs::remove_file(lib);
    let ok = Command::new(&ar).arg("crs").arg(lib).args(objs).status().is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err(format!("{}: llvm-ar schlug fehl", lib.display())) }
}
