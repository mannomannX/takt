//! Legt `memory.x` bereit und uebersetzt das Takt-Programm mit.
//!
//! **Zwei Aufgaben, beide mit demselben Grund: Der Linker muss finden,
//! was er braucht.**
//!
//! `link.x` aus `cortex-m-rt` enthaelt ein `INCLUDE memory.x`, und der
//! Linker sucht die Datei in seinen `-L`-Pfaden. Baut man aus dem
//! Crate-Verzeichnis, findet er sie ueber das Arbeitsverzeichnis und
//! alles scheint zu gehen; von aussen gebaut bricht er ab oder, schlimmer,
//! zieht eine fremde `memory.x` — dann laege die Anwendung bei
//! 0x0800_0000 und ueberschriebe beim ersten Flashen den Bootloader.
//!
//! Dasselbe gilt fuer das Takt-Programm: Das Binary `takt` ruft
//! `app_init` und `app_tick`, und die entstehen erst, wenn eine
//! `.takt`-Datei uebersetzt und der MCU-Rahmen erzeugt wurde. Beides
//! passiert hier, damit `cargo build` genuegt — wie bei jedem Wirt ueber den
//! Bauhelfer aus `takt-embed` (12.11, M11 Schritt 10): Bibliothek `libapp.a`,
//! Modul `app.rs`.
//!
//! **Welches Programm?** `takt.toml` neben `Cargo.toml` nennt den Pfad
//! (FB-141); `TAKT_PROGRAM` sticht nur fuer einen einmaligen Versuch.
//!
//! **Die Treiber** (12.6) erzeugt `bringup::rig` als Pruefstand nach der
//! Verdrahtung des Boards und des Pruefgeraets.
//!
//! **Zwei Programme auf einem Kern** (12.11, M11 Schritt 16): Mit dem
//! Merkmal `pair` baut das Skript statt eines Programms zwei, `fast` aus
//! `TAKT_PROGRAM` und `slow` aus `TAKT_PAIR_SLOW`, ohne sie `pair_fast.takt`
//! und `pair_slow.takt` der Konformanztests, jedes mit seinem Praefix,
//! Pruefstand und Abschnitt der Arena, fuer das Binary `pair`.
//!
//! **Das Messprogramm von `takt bench`** (13.8) bindet das Merkmal `bench`
//! wie jeder Wirt: `takt bench --emit embed` ueber den Bauhelfer. Der F401
//! fuehrt aus dem Flash aus, und eine Messung haengt an der Lage des Codes
//! (FB-367): `TAKT_BENCH_SHIFT` verschiebt `.text` um so viele Byte.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use takt_conformance::bringup;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).expect("memory.x schreiben");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // **Die Linker-Argumente gehoeren hierher, nicht in
    // `.cargo/config.toml`.** Jene Datei gilt nur, wenn `cargo` aus
    // diesem Verzeichnis laeuft. Von aussen gebaut fehlte `-Tlink.x`, und
    // es entstuende eine Binaerdatei ohne Vektortabelle: Sie baut, sie
    // linkt, und sie ist unbrauchbar.
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=--nmagic");
    text_shift(&out);
    image_key();

    // 4.2, 12.11: die Fliesskomma-Umgebung vor dem Lauf verstellen, als
    // Pruefung, dass jeder Einstieg seine eigene herstellt.
    println!("cargo:rerun-if-env-changed=TAKT_HOSTILE_FPU");
    if env::var("TAKT_HOSTILE_FPU").is_ok() {
        println!("cargo:rustc-env=TAKT_HOSTILE_FPU=1");
    }

    let form = form();
    if env::var_os("CARGO_FEATURE_PAIR").is_some() {
        build_pair(&out);
    } else {
        build_takt_program(&out, form);
    }
    bench();
    native_vectors(&out);
    math_vectors(&out);
}

/// Die Lage von `.text` im Flash (FB-367): `memory.x` liest
/// `TAKT_TEXT_SHIFT` aus `takt_shift.x`. In Achteln, denn die f64-Routinen
/// aus `compiler_builtins` verlangen 8 Byte Ausrichtung (siehe `memory.x`).
fn text_shift(out: &Path) {
    println!("cargo:rerun-if-env-changed=TAKT_BENCH_SHIFT");
    let shift: u32 = env::var("TAKT_BENCH_SHIFT")
        .map_or(Ok(0), |s| s.parse())
        .unwrap_or_else(|e| panic!("TAKT_BENCH_SHIFT: keine Zahl ({e})"));
    assert!(shift % 8 == 0, "TAKT_BENCH_SHIFT = {shift}: nur Vielfache von 8 Byte (`memory.x`)");
    let text =
        format!("/* Erzeugt von `build.rs`: die Verschiebung von `.text` (FB-367). */\nTAKT_TEXT_SHIFT = {shift};\n");
    fs::write(out.join("takt_shift.x"), text).expect("takt_shift.x schreiben");
    println!("cargo:rustc-env=TAKT_BENCH_SHIFT={shift}");
}

