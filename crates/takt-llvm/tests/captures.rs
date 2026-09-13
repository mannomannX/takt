//! Der erzeugte Mustervergleich gegen `takt-match` (8.7, Satz 9.4.4).
//!
//! **Warum zwei Implementierungen und nicht eine geteilte.** Die Regeln
//! stehen in `takt-match`; sie aus dem erzeugten Code zu *rufen*
//! braeuchte Rohzeiger ueber die C-ABI, und `unsafe_code = "forbid"`
//! gilt workspace-weit (13.4). Der Codegen baut sie darum als IR — und
//! dieser Test macht daraus einen Gewinn statt eines Risikos: Zwei
//! Implementierungen, die uebereinstimmen muessen, decken auf, was eine
//! gemeinsame stillschweigend teilt. FB-114 wurde so gefunden.
//!
//! **Was verglichen wird.** Nicht nur das Urteil, sondern die *Werte*:
//! Ein Vergleich der Ja/Nein-Antwort uebersaehe eine Spanne, die um ein
//! Zeichen danebenliegt. Der Test uebersetzt je Muster eine kleine
//! Funktion, laesst sie ueber viele Texte laufen und misst beides.
//!
//! Er ueberspringt sich ohne clang, wie die uebrigen LLVM-Tests.

use takt_llvm::emit::Module;
use takt_llvm::toolchain::{Clang, find};
use takt_match::{Kind, Piece};
use takt_mir::pattern::{CaptureKind, PatternPiece};

/// Ein Muster in beiden Darstellungen: fuer den Codegen und fuer das
/// Orakel.
struct Muster {
    /// Wie der Mensch es schreibt — nur fuer die Fehlermeldung.
    text: &'static str,
    /// Fuer den Codegen.
    pieces: Vec<PatternPiece>,
    /// Fuer `takt-match`.
    stuecke: Vec<Piece<'static>>,
}

fn lit(s: &'static str) -> (PatternPiece, Piece<'static>) {
    (PatternPiece::Text(s.to_string()), Piece::Text(s.as_bytes()))
}

fn cap(kind: CaptureKind, k: Kind) -> (PatternPiece, Piece<'static>) {
    (PatternPiece::Capture { name: "x".to_string(), kind }, Piece::Capture(k))
}

fn muster(text: &'static str, teile: Vec<(PatternPiece, Piece<'static>)>) -> Muster {
    let (pieces, stuecke) = teile.into_iter().unzip();
    Muster { text, pieces, stuecke }
}

/// Die Muster, die beide Seiten sehen.
fn alle() -> Vec<Muster> {
    vec![
        muster("READY", vec![lit("READY")]),
        muster("Boot v{n:int}", vec![lit("Boot v"), cap(CaptureKind::Int, Kind::Int)]),
        muster("Erasing sector {n:int}", vec![lit("Erasing sector "), cap(CaptureKind::Int, Kind::Int)]),
        muster("{a:int}-{b:int}", vec![cap(CaptureKind::Int, Kind::Int), lit("-"), cap(CaptureKind::Int, Kind::Int)]),
        muster("addr {x:hex}", vec![lit("addr "), cap(CaptureKind::Hex, Kind::Hex)]),
        muster("user {w:word}", vec![lit("user "), cap(CaptureKind::Word, Kind::Word)]),
        muster(
            "{w:word}={n:int}",
            vec![cap(CaptureKind::Word, Kind::Word), lit("="), cap(CaptureKind::Int, Kind::Int)],
        ),
        muster("[{s:str<8>}]", vec![lit("["), cap(CaptureKind::Str(8), Kind::Str(8)), lit("]")]),
        muster("T{n:int}C", vec![lit("T"), cap(CaptureKind::Int, Kind::Int), lit("C")]),
    ]
}

/// Die Texte, gegen die jedes Muster laeuft.
///
/// Sie decken die Raender ab: leer, knapp daneben, der kleinste `int`
/// (FB-114/FB-95), fuehrende Nullen, Hex in beiden Schreibweisen, und
/// Text, der nur teilweise passt.
const TEXTE: [&str; 28] = [
    "",
    "READY",
    "READ",
    "READYX",
    "XREADY",
    "Boot v1",
    "Boot v0",
    "Boot v-1",
    "Boot v-9223372036854775808",
    "Boot v9223372036854775807",
    "Boot v007",
    "Boot v",
    "Boot vx",
    "Erasing sector 42",
    "Erasing sector -7",
    "1-2",
    "-1--2",
    "12-34",
    "addr 0x7E8",
    "addr 0X7e8",
    "addr 7E8",
    "addr ",
    "user root",
    "user r_1",
    "user ",
    "a=1",
    "[abc]",
    "[abcdefghij]",
];

