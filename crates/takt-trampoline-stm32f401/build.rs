//! Legt `memory.x` dorthin, wo der Linker sie sucht, und bindet mit
//! `link.x` aus `cortex-m-rt` — wie das Bring-up, damit auch ein Bau von
//! aussen die Lage in Sektor 1 bekommt.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).expect("memory.x schreiben");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=--nmagic");
}
