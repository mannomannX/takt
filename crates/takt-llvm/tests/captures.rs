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
//! Zeichen danebenliegt. Der Test uebersetzt je Muster zwei kleine
//! Funktionen, `matches` und `has`, laesst sie ueber viele Texte laufen
//! und misst beides. Erst `has` zeigt, wo eine Klasse am Musterende endet:
//! Unter `matches` muesste ohnehin der ganze Text verbraucht sein.
//!
//! Er ueberspringt sich ohne clang, wie die uebrigen LLVM-Tests.

use takt_llvm::emit::Module;
use takt_llvm::toolchain::{Clang, find};
use takt_llvm::ty::LlvmType;
use takt_match::{Kind, Piece};
use takt_mir::pattern::{CaptureKind, PatternPiece};

/// Ein Muster in beiden Darstellungen: fuer den Codegen und fuer das
/// Orakel.
struct Pattern {
    /// Wie der Mensch es schreibt — nur fuer die Fehlermeldung.
    text: &'static str,
    /// Fuer den Codegen.
    pieces: Vec<PatternPiece>,
    /// Fuer `takt-match`.
    pieces_of: Vec<Piece<'static>>,
}

fn lit(s: &'static str) -> (PatternPiece, Piece<'static>) {
    (PatternPiece::Text(s.to_string()), Piece::Text(s.as_bytes()))
}

fn cap(kind: CaptureKind, k: Kind) -> (PatternPiece, Piece<'static>) {
    (PatternPiece::Capture { name: "x".to_string(), kind }, Piece::Capture(k))
}

fn pattern_of(text: &'static str, parts: Vec<(PatternPiece, Piece<'static>)>) -> Pattern {
    let (pieces, pieces_of) = parts.into_iter().unzip();
    Pattern { text, pieces, pieces_of }
}

/// Die Muster, die beide Seiten sehen.
fn all_of() -> Vec<Pattern> {
    vec![
        pattern_of("READY", vec![lit("READY")]),
        pattern_of("Boot v{n:int}", vec![lit("Boot v"), cap(CaptureKind::Int, Kind::Int)]),
        pattern_of("Erasing sector {n:int}", vec![lit("Erasing sector "), cap(CaptureKind::Int, Kind::Int)]),
        pattern_of(
            "{a:int}-{b:int}",
            vec![cap(CaptureKind::Int, Kind::Int), lit("-"), cap(CaptureKind::Int, Kind::Int)],
        ),
        pattern_of("addr {x:hex}", vec![lit("addr "), cap(CaptureKind::Hex, Kind::Hex)]),
        pattern_of("user {w:word}", vec![lit("user "), cap(CaptureKind::Word, Kind::Word)]),
        pattern_of(
            "{w:word}={n:int}",
            vec![cap(CaptureKind::Word, Kind::Word), lit("="), cap(CaptureKind::Int, Kind::Int)],
        ),
        pattern_of("[{s:str<8>}]", vec![lit("["), cap(CaptureKind::Str(8), Kind::Str(8)), lit("]")]),
        pattern_of("T{n:int}C", vec![lit("T"), cap(CaptureKind::Int, Kind::Int), lit("C")]),
        // Ein offenes Ende vorn: `has` laeuft ueber den Durchlaufautomaten
        // (`takt_mir::scan`), nicht ueber den Ansatz an jeder Stelle.
        pattern_of(
            "{_}={n:int}",
            vec![(PatternPiece::Any, Piece::Capture(Kind::Any)), lit("="), cap(CaptureKind::Int, Kind::Int)],
        ),
        pattern_of("{s:str<8>}", vec![cap(CaptureKind::Str(8), Kind::Str(8))]),
    ]
}

/// Die Kapazitaet der Zeile, die `has` sieht; der Treiber haelt so viele
/// Bytes.
const LINE_CAP: u32 = 128;

