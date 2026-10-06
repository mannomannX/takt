//! Baut das Takt-Programm fuer den ESP32-C6 und bindet es ein.
//!
//! **Wie jeder Wirt** (12.11, M11 Schritt 10): Der Bauhelfer aus
//! `takt-embed` ruft `takt build --emit embed` und bindet die Bibliothek
//! `libapp.a` mit Rahmen und erzeugtem Code; das Modul `app.rs` traegt
//! Konstanten, Arena, Treiber-Traits und die Huelle. Die Linker-Argumente
//! stehen hier und nicht in `.cargo/config.toml`: Die Konfigurationsdatei
//! gilt nur, wenn `cargo` aus diesem Verzeichnis laeuft, und ein Bau von
//! aussen erzeugte sonst still ein Binary ohne Speicherkarte.
//!
//! **Die Treiber** (12.6) erzeugt `bringup::rig` als Pruefstand nach der
//! Verdrahtung des Boards und des Pruefgeraets.
//!
//! **Das Messprogramm von `takt bench`** (13.8) bindet das Merkmal `bench`
//! wie jeder Wirt: `takt bench --emit embed` ueber den Bauhelfer, im RAM wie
//! das Programm (12.3).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_conformance::bringup;

fn main() {
    // `linkall.x` liefert `esp-hal`: Speicherkarte, Vektortabelle, Cache-Mapping.
    println!("cargo:rustc-link-arg=-Tlinkall.x");
    image_key();
    // Konformitaetslauf (plan/esp32c6.md 5): so viele Ticks, dann `takt end`.
    println!("cargo:rerun-if-env-changed=TAKT_TICKS");
    if let Ok(ticks) = env::var("TAKT_TICKS") {
        println!("cargo:rustc-env=TAKT_TICKS={ticks}");
    }
    // `TAKT_FRESH_JOURNAL`: das Journal vor dem Lauf loeschen — wie der
    // Interpreter ohne Speicher beginnen (Konformitaetslauf).
    println!("cargo:rerun-if-env-changed=TAKT_FRESH_JOURNAL");
    if env::var("TAKT_FRESH_JOURNAL").is_ok() {
        println!("cargo:rustc-env=TAKT_FRESH_JOURNAL=1");
    }
    // 11.2: `statements` fuellt `pc` je Maschine (`takt build --instrument`);
    // Default auf `baremetal` ist `states`, also aus.
    println!("cargo:rerun-if-env-changed=TAKT_INSTRUMENT");
    println!("cargo:rerun-if-env-changed=TAKT_DIAGNOSTICS");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let built = build_takt_program(&out);
    let bench = bench();
    ram_resident(&out, &built, bench.as_ref());
    native_vectors(&out);
    math_vectors(&out);
}

/// Der Schluessel des Baus (KON1-009) als Symbol ins ELF
/// (`takt_board_support::image_key`): Wer parallel in dasselbe
/// Zielverzeichnis baut, prueft an seiner Kopie, dass sie die eigene ist.
/// Ohne `TAKT_IMAGE_KEY` traegt das Abbild keinen.
fn image_key() {
    use takt_board_support::image_key::{SYMBOL, valid};
    println!("cargo:rerun-if-env-changed=TAKT_IMAGE_KEY");
    if let Ok(key) = env::var("TAKT_IMAGE_KEY") {
        assert!(valid(&key), "TAKT_IMAGE_KEY: 1 bis 32 Hexziffern erwartet, `{key}` gefunden");
        println!("cargo:rustc-link-arg=--defsym={SYMBOL}{key}=0");
    }
}

/// Das Messprogramm von `takt bench` (13.8) fuer das Binary `bench`.
fn bench() -> Option<takt_embed::build::Built> {
    env::var_os("CARGO_FEATURE_BENCH")
        .map(|_| takt_embed::build::Bench::new().tool(bringup::takt()).hardware(hardware()).build())
}

/// Legt den Tick-Pfad ins RAM (12.3): das Fragment der Lieferform
/// (`app_ram.x`), mit dem Messprogramm dessen Fragment, und dahinter, was
/// das Board dazulegt (`board_ram.x`).
///
/// `esp-hal` bindet `rwtext_hook.x` in `.rwtext` ein; die Datei muss im
/// Suchpfad des Linkers liegen, und `OUT_DIR` steht dort. Eingeschaltet
/// wird der Haken ueber `ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK` — als echte
/// Umgebungsvariable zur Bauzeit von `esp-hal`, darum in
/// `.cargo/config.toml` und nicht hier.
fn ram_resident(out: &Path, built: &takt_embed::build::Built, bench: Option<&takt_embed::build::Built>) {
    // Ohne den Schalter bindet `esp-hal` den Haken nicht ein, und der
    // Tick-Pfad laege still im Flash (FB-449). Ein Bau von aussen
    // (`--manifest-path`) sieht `.cargo/config.toml` nicht.
    println!("cargo:rerun-if-env-changed=ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK");
    assert!(
        env::var("ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK").as_deref() == Ok("true"),
        "ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK=true fehlt: ohne ihn laege der Tick-Pfad im Flash (12.3); \
         `.cargo/config.toml` setzt ihn nur fuer einen Bau aus diesem Verzeichnis"
    );
    let Some(fragment) = built.ram_fragment() else {
        panic!("{}: kein Fragment fuer den RAM; die Konfiguration nennt `iram` (12.3)", built.manifest.display())
    };
    let board = Path::new(env!("CARGO_MANIFEST_DIR")).join("board_ram.x");
    println!("cargo:rerun-if-changed={}", board.display());
    let read = |p: &Path| fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let mut hook = read(&fragment);
    if let Some(bench) = bench {
        let Some(own) = bench.ram_fragment() else {
            panic!("{}: kein Fragment fuer den RAM (12.3)", bench.manifest.display())
        };
        hook = format!("{hook}\n{}", read(&own));
    }
    let hook = format!("{hook}\n{}", read(&board));
    fs::write(out.join("rwtext_hook.x"), hook).expect("rwtext_hook.x schreiben");
    println!("cargo:rustc-link-search={}", out.display());
}

