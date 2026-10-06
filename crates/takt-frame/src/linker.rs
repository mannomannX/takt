//! Die Linker-Fragmente der Lieferform (12.3, 12.11; M11 Schritt 11): unter
//! `xip_flash` legen sie den Tick-Pfad eines Programms in den RAM.
//!
//! Ein Wirt bindet sie in den Ausgabeabschnitt seines Instruktions-RAM:
//! `P_ram.x` als Eingabeabschnitte fuer GNU ld und lld (unter `esp-hal`
//! ueber `rwtext_hook.x` in `.rwtext`), `P.lf` als Fragment fuer `ldgen`
//! unter ESP-IDF. Ob das gebundene Abbild haelt, was sie versprechen, prueft
//! `takt check-image`: Jede Funktion, die vom Tick-Pfad aus erreichbar ist,
//! muss im RAM liegen — auch die Treiber des Wirts, die kein Fragment kennt.
//!
//! **Namen statt Archive fuer Rust.** Ein Wirt in Rust baut mit LTO; dort
//! stehen Schleife, Treiberrand, Mathematik, Natives und die Huelle in
//! keinem eigenen Archiv, sondern tragen ihre Crates und Module im Namen
//! ihrer Abschnitte (`.text.<Symbol>`). Ein Bezeichner steht im
//! Symbol mit seiner Laenge davor (`4takt3app`, v0 wie Legacy), und danach
//! folgt eine Ziffer, ein Grossbuchstabe oder `_`. Das Modul der Huelle
//! heisst wie das Praefix, der Kleber zu den Treibern beginnt mit ihm.

use std::fmt::Write as _;

use takt_llvm::symbols::Prefix;

/// Die Crates, die jedes Programm im Tick ruft, mit ihrem Namen im Symbol:
/// Schleife und Profilaufsaetze, Huelle der Einbettung, Treiberrand,
/// Mathematik und Natives.
const SHARED_CRATES: [&str; 8] = [
    "takt_rt_core",
    "takt_rt_baremetal",
    "takt_rt_rtos",
    "takt_embed",
    "takt_hal",
    "libtaktm",
    "takt_native",
    "takt_native_abi",
];

/// Was Mathematik und Natives aus der Kernbibliothek rufen: `core::num`
/// (`isqrt` in `libtaktm`) und `core::str` (UTF-8 in `takt_native::bytes`);
/// der Linker behaelt davon nur, was gerufen wird.
const CORE_MODULES: [&str; 2] = ["4core3num", "4core3str"];

/// Die C-Einstiege der geteilten Bibliotheken: `libtaktm`, die kuratierten
/// Natives und der Randkern.
const SHARED_C: [&str; 3] = ["takt_m_", "takt_native_", "takt_edge_"];

/// Ein Bezeichner, wie er im Namen eines Symbols steht: Laenge und Name,
/// danach der Rest des Pfads.
fn mangled(ident: &str) -> String {
    format!("*{}{ident}[0-9A-Z_]*", ident.len())
}

/// `P_ram.x`: die Eingabeabschnitte des Tick-Pfads fuer GNU ld und lld.
pub fn ld_fragment(x: &Prefix) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Erzeugt von `takt build --emit embed` (12.3, 12.11): der Tick-Pfad des");
    let _ = writeln!(s, " * Programms `{x}` im RAM (`xip_flash`). Ein Wirt bindet diese Zeilen in den");
    let _ = writeln!(s, " * Ausgabeabschnitt seines Instruktions-RAM; `takt check-image` prueft das");
    let _ = writeln!(s, " * gebundene Abbild. Nicht von Hand aendern.");
    let _ = writeln!(s, " */\n");
    let _ = writeln!(s, "/* Die Bibliothek: erzeugter Code und Rahmen mit den Konstanten, die der Tick liest. */");
    let _ = writeln!(s, "*lib{x}.a:(.text .text.* .rodata .rodata.* .srodata .srodata.*)\n");
    let _ = writeln!(s, "/* Im Wirt: der Kleber zu den Treibern (`{x}_*`) und die Huelle (Modul `{x}`). */");
    owned(&mut s, x.as_str());
    shared(&mut s);
    s
}

/// `takt_bench_ram.x`: das Messprogramm von `takt bench` im RAM (13.8). Ein
/// Kern, der aus dem Flash liefe, maesse den Cache mit; gemessen wird, wie
/// ein Programm unter `xip_flash` laeuft.
pub fn bench_ld_fragment() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "/* Erzeugt von `takt bench --emit embed` (12.3, 13.8): das Messprogramm im RAM");
    let _ = writeln!(s, " * (`xip_flash`), wie die Programme, deren Kosten es misst. Ein Wirt bindet");
    let _ = writeln!(s, " * diese Zeilen in den Ausgabeabschnitt seines Instruktions-RAM. Nicht von Hand");
    let _ = writeln!(s, " * aendern.");
    let _ = writeln!(s, " */\n");
    let _ = writeln!(s, "/* Die Bibliothek: Kerne, Rahmen, C-Referenzen und der Laeufer. */");
    let _ = writeln!(s, "*libtakt_bench.a:(.text .text.* .rodata .rodata.* .srodata .srodata.*)\n");
    let _ = writeln!(s, "/* Im Wirt: die Haken (`takt_bench_*`) und das Modul `takt_bench`. */");
    owned(&mut s, "takt_bench");
    shared(&mut s);
    s
}