/// Die Texte, gegen die jedes Muster laeuft.
///
/// Sie decken die Raender ab: leer, knapp daneben, der kleinste `int`
/// (FB-114/FB-95), das Vorzeichen `+` und der Ueberlauf (FB-349),
/// fuehrende Nullen, Hex in beiden Schreibweisen, und Text, der nur
/// teilweise passt. Dazu die Klassengrenzen aus 8.7 (GEN-014, SYN-039):
/// `int` mit 19 und 20 Ziffern, `hex` mit 16 und 17 und blossem `0x`,
/// `word` mit 64 und 65 Zeichen, `str<8>` mit 8 und 9 Bytes, auch aus
/// Mehrbytezeichen, und ein `has`-Ansatz, der erst hinter einem
/// Mehrbytezeichen beginnt.
const TEXTS: [&str; 62] = [
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
    "Boot v9223372036854775808",
    "Boot v-9223372036854775809",
    "Boot v9999999999999999999",
    "Boot v+5",
    "Erasing sector +7",
    "Boot v007",
    "Boot v",
    "Boot vx",
    "Erasing sector 42",
    "Erasing sector -7",
    "1-2",
    "-1--2",
    "12-34",
    "addr 0x7E8",
    "addr 0x7fffffffffffffff",
    "addr 0x8000000000000000",
    "addr 0X7e8",
    "addr 7E8",
    "addr ",
    "user root",
    "user r_1",
    "user ",
    "a=1",
    "[abc]",
    "[abcdefghij]",
    "Boot v1234567890123456789",
    "Boot v12345678901234567890",
    "Boot v1234567890123456789 ok",
    "Boot v12345678901234567890 ok",
    "Boot v-1234567890123456789",
    "Boot v-12345678901234567890",
    "addr 0x123456789abcdef0",
    "addr 0x123456789abcdef01",
    "addr 123456789abcdef0",
    "addr 123456789abcdef01",
    "addr 0x",
    "addr 0xg",
    "addr 0x123456789abcdef01 ok",
    "user abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_x",
    "user abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_xy",
    "user abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_xy!",
    "[abcdefgh]",
    "[abcdefghi]",
    "[\u{e4}\u{f6}\u{fc}\u{e4}]",
    "[\u{e4}\u{f6}\u{fc}\u{e4}b]",
    "\u{fc}[\u{e4}\u{f6}]",
    "abcdefghij",
    "a=12345678901234567890",
    "a=1234567890123456789",
    "x=1 y=2",
    "Boot v1 Boot v2",
    "\u{e9}x",
];

/// Die beiden Lesarten eines Musters (8.7).
const MODES: [&str; 2] = ["pruef", "such"];

/// Baut ein Modul mit je einer Funktion `pruef<i>` (`matches`) und
/// `such<i>` (`has`) pro Muster.
///
/// Die Funktion nimmt einen Zeiger auf `{ i32 len, [128 x i8] }` und
/// schreibt das Ergebnis: das Urteil und die Werte der Captures. Mehr
/// braucht der Vergleich nicht, und weniger waere zu wenig.
fn module_of(pattern_of: &[Pattern]) -> String {
    // Der Test uebersetzt *und* laeuft, also muss das Triple das des
    // Wirts sein.
    let triple = if cfg!(windows) { "x86_64-pc-windows-msvc" } else { "x86_64-unknown-linux-gnu" };
    let mut m = Module::new("captures", triple);
    for ((i, mu), mode) in pattern_of.iter().enumerate().flat_map(|p| MODES.map(|mode| (p, mode))) {
        // `pruef(text, out, arena) -> i1`; `out` nimmt den Aufnahmerecord.
        // Von aussen gerufen, also mit aeusserer Bindung; die Arena haengt
        // an jeder erzeugten Funktion (12.11).
        let regs = m.begin_with("", &format!("{mode}{i}"), &LlvmType::Int(1), &[LlvmType::Ptr, LlvmType::Ptr], &[], "");
        let (text, out) = (regs[0], regs[1]);
        let count_of = mu.pieces.iter().filter(|p| matches!(p, PatternPiece::Capture { .. })).count();
        // Der Aufnahmerecord: je Capture ein Feld. `int` und `hex` sind
        // i64, `word` und `str` ein Textpuffer wie `str<N>` (3.9).
        let fields: Vec<LlvmType> = mu
            .pieces
            .iter()
            .filter_map(|p| match p {
                PatternPiece::Capture { kind: CaptureKind::Int | CaptureKind::Hex, .. } => Some(LlvmType::Int(64)),
                PatternPiece::Capture { kind: CaptureKind::Word | CaptureKind::Str(_), .. } => {
                    Some(LlvmType::Struct(vec![LlvmType::Int(32), LlvmType::Array(Box::new(LlvmType::Int(8)), 64)]))
                }
                _ => None,
            })
            .collect();
        let rec = LlvmType::Struct(fields.clone());
        let list: Vec<(u32, LlvmType)> = fields.iter().enumerate().map(|(j, f)| (j as u32, f.clone())).collect();
        let into = (count_of > 0).then(|| takt_llvm::captures::Target { slot: out, record: &rec, fields: &list });
        let hit = if mode == "such" {
            takt_llvm::captures::has(&mu.pieces, text, LINE_CAP, into.as_ref(), &mut m).expect("has")
        } else {
            let zero = m.inst("add i32 0, 0");
            let (ok, at) = takt_llvm::captures::walk(&mu.pieces, text, into.as_ref(), zero, &mut m).expect("walk");
            // `matches`: der ganze Text muss verbraucht sein (8.7).
            let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
            let len = m.inst(&format!("load i32, ptr {len_ptr}"));
            let whole = m.inst(&format!("icmp eq i32 {at}, {len}"));
            m.inst(&format!("and i1 {ok}, {whole}"))
        };
        m.end(Some((&LlvmType::Int(1), hit.to_string())));
    }
    m.finish()
}

