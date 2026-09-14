//! Legt `memory.x` dorthin, wo der Linker es findet.
//!
//! **Ohne diese Datei ist der Bau vom Zufall abhaengig.** `link.x` aus
//! `cortex-m-rt` enthaelt ein `INCLUDE memory.x`, und der Linker sucht die
//! Datei in seinen `-L`-Pfaden. Baut man aus dem Crate-Verzeichnis, findet
//! er sie ueber das Arbeitsverzeichnis und alles scheint zu gehen; baut man
//! aus dem Workspace-Wurzelverzeichnis — wie `tools/embedded.sh` es tut —,
//! bricht er ab oder, schlimmer, zieht eine fremde `memory.x`. Dann laege
//! die Anwendung bei 0x0800_0000 und ueberschriebe beim ersten Flashen den
//! Bootloader.
//!
//! Der uebliche Weg: Die Datei nach `OUT_DIR` kopieren und das Verzeichnis
//! als Suchpfad anmelden.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("memory.x"), include_bytes!("memory.x")).expect("memory.x schreiben");
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // **Die Linker-Argumente gehoeren hierher, nicht in
    // `.cargo/config.toml`.** Jene Datei gilt nur, wenn `cargo` aus
    // diesem Verzeichnis laeuft. Von aussen gebaut — etwa mit
    // `--manifest-path`, wie `tools/embedded.sh` es koennte — fehlte
    // `-Tlink.x`, und es entstuende eine Binaerdatei ohne Vektortabelle:
    // Sie baut, sie linkt, und sie ist unbrauchbar. Ein Fehler, der sich
    // erst am Board zeigt, und dort als „tut nichts".
    println!("cargo:rustc-link-arg=-Tlink.x");
    println!("cargo:rustc-link-arg=--nmagic");
}
