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
    // 8.10: was das Ziel ueber Speicher und Stack sagt.
    let memory = e.hardware.and_then(|hw| hw.target(e.target.name)).map(|t| t.memory).unwrap_or_default();
    let window = protection(e.program, memory.protect)?;
    let job_stack_reserve = match memory.job_stack_reserve {
        Some(n) => u32::try_from(n).map_err(|_| format!("job_stack_reserve = {n}: passt nicht in 32 Bit"))?,
        None => takt_frame::mcu::JOB_STACK_RESERVE,
    };
    let frame = takt_frame::mcu::build_with(
        e.program,
        takt_frame::mcu::Frame {
            diagnostics: e.diagnostics,
            hardware: e.hardware,
            protect: window.map(|w| w.protected),
            prefix: x.clone(),
            stubs: false,
            job_stack_reserve,
        },
    );
    let flags: Vec<String> =
        if e.target.march.is_empty() { Vec::new() } else { vec![format!("-march={}", e.target.march)] };
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let (bytes, align) = takt_frame::mcu::arena_layout(&frame, x, e.target.triple, &flags)?;
    // 12.3: Die Region ist an ihrer Groesse ausgerichtet, also auch die Arena.
    let align = window.map_or(align, |w| align.max(w.size));

    let (ll, c) = (e.out.join(format!("{x}.ll")), e.out.join(format!("{x}_frame.c")));
    write(&ll, e.ir)?;
    write(&c, &frame.source)?;
    let (obj, obj_frame) = (e.out.join(format!("{x}.o")), e.out.join(format!("{x}_frame.o")));
    compile(&ll, &obj, e.target, Code::Generated, &flags)?;
    compile(&c, &obj_frame, e.target, Code::Frame, &flags)?;
    let lib = e.out.join(if e.triple.ends_with("-msvc") { format!("{x}.lib") } else { format!("lib{x}.a") });
    archive(&lib, &[&obj, &obj_frame])?;
    let tick_stack = tick_stack(e, &obj, &memory)?;

    let job_stack = frame.job_stack_bytes;
    let stacks = Stacks { tick: tick_stack.bytes(), job: job_stack };
    write(&e.out.join(format!("{x}.h")), &header(&frame.header, e.program, x, e.nvm_blocking_ns, &stacks))?;
    let layout = takt_frame::layout::of(e.program);
    let drivers = takt_frame::drivers::of(e.program, &layout);
    let logic = takt_frame::mcu::logic_hex(e.program);
    let module = takt_frame::embed::Module {
        prefix: x,
        consts: &e.consts_rs,
        arena: (bytes, align),
        tick_stack_bytes: tick_stack.bytes(),
        job_stack_bytes: job_stack,
        logic: &logic,
        drivers: &drivers,
        drivers_type: e.drivers_type,
    };
    write(&e.out.join(format!("{x}.rs")), &takt_frame::embed::rust_module(&module))?;
    // 12.3: Unter `xip_flash` gehoert der Tick-Pfad in den RAM; die
    // Fragmente sagen dem Linker des Wirts, was dazu gehoert.
    let xip = memory.iram.is_some();
    if xip {
        write(&e.out.join(format!("{x}_ram.x")), &takt_frame::linker::ld_fragment(x))?;
        write(&e.out.join(format!("{x}.lf")), &takt_frame::linker::ldgen_fragment(x))?;
    }
    let placement = Placement { bytes, align, tick_at: frame.tick_at, protect: memory.protect.zip(window) };
    let layout_lines = Layout { arena: &placement, tick_stack: &tick_stack, job_stack, xip };
    write(&e.out.join(format!("{x}.manifest")), &manifest(e, &drivers, &layout_lines))?;
    let sys = drivers.iter().filter(|d| d.kind == takt_frame::drivers::Kind::Sys).count();
    println!(
        "{}: `{x}` fuer {} als {}, Arena {bytes} Byte (Ausrichtung {align}), {} Treiber, {sys} sys-Kanaele",
        e.out.display(),
        e.triple,
        e.form.name(),
        drivers.len() - sys
    );
    Ok(())
}

/// Die Region der Schutzeinheit ueber dem Programmbereich der Arena (12.3),
/// wenn die Konfiguration eine nennt.
fn protection(
    p: &takt_mir::Program,
    unit: Option<takt_mir::hardware::Protect>,
) -> Result<Option<takt_mir::hardware::Window>, String> {
    let Some(unit) = unit else { return Ok(None) };
    let bytes = takt_llvm::arena::of(p).bytes;
    unit.window(bytes)
        .map(Some)
        .ok_or_else(|| format!("protect = {}: keine Region deckt {bytes} Byte Programmbereich", unit.name()))
}

