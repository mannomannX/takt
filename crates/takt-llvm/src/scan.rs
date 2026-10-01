//! Der Durchlaufautomat eines Musters im erzeugten Code (8.7, 11.2;
//! FB-351): die frueheste Stelle, ab der `has` gelingt, in einem Lauf ueber
//! die Zeile.
//!
//! **Was der Lauf haelt.** Je Zustand des Automaten den fruehesten Start
//! eines Fadens, der dort steht (`-1`: keiner), und die Liste der lebenden
//! Zustaende — beides zweimal, fuer das laufende und das naechste Byte. Ein
//! Faden beginnt an jedem Zeichenanfang und hinter dem letzten Zeichen,
//! solange kein Treffer feststeht; ein Faden, der spaeter als ein Treffer
//! begann, faellt weg. Am Ende steht der frueheste Treffer fest. Dieselbe
//! Rechnung steht in `takt_mir::scan::Scan::first`, und der Test gegen den
//! Interpreter prueft sie dort.
//!
//! **Was er kostet.** Je Byte so viele Schritte, wie Faeden leben —
//! hoechstens so viele, wie der Automat Zustaende hat, und auf einer Zeile
//! ohne Treffer meist einer oder keiner. Der Rahmen traegt die beiden
//! Felder: sechs Byte je Zustand und Haelfte.
//!
//! **Die Tabellen** heissen nach ihrem Schluessel (`Scan::key`), wie die des
//! Produkt-DFA (`dfa.rs`), und liegen wie diese als Konstanten im Objekt.
//! Das Zeilenende ist die letzte Spalte der Uebergaenge: So laeuft es durch
//! dieselbe Schleife wie jedes Byte.

use takt_mir::scan::{ACCEPT, FAIL, Scan};

use crate::emit::{Module, Reg};

/// Der Name einer Tabelle des Automaten.
fn name(scan: &Scan, what: &str) -> String {
    format!("@takt_scan_{:016x}_{what}", scan.key())
}

/// Die Pruefung, ob eine Zahl in `i64` passt (`takt_mir::scan::fits`): die
/// 19 Ziffern eines `int` gegen `9223372036854775807` (mit `-` gegen
/// `…808`), die erste von 16 Ziffern eines `hex` unter `8`. Einmal je
/// Modul.
const FITS: &str = "
@takt_scan_i64_max = private constant [19 x i8] c\"9223372036854775807\"

define internal i1 @takt_scan_fits(ptr %bytes, i32 %end, i32 %check) {
entry:
  %is_hex = icmp eq i32 %check, 3
  br i1 %is_hex, label %hex, label %int
hex:
  %h_at = sub i32 %end, 16
  %h_p = getelementptr inbounds i8, ptr %bytes, i32 %h_at
  %h = load i8, ptr %h_p
  %small = icmp ult i8 %h, 56
  ret i1 %small
int:
  %neg = icmp eq i32 %check, 2
  %last = select i1 %neg, i8 56, i8 55
  %from = sub i32 %end, 19
  br label %digit
digit:
  %j = phi i32 [ 0, %int ], [ %j1, %next ]
  %at = add i32 %from, %j
  %p = getelementptr inbounds i8, ptr %bytes, i32 %at
  %c = load i8, ptr %p
  %lp = getelementptr inbounds [19 x i8], ptr @takt_scan_i64_max, i32 0, i32 %j
  %l0 = load i8, ptr %lp
  %is_last = icmp eq i32 %j, 18
  %l = select i1 %is_last, i8 %last, i8 %l0
  %below = icmp ult i8 %c, %l
  br i1 %below, label %yes, label %not_below
not_below:
  %above = icmp ugt i8 %c, %l
  br i1 %above, label %no, label %next
next:
  %j1 = add i32 %j, 1
  %more = icmp ult i32 %j1, 19
  br i1 %more, label %digit, label %yes
yes:
  ret i1 true
no:
  ret i1 false
}";