/// Baut ein Modul mit je einer Funktion `pruef<i>` pro Muster.
///
/// Die Funktion nimmt einen Zeiger auf `{ i32 len, [64 x i8] }` und
/// schreibt das Ergebnis: das Urteil und die Werte der Captures. Mehr
/// braucht der Vergleich nicht, und weniger waere zu wenig.
fn modul(muster: &[Muster]) -> String {
    // Der Test uebersetzt *und* laeuft, also muss das Triple das des
    // Wirts sein.
    let triple = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let mut m = Module::new("captures", triple);
    for (i, mu) in muster.iter().enumerate() {
        // `pruef(text, out) -> i1`; `out` nimmt den Aufnahmerecord.
        let regs = m.begin(
            &format!("pruef{i}"),
            &takt_llvm::ty::LlvmType::Int(1),
            &[takt_llvm::ty::LlvmType::Ptr, takt_llvm::ty::LlvmType::Ptr],
        );
        let (text, out) = (regs[0], regs[1]);
        let anzahl = mu.pieces.iter().filter(|p| matches!(p, PatternPiece::Capture { .. })).count();
        // Der Aufnahmerecord: je Capture ein Feld. `int` und `hex` sind
        // i64, `word` und `str` ein Textpuffer wie `str<N>` (3.9).
        let felder: Vec<takt_llvm::ty::LlvmType> = mu
            .pieces
            .iter()
            .filter_map(|p| match p {
                PatternPiece::Capture { kind: CaptureKind::Int | CaptureKind::Hex, .. } => {
                    Some(takt_llvm::ty::LlvmType::Int(64))
                }
                PatternPiece::Capture { kind: CaptureKind::Word | CaptureKind::Str(_), .. } => {
                    Some(takt_llvm::ty::LlvmType::Struct(vec![
                        takt_llvm::ty::LlvmType::Int(32),
                        takt_llvm::ty::LlvmType::Array(Box::new(takt_llvm::ty::LlvmType::Int(8)), 64),
                    ]))
                }
                _ => None,
            })
            .collect();
        let rec = takt_llvm::ty::LlvmType::Struct(felder.clone());
        let liste: Vec<(u32, takt_llvm::ty::LlvmType)> =
            felder.iter().enumerate().map(|(j, f)| (j as u32, f.clone())).collect();
        let wohin = (anzahl > 0).then(|| takt_llvm::captures::Ziel { slot: out, record: &rec, felder: &liste });
        let null = m.inst("add i32 0, 0");
        let (ok, at) = takt_llvm::captures::walk(&mu.pieces, text, wohin.as_ref(), null, &mut m).expect("walk");
        // `matches`: der ganze Text muss verbraucht sein (8.7).
        let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
        let len = m.inst(&format!("load i32, ptr {len_ptr}"));
        let ganz = m.inst(&format!("icmp eq i32 {at}, {len}"));
        let hit = m.inst(&format!("and i1 {ok}, {ganz}"));
        m.end(Some((&takt_llvm::ty::LlvmType::Int(1), hit.to_string())));
    }
    m.finish()
}