/// Was der Wirt in seinen Speicher- und Bauplan uebernimmt (12.11): die
/// Arena, die Stacks und ob der Tick-Pfad im RAM liegen muss.
struct Layout<'a> {
    arena: &'a Placement,
    tick_stack: &'a TickStack,
    job_stack: u64,
    xip: bool,
}

/// Die Groessen der beiden Stacks, die der Wirt stellt (12.3, 12.11).
struct Stacks {
    tick: u64,
    job: u64,
}

/// Der Schritt-Stack (12.3): das Programm exakt aus seinem Objekt, dazu die
/// Reserve des Ports und die Marge aus der Hardware-Konfiguration.
struct TickStack {
    /// Was das Programm selbst braucht, entlang seines Aufrufgraphen.
    program: u64,
    /// Runtime, Treiber und ISRs des Ports, gemessen (13.8); `None`, wenn
    /// die Konfiguration keine nennt.
    reserve: Option<u64>,
    /// Die Marge, die das Projekt waehlt.
    margin: u64,
}

impl TickStack {
    /// `TICK_STACK_BYTES`; ohne Reserve nur der Anteil des Programms.
    fn bytes(&self) -> u64 {
        self.program + self.reserve.unwrap_or(0) + self.margin
    }
}

/// Bemisst den Schritt-Stack am Objekt des Programms (12.3).
///
/// Ohne Reserve in der Konfiguration ist `TICK_STACK_BYTES` keine Schranke
/// fuer den Wirt, nur der Anteil des Programms; das Manifest sagt es, und
/// ausser in logischer Zeit, wo kein Stack des Wirts zu bemessen ist, meldet
/// der Bau es.
fn tick_stack(e: &Embed<'_>, obj: &Path, memory: &takt_mir::hardware::Memory) -> Result<TickStack, String> {
    let Some(depth) = crate::program_stack(e.program, obj, e.prefix)? else {
        return Err(format!(
            "{}: der Stack des Programms ist aus seinen Rahmen nicht zu rechnen (12.3)",
            obj.display()
        ));
    };
    let stack =
        TickStack { program: depth.bytes, reserve: memory.stack_reserve, margin: memory.stack_margin.unwrap_or(0) };
    if stack.reserve.is_none() && e.form != Form::Logical {
        eprintln!(
            "{}: keine `stack_reserve` fuer {} in der Hardware-Konfiguration; `TICK_STACK_BYTES` ist nur der \
             Anteil des Programms ({} Byte), ohne Runtime, Treiber und ISRs (12.3, 13.8)",
            e.out.display(),
            e.target.name,
            stack.program
        );
    }
    Ok(stack)
}

