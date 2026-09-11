//! Zeigt die erzeugte IR fuer einen kleinen Ausdruck.
//!
//! `cargo run -p takt-llvm --example dump` — und wo LLVM steht, prueft
//! `llvm-as` die Datei.

use takt_llvm::emit::Module;
use takt_llvm::ty::LlvmType;

fn main() {
    let mut m = Module::new("beispiel", "x86_64-unknown-linux-gnu");
    let d = LlvmType::F64;
    let a = m.begin("mul_add_one", &d, &[d.clone(), d.clone()]);
    // Bewusst getrennt: 4.2 verbietet die Kontraktion zu einem `fma`.
    let p = m.inst(&format!("fmul double {}, {}", a[0], a[1]));
    let s = m.inst(&format!("fadd double {p}, 0x3FF0000000000000"));
    m.end(Some((&d, s.to_string())));
    print!("{}", m.finish());
}