/// Schreibt die Tabellen als Konstanten, einmal je Automat: die
/// Klassenabbildung (256 Byte) und die Uebergaenge (`states × (classes + 1)`
/// in `i16`, die letzte Spalte das Zeilenende).
fn declare(scan: &Scan, m: &mut Module) {
    if !m.has_declared(FITS) {
        m.declare(FITS);
    }
    let width = scan.class_count as usize;
    let classes: Vec<String> = scan.classes.iter().map(|c| format!("i8 {c}")).collect();
    let table: Vec<String> = (0..scan.states())
        .flat_map(|s| scan.table[s * width..(s + 1) * width].iter().chain(std::iter::once(&scan.eot[s])))
        .map(|e| format!("i16 {}", *e as i16))
        .collect();
    let mut text = format!(
        "\n; Durchlaufautomat (8.7, 11.2; FB-351): {} Zustaende, {} Klassen und das Zeilenende",
        scan.states(),
        scan.class_count
    );
    text.push_str(&format!("\n{} = private constant [256 x i8] [{}]", name(scan, "classes"), classes.join(", ")));
    text.push_str(&format!(
        "\n{} = private constant [{} x i16] [{}]",
        name(scan, "table"),
        table.len(),
        table.join(", ")
    ));
    if !m.has_declared(&text) {
        m.declare(&text);
    }
}