/// Wie die Arena liegt: Groesse, Ausrichtung, die Stelle des Ticks und die
/// Schutzregion (12.11).
struct Placement {
    bytes: u64,
    align: u64,
    tick_at: u64,
    protect: Option<(takt_mir::hardware::Protect, takt_mir::hardware::Window)>,
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
fn header(frame_header: &str, p: &takt_mir::Program, x: &Prefix, nvm_blocking_ns: i64, stacks: &Stacks) -> String {
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
    // 12.3, 12.11: Beide Stacks stellt der Wirt, den Job-Stack an 32 Byte ausgerichtet.
    let _ = writeln!(consts, "#define {upper}_TICK_STACK_BYTES {}u", stacks.tick);
    let _ = writeln!(consts, "#define {upper}_JOB_STACK_BYTES {}u", stacks.job);
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
fn manifest(e: &Embed<'_>, drivers: &[takt_frame::drivers::Driver], plan: &Layout<'_>) -> String {
    let p = e.program;
    let x = e.prefix;
    let arena = plan.arena;
    let mut s = String::new();
    let _ = writeln!(s, "# takt-manifest 1");
    let _ = writeln!(s, "# Erzeugt von `takt build --emit embed` (12.11).");
    let _ = writeln!(s, "prefix = {x}");
    let _ = writeln!(s, "abi = {}", takt_frame::embed::ABI);
    let _ = writeln!(s, "logic_hash = {}", takt_frame::mcu::logic_hex(p));
    let _ = writeln!(s, "target = {}", e.target.name);
    let _ = writeln!(s, "triple = {}", e.triple);
    let _ = writeln!(s, "form = {}", e.form.name());
    let _ = writeln!(s, "profile = {}", e.form.profile().map_or("none", RuntimeProfile::name));
    // 8.4: das Parameterprofil, dessen Werte das Abbild traegt.
    let _ = writeln!(s, "params_profile = {}", p.config.params_profile.as_deref().unwrap_or("none"));
    let _ = writeln!(s, "tick_ns = {}", p.config.tick);
    // Die Arena stellt der Wirt unter diesem Namen, damit `takt check-image`
    // sie im Abbild findet (12.11).
    let _ = writeln!(s, "arena_symbol = {x}_arena");
    let _ = writeln!(s, "arena_bytes = {}", arena.bytes);
    let _ = writeln!(s, "arena_align = {}", arena.align);
    // 12.3: der Schritt-Stack mit seiner Herkunft; ohne Reserve nur das Programm.
    let t = plan.tick_stack;
    let reserve = t.reserve.map_or_else(|| "none".to_string(), |r| r.to_string());
    let _ = writeln!(s, "# Schritt-Stack = Programm + Reserve + Marge (12.3)");
    let _ = writeln!(s, "tick_stack_bytes = {}", t.bytes());
    let _ = writeln!(s, "tick_stack_program = {}", t.program);
    let _ = writeln!(s, "tick_stack_reserve = {reserve}");
    let _ = writeln!(s, "tick_stack_margin = {}", t.margin);
    let _ = writeln!(s, "job_stack_bytes = {}", plan.job_stack);
    // Stellt der Wirt einen Stack statisch, dann unter diesem Namen: `takt
    // check-image` prueft seine Groesse (12.3).
    let _ = writeln!(s, "tick_stack_symbol = {x}_tick_stack");
    let _ = writeln!(s, "job_stack_symbol = {x}_job_stack");
    // Wo der Tick in der Arena steht: Eine Probe liest ihn, wenn die Leitung schweigt.
    let _ = writeln!(s, "tick_at = {}", arena.tick_at);
    // 12.3: die Schutzregion und wie viel der Arena sie deckt.
    match arena.protect {
        Some((unit, w)) => {
            let _ = writeln!(s, "protect = {}", unit.name());
            let _ = writeln!(s, "protect_bytes = {}", w.protected);
        }
        None => {
            let _ = writeln!(s, "protect = none");
        }
    }
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
    // Die Kanaele des Geraets `sys` stellt der Wirt im Trait `Sys`; Treiber
    // sind sie nicht (12.7, 12.11).
    let (sys, drivers): (Vec<_>, Vec<_>) = drivers.iter().partition(|d| d.kind == takt_frame::drivers::Kind::Sys);
    let symbols =
        |list: &[&takt_frame::drivers::Driver]| list.iter().map(|d| d.symbol(x)).collect::<Vec<_>>().join(", ");
    let _ = writeln!(s, "drivers = {}", symbols(&drivers));
    let _ = writeln!(s, "sys = {}", symbols(&sys));
    // 12.3: Unter `xip_flash` liegt alles, was diese Einstiege rufen, im RAM.
    let _ = writeln!(s, "xip_flash = {}", plan.xip);
    let _ = writeln!(s, "tick_path = {}", takt_frame::mcu::tick_path(x).join(", "));
    s
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Was eine Quelle der Lieferform ist; danach richten sich die Flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    /// Erzeugter Code und C, das wie er uebersetzt wird.
    Generated,
    /// Der Rahmen: der Weg jedes Ticks, ohne Outliner (FB-366).
    Frame,
}

/// Uebersetzt eine Quelle (IR oder C) fuer das Ziel mit den Flags ihrer Art
/// und `flags`.
pub(crate) fn compile(src: &Path, obj: &Path, target: Target, code: Code, flags: &[&str]) -> Result<(), String> {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        return Err("clang fehlt; ohne ihn entsteht keine Bibliothek".into());
    };
    let mut cmd = Command::new(&clang);
    let cmd = takt_llvm::toolchain::Clang::deterministic(&mut cmd)
        .args(["-c", "-Wno-override-module", "-ffreestanding"])
        .args(match code {
            Code::Generated => takt_llvm::toolchain::object_flags(target.triple),
            Code::Frame => takt_llvm::toolchain::frame_flags(target.triple),
        })
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
pub(crate) fn archive(lib: &Path, objs: &[&Path]) -> Result<(), String> {
    let takt_llvm::toolchain::Clang::At(clang) = takt_llvm::toolchain::find() else {
        return Err("clang fehlt".into());
    };
    let ar = clang.with_file_name(if cfg!(windows) { "llvm-ar.exe" } else { "llvm-ar" });
    let _ = std::fs::remove_file(lib);
    let ok = Command::new(&ar).arg("crs").arg(lib).args(objs).status().is_ok_and(|s| s.success());
    if ok { Ok(()) } else { Err(format!("{}: llvm-ar schlug fehl", lib.display())) }
}