#[test]
fn the_generated_matcher_agrees_with_takt_match() {
    let Clang::At(path) = find() else {
        eprintln!("uebersprungen: clang nicht gefunden");
        return;
    };
    let clang = Clang::At(path.clone());
    let _ = &clang;
    let muster = alle();
    let ir = modul(&muster);

    let dir = std::env::temp_dir().join("takt-llvm-captures");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let ll = dir.join("captures.ll");
    let c = dir.join("treiber.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, &ir).expect("IR");
    std::fs::write(&c, treiber(&muster)).expect("Treiber");

    let build = std::process::Command::new(path)
        .args(["-Wno-override-module", "-O1"])
        .arg(&ll)
        .arg(&c)
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("clang");
    assert!(
        build.status.success(),
        "die IR uebersetzt nicht:\n{}\n--- IR ---\n{ir}",
        String::from_utf8_lossy(&build.stderr)
    );
    let lauf = std::process::Command::new(&exe).output().expect("Lauf");
    assert!(lauf.status.success(), "der Lauf scheitert: {}", String::from_utf8_lossy(&lauf.stderr));
    let nativ = String::from_utf8_lossy(&lauf.stdout);

    // Dasselbe durch `takt-match`, das Orakel.
    let erwartet = orakel(&muster);
    let ist: Vec<&str> = nativ.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(ist.len(), erwartet.len(), "verschieden viele Zeilen");
    let mut abweichungen = Vec::new();
    for (i, (a, b)) in ist.iter().zip(erwartet.iter()).enumerate() {
        if a != b {
            let m = &muster[i / TEXTE.len()];
            let t = TEXTE[i % TEXTE.len()];
            abweichungen.push(format!("  `{}` gegen {t:?}: nativ `{a}`, `takt-match` `{b}`", m.text));
        }
    }
    assert!(
        abweichungen.is_empty(),
        "{} Abweichungen zwischen erzeugtem Code und `takt-match`:\n{}",
        abweichungen.len(),
        abweichungen.join("\n")
    );
}

/// Was `takt-match` zu jedem Paar sagt, in derselben Schreibweise wie
/// der Treiber.
fn orakel(muster: &[Muster]) -> Vec<String> {
    let mut out = Vec::new();
    for mu in muster {
        for t in TEXTE {
            let Some(treffer) = takt_match::matches(&mu.stuecke, t.as_bytes()) else {
                out.push("0".to_string());
                continue;
            };
            let mut zeile = String::from("1");
            let mut kinds = mu.pieces.iter().filter_map(|p| match p {
                PatternPiece::Capture { kind, .. } => Some(kind),
                _ => None,
            });
            for s in &treffer.spans[..treffer.count as usize] {
                let roh = s.of(t.as_bytes());
                let wert = match kinds.next() {
                    Some(CaptureKind::Int) => takt_match::parse_int(roh).map_or("?".into(), |v| v.to_string()),
                    Some(CaptureKind::Hex) => takt_match::parse_hex(roh).map_or("?".into(), |v| v.to_string()),
                    _ => String::from_utf8_lossy(roh).to_string(),
                };
                zeile.push(' ');
                zeile.push_str(&wert);
            }
            out.push(zeile);
        }
    }
    out
}

/// Der C-Treiber: Er ruft je Muster und Text die erzeugte Funktion und
/// schreibt Urteil und Werte.
fn treiber(muster: &[Muster]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "#include <stdio.h>");
    let _ = writeln!(s, "#include <string.h>\n");
    let _ = writeln!(s, "struct text {{ int len; char bytes[64]; }};");
    let _ = writeln!(s, "struct wort {{ int len; char bytes[64]; }};");
    // Je Muster ein eigener Record: Die Ausrichtung rechnet der
    // C-Compiler, wie LLVM sie auf der anderen Seite rechnet. Versaetze
    // von Hand waeren eine dritte Meinung dazu.
    for (i, mu) in muster.iter().enumerate() {
        let felder: Vec<String> = mu
            .pieces
            .iter()
            .filter_map(|p| match p {
                PatternPiece::Capture { kind: CaptureKind::Int | CaptureKind::Hex, .. } => Some("long long"),
                PatternPiece::Capture { kind: CaptureKind::Word | CaptureKind::Str(_), .. } => Some("struct wort"),
                _ => None,
            })
            .enumerate()
            .map(|(j, ct)| format!("{ct} f{j};"))
            .collect();
        if felder.is_empty() {
            let _ = writeln!(s, "struct rec{i} {{ char leer; }};");
        } else {
            let _ = writeln!(s, "struct rec{i} {{ {} }};", felder.join(" "));
        }
    }
    let _ = writeln!(s);
    for (i, _) in muster.iter().enumerate() {
        let _ = writeln!(s, "_Bool pruef{i}(struct text *, void *);");
    }
    let _ = writeln!(s, "\nint main(void) {{");
    let _ = writeln!(s, "    struct text t;");
    for (i, _) in muster.iter().enumerate() {
        let _ = writeln!(s, "    struct rec{i} r{i};");
    }
    for (i, mu) in muster.iter().enumerate() {
        for text in TEXTE {
            let bytes: String = text.as_bytes().iter().map(|b| format!("\\x{b:02x}")).collect();
            let _ = writeln!(s, "    memset(&t, 0, sizeof t);");
            let _ = writeln!(s, "    t.len = {};", text.len());
            if !text.is_empty() {
                let _ = writeln!(s, "    memcpy(t.bytes, \"{bytes}\", {});", text.len());
            }
            let _ = writeln!(s, "    memset(&r{i}, 0, sizeof r{i});");
            let _ = writeln!(s, "    if (!pruef{i}(&t, &r{i})) {{ printf(\"0\\n\"); }} else {{");
            let _ = writeln!(s, "        printf(\"1\");");
            // Die Felder in der Reihenfolge des Musters auslesen.
            let mut j = 0usize;
            for p in &mu.pieces {
                let PatternPiece::Capture { kind, .. } = p else { continue };
                match kind {
                    CaptureKind::Int | CaptureKind::Hex => {
                        let _ = writeln!(s, "        printf(\" %lld\", r{i}.f{j});");
                    }
                    CaptureKind::Word | CaptureKind::Str(_) => {
                        let _ = writeln!(s, "        printf(\" %.*s\", r{i}.f{j}.len, r{i}.f{j}.bytes);");
                    }
                    CaptureKind::Float => {}
                }
                j += 1;
            }
            let _ = writeln!(s, "        printf(\"\\n\"); }}");
        }
    }
    let _ = writeln!(s, "    return 0;");
    let _ = writeln!(s, "}}");
    s
}