/// `{x:float}` baut der Codegen nicht: Eine eigene Dezimalkonversion waere
/// eine zweite Rundungsquelle neben `libtaktm` (4.2). `matches` und `has`
/// lehnen das Muster darum ab, statt still anders zu binden — auch `has`
/// hinter einem offenen Ende, das sonst den Durchlaufautomaten naehme.
#[test]
fn a_float_capture_is_not_built() {
    let float = PatternPiece::Capture { name: "x".to_string(), kind: CaptureKind::Float };
    for pieces in [vec![PatternPiece::Text("t=".to_string()), float.clone()], vec![PatternPiece::Any, float]] {
        let mut m = Module::new("float", "x86_64-unknown-linux-gnu");
        let regs = m.begin_with("", "f", &LlvmType::Int(1), &[LlvmType::Ptr], &[], "");
        let zero = m.inst("add i32 0, 0");
        assert!(takt_llvm::captures::walk(&pieces, regs[0], None, zero, &mut m).is_err(), "{pieces:?}");
        assert!(takt_llvm::captures::has(&pieces, regs[0], LINE_CAP, None, &mut m).is_err(), "{pieces:?}");
    }
}

#[test]
fn the_generated_matcher_agrees_with_takt_match() {
    let Some(path) =
        takt_testkit::require("clang", find().path().cloned(), "`TAKT_CLANG` setzen oder LLVM installieren")
    else {
        return;
    };
    let clang = Clang::At(path.clone());
    let _ = &clang;
    let pattern_of = all_of();
    let ir = module_of(&pattern_of);

    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takt-llvm-captures");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("Verzeichnis");
    let ll = dir.join("captures.ll");
    let c = dir.join("treiber.c");
    let exe = dir.join(if cfg!(windows) { "lauf.exe" } else { "lauf" });
    std::fs::write(&ll, &ir).expect("IR");
    std::fs::write(&c, driver(&pattern_of)).expect("Treiber");

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
    let expected = oracle(&pattern_of);
    let actual: Vec<&str> = nativ.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(actual.len(), expected.len(), "verschieden viele Zeilen");
    let mut differences = Vec::new();
    for (i, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
        if a != b {
            let m = &pattern_of[i / (TEXTS.len() * MODES.len())];
            let t = TEXTS[i / MODES.len() % TEXTS.len()];
            let mode = ["matches", "has"][i % MODES.len()];
            differences.push(format!("  `{}` {mode} {t:?}: nativ `{a}`, `takt-match` `{b}`", m.text));
        }
    }
    assert!(
        differences.is_empty(),
        "{} Abweichungen zwischen erzeugtem Code und `takt-match`:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

/// Was `takt-match` zu jedem Paar sagt, je `matches` und `has`, in
/// derselben Schreibweise wie der Treiber. Ein Wert ausserhalb des
/// Bereichs ist kein Treffer (8.7).
fn oracle(pattern_of: &[Pattern]) -> Vec<String> {
    let mut out = Vec::new();
    for mu in pattern_of {
        for (t, has) in TEXTS.iter().flat_map(|t| [(t, false), (t, true)]) {
            let hit = if has {
                takt_match::has(&mu.pieces_of, t.as_bytes())
            } else {
                takt_match::matches(&mu.pieces_of, t.as_bytes())
            };
            let Some(hit) = hit else {
                out.push("0".to_string());
                continue;
            };
            let mut line_of = String::from("1");
            let mut kinds = mu.pieces.iter().filter_map(|p| match p {
                PatternPiece::Capture { kind, .. } => Some(kind),
                _ => None,
            });
            let values: Option<Vec<String>> = hit.spans[..hit.count as usize]
                .iter()
                .map(|s| {
                    let roh = s.of(t.as_bytes());
                    match kinds.next() {
                        Some(CaptureKind::Int) => takt_match::parse_int(roh).map(|v| v.to_string()),
                        Some(CaptureKind::Hex) => takt_match::parse_hex(roh).map(|v| v.to_string()),
                        _ => Some(String::from_utf8_lossy(roh).to_string()),
                    }
                })
                .collect();
            let Some(values) = values else {
                out.push("0".to_string());
                continue;
            };
            for v in values {
                line_of.push(' ');
                line_of.push_str(&v);
            }
            out.push(line_of);
        }
    }
    out
}

/// Der C-Treiber: Er ruft je Muster und Text die erzeugte Funktion und
/// schreibt Urteil und Werte.
fn driver(pattern_of: &[Pattern]) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "#include <stdio.h>");
    let _ = writeln!(s, "#include <string.h>\n");
    let _ = writeln!(s, "struct text {{ int len; char bytes[{LINE_CAP}]; }};");
    let _ = writeln!(s, "struct wort {{ int len; char bytes[64]; }};");
    // Je Muster ein eigener Record: Die Ausrichtung rechnet der
    // C-Compiler, wie LLVM sie auf der anderen Seite rechnet. Versaetze
    // von Hand waeren eine dritte Meinung dazu.
    for (i, mu) in pattern_of.iter().enumerate() {
        let fields: Vec<String> = mu
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
        if fields.is_empty() {
            let _ = writeln!(s, "struct rec{i} {{ char leer; }};");
        } else {
            let _ = writeln!(s, "struct rec{i} {{ {} }};", fields.join(" "));
        }
    }
    let _ = writeln!(s);
    for (i, mode) in (0..pattern_of.len()).flat_map(|i| MODES.map(|mode| (i, mode))) {
        let _ = writeln!(s, "_Bool {mode}{i}(struct text *, void *, void *);");
    }
    // Die Fault-Ablagen am Anfang der Arena genuegen: Mehr beruehrt der
    // Mustervergleich nicht.
    let _ = writeln!(s, "static _Alignas(8) unsigned char arena[{}];", takt_llvm::arena::fault::BYTES);
    let _ = writeln!(s, "\nint main(void) {{");
    let _ = writeln!(s, "    struct text t;");
    for (i, _) in pattern_of.iter().enumerate() {
        let _ = writeln!(s, "    struct rec{i} r{i};");
    }
    for (i, mu) in pattern_of.iter().enumerate() {
        for (text, mode) in TEXTS.iter().flat_map(|t| MODES.map(|mode| (t, mode))) {
            let bytes: String = text.as_bytes().iter().map(|b| format!("\\x{b:02x}")).collect();
            let _ = writeln!(s, "    memset(&t, 0, sizeof t);");
            let _ = writeln!(s, "    t.len = {};", text.len());
            if !text.is_empty() {
                let _ = writeln!(s, "    memcpy(t.bytes, \"{bytes}\", {});", text.len());
            }
            let _ = writeln!(s, "    memset(&r{i}, 0, sizeof r{i});");
            let _ = writeln!(s, "    if (!{mode}{i}(&t, &r{i}, arena)) {{ printf(\"0\\n\"); }} else {{");
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