fn build_takt_program(out: &Path) -> takt_embed::build::Built {
    let program = program_path();
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    println!("cargo:rerun-if-changed={program}");
    if !Path::new(&program).exists() {
        panic!("Takt-Programm nicht gefunden: {program}");
    }
    let Some(p) = bringup::compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let wiring =
        bringup::wiring(&[&here.join(bringup::WIRING), &here.join("../takt-driver-probe").join(bringup::WIRING)]);
    bringup::rig(&p, &wiring, &out.join("takt_rig.rs"));
    let mut embed = takt_embed::build::Program::new(&program)
        .tool(bringup::takt())
        .prefix(takt_llvm::symbols::Prefix::default().as_str())
        .form("own")
        .build_for(bringup::build_name())
        .drivers("crate::drivers::Rig")
        .hardware(hardware());
    // 11.2: `statements` fuellt `pc` je Maschine; Default auf `baremetal`
    // ist `states`, also aus.
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        embed = embed.instrument(&mode);
    }
    if let Ok(level) = env::var("TAKT_DIAGNOSTICS") {
        embed = embed.diagnostics(&level);
    }
    let built = embed.build();
    // Wo der Tick in der Arena steht, als absolutes Symbol: Die Probe liest
    // ihn ueber JTAG, wenn die Konsole schweigt (`Esp32c6::tick_over_jtag`).
    println!("cargo:rustc-link-arg=--defsym=__takt_tick_at={}", built.value("tick_at"));
    // Was das Board dazulegt: die Millicode-Routinen fuer `-msave-restore`,
    // wie das Programm im RAM.
    let millicode = here.join("millicode.S");
    println!("cargo:rerun-if-changed={}", millicode.display());
    let obj_mc = out.join("millicode.o");
    assemble(&millicode, &obj_mc);
    bringup::archive(out, "taktboard", &[&obj_mc]);
    println!("cargo:rustc-link-arg=--icf=all");
    built
}

/// Die Hardware-Konfiguration des Boards (8.10): Anschluesse, Kalibrierung,
/// NVM-Zeiten und die Reserve des Job-Stacks. `TAKT_HARDWARE` sticht, etwa
/// fuer `takt driver-test` mit gemessenem `guard`.
fn hardware() -> String {
    bringup::hardware_path().unwrap_or_else(|| {
        let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/hw/esp32c6.hw");
        println!("cargo:rerun-if-changed={}", own.display());
        own.display().to_string()
    })
}

/// Der Pfad des Programms aus `takt.toml`; `TAKT_PROGRAM` sticht fuer
/// einen einmaligen Versuch (FB-141 erklaert, warum die Datei der Normalfall ist).
fn program_path() -> String {
    if let Ok(p) = env::var("TAKT_PROGRAM") {
        return p;
    }
    let here = env!("CARGO_MANIFEST_DIR");
    let config = format!("{here}/takt.toml");
    println!("cargo:rerun-if-changed={config}");
    let text = fs::read_to_string(&config).unwrap_or_else(|e| panic!("{config}: {e}"));
    let value = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find_map(|l| {
            l.strip_prefix("program")?.trim_start().strip_prefix('=')?.trim().strip_prefix('"')?.strip_suffix('"')
        })
        .unwrap_or_else(|| panic!("{config}: kein `program = \"…\"`"));
    format!("{here}/{value}")
}

/// Die Vektoren der kuratierten Natives fuer das Messprogramm `natives`
/// (13.8), aus der Spezifikation `grammar/takt-native.md`.
fn native_vectors(out: &Path) {
    let spec = takt_conformance::natives::spec_path();
    println!("cargo:rerun-if-changed={}", spec.display());
    let text = fs::read_to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    let vectors = takt_conformance::natives::vectors(&text).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    fs::write(out.join("native_vectors.rs"), takt_conformance::natives::table_source(&vectors))
        .expect("native_vectors.rs schreiben");
}

/// Die Vektoren der korrekt gerundeten Mathematik fuer dasselbe
/// Messprogramm (4.2, 13.8).
fn math_vectors(out: &Path) {
    let spec = takt_conformance::math::spec_path();
    println!("cargo:rerun-if-changed={}", spec.display());
    let text = fs::read_to_string(&spec).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    let vectors = takt_conformance::math::vectors(&text).unwrap_or_else(|e| panic!("{}: {e}", spec.display()));
    fs::write(out.join("math_vectors.rs"), takt_conformance::math::table_source(&vectors))
        .expect("math_vectors.rs schreiben");
}

/// Uebersetzt die Millicode-Routinen fuer `-msave-restore`.
fn assemble(src: &Path, obj: &Path) {
    let ok = Command::new(bringup::clang())
        .args(["-c", "--target=riscv32-unknown-none-elf", "-march=rv32imac", "-mabi=ilp32"])
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}
