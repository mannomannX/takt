//! Der Rahmen, der ein Takt-Programm auf einer MCU ausfuehrt (12.1, 12.3).
//!
//! **Dieselbe Konstruktion wie [`crate::harness`], ein anderes Ziel.** Der
//! erzeugte Code spricht die C-ABI, und der Workspace verbietet
//! `extern "C"` (13.4) — also erzeugt auch dieser Rahmen C. Was ihn
//! unterscheidet, sind drei Dinge, und jedes folgt aus 12.3:
//!
//! 1. **Keine `stdio`.** Auf der MCU gibt es keine libc; die Telemetrie
//!    geht ueber USART, und das Board stellt die Funktion.
//! 2. **Kein `main`.** Die Tickschleife liegt in `takt-rt-baremetal`; der
//!    Rahmen liefert ihr `takt_mcu_init` und `takt_mcu_tick` und nichts
//!    weiter.
//! 3. **Statischer Speicher.** Prozessabbild, Latch und Maschinenzustaende
//!    stehen in `.bss` (12.3: „Gesamter Zustand statisch; kein Heap").
//!
//! **Was der Rahmen nicht ist.** Er ist keine Runtime: Er hat keine Uhr
//! (die kommt vom Timer), keine Treiber (die kommen vom Board) und keine
//! Fault-Behandlung ueber die Abort-Phase hinaus. Er ist das Bindeglied
//! zwischen erzeugtem Code und Tickschleife — die Stelle, an der ein
//! Takt-Programm auf einer MCU zum ersten Mal laeuft.

use std::fmt::Write as _;

use takt_mir::machine::MachineKind;
use takt_mir::program::Program;

use crate::layout::{Layout, c_type};

/// Der erzeugte MCU-Rahmen.
pub struct McuHarness {
    /// Der C-Quelltext.
    pub source: String,
    /// Die Speicherform, die er erwartet.
    pub layout: Layout,
}

/// Baut den Rahmen fuer alle Maschinen eines Programms.
///
/// Anders als der Linux-Rahmen kennt dieser keine Tickzahl: Die Schleife
/// laeuft, bis das Board ausgeht.
pub fn build(p: &Program) -> McuHarness {
    let layout = crate::layout::of(p);
    let driven: Vec<&takt_mir::machine::Machine> =
        p.machines.iter().filter(|m| m.kind != MachineKind::Template).collect();

    let mut s = String::new();
    prologue(&mut s, p);
    runtime_abi(&mut s, p);
    storage(&mut s, &layout, &driven);
    declarations(&mut s, &driven);
    init(&mut s, p, &layout, &driven);
    tick(&mut s, p, &driven);
    telemetry(&mut s, p, &layout);

    McuHarness { source: s, layout }
}

/// Kopf und Vorwaertsdeklarationen.
fn prologue(s: &mut String, p: &Program) {
    let _ = writeln!(s, "/* MCU-Rahmen (12.1, 12.3); erzeugt von takt-conformance. */");
    let _ = writeln!(s, "/* Tick: {} ns. Kein Heap, keine libc. */\n", p.config.tick);
    // Nur die Typen, nicht die Funktionen: `stdint.h` ist Teil der
    // freistehenden Umgebung und steht auch ohne libc zur Verfuegung.
    let _ = writeln!(s, "#include <stdint.h>\n");
    // Das Board stellt sie bereit; der Rahmen ruft sie nur.
    let _ = writeln!(s, "void takt_board_trace(const char *line);");
    let _ = writeln!(s, "void takt_board_trace_i64(long long value);\n");
}