/// Was dem Wirt gehoert und den Namen `x` traegt: Symbole `x_*` und das
/// Modul `x`.
fn owned(s: &mut String, x: &str) {
    let _ = writeln!(s, "*(.text.{x}_* .rodata.{x}_* .srodata.{x}_*)");
    let hull = mangled(x);
    let _ = writeln!(s, "*(.text.{hull} .rodata.{hull} .srodata.{hull})\n");
}

/// Was alle Programme teilen.
fn shared(s: &mut String) {
    let _ = writeln!(s, "/* Was alle Programme teilen: Schleife, Huelle, Treiberrand, Mathematik, Natives. */");
    let c: Vec<String> = SHARED_C.iter().map(|p| format!(".text.{p}* .rodata.{p}* .srodata.{p}*")).collect();
    let _ = writeln!(s, "*({})", c.join(" "));
    for krate in SHARED_CRATES {
        let m = mangled(krate);
        let _ = writeln!(s, "*(.text.{m} .rodata.{m} .srodata.{m})");
    }
    for path in CORE_MODULES {
        let m = format!("*{path}[0-9A-Z_]*");
        let _ = writeln!(s, "*(.text.{m} .rodata.{m} .srodata.{m})");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "/* Namenlose Konstanten (`.Lanon`), die ein Aufruf per Referenz uebergibt. */");
    let _ = writeln!(s, "*(.rodata..Lanon.* .srodata..Lanon.*)\n");
    let _ = writeln!(s, "/* Die Grundrechenarten, `fma` und `sqrt`, die der erzeugte Code ruft (FB-301). */");
    let _ = writeln!(s, "*libcompiler_builtins-*.rlib:*(.text .text.* .rodata .rodata.* .srodata .srodata.*)");
}

/// `takt_bench.lf`: das Messprogramm im RAM fuer einen Wirt unter ESP-IDF.
pub fn bench_ldgen_fragment() -> String {
    "# Erzeugt von `takt bench --emit embed` (12.3, 13.8): das Messprogramm im RAM\n\
     # (`xip_flash`) fuer einen Wirt unter ESP-IDF.\n\
     [mapping:takt_bench]\n\
     archive: libtakt_bench.a\n\
     entries:\n    \
     * (noflash)\n"
        .to_string()
}

/// `P.lf`: das Fragment fuer `ldgen` unter ESP-IDF. `noflash` legt Code
/// und Konstanten der Bibliothek in den RAM.
pub fn ldgen_fragment(x: &Prefix) -> String {
    format!(
        "# Erzeugt von `takt build --emit embed` (12.3, 12.11): der Tick-Pfad des\n\
         # Programms `{x}` im RAM (`xip_flash`) fuer einen Wirt unter ESP-IDF.\n\
         # `takt check-image` prueft das gebundene Abbild.\n\
         [mapping:takt_{x}]\n\
         archive: lib{x}.a\n\
         entries:\n    \
         * (noflash)\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fragment_names_the_library_the_glue_and_the_hull() {
        let x = Prefix::new("valve").expect("Praefix");
        let s = ld_fragment(&x);
        assert!(s.contains("*libvalve.a:(.text .text.*"), "{s}");
        assert!(s.contains("*(.text.valve_* .rodata.valve_*"), "{s}");
        assert!(s.contains(".text.*5valve[0-9A-Z_]*"), "{s}");
        assert!(s.contains(".text.*12takt_rt_core[0-9A-Z_]*"), "{s}");
        assert!(s.contains(".text.*4core3num[0-9A-Z_]*"), "die Mathematik ruft `isqrt`: {s}");
        assert!(s.contains(".text.*4core3str[0-9A-Z_]*"), "die Natives pruefen UTF-8: {s}");
        assert!(!s.contains("app"), "kein fremdes Praefix: {s}");
    }

    /// Das Messprogramm liegt mit Haken, Modul und allem Geteilten im RAM.
    #[test]
    fn the_bench_fragment_names_the_library_and_the_hooks() {
        let s = bench_ld_fragment();
        assert!(s.contains("*libtakt_bench.a:(.text .text.*"), "{s}");
        assert!(s.contains("*(.text.takt_bench_* "), "{s}");
        assert!(s.contains(".text.*10takt_bench[0-9A-Z_]*"), "{s}");
        assert!(s.contains(".text.*8libtaktm[0-9A-Z_]*") && s.contains("libcompiler_builtins"), "{s}");
        assert!(bench_ldgen_fragment().contains("archive: libtakt_bench.a\nentries:\n    * (noflash)\n"));
    }

    #[test]
    fn the_ldgen_fragment_maps_the_library_to_ram() {
        let x = Prefix::new("valve").expect("Praefix");
        let s = ldgen_fragment(&x);
        assert!(s.contains("[mapping:takt_valve]\narchive: libvalve.a\nentries:\n    * (noflash)\n"), "{s}");
    }
}