/// Die frueheste Stelle, ab der das Muster in `text` gelingt, als `i32`;
/// `-1` ohne Treffer.
///
/// `text` zeigt auf `{ i32 len, [N x i8] bytes, ... }` — den Aufbau von
/// `str<N>` und `line<N>` (`ty::lower`). Die Schleife ist durch die Laenge
/// beschraenkt, die innere durch die Zahl der Zustaende (4.1).
pub fn first(scan: &Scan, text: Reg, m: &mut Module) -> Reg {
    if scan.start == ACCEPT {
        return m.inst("add i32 0, 0");
    }
    declare(scan, m);
    let states = scan.states() as u32;
    let columns = scan.class_count + 1;
    let k = m.next_label();
    let label = |what: &str| format!("scan{k}_{what}");

    let len_ptr = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 0"));
    let len = m.inst(&format!("load i32, ptr {len_ptr}"));
    let bytes = m.inst(&format!("getelementptr inbounds i8, ptr {text}, i64 4"));

    // `starts[side + s]`: der frueheste Start in Zustand `s`, `-1` fuer
    // keinen; `lists[side + i]`: der i-te lebende Zustand. `side` ist 0 oder
    // `states` und wechselt je Byte.
    // Die Plaetze leben nur waehrend der Suche: So teilen sich die Suchen
    // eines Schritts einen Rahmen.
    let slots = m.slot_mark();
    let starts = m.alloca(&format!("[{} x i32]", 2 * states));
    m.needs_intrinsic("void @llvm.memset.p0.i32(ptr, i8, i32, i1)");
    m.void_inst(&format!("call void @llvm.memset.p0.i32(ptr {starts}, i8 -1, i32 {}, i1 false)", 8 * states));
    let lists = m.alloca(&format!("[{} x i16]", 2 * states));
    let slot = |m: &mut Module, init: &str| {
        let p = m.alloca("i32");
        m.void_inst(&format!("store i32 {init}, ptr {p}"));
        p
    };
    let side_p = slot(m, "0");
    let live_p = slot(m, "0");
    let born_p = slot(m, "0");
    let best_p = slot(m, "-1");
    let pos_p = slot(m, "0");
    let i_p = slot(m, "0");
    let at = |m: &mut Module, base: Reg, ty: &str, idx: &str| {
        m.inst(&format!("getelementptr inbounds {ty}, ptr {base}, i32 {idx}"))
    };
    m.void_inst(&format!("br label %{}", label("kopf")));

    // Je Stelle: das Byte und seine Klasse, am Zeilenende die letzte Spalte.
    m.label(&label("kopf"));
    let pos = m.inst(&format!("load i32, ptr {pos_p}"));
    let at_end = m.inst(&format!("icmp eq i32 {pos}, {len}"));
    m.void_inst(&format!("br i1 {at_end}, label %{}, label %{}", label("ende"), label("byte")));
    m.label(&label("byte"));
    let byte_p = at(m, bytes, "i8", &pos.to_string());
    let byte = m.inst(&format!("load i8, ptr {byte_p}"));
    let wide = m.inst(&format!("zext i8 {byte} to i32"));
    let class_p =
        m.inst(&format!("getelementptr inbounds [256 x i8], ptr {}, i32 0, i32 {wide}", name(scan, "classes")));
    let class8 = m.inst(&format!("load i8, ptr {class_p}"));
    let class_b = m.inst(&format!("zext i8 {class8} to i32"));
    let high = m.inst(&format!("and i8 {byte}, -64"));
    let char_b = m.inst(&format!("icmp ne i8 {high}, -128"));
    m.void_inst(&format!("br label %{}", label("stelle")));
    m.label(&label("ende"));
    m.void_inst(&format!("br label %{}", label("stelle")));
    m.label(&label("stelle"));
    let class =
        m.inst(&format!("phi i32 [ {class_b}, %{} ], [ {}, %{} ]", label("byte"), scan.class_count, label("ende")));
    let char_start = m.inst(&format!("phi i1 [ {char_b}, %{} ], [ true, %{} ]", label("byte"), label("ende")));

    // Ein neuer Faden, solange kein Treffer feststeht.
    let best = m.inst(&format!("load i32, ptr {best_p}"));
    let open = m.inst(&format!("icmp slt i32 {best}, 0"));
    let inject = m.inst(&format!("and i1 {char_start}, {open}"));
    m.void_inst(&format!("br i1 {inject}, label %{}, label %{}", label("neu"), label("faeden")));
    m.label(&label("neu"));
    let side = m.inst(&format!("load i32, ptr {side_p}"));
    let idx = m.inst(&format!("add i32 {side}, {}", scan.start));
    let start_p = at(m, starts, "i32", &idx.to_string());
    let old = m.inst(&format!("load i32, ptr {start_p}"));
    let free = m.inst(&format!("icmp slt i32 {old}, 0"));
    m.void_inst(&format!("br i1 {free}, label %{}, label %{}", label("neu_eintrag"), label("faeden")));
    m.label(&label("neu_eintrag"));
    m.void_inst(&format!("store i32 {pos}, ptr {start_p}"));
    let live = m.inst(&format!("load i32, ptr {live_p}"));
    let slot_i = m.inst(&format!("add i32 {side}, {live}"));
    let list_p = at(m, lists, "i16", &slot_i.to_string());
    m.void_inst(&format!("store i16 {}, ptr {list_p}", scan.start));
    let live1 = m.inst(&format!("add i32 {live}, 1"));
    m.void_inst(&format!("store i32 {live1}, ptr {live_p}"));
    m.void_inst(&format!("br label %{}", label("faeden")));

    // Jeder lebende Faden ein Schritt.
    m.label(&label("faeden"));
    m.void_inst(&format!("store i32 0, ptr {i_p}"));
    m.void_inst(&format!("br label %{}", label("faden")));
    m.label(&label("faden"));
    let i = m.inst(&format!("load i32, ptr {i_p}"));
    let live = m.inst(&format!("load i32, ptr {live_p}"));
    let more = m.inst(&format!("icmp slt i32 {i}, {live}"));
    m.void_inst(&format!("br i1 {more}, label %{}, label %{}", label("schritt"), label("gelesen")));

    m.label(&label("schritt"));
    let side = m.inst(&format!("load i32, ptr {side_p}"));
    let slot_i = m.inst(&format!("add i32 {side}, {i}"));
    let list_p = at(m, lists, "i16", &slot_i.to_string());
    let s16 = m.inst(&format!("load i16, ptr {list_p}"));
    let s = m.inst(&format!("zext i16 {s16} to i32"));
    let idx = m.inst(&format!("add i32 {side}, {s}"));
    let start_p = at(m, starts, "i32", &idx.to_string());
    let st = m.inst(&format!("load i32, ptr {start_p}"));
    m.void_inst(&format!("store i32 -1, ptr {start_p}"));
    let best = m.inst(&format!("load i32, ptr {best_p}"));
    let found = m.inst(&format!("icmp sge i32 {best}, 0"));
    let later = m.inst(&format!("icmp sgt i32 {st}, {best}"));
    let useless = m.inst(&format!("and i1 {found}, {later}"));
    m.void_inst(&format!("br i1 {useless}, label %{}, label %{}", label("weiter"), label("eintrag")));

    m.label(&label("eintrag"));
    let row = m.inst(&format!("mul i32 {s}, {columns}"));
    let cell = m.inst(&format!("add i32 {row}, {class}"));
    let entry_p = m.inst(&format!(
        "getelementptr inbounds [{} x i16], ptr {}, i32 0, i32 {cell}",
        states * columns,
        name(scan, "table")
    ));
    let entry16 = m.inst(&format!("load i16, ptr {entry_p}"));
    let entry = m.inst(&format!("zext i16 {entry16} to i32"));
    let target = m.inst(&format!("and i32 {entry}, 16383"));
    let check = m.inst(&format!("lshr i32 {entry}, 14"));
    let dead = m.inst(&format!("icmp eq i32 {target}, {FAIL}"));
    m.void_inst(&format!("br i1 {dead}, label %{}, label %{}", label("weiter"), label("pruefung")));
    m.label(&label("pruefung"));
    let needs = m.inst(&format!("icmp ne i32 {check}, 0"));
    m.void_inst(&format!("br i1 {needs}, label %{}, label %{}", label("passt"), label("ziel")));
    m.label(&label("passt"));
    let fits = m.inst(&format!("call i1 @takt_scan_fits(ptr {bytes}, i32 {pos}, i32 {check})"));
    m.void_inst(&format!("br i1 {fits}, label %{}, label %{}", label("ziel"), label("weiter")));

    m.label(&label("ziel"));
    let hit = m.inst(&format!("icmp eq i32 {target}, {ACCEPT}"));
    m.void_inst(&format!("br i1 {hit}, label %{}, label %{}", label("treffer"), label("uebergang")));
    m.label(&label("treffer"));
    let best = m.inst(&format!("load i32, ptr {best_p}"));
    let none = m.inst(&format!("icmp slt i32 {best}, 0"));
    let earlier = m.inst(&format!("icmp slt i32 {st}, {best}"));
    let better = m.inst(&format!("or i1 {none}, {earlier}"));
    let new_best = m.inst(&format!("select i1 {better}, i32 {st}, i32 {best}"));
    m.void_inst(&format!("store i32 {new_best}, ptr {best_p}"));
    m.void_inst(&format!("br label %{}", label("weiter")));

    // In die andere Haelfte: der frueheste Start je Zielzustand.
    m.label(&label("uebergang"));
    let other = m.inst(&format!("sub i32 {states}, {side}"));
    let tidx = m.inst(&format!("add i32 {other}, {target}"));
    let target_p = at(m, starts, "i32", &tidx.to_string());
    let there = m.inst(&format!("load i32, ptr {target_p}"));
    let empty = m.inst(&format!("icmp slt i32 {there}, 0"));
    m.void_inst(&format!("br i1 {empty}, label %{}, label %{}", label("geboren"), label("frueher")));
    m.label(&label("geboren"));
    m.void_inst(&format!("store i32 {st}, ptr {target_p}"));
    let born = m.inst(&format!("load i32, ptr {born_p}"));
    let slot_n = m.inst(&format!("add i32 {other}, {born}"));
    let list_n = at(m, lists, "i16", &slot_n.to_string());
    let t16 = m.inst(&format!("trunc i32 {target} to i16"));
    m.void_inst(&format!("store i16 {t16}, ptr {list_n}"));
    let born1 = m.inst(&format!("add i32 {born}, 1"));
    m.void_inst(&format!("store i32 {born1}, ptr {born_p}"));
    m.void_inst(&format!("br label %{}", label("weiter")));
    m.label(&label("frueher"));
    let smaller = m.inst(&format!("icmp slt i32 {st}, {there}"));
    let kept = m.inst(&format!("select i1 {smaller}, i32 {st}, i32 {there}"));
    m.void_inst(&format!("store i32 {kept}, ptr {target_p}"));
    m.void_inst(&format!("br label %{}", label("weiter")));

    m.label(&label("weiter"));
    let i1 = m.inst(&format!("add i32 {i}, 1"));
    m.void_inst(&format!("store i32 {i1}, ptr {i_p}"));
    m.void_inst(&format!("br label %{}", label("faden")));

    // Die Haelften tauschen; am Zeilenende oder wenn ein Treffer feststeht
    // und kein frueherer Faden mehr lebt, ist der Lauf fertig.
    m.label(&label("gelesen"));
    let side = m.inst(&format!("load i32, ptr {side_p}"));
    let flipped = m.inst(&format!("sub i32 {states}, {side}"));
    m.void_inst(&format!("store i32 {flipped}, ptr {side_p}"));
    let born = m.inst(&format!("load i32, ptr {born_p}"));
    m.void_inst(&format!("store i32 {born}, ptr {live_p}"));
    m.void_inst(&format!("store i32 0, ptr {born_p}"));
    let next_pos = m.inst(&format!("add i32 {pos}, 1"));
    m.void_inst(&format!("store i32 {next_pos}, ptr {pos_p}"));
    let best = m.inst(&format!("load i32, ptr {best_p}"));
    let found = m.inst(&format!("icmp sge i32 {best}, 0"));
    let gone = m.inst(&format!("icmp eq i32 {born}, 0"));
    let settled = m.inst(&format!("and i1 {found}, {gone}"));
    let stop = m.inst(&format!("or i1 {settled}, {at_end}"));
    m.void_inst(&format!("br i1 {stop}, label %{}, label %{}", label("fertig"), label("kopf")));

    m.label(&label("fertig"));
    let found = m.inst(&format!("load i32, ptr {best_p}"));
    m.end_slots(slots);
    found
}
