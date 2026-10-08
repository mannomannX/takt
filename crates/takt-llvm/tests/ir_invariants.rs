//! Regeln, die jede erzeugte IR einhaelt, ueber den ganzen Korpus.
//!
//! **Kein grosses Aggregat als Wert** (11.2, Grundsatz 22, FB-455). Ein
//! Aggregat als SSA-Wert legalisiert LLVM in Einzelwerte; ueber einer
//! Cache-Zeile wird daraus eine Kopie ueber den Stack, oft mehrere: Der
//! Schritt von `long_job` legte fuer einen Puffer von 4 KiB 23 KiB an. Der
//! Codegen schreibt einen grossen Wert darum an seine Stelle und kopiert ihn
//! von Stelle zu Stelle: kein `load` und kein `store` eines Aggregats ueber
//! [`VALUE_MAX`].
//!
//! **Kein Platz endet, der nicht begann** (11.2, FB-462). Ein Platz ohne
//! `lifetime.start` lebt in der ganzen Funktion — der Platz fuer die Art
//! eines Faults, den die Fault-Pfade jeder Anweisung schreiben. Beendete
//! ihn das Ende der Anweisung, in der er entstand, schrieben spaetere in
//! einen toten Platz, und LLVM duerfte ihn mit anderen uebereinanderlegen.

use std::path::{Path, PathBuf};

use takt_llvm::ty::VALUE_MAX;

/// Die Programme: der Differentialkorpus und die Programme der Boards.
fn programs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    for dir in [root.join("corpus-try"), root.join("crates/takt-conformance/tests/programs")] {
        let entries = std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        out.extend(entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "takt")));
    }
    out.sort();
    out
}

/// Die Groesse eines LLVM-Typs am Anfang von `t` in Byte, ohne Ausrichtung,
/// und was dahinter steht.
fn size(t: &str) -> Option<(u64, &str)> {
    let t = t.trim_start();
    if let Some(mut rest) = t.strip_prefix('{') {
        let mut total = 0;
        loop {
            rest = rest.trim_start().trim_start_matches(',').trim_start();
            if let Some(after) = rest.strip_prefix('}') {
                return Some((total, after));
            }
            let (s, after) = size(rest)?;
            total += s;
            rest = after;
        }
    }
    if let Some(rest) = t.strip_prefix('[') {
        let (n, rest) = rest.split_once(" x ")?;
        let (s, rest) = size(rest)?;
        return Some((n.trim().parse::<u64>().ok()? * s, rest.trim_start().strip_prefix(']')?));
    }
    // Die laengeren Namen zuerst: `i1` ist ein Praefix von `i16`.
    [("i16", 2), ("i32", 4), ("i64", 8), ("i8", 1), ("i1", 1), ("float", 4), ("double", 8), ("ptr", 8)]
        .into_iter()
        .find_map(|(name, bytes)| t.strip_prefix(name).map(|rest| (bytes, rest)))
}

/// Die Stellen einer IR, an denen ein Aggregat ueber [`VALUE_MAX`] als Wert
/// geladen oder gespeichert wird: Funktion, Befehl, Typ und Groesse.
fn large_values(ir: &str) -> Vec<String> {
    let mut function = "";
    let mut out = Vec::new();
    for line in ir.lines() {
        if let Some(head) = line.strip_prefix("define ") {
            function = head.split_once('@').and_then(|(_, r)| r.split_once('(')).map_or("", |(n, _)| n);
            continue;
        }
        let code = line.split_once(" = ").map_or(line, |(_, r)| r).trim_start();
        for op in ["load ", "store "] {
            let Some(ty) = code.strip_prefix(op) else { continue };
            if !ty.starts_with(['[', '{']) {
                continue;
            }
            // Eine Konstante ist kein Wert im Register; LLVM legt sie ab.
            let value = |rest: &str| op == "load " || rest.trim_start().starts_with('%');
            if let Some((bytes, rest)) = size(ty).filter(|(b, rest)| *b > VALUE_MAX && value(rest)) {
                let shown = &ty[..ty.len() - rest.len()];
                out.push(format!("{function}: {} {shown} ({bytes} Byte)", op.trim_end()));
            }
        }
    }
    out
}

