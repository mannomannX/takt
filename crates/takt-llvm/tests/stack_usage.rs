//! Die Stackrahmen aus LLVM (12.3): `-fstack-usage` neben dem Objekt.
//!
//! Ohne clang scheitert der Test wie die LLVM-Tests (`takt_testkit::require`).

use std::process::Command;

use takt_llvm::Target;
use takt_llvm::toolchain::{find, object_flags};

/// **Ein grosser Rahmen zaehlt ganz** (FB-454): RISC-V legt ihn in zwei
/// Schritten an, Thumb mit `push` und `sub sp`; die Disassemblierung sah von
/// einem Schritt mit 23 200 Byte nur 720.
#[test]
fn a_large_frame_counts_whole() {
    let Some(clang) = takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen") else { return };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-stack-usage");
    std::fs::create_dir_all(&dir).expect("Verzeichnis anlegbar");
    let ir = "declare void @sink(ptr)\n\ndefine void @big() {\n  %b = alloca [23200 x i8]\n  call void @sink(ptr %b)\n  \
              ret void\n}\n";
    for target in [Target::RISCV32IMAC, Target::THUMBV7EM] {
        let (ll, obj) = (dir.join(format!("big_{}.ll", target.name)), dir.join(format!("big_{}.o", target.name)));
        std::fs::write(&ll, ir).expect("IR schreibbar");
        let mut cmd = Command::new(&clang);
        cmd.args(["-c", "-Wno-override-module", "-ffreestanding", "-nostdlib"])
            .arg(format!("--target={}", target.triple));
        if !target.march.is_empty() {
            cmd.arg(format!("-march={}", target.march));
        }
        let ok = cmd.args(object_flags(target.triple)).arg(&ll).arg("-o").arg(&obj).status().is_ok_and(|s| s.success());
        assert!(ok, "clang schlug fehl fuer {}", target.name);
        let usage = takt_llvm::inspect::stack_usage_of(&obj).unwrap_or_else(|e| panic!("{e}"));
        let big = usage.get("big").copied().unwrap_or(0);
        assert!(big >= 23_200, "{}: {big} Byte", target.name);
    }
}
