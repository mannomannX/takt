//! Misst, wie weit der Codegen ueber dem Korpus reicht.
//!
//! `cargo run -p takt-llvm --example coverage`
//!
//! Zwei Zahlen, die verschiedene Fragen beantworten:
//!
//! - **Knoten**: Wie viel von dem, was in echten Programmen steht, senkt
//!   der Codegen? Das ist das Mass fuer den Weg, der noch fehlt.
//! - **Maschinen**: Wie viele Maschinen gehen *vollstaendig* durch? Das
//!   ist das Mass fuer den Nutzen — eine Maschine, der ein Knoten fehlt,
//!   laeuft gar nicht.
//!
//! Die zweite Zahl ist immer die kleinere, und das ist keine
//! Ungenauigkeit: Ein Programm ist erst uebersetzbar, wenn *alles* darin
//! uebersetzbar ist.

use takt_llvm::emit::Module;
use takt_llvm::machine::state_struct;
use takt_llvm::scope::{Coverage, machine as measure};
use takt_llvm::step::step_function;

fn main() {
    let mut files: Vec<_> = std::fs::read_dir("corpus-try")
        .expect("corpus-try lesbar")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "takt"))
        .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("n0")))
        .collect();
    files.sort();

    let mut cov = Coverage::default();
    let (mut ganze_dateien, mut dateien, mut maschinen, mut fertig) = (0, 0, 0, 0);

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else { continue };
        let options =
            takt_sema::Options { policy: takt_diag::Policy::default(), build: takt_sema::Build::Sim, profile: None };
        let out = takt_sema::compile(&src, &options);
        if out.diagnostics.iter().any(|d| d.is_error()) {
            continue;
        }
        let Some(p) = out.program else { continue };
        dateien += 1;
        let mut m = Module::new("x", "x86_64-pc-windows-msvc");
        takt_llvm::abi::Abi::declare(&mut m);
        let mut alle = true;
        for mm in &p.machines {
            maschinen += 1;
            measure(mm, &mut cov);
            let ok = state_struct(mm, &p).is_some_and(|st| step_function(mm, &st, &p, &mut m).is_ok());
            if ok {
                fertig += 1;
            } else {
                alle = false;
            }
        }
        if alle {
            ganze_dateien += 1;
        }
    }

    println!("Korpus: {dateien} Dateien uebersetzen fehlerfrei zu MIR\n");
    println!(
        "MIR-Knoten:  {} von {} gesenkt  ({:.0} %)",
        cov.total() - cov.open.values().sum::<usize>(),
        cov.total(),
        cov.percent()
    );
    println!("Maschinen:   {fertig} von {maschinen} vollstaendig");
    println!("Dateien:     {ganze_dateien} von {dateien} vollstaendig\n");

    let mut offen: Vec<_> = cov.open.iter().collect();
    offen.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!("Was noch fehlt, nach Haeufigkeit im Korpus:");
    for (was, n) in &offen {
        println!("  {n:4}  {was}");
    }

    let mut gedeckt: Vec<_> = cov.covered.iter().collect();
    gedeckt.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    println!("\nWas der Codegen senkt:");
    for (was, n) in &gedeckt {
        println!("  {n:4}  {was}");
    }
}