/// Das Messprogramm von `takt bench` (13.8) fuer das Binary `bench`; in die
/// 192 KiB der Anwendung passt es nur in Teilen (`TAKT_BENCH_PART`, FB-740).
fn bench() {
    if env::var_os("CARGO_FEATURE_BENCH").is_some() {
        let mut bench = takt_embed::build::Bench::new().tool(bringup::takt()).hardware(hardware());
        if let Some(part) = bringup::bench_part() {
            bench = bench.part(part);
        }
        bench.build();
    }
}

/// Uebersetzt das Takt-Programm und bindet es als Objekt ein.
///
/// **Scheitert es, scheitert der Bau.** Eine erste Fassung begnuegte sich
/// mit einer Warnung, weil `blink` und `minimal` das Programm nicht
/// brauchen — und genau das verdeckte einen Ausfall: Das Build-Skript lief
/// nicht mehr, niemand sah die Warnung im Rauschen, und gebaut wurde
/// weiter gegen ein Objekt aus einem frueheren Lauf. Ein Binary, das
/// reproduzierbar sein soll (13.8), darf nicht davon abhaengen, was
/// zufaellig noch im Zielverzeichnis liegt.
///
/// `blink` und `minimal` bleiben baubar: Sie binden weder die Bibliothek
/// noch die erzeugten Konstanten ein.
fn build_takt_program(out: &Path, form: &str) {
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
        .form(form)
        .build_for(bringup::build_name())
        .drivers("crate::drivers::Rig")
        .hardware(hardware());
    // 11.2: Instrumentierung und Diagnosestufe, wenn der Lauf sie verlangt.
    if let Ok(mode) = env::var("TAKT_INSTRUMENT") {
        embed = embed.instrument(&mode);
    }
    if let Ok(level) = env::var("TAKT_DIAGNOSTICS") {
        embed = embed.diagnostics(&level);
    }
    let built = embed.build();
    // 12.3: Den Programmbereich schuetzt die MPU ausserhalb des Ticks; ohne
    // Schutzregion richtete der Speicherschutz des Boards nichts ein.
    let unit = built.value("protect");
    assert_eq!(
        unit, "armv7m_mpu",
        "das F401 schuetzt die Arena mit seiner MPU: `protect = armv7m_mpu` in der Hardware-Konfiguration (12.3)"
    );
    state_section(out, &built.value("protect_bytes"));
    // 12.11: Unter RTIC und in der Interruptform rechnen die Jobs auf dem
    // Hauptstack, und der Schritt unterbricht sie dort; der Hauptstack fasst
    // dann beide (FB-459).
    let tick: u64 = built.value("tick_stack_bytes").parse().expect("tick_stack_bytes: Zahl");
    let job: u64 = built.value("job_stack_bytes").parse().expect("job_stack_bytes: Zahl");
    tick_stack(out, &(tick + if form == "own" { 0 } else { job }).to_string());
    // 13.8: Die Bilanz nennt den Anteil des Programms; `takt bench` zieht ihn
    // von der Tiefe ab.
    println!("cargo:rustc-env=TAKT_TICK_STACK_PROGRAM={}", built.value("tick_stack_program"));
    println!("cargo:rustc-link-arg=--icf=all");
}

