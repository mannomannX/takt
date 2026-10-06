//! `takt check-image` gegen ein gebundenes Abbild (12.11, M11 Schritt 11):
//! die Lieferform eines Programms fuer den ESP32-C6 (`xip_flash`), ein
//! kleiner Wirt in C und `ld.lld` mit einem Skript, das das erzeugte
//! Fragment in den RAM-Abschnitt `.rwtext` einbindet. Was der Wirt nicht
//! stellt — Randkern, Leitung —, bleibt ungeloest: Geprueft wird das Abbild,
//! nicht sein Lauf. Ein Abbild ohne Befund, und die drei absichtlichen
//! Fehler: ein Treiber im Flash, eine zweite beschreibbare Variable, eine zu
//! kleine Arena.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use takt_llvm::toolchain::find;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn takt(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_takt")).current_dir(root()).args(args).output().expect("takt startet")
}

const PROGRAM: &str = "system:\n    language = 1\n    tick     = 10 ms\n\n\
                       input  x : int in 0..100 @ hw(\"io/x\")\n\
                       output y : int in 0..100 @ hw(\"io/y\") with safe = 0\n\n\
                       machine m:\n    initial RUN\n\n    state RUN:\n        loop:\n            y = x\n";

/// Das Skript des Wirts: RAM ab `0x4080_0000` mit dem Fragment in `.rwtext`,
/// Flash ab `0x4200_0000` fuer alles andere — die Karte des C6.
const SCRIPT: &str = "ENTRY(_start)\n\
MEMORY {\n  IRAM (rwx) : ORIGIN = 0x40800000, LENGTH = 512K\n  FLASH (rx) : ORIGIN = 0x42000000, LENGTH = 4M\n}\n\
SECTIONS {\n  .rwtext : { INCLUDE app_ram.x\n    *(.rwtext .rwtext.*) } > IRAM\n\
  .data : { *(.data .data.* .sdata .sdata.*) } > IRAM\n\
  .bss (NOLOAD) : { *(.bss .bss.* .sbss .sbss.* COMMON) } > IRAM\n\
  .text : { *(.text .text.*) } > FLASH\n  .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } > FLASH\n}\n";

/// Was ein Wirt falsch machen kann.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    Clean,
    DriverInFlash,
    SecondWritable,
    SmallArena,
}

/// Die Lieferform in `dir`; `None` ohne die Werkzeuge, die das Abbild braucht.
fn delivery(dir: &Path) -> Option<(PathBuf, String)> {
    let clang = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")?;
    // Das `ld.lld`, das `clang -fuse-ld=lld` naehme.
    let lld = Command::new(&clang)
        .arg("-print-prog-name=ld.lld")
        .output()
        .ok()
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    let exists = |p: &PathBuf| p.is_file() || PathBuf::from(format!("{}.exe", p.display())).is_file();
    takt_testkit::require("ld.lld", lld.filter(exists), "LLVM mit lld installieren")?;
    takt_testkit::require("llvm-tools", takt_llvm::inspect::Binutils::llvm(), "`rustup component add llvm-tools`")?;
    let source = dir.join("probe.takt");
    std::fs::write(&source, PROGRAM).expect("Programm");
    let run = takt(&[
        "build",
        source.to_str().expect("Pfad"),
        "--emit",
        "embed",
        "--target",
        "riscv32imac-unknown-none-elf",
        "--form",
        "own",
        "--prefix",
        "app",
        "--hardware",
        "corpus-try/hw/esp32c6.hw",
        "--out",
        dir.to_str().expect("Pfad"),
    ]);
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    let manifest = std::fs::read_to_string(dir.join("app.manifest")).expect("Manifest");
    Some((clang, manifest))
}

fn value<'a>(manifest: &'a str, key: &str) -> &'a str {
    manifest.lines().find_map(|l| l.strip_prefix(&format!("{key} = "))).unwrap_or_else(|| panic!("{key} fehlt"))
}