/// Die Plaetze einer IR, deren Lebensdauer endet, ohne in ihrer Funktion
/// begonnen zu haben: Funktion und Platz.
fn ended_without_start(ir: &str) -> Vec<String> {
    let (mut function, mut started, mut ended) = ("", Vec::new(), Vec::new());
    let mut out = Vec::new();
    let mut close = |function: &str, started: &mut Vec<&str>, ended: &mut Vec<&str>| {
        out.extend(ended.drain(..).filter(|e| !started.contains(e)).map(|e| format!("{function}: {e}")));
        started.clear();
    };
    for line in ir.lines() {
        if let Some(head) = line.strip_prefix("define ") {
            close(function, &mut started, &mut ended);
            function = head.split_once('@').and_then(|(_, r)| r.split_once('(')).map_or("", |(n, _)| n);
            continue;
        }
        let slot = |intrinsic: &str| line.trim().strip_prefix(intrinsic)?.strip_suffix(')');
        if let Some(s) = slot("call void @llvm.lifetime.start.p0(ptr ") {
            started.push(s);
        } else if let Some(s) = slot("call void @llvm.lifetime.end.p0(ptr ") {
            ended.push(s);
        }
    }
    close(function, &mut started, &mut ended);
    out
}

/// IR eines Korpusprogramms; `None`, wenn die Sema es nicht annimmt.
fn ir_of(path: &Path) -> Option<String> {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let options = takt_sema::Options { build: takt_sema::Build::Sim, ..Default::default() };
    let p = takt_sema::compile(&src, &options).program?;
    Some(takt_llvm::lower::program(&p, "riscv32-unknown-none-elf", &takt_llvm::symbols::Prefix::default()).ir)
}

/// Prueft `rule` ueber jede IR des Korpus.
fn over_corpus(what: &str, rule: fn(&str) -> Vec<String>) {
    let mut found = Vec::new();
    for path in programs() {
        let Some(ir) = ir_of(&path) else { continue };
        let name = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        found.extend(rule(&ir).into_iter().map(|at| format!("{name}: {at}")));
    }
    let shown: Vec<&String> = found.iter().take(30).collect();
    assert!(found.is_empty(), "{} Stellen {what}:\n{shown:#?}", found.len());
}

#[test]
fn the_size_reader_knows_nested_types() {
    assert_eq!(size("{ i32, [256 x i8] }, ptr %x").map(|(b, _)| b), Some(260));
    assert_eq!(size("[4 x [4 x double]] %v").map(|(b, _)| b), Some(128));
    assert_eq!(size("i16 %v").map(|(b, _)| b), Some(2));
    let ir = "define void @f(ptr %0) {\n  %1 = load { i32, [256 x i8] }, ptr %0\n  store [2 x i64] %2, ptr %0\n  \
              store [12 x double] [double 1.0, double 2.0], ptr %0\n  store [12 x double] %3, ptr %0\n}\n";
    assert_eq!(large_values(ir), ["f: load { i32, [256 x i8] } (260 Byte)", "f: store [12 x double] (96 Byte)"]);
}

#[test]
fn no_large_aggregate_is_an_ssa_value() {
    over_corpus("mit grossem Aggregat als Wert", large_values);
}

#[test]
fn the_end_finder_needs_a_start_in_the_same_function() {
    let ir = "define void @f() {\n  call void @llvm.lifetime.start.p0(ptr %slot0)\n  \
              call void @llvm.lifetime.end.p0(ptr %slot0)\n  call void @llvm.lifetime.end.p0(ptr %slot1)\n}\n\
              define void @g() {\n  call void @llvm.lifetime.end.p0(ptr %slot0)\n}\n";
    assert_eq!(ended_without_start(ir), ["f: %slot1", "g: %slot0"]);
}

#[test]
fn no_place_ends_without_having_started() {
    over_corpus("mit einem Ende ohne Anfang", ended_without_start);
}