/// Die Runtime-Aufrufe aus `takt-llvm/src/abi.rs`.
///
/// **Sie schreiben in die Telemetrie, nicht auf `stdout`.** Was der
/// Linux-Rahmen mit `printf` macht, macht dieser ueber das Board — und
/// weil UART langsam ist, bleibt die Ausgabe knapp: Ereignisart, Maschine,
/// Stelle. Der Vergleich mit dem Interpreter braucht nicht mehr.
fn runtime_abi(s: &mut String, p: &Program) {
    let _ = writeln!(s, "static long long g_tick = 0;");
    let _ = writeln!(s, "unsigned char takt_fn_fault = 0;\n");

    // 3.3: `now` ist die Dauer seit dem Start — Tickzahl mal T0.
    let _ = writeln!(s, "long long takt_now(void) {{ return g_tick * {}LL; }}\n", p.config.tick);

    // Jede Beobachtungszeile traegt ihren Tick, wie beim Interpreter
    // (`grammar/trace.md`): Ohne ihn laesst sie sich keinem Tick zuordnen.
    for (name, args, kind, flag) in [
        ("takt_alert", "int m, int site, unsigned char on", "alert", Some("on")),
        ("takt_log", "int m, int site", "log", None),
        ("takt_abort", "int m, int site", "abort", None),
        ("takt_verify", "int m, int site, unsigned char ok", "verify", Some("ok")),
        ("takt_verdict", "int m, int site, unsigned char pass", "verdict", Some("pass")),
    ] {
        let _ = writeln!(s, "void {name}({args}) {{");
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "    takt_board_trace(\"{kind} \");");
        let _ = writeln!(s, "    takt_board_trace_i64(m);");
        let _ = writeln!(s, "    takt_board_trace_i64(site);");
        if let Some(f) = flag {
            let _ = writeln!(s, "    takt_board_trace_i64({f} ? 1 : 0);");
        }
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
        let _ = writeln!(s, "}}");
    }

    // **Das Bitmuster, nicht der gerechnete Wert.** Eine erste Fassung gab
    // `(long long)(v * 1000000.0)` aus — Mikroeinheiten, weil es ohne
    // `printf` kein `%g` gibt. Das kostete auf einem Kern ohne f64-Hardware
    // die ganze Software-Emulation: `__muldf3`, `u64_div_rem` und
    // `__aeabi_d2lz`, zusammen 1778 Byte, und das in einem Binary von
    // 4866 Byte — fuer eine Funktion, die das Programm nie rief (FB-143).
    //
    // Die Bits kosten nichts und sagen mehr: 4.2 verlangt bitgleiche
    // Ergebnisse ueber alle Targets, und `same_number` vergleicht
    // Fliesskomma ohnehin bitweise (9.4.4). Eine Multiplikation waere eine
    // zweite Rundungsquelle vor genau diesem Vergleich.
    //
    // `memcpy` statt eines Zeiger-Casts: Ein `*(long long *)&v` waere ein
    // Verstoss gegen die Aliasing-Regeln von C, und ein Compiler darf ihn
    // wegoptimieren. Fuer acht Byte erzeugt jeder Compiler daraus einen
    // Registertausch.
    let _ = writeln!(s, "void takt_measure(int m, int site, double v) {{");
    let _ = writeln!(s, "    unsigned long long bits;");
    let _ = writeln!(s, "    __builtin_memcpy(&bits, &v, sizeof bits);");
    let _ = writeln!(s, "    takt_board_trace(\"t=\");");
    let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
    let _ = writeln!(s, "    takt_board_trace(\"measure \");");
    let _ = writeln!(s, "    takt_board_trace_i64(m);");
    let _ = writeln!(s, "    takt_board_trace_i64(site);");
    let _ = writeln!(s, "    takt_board_trace(\"bits \");");
    let _ = writeln!(s, "    takt_board_trace_i64((long long)bits);");
    let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    let _ = writeln!(s, "}}\n");
}

/// Prozessabbild, Latch und Maschinenzustaende — alles statisch.
///
/// 12.3: „Gesamter Zustand statisch in `.bss`; kein Heap." Die Groessen
/// kommen aus `takt size` (11.5), also aus derselben Rechnung, die der
/// Compiler gegen das Speicherbudget haelt.
fn storage(s: &mut String, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Statischer Zustand (12.3), ausgerichtet fuer die ABI. */");
    let _ = writeln!(s, "{}", crate::layout::c_buffer("image", layout.image));
    let _ = writeln!(s, "{}", crate::layout::c_buffer("latch", layout.latch));
    let _ = writeln!(s, "{}", crate::layout::c_buffer("params", layout.params));
    for m in driven {
        // Die Zustandsgroesse kennt der Rahmen nicht genau; er nimmt die
        // Obergrenze aus dem Overlay (11.2). Zu gross ist verschwendeter
        // RAM, zu klein waere ein Ueberschreiben — darum grosszuegig.
        let _ = writeln!(s, "{}", crate::layout::c_buffer(&format!("state_{}", m.name), state_bytes(m)));
    }
    let _ = writeln!(s);
}

/// Die Zustandsgroesse einer Maschine, aufgerundet.
fn state_bytes(m: &takt_mir::machine::Machine) -> u64 {
    // `takt size` rechnet es genau (11.5); hier genuegt eine Schranke,
    // weil der Rahmen den Speicher nur bereitstellt.
    let vars = m.vars.len() as u64 * 8;
    let states = m.states.len() as u64 * 16;
    (vars + states + 64).next_multiple_of(8)
}

/// Die Signaturen des erzeugten Codes (11.2).
fn declarations(s: &mut String, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Der erzeugte Code (11.2). */");
    for m in driven {
        let _ = writeln!(s, "void {}_init(void *st, void *in, void *par, void *out);", m.name);
        let _ = writeln!(s, "void {}_step(void *st, void *in, void *par, void *out);", m.name);
    }
    let _ = writeln!(s);
}