/// Zwei Programme fuer das Binary `pair` (12.11, M11 Schritt 16): `fast` und
/// `slow` in der Interruptform, jedes mit seinem Praefix und Pruefstand
/// (`takt-drivers-pair.toml`), gebaut gegen die Hardware-Konfiguration des
/// Boards mit den Kopplungen aus `takt-pair.hw`. Die Arena von `fast` liegt
/// in `.takt_state`, die von `slow` in `.takt_state2`, jede eine Region der
/// MPU (`takt_state_pair.x`); der Hauptstack traegt beide Schritte, denn der
/// von `fast` unterbricht den von `slow`.
fn build_pair(out: &Path) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let programs = here.join("../takt-conformance/tests/programs");
    let given = |var: &str, default: &str| {
        println!("cargo:rerun-if-env-changed={var}");
        env::var(var).unwrap_or_else(|_| programs.join(default).display().to_string())
    };
    let (fast, slow) = (given("TAKT_PROGRAM", "pair_fast.takt"), given("TAKT_PAIR_SLOW", "pair_slow.takt"));
    let wiring = bringup::wiring(&[&here.join("takt-drivers-pair.toml")]);
    let hw = pair_hardware(out, here);
    let mut built = Vec::new();
    for (prefix, program) in [("fast", fast), ("slow", slow)] {
        println!("cargo:rerun-if-changed={program}");
        let Some(p) = bringup::compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
        bringup::rig_in(&p, &wiring, &out.join(format!("{prefix}_rig.rs")), prefix);
        let drivers = format!("crate::{prefix}_drivers::Rig");
        let b = takt_embed::build::Program::new(&program)
            .tool(bringup::takt())
            .prefix(prefix)
            .form("interrupt")
            .build_for(bringup::build_name())
            .drivers(&drivers)
            .hardware(hw.clone())
            .build();
        assert_eq!(b.value("protect"), "armv7m_mpu", "{prefix}: das F401 schuetzt jede Arena mit seiner MPU (12.3)");
        built.push(b);
    }
    let [fast, slow] = built.as_slice() else { panic!("zwei Programme") };
    let number = |b: &takt_embed::build::Built, key: &str| -> u64 {
        b.value(key).parse().unwrap_or_else(|_| panic!("{key}: keine Zahl"))
    };
    println!("cargo:rerun-if-changed=takt_state_pair.x");
    fs::write(out.join("takt_state_pair.x"), include_bytes!("takt_state_pair.x")).expect("takt_state_pair.x schreiben");
    let (fast_bytes, slow_bytes) = (number(fast, "protect_bytes"), number(slow, "protect_bytes"));
    // Eine Region ist an ihrer Groesse ausgerichtet, einer Zweierpotenz ab
    // 32 Byte, die den geschuetzten Teil deckt.
    let slow_region = slow_bytes.next_power_of_two().max(32);
    println!("cargo:rustc-link-arg-bin=pair=--defsym=__takt_state_size={fast_bytes}");
    println!("cargo:rustc-link-arg-bin=pair=--defsym=__takt_state2_size={slow_bytes}");
    println!("cargo:rustc-link-arg-bin=pair=--defsym=__takt_state2_align={slow_region}");
    println!("cargo:rustc-link-arg-bin=pair=-Ttakt_state_pair.x");
    let stack = number(fast, "tick_stack_bytes").saturating_add(number(slow, "tick_stack_bytes"));
    println!("cargo:rerun-if-changed=takt_stack.x");
    fs::write(out.join("takt_stack.x"), include_bytes!("takt_stack.x")).expect("takt_stack.x schreiben");
    println!("cargo:rustc-link-arg-bin=pair=--defsym=__takt_tick_stack_bytes={stack}");
    println!("cargo:rustc-link-arg-bin=pair=-Ttakt_stack.x");
    println!("cargo:rustc-link-arg-bin=pair=--icf=all");
}

/// Die Hardware-Konfiguration fuer `pair`: die des Boards und dahinter die
/// Kopplungen aus `takt-pair.hw` (8.10, 12.11).
fn pair_hardware(out: &Path, here: &Path) -> String {
    let board = hardware();
    let fragment = here.join("takt-pair.hw");
    println!("cargo:rerun-if-changed={}", fragment.display());
    let text = fs::read_to_string(&board).unwrap_or_else(|e| panic!("{board}: {e}"))
        + "\n"
        + &fs::read_to_string(&fragment).unwrap_or_else(|e| panic!("{}: {e}", fragment.display()));
    let path = out.join("pair.hw");
    fs::write(&path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    path.display().to_string()
}

/// Die Form des Ports (12.11) aus den Merkmalen: `interrupt`, `poll`,
/// `rtic` (die Form `rtos` unter RTIC 2), ohne eines der eigene Kern. Das
/// Binary waehlt seinen Pfad mit `cfg(form = "…")`, und mit `cfg(timer =
/// "…")`, ob TIM2 periodisch tickt (eigener Kern) oder frei als Zeitachse
/// eines Alarms laeuft (alle anderen).
fn form() -> &'static str {
    let chosen: Vec<&str> = [("interrupt", "interrupt"), ("poll", "poll"), ("rtic", "rtos"), ("embassy", "embassy")]
        .into_iter()
        .filter(|(feature, _)| env::var_os(format!("CARGO_FEATURE_{}", feature.to_uppercase())).is_some())
        .map(|(_, form)| form)
        .collect();
    assert!(chosen.len() <= 1, "{chosen:?} sind mehrere Formen; ein Bau hat eine (12.11)");
    let form = chosen.first().copied().unwrap_or("own");
    let timer = if form == "own" { "periodic" } else { "free" };
    println!("cargo::rustc-check-cfg=cfg(form, values(\"own\", \"interrupt\", \"poll\", \"rtos\", \"embassy\"))");
    println!("cargo::rustc-check-cfg=cfg(timer, values(\"periodic\", \"free\"))");
    println!("cargo:rustc-cfg=form=\"{form}\"");
    println!("cargo:rustc-cfg=timer=\"{timer}\"");
    // 12.11: Embassy ist die Form `rtos` mit dem Planer von Embassy; so heisst
    // sie im Kopf des Laufs und fuer die Lieferform.
    let named = if form == "embassy" { "rtos" } else { form };
    println!("cargo:rustc-env=TAKT_FORM={named}");
    named
}

