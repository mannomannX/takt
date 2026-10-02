//! Uebersetzt das Programm aus `TAKT_PROGRAM` fuer den Wirt und bindet es
//! mit seinem MCU-Rahmen (12.1) — wie die Bring-ups der Boards, nur ohne
//! Board: kein Speicherschutz, keine Vektortabelle, kein Linkerskript.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use takt_conformance::bringup;

fn main() {
    println!("cargo:rerun-if-env-changed=TAKT_PROGRAM");
    let program = env::var("TAKT_PROGRAM").unwrap_or_else(|_| panic!("`TAKT_PROGRAM` nennt kein Programm"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let triple = env::var("TARGET").expect("TARGET");
    let target = match triple.as_str() {
        "x86_64-pc-windows-msvc" => "x86_64-windows",
        "x86_64-unknown-linux-gnu" => "x86_64",
        "aarch64-unknown-linux-gnu" => "aarch64",
        other => panic!("kein Wirt, fuer den `takt build` uebersetzt: {other}"),
    };

    let Some(p) = bringup::compile(&program) else { panic!("{program}: uebersetzt nicht; die Fehler stehen oben") };
    let hardware = bringup::hardware();
    let frame = takt_frame::mcu::build_with(
        &p,
        takt_frame::mcu::Frame { hardware: hardware.as_ref(), ..Default::default() },
    );
    let rahmen = out.join("takt_rahmen.c");
    fs::write(&rahmen, frame.source).unwrap_or_else(|e| panic!("Rahmen nicht schreibbar: {e}"));

    let ir = out.join("takt_programm.ll");
    bringup::takt_build(&program, target, &["--emit", "ir"], &ir);
    bringup::takt_build(&program, target, &["--emit", "consts-rs"], &out.join("takt_consts.rs"));
    let (obj, obj_rahmen) = (out.join("takt_programm.o"), out.join("takt_rahmen.o"));
    translate(&triple, &ir, &obj);
    translate(&triple, &rahmen, &obj_rahmen);
    bringup::archive(&out, &[&obj, &obj_rahmen]);
}

/// Uebersetzt eine Quelle (IR oder C) fuer den Wirt. Der Rahmen bleibt
/// freistehend wie auf den Boards; was er braucht, bringt er mit.
fn translate(triple: &str, src: &Path, obj: &Path) {
    let ok = Command::new(bringup::clang())
        .args(["-c", "-Wno-override-module", "-ffreestanding", &format!("--target={triple}")])
        .args(takt_llvm::toolchain::object_flags(triple))
        .arg(src)
        .arg("-o")
        .arg(obj)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "{}: uebersetzt nicht", src.display());
}