/// `takt_mcu_init`: einmal vor dem ersten Tick.
fn init(s: &mut String, p: &Program, layout: &Layout, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Einmal vor dem ersten Tick (12.1, Schritt 1). */");
    let _ = writeln!(s, "void takt_mcu_init(void) {{");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof image; i++) image[i] = 0;");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof latch; i++) latch[i] = 0;");
    let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof params; i++) params[i] = 0;");
    for m in driven {
        let _ = writeln!(s, "    for (unsigned i = 0; i < sizeof state_{0}; i++) state_{0}[i] = 0;", m.name);
    }

    // 3.5: Ein Input ohne Treiber ist `Bad`. Ein genullter Eintrag hiesse
    // `Good`, und das waere eine Zusage, die kein Treiber gegeben hat.
    for slot in &layout.inputs {
        if let Some(entry) = crate::harness::quality_offset(p, &slot.name) {
            let _ = writeln!(s, "    image[{entry}] = 3; /* {} ist Bad (3.5) */", slot.name);
        }
    }

    // Die Parameter stehen fuer den Lauf fest (8.4).
    for (i, slot) in layout.parameters.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let Some(value) = crate::harness::param_literal(p, i) else { continue };
        let _ = writeln!(s, "    *({ct} *)(params + {}) = {value}; /* {} */", slot.offset, slot.name);
    }

    for m in driven {
        let _ = writeln!(s, "    {0}_init(state_{0}, image, params, latch);", m.name);
    }
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_tick`: ein Tick, von der Schleife gerufen.
fn tick(s: &mut String, _p: &Program, driven: &[&takt_mir::machine::Machine]) {
    let _ = writeln!(s, "/* Ein Tick (12.1, Schritte 2 bis 10). */");
    let _ = writeln!(s, "void takt_mcu_tick(long long k) {{");
    let _ = writeln!(s, "    g_tick = k;");
    let _ = writeln!(s, "    takt_fn_fault = 0;");
    for m in driven {
        // Die Periode: Eine Maschine mit `n_m > 1` laeuft nur jeden
        // n-ten Tick (7.2, Zaehler-Scheduling).
        if m.period > 1 {
            let _ = writeln!(s, "    if (k % {} == 0)", m.period);
            let _ = writeln!(s, "        {0}_step(state_{0}, image, params, latch);", m.name);
        } else {
            let _ = writeln!(s, "    {0}_step(state_{0}, image, params, latch);", m.name);
        }
    }
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_dump`: den Latch ausgeben, fuer den Vergleich.
///
/// **Dieselbe Form wie der Linux-Rahmen**, und das ist der Punkt: Ohne
/// `t=<tick>` laesst sich keine Zeile zuordnen, und der Vergleich mit
/// `takt sim` — der Kern des M5-Exits — waere nicht moeglich. Eine erste
/// Fassung schrieb `out led 1` ohne Tick; damit war der Hardwarelauf
/// nicht gegen den Interpreter zu halten, und das fiel nicht auf, weil
/// niemand es versucht hat.
///
/// [`crate::run::compare`] liest genau diese Form.
fn telemetry(s: &mut String, p: &Program, layout: &Layout) {
    let _ = writeln!(s, "/* Die Ausgaenge als Trace-Zeilen (grammar/trace.md). */");
    let _ = writeln!(s, "void takt_mcu_dump(void) {{");
    for slot in &layout.outputs {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    takt_board_trace(\"t=\");");
        let _ = writeln!(s, "    takt_board_trace_i64(g_tick);");
        let _ = writeln!(s, "    takt_board_trace(\"out {} \");", slot.name);
        // Ein Enum mit seinem Variantennamen, nicht mit der Diskriminante:
        // Der Interpreter schreibt den Namen (9.3), und `same_number`
        // gliche `CLOSED` gegen `0` nicht aus — das ist keine Zahl.
        if let Some(varianten) = enum_variants(p, &slot.name) {
            let _ = writeln!(s, "    switch (*({ct} *)(latch + {})) {{", slot.offset);
            for (d, name) in varianten {
                let _ = writeln!(s, "    case {d}: takt_board_trace(\"{name}\"); break;");
            }
            let _ = writeln!(s, "    default: takt_board_trace(\"?\");");
            let _ = writeln!(s, "    }}");
        } else {
            // Zahlen als `i64`, auch `bool` und vorzeichenlose: Der
            // Vergleich normalisiert `true`/`false` gegen 1/0 und prueft
            // sonst den Zahlenwert.
            let _ = writeln!(s, "    takt_board_trace_i64((long long)*({ct} *)(latch + {}));", slot.offset);
        }
        let _ = writeln!(s, "    takt_board_trace(\"\\n\");");
    }
    let _ = writeln!(s, "}}\n");

    commit(s, layout);
    outputs(s, layout);
}