/// Bindet das Abbild mit dem Wirt `host` und prueft es.
fn checked(name: &str, host: Host) -> Option<Output> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("takt-check-image-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (clang, manifest) = delivery(&dir)?;
    let bytes: u64 = value(&manifest, "arena_bytes").parse().expect("Zahl");
    let align = value(&manifest, "arena_align");
    let arena = if host == Host::SmallArena { bytes - 8 } else { bytes };
    let mut main = format!(
        "#define RAM __attribute__((section(\".rwtext.host\")))\n\
         #define FLASH __attribute__((section(\".text.host\")))\n\
         unsigned char app_arena[{arena}] __attribute__((aligned({align})));\n\
         int app_init_with(void *, void *, const void *, int);\nvoid app_tick(void *, long long);\n\
         RAM void _start(void) {{ app_init_with(app_arena, 0, 0, 0); app_tick(app_arena, 1); for (;;) {{}} }}\n"
    );
    for driver in value(&manifest, "drivers").split(", ") {
        let place = if host == Host::DriverInFlash && driver.starts_with("app_in_") { "FLASH" } else { "RAM" };
        main.push_str(&format!("{place} int {driver}(void) {{ return 1; }}\n"));
    }
    if host == Host::SecondWritable {
        main.push_str("int app_shadow;\n");
    }
    std::fs::write(dir.join("main.c"), main).expect("Wirt");
    std::fs::write(dir.join("image.ld"), SCRIPT).expect("Skript");
    let image = dir.join("image.elf");
    let link = Command::new(&clang)
        .args(["--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32", "-nostdlib", "-ffreestanding"])
        .args(["-fuse-ld=lld", "-Wl,-T,image.ld", "-Wl,-L,.", "-Wl,--unresolved-symbols=ignore-all"])
        .args(["-o", "image.elf", "main.c", "libapp.a"])
        .current_dir(&dir)
        .output()
        .expect("clang startet");
    assert!(link.status.success(), "{}", String::from_utf8_lossy(&link.stderr));
    Some(takt(&[
        "check-image",
        image.to_str().expect("Pfad"),
        "--manifest",
        dir.join("app.manifest").to_str().expect("Pfad"),
    ]))
}

fn report(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

/// **Ein Abbild ohne Befund** (12.11): ABI und Logik-Hash gebunden, die
/// Arena wie im Manifest, nichts Beschreibbares daneben, der Tick-Pfad im
/// RAM — die Bibliothek ueber das Fragment, der Treiber ueber den Wirt.
#[test]
fn a_well_linked_image_has_no_finding() {
    let Some(out) = checked("clean", Host::Clean) else { return };
    let text = report(&out);
    assert!(out.status.success(), "{text}");
    for want in ["ABI 4 gebunden", "Logik-Hash", "Arena `app_arena`", "alle im RAM oder ROM", "0 Befunde"] {
        assert!(text.contains(want), "`{want}` fehlt:\n{text}");
    }
}

/// **Ein Treiber im Flash** (12.3): Der Tick ruft ihn, und unter
/// `xip_flash` stuende der Kern waehrend eines Flash-Zugriffs.
#[test]
fn a_driver_in_flash_is_a_finding() {
    let Some(out) = checked("flash", Host::DriverInFlash) else { return };
    let text = report(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("Tick-Pfad im Flash: `app_in_io_x`") && text.contains("app_tick →"), "{text}");
}

/// **Eine zweite beschreibbare Variable** des Programms ausserhalb der
/// Arena: Zustand, den kein Schutz und kein Journal sieht.
#[test]
fn a_second_writable_variable_is_a_finding() {
    let Some(out) = checked("writable", Host::SecondWritable) else { return };
    let text = report(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("beschreibbar ausserhalb der Arena: `app_shadow`"), "{text}");
}

/// **Eine zu kleine Arena**: Der Wirt legte weniger an, als das Programm
/// braucht.
#[test]
fn a_too_small_arena_is_a_finding() {
    let Some(out) = checked("small", Host::SmallArena) else { return };
    let text = report(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("Arena `app_arena` hat") && text.contains("das Programm braucht"), "{text}");
}