/// Die Hardware-Konfiguration des Boards (8.10): Kalibrierung, Speicher,
/// Schutzeinheit. `TAKT_HARDWARE` sticht, etwa fuer `takt driver-test` mit
/// gemessenem `guard`.
fn hardware() -> String {
    bringup::hardware_path().unwrap_or_else(|| {
        let own = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus-try/hw/stm32f401.hw");
        println!("cargo:rerun-if-changed={}", own.display());
        own.display().to_string()
    })
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

/// Die Arena als eigener Abschnitt am Anfang des RAM (12.3, 12.11,
/// `takt_state.x`). `protected` ist, was die MPU-Region von ihr deckt: der
/// Programmbereich in ganzen Achteln, wie die Lieferform ihn aufgefuellt hat
/// (`protect_bytes` im Manifest); die Runtime dahinter bleibt ungeschuetzt.
fn state_section(out: &Path, protected: &str) {
    println!("cargo:rerun-if-changed=takt_state.x");
    fs::write(out.join("takt_state.x"), include_bytes!("takt_state.x")).expect("takt_state.x schreiben");
    println!("cargo:rustc-link-arg=--defsym=__takt_state_size={protected}");
    println!("cargo:rustc-link-arg=-Ttakt_state.x");
}

/// Der Hauptstack ist der Schritt-Stack (12.3): Der Linker prueft, dass
/// zwischen dem Waechter und `_stack_start` `TICK_STACK_BYTES` Platz haben
/// (`takt_stack.x`).
fn tick_stack(out: &Path, bytes: &str) {
    println!("cargo:rerun-if-changed=takt_stack.x");
    fs::write(out.join("takt_stack.x"), include_bytes!("takt_stack.x")).expect("takt_stack.x schreiben");
    println!("cargo:rustc-link-arg=--defsym=__takt_tick_stack_bytes={bytes}");
    println!("cargo:rustc-link-arg=-Ttakt_stack.x");
}

/// Welches Programm gebaut wird.
///
/// **Aus `takt.toml`, nicht aus der Umgebung.** Der Pfad bestimmt den
/// Inhalt des Binaries, und eine Umgebungsvariable taete das unsichtbar:
/// Einem fertigen Binary sieht man nicht an, mit welchem Wert es entstand.
/// Ein Bau ohne `TAKT_PROGRAM` fiel darum still auf einen Default zurueck
/// und erzeugte ein Programm, das auf dem Board korrekt lief und dabei
/// dunkel blieb — es hing an einem Kommando, das dort niemand sendet
/// (FB-141).
///
/// `TAKT_PROGRAM` sticht weiterhin: fuer einen einmaligen Versuch, nicht
/// fuer den Normalfall.
fn program_path() -> String {
    if let Ok(p) = env::var("TAKT_PROGRAM") {
        return p;
    }
    let here = env!("CARGO_MANIFEST_DIR");
    let config = format!("{here}/takt.toml");
    println!("cargo:rerun-if-changed={config}");
    let text = fs::read_to_string(&config).unwrap_or_else(|e| panic!("{config}: {e}"));
    // Eine Zeile `program = "…"`; ein TOML-Parser waere fuer einen
    // Schluessel eine Abhaengigkeit zu viel.
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