/// Die Varianten eines Enum-Ausgangs mit ihren Diskriminanten.
///
/// Dieselbe Abfrage wie im Linux-Rahmen: Beide muessen den Namen
/// schreiben, den der Interpreter schreibt (9.3), sonst vergliche der
/// Exit-Test Schreibweisen statt Werte.
fn enum_variants(p: &Program, name: &str) -> Option<Vec<(i64, String)>> {
    let c = p.channels.iter().find(|c| c.name == name)?;
    let takt_mir::types::Type::Enum(e) = p.types.list.get(c.ty.index())? else { return None };
    let def = p.enums.get(e.index())?;
    Some(def.variants.iter().map(|v| (v.discriminant, v.name.clone())).collect())
}

/// `takt_mcu_commit`: den Latch an die Treiber geben (12.1).
///
/// **Der Pfad aus `@ hw(...)` wird zum Symbolnamen.** 8.10 verlangt, dass
/// die Abbildung auf Geraete ausserhalb des Programms steht — „gehoeren in
/// die Hardware-Konfiguration, nicht in die Steuerlogik" —, und nennt den
/// Pfad einen symbolischen Verweis dorthin. Der Rahmen loest ihn nicht
/// auf: Er erzeugt aus `hw("ui/led")` einen Aufruf von
/// `takt_out_ui_led(...)` und ueberlaesst die Peripherie dem, der sie
/// besitzt (9.5 fuehrt Treiber in der TCB, den Rahmen nicht).
///
/// **Warum ein Symbol und keine Tabelle.** Eine Registrierung zur Laufzeit
/// waere flexibler, aber ein nicht eingetragener Ausgang fiele still aus —
/// dieselbe Fehlerklasse, die FB-137 und FB-139 gekostet haben. Als Symbol
/// prueft der Linker die Vollstaendigkeit: Wer einen Ausgang bindet und
/// keinen Treiber stellt, bekommt einen Linkfehler mit dem Namen darin,
/// und zwar bevor etwas laeuft.
///
/// Ausgaenge mit `sim(...)` oder ohne Bindung bekommen keinen Aufruf: Zu
/// ihnen gehoert kein Geraet. Der Latch bleibt trotzdem lesbar, dafuer ist
/// [`outputs`] da.
fn commit(s: &mut String, layout: &Layout) {
    let bound: Vec<(&crate::layout::Slot, String)> = layout
        .outputs
        .iter()
        .filter_map(|slot| slot.address.as_ref().map(|a| (slot, format!("takt_out_{}", a.ident()))))
        .collect();

    let _ = writeln!(s, "/* Die Treiber, die das Board stellt (8.10, 12.1). */");
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "void {fname}({ct} value); /* {} */", slot.name);
    }
    if bound.is_empty() {
        let _ = writeln!(s, "/*   keine — kein Ausgang ist an Hardware gebunden */");
    }

    let _ = writeln!(s, "\n/* Schritt 10: der Latch geht an die Geraete (12.1). */");
    let _ = writeln!(s, "void takt_mcu_commit(void) {{");
    for (slot, fname) in &bound {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    {fname}(*({ct} *)(latch + {}));", slot.offset);
    }
    let _ = writeln!(s, "}}\n");
}

/// `takt_mcu_output`: einen Ausgang lesen, nach Stellung.
///
/// **Fuer Diagnose, nicht fuer Treiber.** Das Stellen macht
/// [`commit`] ueber benannte Symbole; diese Funktion ist der Weg, einen
/// Latch-Wert anzusehen, ohne ihn zu stellen — der Bring-up nutzt sie,
/// bevor ein Treiber existiert, und ein Testrahmen, der den Latch prueft.
///
/// Der Index ist die Stellung in `layout.outputs`; die Zuordnung steht im
/// Kopf des erzeugten Textes, damit sie nachlesbar ist.
fn outputs(s: &mut String, layout: &Layout) {
    let _ = writeln!(s, "/* Die Ausgaenge nach Stellung, fuer Diagnose. */");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let _ = writeln!(s, "/*   {i} = {} */", slot.name);
    }
    let _ = writeln!(s, "long long takt_mcu_output(int index) {{");
    let _ = writeln!(s, "    switch (index) {{");
    for (i, slot) in layout.outputs.iter().enumerate() {
        let Some(ct) = c_type(&slot.ty, slot.signed) else { continue };
        let _ = writeln!(s, "    case {i}: return (long long)*({ct} *)(latch + {});", slot.offset);
    }
    // Ein unbekannter Index ist kein Absturz: Der Aufrufer bekommt eine
    // Null und der Lauf geht weiter. 4.1 verlangt Totalitaet, und ein
    // Treiber, der nach einem entfallenen Ausgang fragt, ist ein
    // Uebersetzungsfehler — keiner, der zur Laufzeit stehen bleiben darf.
    let _ = writeln!(s, "    default: return 0;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}}");
}
