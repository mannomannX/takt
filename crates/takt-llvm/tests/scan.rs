//! Der Durchlaufautomat im erzeugten Code (`takt_llvm::scan`, FB-351) gegen
//! seine Rechnung in Rust (`takt_mir::scan::Scan::first`), die ihrerseits
//! gegen den Interpreter geprueft ist (`takt-interp/tests/scan.rs`).
//!
//! Je Zufallsmuster eine Funktion `such(text) -> i32`, ein C-Treiber laesst
//! jede ueber dieselben Zufallszeilen laufen und schreibt die Fundstellen.
//! Der Test ueberspringt sich ohne clang, wie die uebrigen LLVM-Tests.

use std::fmt::Write as _;

use takt_llvm::emit::Module;
use takt_llvm::toolchain::{Clang, find};
use takt_mir::pattern::{CaptureKind, PatternPiece};
use takt_mir::scan::Scan;

/// xorshift64*, deterministisch.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const LITERALS: [&str; 8] = ["a", "ab", ":", "=", "x", "0", "-", "ä"];
const PARTS: [&str; 16] = [
    "a",
    "b",
    ":",
    "=",
    "x",
    "0",
    "7",
    "-",
    "+",
    "f",
    "ä",
    " ",
    "0x",
    "9223372036854775808",
    "9223372036854775807",
    "8000000000000000",
];

fn pattern(rng: &mut Rng) -> Vec<PatternPiece> {
    let mut pieces = Vec::new();
    let mut placeholder_last = false;
    for _ in 0..1 + rng.below(4) {
        if placeholder_last || rng.below(2) == 0 {
            pieces.push(PatternPiece::Text(rng.pick(&LITERALS).to_string()));
            placeholder_last = false;
            continue;
        }
        pieces.push(match rng.below(6) {
            0 => PatternPiece::Any,
            1 => PatternPiece::Capture { name: "x".into(), kind: CaptureKind::Int },
            2 => PatternPiece::Capture { name: "x".into(), kind: CaptureKind::Hex },
            3 => PatternPiece::Capture { name: "x".into(), kind: CaptureKind::Word },
            _ => PatternPiece::Capture { name: "x".into(), kind: CaptureKind::Str(1 + rng.below(6) as u32) },
        });
        placeholder_last = true;
    }
    pieces
}

/// Eine Zeile, hoechstens `MAX` Bytes.
fn line(rng: &mut Rng) -> Vec<u8> {
    let mut s = Vec::new();
    for _ in 0..rng.below(20) {
        let part = rng.pick(&PARTS).as_bytes();
        if s.len() + part.len() > MAX {
            break;
        }
        s.extend_from_slice(part);
    }
    s
}

const MAX: usize = 96;
const PATTERNS: usize = 120;
const LINES: usize = 60;

#[test]
fn the_generated_automaton_finds_what_the_rust_one_finds() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let mut rng = Rng(0x2026_1001_0351);
    let scans: Vec<(Vec<PatternPiece>, Scan)> = (0..PATTERNS)
        .map(|_| {
            let p = pattern(&mut rng);
            let s = Scan::of(&p, MAX as u32).expect("Automat");
            (p, s)
        })
        .collect();
    let lines: Vec<Vec<u8>> = (0..LINES).map(|_| line(&mut rng)).collect();

    // Der Test uebersetzt *und* laeuft, also das Triple des Wirts.
    let triple = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let mut m = Module::new("scan", triple);
    for (i, (_, scan)) in scans.iter().enumerate() {
        let regs = m.begin(&format!("such{i}"), &takt_llvm::ty::LlvmType::Int(32), &[takt_llvm::ty::LlvmType::Ptr]);
        let start = takt_llvm::scan::first(scan, regs[0], &mut m);
        m.end(Some((&takt_llvm::ty::LlvmType::Int(32), start.to_string())));
    }
    let ir = m.finish();

    let mut c = String::from("#include <stdio.h>\n#include <string.h>\n\n");
    let _ = writeln!(c, "struct text {{ int len; unsigned char bytes[{MAX}]; }};");
    for i in 0..PATTERNS {
        let _ = writeln!(c, "int such{i}(struct text *);");
    }
    let _ = writeln!(c, "\nint main(void) {{\n    struct text t;");
    for l in &lines {
        let bytes: Vec<String> = l.iter().map(|b| format!("0x{b:02x}")).collect();
        let _ = writeln!(
            c,
            "    {{ static const unsigned char z[] = {{ 0{} }};",
            bytes.iter().map(|b| format!(", {b}")).collect::<String>()
        );
        let _ =
            writeln!(c, "      memset(&t, 0, sizeof t); t.len = {}; memcpy(t.bytes, z + 1, {}); }}", l.len(), l.len());
        for i in 0..PATTERNS {
            let _ = writeln!(c, "    printf(\"%d\\n\", such{i}(&t));");
        }
    }
    let _ = writeln!(c, "    return 0;\n}}");

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-llvm-scan");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let (ll, driver) = (dir.join("scan.ll"), dir.join("treiber.c"));
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, &ir).expect("IR");
    std::fs::write(&driver, &c).expect("Treiber");
    let build = std::process::Command::new(path)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&driver)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(build.status.success(), "die IR uebersetzt nicht:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(run.status.success(), "der Lauf scheitert");
    let native: Vec<i64> =
        String::from_utf8_lossy(&run.stdout).lines().map(|l| l.trim().parse().expect("Zahl")).collect();

    let mut want = Vec::new();
    for l in &lines {
        for (_, scan) in &scans {
            want.push(scan.first(l).map_or(-1, |s| s as i64));
        }
    }
    assert_eq!(native.len(), want.len(), "verschieden viele Zeilen");
    let wrong: Vec<String> = native
        .iter()
        .zip(&want)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .take(10)
        .map(|(k, (a, b))| {
            let (l, p) = (k / PATTERNS, k % PATTERNS);
            format!("  {:?} auf {:?}: nativ {a}, Rust {b}", scans[p].0, String::from_utf8_lossy(&lines[l]))
        })
        .collect();
    assert!(wrong.is_empty(), "Abweichungen:\n{}", wrong.join("\n"));
    assert!(want.iter().filter(|w| **w >= 0).count() > want.len() / 10, "zu wenig Treffer, um etwas zu pruefen");
}
