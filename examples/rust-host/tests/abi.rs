//! Bibliothek und Huelle muessen dieselbe Schnittstelle sprechen (12.11):
//! Die Huelle liest das Symbol `valve_abi_<n>`, und eine Huelle anderer
//! Version bindet nicht gegen `libvalve` — der Link scheitert, nicht der Lauf.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Das Verzeichnis, in das `build.rs` die Bibliothek `valve` gelegt hat.
fn library_dir() -> PathBuf {
    Path::new(env!("TAKT_VALVE_RS")).parent().expect("Verzeichnis des Moduls").to_path_buf()
}

/// Bindet ein Programm, das das Symbol `valve_abi_<version>` der Bibliothek
/// liest, und liefert die Meldungen des Linkers.
fn link_reading(version: u32) -> (bool, String) {
    let symbol = format!("valve_abi_{version}");
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("link-v{version}"));
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let main = dir.join("main.rs");
    std::fs::write(
        &main,
        format!(
            "unsafe extern \"C\" {{ static {symbol}: u8; }}\n\
             fn main() {{ let _ = unsafe {{ core::ptr::read_volatile(&raw const {symbol}) }}; }}\n"
        ),
    )
    .expect("Quelle");
    let out = Command::new("rustc")
        .args(["--edition", "2024", "--crate-type", "bin", "-o"])
        .arg(dir.join("main.exe"))
        .arg("-L")
        .arg(format!("native={}", library_dir().display()))
        .args(["-l", "static=valve"])
        .arg(&main)
        .output()
        .expect("rustc aufrufbar");
    let text = String::from_utf8_lossy(&out.stderr).into_owned() + &String::from_utf8_lossy(&out.stdout);
    (out.status.success(), text)
}

/// **Eine Huelle der Version 1 bindet nicht gegen eine Bibliothek der
/// Version 2** (sie kennt `valve_output_timing` nicht, FB-417), und die
/// Meldung nennt das Symbol. Die Gegenprobe: Mit dem Symbol der Bibliothek
/// fehlt es nicht (andere Symbole des Wirts fehlen diesem Programm ohnehin).
#[test]
fn a_hull_of_another_abi_version_does_not_link() {
    let (linked, text) = link_reading(1);
    assert!(!linked, "eine fremde Version band:\n{text}");
    assert!(text.contains("valve_abi_1"), "die Meldung nennt das Symbol:\n{text}");
    let (_, text) = link_reading(2);
    assert!(!text.contains("valve_abi_2"), "die eigene Version fehlt nicht:\n{text}");
}
