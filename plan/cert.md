# Zertifizierungspfad (Referenz 13.4)

Stand 2026-09-28, M10 Schritt 24. Dieses Dokument sammelt, was ein
Gutachter nach IEC 61508, IEC 61513 oder DO-178C über Takt wissen muss:
welche Eigenschaften die Sprache durch Konstruktion hat und welche
Prüfung sie erzwingt, wo die Trusted Computing Base liegt, wie
Anforderungen zu Prüfstellen zurückführen und welche Messungen die
Zeitzusagen belegen. Es behauptet nichts, was nicht ein Test hält; wo
ein Test fehlt, steht es als offen da.

## 1. Teilmenge im Sinne von MISRA und JPL Power of Ten

Jede Eigenschaft ist keine Konvention, sondern eine Prüfung des Compilers
(Referenz 10) oder eine Eigenschaft der Sprache, die kein Programm
umgehen kann.

| Eigenschaft | Durchgesetzt durch | Beleg |
|---|---|---|
| Keine Rekursion | Prüfung 11: Aufrufgraph azyklisch; Prüfung 52: Instanziierungsgraph der Generics azyklisch | `corpus-try/checks/SC-11/`, `SC-52/` |
| Beschränkte Schleifen | Prüfung 11: `for`-Schranken sind Konstanten; Ströme iterieren über ein Fenster fester Kapazität (9.6, Lemma 9.6.1) | `checks/SC-11/`, `takt-sema/tests/range_boundaries.rs` |
| Keine dynamische Allokation | Die Sprache hat keinen Heap: Sammlungen haben feste Kapazität (3.9), Ströme `CAP` und `CAPB` (8.6); `takt-rt-core` ist `no_std` ohne `alloc`; der MCU-Rahmen ruft weder `malloc` noch `printf` | `takt-conformance/tests/mcu_harness.rs` (`the_harness_is_freestanding`) |
| Keine Exceptions, kein undefinierter Zustand | Jede Operation ist total (4.1): Überlauf, Division durch null, Range und Index haben einen Fault-Pfad mit definiertem Ziel (5.3); Prüfung 9: Fault-Wald azyklisch, `FAULTED` erreichbar | Sätze 9.4.x als Tests, Differential Interpreter ≡ Codegen |
| Keine Zeiger, keine Funktionszeiger | Die Sprache hat Wertsemantik ohne Referenzen (3.9); `inout` ist ein Zeiger, den Prüfung 47 gegen Aliasing hält | `checks/SC-47/` |
| Rückgabewerte werden geprüft | `T?` und `T!E` erzwingen `.valid`/`.or` (Prüfung 45) | `checks/SC-45/` |
| Jede Prüfung zur Übersetzungszeit bewiesen oder gezählt | Intervallanalyse (3.4); im Zertifizierungsmodus (`--certification`) ist jede unbewiesene implizite Prüfung ein Fehler (Prüfung 24) | `takt-sema/tests/analysis.rs` |
| Kein `unsafe` im Werkzeug | `unsafe_code = "forbid"` für jedes Crate des Workspace; die Ausnahmen stehen in Abschnitt 2 | `takt-conformance/tests/tcb_manifest.rs` |

## 2. Trusted Computing Base (9.5)

Die Sätze aus Abschnitt 9 sagen, was ein *Programm* garantiert. Sie
decken nicht ab: Compiler und LLVM, `libtaktm`, native Funktionen, die
Runtime, Treiber, Betriebssystem und Hardware. Diese Teile sind die TCB;
ihre Grenze muss sichtbar sein, nicht verwischt.

**Die Grenze im Code.** Jedes Crate des Workspace erbt
`unsafe_code = "forbid"`, und `forbid` lässt sich nicht lokal aufheben.
Was `unsafe` braucht — Registerzugriff und die C-ABI des erzeugten
Rahmens —, liegt darum in eigenen Workspaces außerhalb, jedes mit
`#![allow(unsafe_code, reason = …)]` am Kopf und einem `SAFETY`-Kommentar
an jedem Zugriff. Die Tabelle ist das Manifest; `tcb_manifest.rs` hält sie
gegen die Verzeichnisse unter `crates/`: kein Crate außerhalb des
Workspace ohne Zeile, keine Zeile ohne Crate.

### Crates mit `unsafe`

| Crate | Grund |
|---|---|
| takt-board-esp32c6 | Register des ESP32-C6 (SYSTIMER, USB-Serial-JTAG, GPIO, MWDT), CSR-Zugriffe, `wfi` |
| takt-board-stm32f401 | Register des STM32F401 (TIM2, USART1, IWDG, GPIO, Flash), DWT, `wfi` |
| takt-native-abi | die C-Einstiege der kuratierten Natives (`takt_native_*`), die der erzeugte Code auf Wirt und Board ruft; gerechnet wird in `takt-native` und `takt-crypto` (4.5, FB-293) |
| takt-bringup-esp32c6 | die Hülle der Lieferform (`app.rs`) hinter der C-ABI des Programms und seines Rahmens (12.11), der erzeugte Kleber der Treiber (`app_in_*`, `app_out_*`, …) zum Prüfstand, Geräte über statische Peripherie, `takt_board_trace*` hinter der C-ABI |
| takt-trampoline-stm32f401 | das Sprungbrett in Sektor 1 der Black Pill (FB-385): liest die Vektortabelle der Anwendung in Sektor 4 und springt hinein, ohne sie in den DFU-Bootloader des ROM (`takt-board-stm32f401::bootloader`) |
| takt-bringup-stm32f401 | dasselbe für die Black Pill; im Profil `shared` dazu RTIC 2 und `rtic-sync` — das RTOS gehört dort zur TCB (12.8) |
| takt-bringup-host | die Schleife auf dem Wirt für `takt driver-test --crate` (13.8): `takt_board_trace*` hinter der C-ABI, die Leitung als statische Konsole; Lieferform, Hülle und Kleber zum Prüfstand bindet das Programm, das `takt-conformance::board::host` erzeugt |
| takt-embed | Takt als Baustein in Rust (12.11): in der Testhilfe `takt_board_trace*` hinter der C-ABI, die Leitung je Faden; die Geräte-Traits und der Bauhelfer stehen unter `deny(unsafe_code)` |
| takt-embed-c | Takt als Baustein in C (12.11, M11 Schritt 18a): der Kern ohne Warten hinter der C-ABI (`takt_core_*`), die Tabelle `P_ops` und das Gerät des Journals als Rückrufe des Wirts, der Kern im Speicher des Wirts (`takt_core`); dazu der Panic-Handler des Archivs (`abort`) |

**Rechnen außerhalb der TCB.** Was in einem Board-Crate keine
Registerarbeit ist — Perioden in Timer-Schritte, Zyklen in Nanosekunden,
die Messschleife von `driver-test` —, steht in `takt-board-support`,
im Workspace, unter `forbid` und mit Tests auf dem Wirt.

### Native Funktionen

Die kuratierte Menge (4.5, 13.8). Jede ist bitgleich über alle Ziele,
panikfrei unter Fuzzing und mit gemessenem Stack gegen ihren Vertrag
geprüft; Projekt-Natives außerhalb dieser Menge verlangen
`tcb_policy = reviewed(…)` und eine Zeile in `natives.review` (4.5).

| Native | Crate | Abhängigkeit |
|---|---|---|
| crc32 | takt-native | keine |
| crc32c | takt-native | keine |
| crc16 | takt-native | keine |
| sum8 | takt-native | keine |
| sha256 | takt-native | keine |
| hmac_sha256 | takt-native | keine |
| sha256_init | takt-native | keine |
| sha256_update | takt-native | keine |
| sha256_final | takt-native | keine |
| ecdsa_p256_verify | takt-crypto | p256 0.13 (RustCrypto), Feature `ecdsa` |
| fft256 | takt-native | keine |
| rsa3072_verify | takt-crypto | crypto-bigint 0.5 (RustCrypto), Feature `rsa`; SHA-256 aus takt-native |
| aes_gcm_decrypt | takt-crypto | aes-gcm 0.10 (RustCrypto), Feature `aes-gcm` |

Der Lauf-Header nennt, was ein Lauf davon benutzt hat (12.5,
`takt-interp/src/record.rs`).

Eine Implementierung je Native: Interpreter und Linux-Runtime rufen
`takt-native` direkt, der erzeugte Code dieselbe Rechnung über die
Einstiege aus `takt-native-abi` — der Rahmen bringt keine eigenen mit.
Gemessen wird der Stack am Einstieg (FB-293).

Die Jobs aus `takt-crypto` laufen auch auf den Boards: Die Bring-ups
binden `takt-native-abi` mit allen drei Features, und das Messprogramm
`natives` misst ihren Stack wie den der uebrigen (M10 Schritt 21). Panikfrei
unter Fuzzing heisst hier: Eingaben gueltiger und falscher Laengen mit
festem Startwert in `takt-native/tests/no_panic.rs` und
`takt-crypto/tests/vectors.rs`, bei RSA mit einem Modulus, der die Potenz
und die PSS-Pruefung erreicht.

## 3. Rückverfolgung (13.4)

- Jede Prüfstelle (`check`, `expect`, `alert`, `verify`) und jede
  Transition trägt eine stabile ID; `req "…"` verbindet sie mit einer
  Anforderung.
- `takt check --report` schließt mit dem Abschnitt „Anforderungen": je ID
  ihre Stellen mit Art, Datei, Zeile, Maschine und Block. `takt test`
  ergänzt je Stelle die Szenarien, die sie durchliefen.
- Davor steht das Gate: je Prüfung, die Zahlen des Ziels braucht (12, 28,
  29, 32, 39, 59, 60), ob sie urteilt — „ok", „verletzt", „nicht
  entscheidbar" mit der fehlenden Zahl oder „ohne Belang". Eine fehlende
  Messung ist dort sichtbar und nie eine stille Annahme.
- `takt test` berichtet die Coverage je Szenario-Lauf und nennt jede nicht
  erreichte Stelle mit Zeile (13.2); `takt driver-test` verlangt, dass jede
  Reaktion einer `driver machine` ausgelöst wurde (13.8).

## 4. Zeitzusagen und Messungen

Die Zeitzusage „der Tick hält" ist eine Rechnung — Operationen je
Aktivierung (9.4.3) mal kalibrierte Kosten je Operationsklasse — und die
Kalibrierung ist eine Messung, kein Datenblattwert.

| Messung | Werkzeug | Ablage |
|---|---|---|
| Kosten je Operationsklasse, `T_IO`, Tick-Jitter, Stack-Reserve | `takt bench --board NAME` | `corpus-try/hw/<board>.hw`, Bericht `corpus-try/hw/<board>.report` (`grammar/conformance-report.md`) |
| `guard`, `jitter` und Abtastlatenz über eine Drahtbrücke | `takt driver-test --board NAME` | Kanäle `gpio/loop_out`, `gpio/loop_in` der Konfiguration |
| Bitgleichheit und Stack der Natives | Board-Suite `the_natives_agree_with_the_host` | Testausgabe |
| Semantik auf dem Ziel | Board-Suiten, Korpus gegen den Interpreter | `takt-conformance/tests/board_*.rs` |

Die Konfiguration trägt das Ergebnis, der Bericht die Umstände (Datum,
Board, Wiederholungen, Streuung); `takt bench` schreibt beide in einem
Lauf (13.8).

## 5. Qualifizierung des Werkzeugs

Der Interpreter ist die ausführbare Semantik (Abschnitt 9). Jede
Compiler-Version wird gegen ihn gehalten: der Korpus mit Stimuli auf dem
Wirt und auf beiden Boards, Golden-Traces und die Fuzzer über Grammatik
und Maschinen (`fuzz.rs`, `fuzz_machines.rs`); eine Abweichung ist ein
Fehler des Codegens, nie der Semantik (Satz 9.4.4). Die Mutationstests der
Fault-Pfade, die 13.1 außerdem nennt, stehen noch aus. Ein qualifizierbarer Codegen nach
dem Vorbild von SCADE ist eine spätere Investition; die Architektur —
kleine MIR, Referenzinterpreter, differentielles Testen — ist darauf
ausgelegt. Versionierte Formate (MIR, Trace, Konfiguration, Bericht)
bleiben lesbar; für die MIR liegt je Version die Datei ihres Compilers
als Golden bei (`takt-sema/tests/mir-golden`).

## 6. Regelwerk des eigenen Codes (M11 Schritt 33)

Abschnitt 1 hält die Teilmenge für Takt-*Programme*. Dieser Abschnitt hält
den Rust- und C-Code, der sie übersetzt und ausführt. MISRA gibt für Rust
kein eigenes Regelwerk heraus; die Grundlage ist **MISRA C:2025
Addendum 6** (März 2025), das jede Regel von MISRA C:2025 auf Rust
abbildet. Der Katalog führt jede Regel, die dort für Rust gilt (Kategorie
Required oder Advisory): 77 auch in sicherem Rust, also für jedes
eigene Crate, und 47 nur für die Crates mit `unsafe` aus
Abschnitt 2. Die Kennung hinter der Regel ist die Kategorie für Rust (R,
A). Fünf Regeln stuft das Addendum als „Disapplied“ ein und verlangt nichts
(R.15.5, R.17.8, R.22.8, R.22.9, R.22.10); die übrigen gelten für Rust
nicht.

Die Regeltexte gehören MISRA. Der Gegenstand jeder Zeile ist eine eigene
Kurzfassung, kein Zitat. R.5.10, R.11.11, R.19.3 und D.4.15 sind in
MISRA C:2025 neu; ihre Kurzfassung ist nicht gegen den Normtext geprüft
(TODO, sobald MISRA C:2025 vorliegt).

**Stufen.** Was ein Lint hält, steht als `deny` am Crate. Cargo erlaubt
neben `lints.workspace = true` keine eigenen Einträge, und `[lints]` gälte
auch für Tests, in denen ein Panic das Ergebnis ist. Beide Stufen stehen
darum als `#![cfg_attr(not(test), deny(…))]` am Kopf jedes Crates ihrer
Liste, bei Programmen unter `src/bin` am Kopf jedes Programms.

| Stufe | Crates | dazu |
|---|---|---|
| 1 | jedes eigene Crate | rustc `deprecated`, `unreachable_code`, `unused_must_use`, `unused_assignments`, `unused_mut`, `unfulfilled_lint_expectations`; Clippy `unwrap_used`, `todo`, `unimplemented`, `dbg_macro`, `exit` |
| 2 | was auf dem Ziel läuft oder ins Abbild gelinkt wird: `takt-rt-core`, `takt-rt-baremetal`, `takt-rt-linux`, `takt-native`, `takt-hal` (der Kern ohne `mir`), `takt-crypto`, `libtaktm`, `takt-board-*`, `takt-bringup-*`, `takt-trampoline-stm32f401`, `takt-native-abi`, `takt-embed`, `takt-embed-c` | Clippy `panic`, `expect_used`, `arithmetic_side_effects`, `indexing_slicing`, `as_conversions`, `fn_to_numeric_cast_any`, `shadow_unrelated`, `else_if_without_else`; rustc `unreachable_pub`; nur `Ordering::SeqCst` |
| 2 mit `unsafe` | die Crates aus Abschnitt 2 | Clippy `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`; rustc `unsafe_op_in_unsafe_fn` |

**Abweichungen** stehen am Ort als
`#[expect(lint, reason = "MISRA C:2025 D.4.1: …")]`: Der Grund beginnt mit
der Regel aus diesem Katalog. `expect` statt `allow`, und
`unfulfilled_lint_expectations` ist `deny` — fällt der Grund weg, scheitert
der Bau. Eine Abweichung im Ganzen steht in der Tabelle selbst.
`tests/rule_catalog.rs` hält, dass jedes Crate die Lints seiner Stufe trägt
und jede Abweichung eine Regel des Katalogs nennt. „Lücke“ heißt: weder
Lint noch Test, nur das Review.

| Regeln | gilt für | Gegenstand | gehalten durch | Beleg |
|---|---|---|---|---|
| D.1.1 (R) | alle | Verhalten, das vom Ziel abhängt, ist bekannt und dokumentiert | Referenz 4.2: bitgleiche Ergebnisse auf allen Zielen; Wirt und beide Boards gegen den Interpreter | `tests/differential.rs`, `tests/board_*.rs` |
| D.1.2 (R) | alle | Spracherweiterungen sind dokumentiert; in Rust die instabilen Features | Prüfung: kein `#![feature(…)]` in einem eigenen Crate | `tests/rule_catalog.rs` |
| D.2.1 (R), R.1.1 (R) | alle | Übersetzen ohne Fehler, ohne Verstoß gegen Syntax und Einschränkungen der Sprache | rustc; die Suite baut jedes Crate | Suite |
| D.3.1 (R) | alle | Code ist auf dokumentierte Anforderungen rückführbar | Fähigkeitsmatrix: jeder Absatz der Referenz hat eine Zeile, jede fertige Zeile nennt einen Test | `tests/capabilities.rs` |
| D.4.1 (R), D.4.11 (A) | alle | Laufzeitfehler minimieren, in Rust meist Panics; Werte an Bibliotheksfunktionen prüfen | Stufe 1: `unwrap_used`, `todo`, `unimplemented`, `exit`; Stufe 2 zusätzlich `panic`, `expect_used`, `arithmetic_side_effects`, `indexing_slicing`. Im Werkzeug ist ein `panic!` mit Meldung so dokumentiert wie ein `expect`: Es liefert keinen Code statt eines falschen | `tests/rule_catalog.rs` |
| D.4.4 (A) | alle | kein auskommentierter Code | Review; kein Lint (Lücke) | — |
| D.4.5 (A) | alle | Bezeichner im selben Namensraum sind typografisch eindeutig | Review; `similar_names` ist pedantisch und nicht gesetzt (Lücke) | — |
| D.4.7 (A), R.17.7 (R) | alle | Rückgabewerte, besonders Fehler, werden ausgewertet | Stufe 1: rustc `unused_must_use` (`Result` und `#[must_use]`); ein Wert wird nie stillschweigend verworfen. `let _ = …` ist das ausdrückliche Verwerfen, das Gegenstück zu `(void)` in MISRA C; `let_underscore_must_use` ist darum nicht gesetzt | `tests/rule_catalog.rs` |
| D.4.9 (A) | alle | Funktion statt funktionsartigem Makro | Review: Makros nur, wo eine Funktion es nicht kann (Lücke) | — |
| D.4.12 (A) | alle | keine dynamische Allokation | Prüfung: kein Crate der Stufe 2 mit `no_std` bindet `alloc`. Abweichung im Ganzen: Die Werkzeug-Crates allozieren; sie laufen auf dem Wirt, nie im Abbild | `tests/rule_catalog.rs` |
| D.4.13 (R), R.22.13 (R), R.22.14 (R), R.22.15 (R), R.22.20 (R) | alle | Ressourcen, Fäden und Synchronisationsobjekte in der richtigen Reihenfolge anlegen, benutzen und freigeben | Sprache: Besitz, Lebensdauern und `Drop` | — |
| D.4.14 (R) | alle | Werte von außen werden geprüft | Referenz 12.6: Inputs degradieren (Qualität, `.valid`); das Journal prüft jeden Satz | `takt-hal/tests/edge.rs`, `takt-rt-core/tests/journal.rs` |
| D.4.15 (R) | alle | Gleitkomma erzeugt Unendlich und NaN nicht unbemerkt | Referenz 4.2 und 5.3: IEEE-754 mit Fault-Pfaden für Takt-Programme; Interpreter und erzeugter Code bitgleich (Satz 9.4.4) | Suite `ieee` aus `corpus-try/manifest.csv` |
| D.5.1 (R), D.5.2 (R), R.22.18 (R), R.22.19 (R) | alle | keine Datenrennen und keine Verklemmungen zwischen Fäden; Mutexe nicht rekursiv sperren; eine Bedingungsvariable gehört zu genau einem Mutex | Sprache für Datenrennen (`Send`, `Sync`, Ausleihregeln); Verklemmung und Sperren: Review | — |
| D.5.3 (R) | alle | keine dynamische Erzeugung von Fäden | Review: Die Runtime erzeugt keine Fäden; `takt-rt-linux` startet seine beim Anlauf | — |
| R.1.3 (R) | alle | kein undefiniertes oder kritisch unspezifiziertes Verhalten | Sprache: sicheres Rust; `forbid(unsafe_code)` im Workspace; die Crates mit `unsafe` nach Abschnitt 2 mit `SAFETY` je Block und Miri (M11 Schritt 33c) | `tests/tcb_manifest.rs` |
| R.1.5 (R) | alle | keine veralteten Sprachmittel und Schnittstellen | Stufe 1: rustc `deprecated` | `tests/rule_catalog.rs` |
| R.2.1 (A) | alle | kein unerreichbarer Code | Stufe 1: rustc `unreachable_code` | `tests/rule_catalog.rs` |
| R.2.2 (R), R.2.3 (A), R.2.5 (A), R.2.6 (A), R.2.7 (A), R.2.8 (A) | alle | kein toter Code; keine unbenutzten Typen, Makros, Marken, Parameter oder Objekte | rustc `dead_code`, `unused_macros`, `unused_labels`, `unused_variables`, Stufe 1 `unused_assignments`; Clippy `no_effect`, `unnecessary_operation` | `tests/rule_catalog.rs` |
| R.3.1 (A) | alle | keine Kommentarzeichen innerhalb eines Kommentars | Review; Blockkommentare schachteln in Rust richtig (Lücke) | — |
| R.5.1 (R), R.5.8 (A) | alle | externe Bezeichner sind eindeutig | Präfix für jedes exportierte Symbol (12.11); ein doppeltes Symbol bricht der Linker | `tests/embed.rs` |
| R.5.2 (R), R.5.6 (A), R.5.9 (A) | alle | Bezeichner im selben Bereich, Typnamen und interne Bezeichner sind eindeutig | rustc: Die Namensauflösung lehnt Doppelte ab; Module trennen die Namensräume | — |
| R.5.3 (A) | alle | ein innerer Bezeichner verdeckt keinen äußeren | Stufe 2: `shadow_unrelated`. Abweichung im Ganzen: Die Neubindung desselben Werts (`let x = x.trim();`, `shadow_reuse`, `shadow_same`) ist Rust-Idiom und verdeckt keine andere Bedeutung | `tests/rule_catalog.rs` |
| R.5.5 (A) | alle | Bezeichner unterscheiden sich von Makronamen | Sprache: Makros tragen `!` | — |
| R.5.10 (A) | alle | keine reservierten Bezeichner deklarieren | rustc: Schlüsselwörter nur als `r#`; Review | — |
| R.7.1 (A) | alle | keine Oktalkonstanten | Clippy `zero_prefixed_literal` (in `clippy::all`); oktal schreibt Rust `0o` | Clippy ohne Warnung |
| R.7.2 (A) | alle | Konstanten ohne Vorzeichen sind gekennzeichnet | rustc `overflowing_literals` (Fehler); ein Literal trägt seinen Typ | — |
| R.8.7 (A) | alle | nichts sichtbar außerhalb, was nur ein Modul braucht | Stufe 2: rustc `unreachable_pub` | `tests/rule_catalog.rs` |
| R.8.9 (A) | alle | ein Objekt im Block, wenn nur eine Funktion es braucht | Review (Lücke) | — |
| R.8.13 (A) | alle | `&mut` nur, wo geschrieben wird | Stufe 1: rustc `unused_mut`; Parameter im Review (Lücke) | `tests/rule_catalog.rs` |
| R.8.17 (A) | alle | höchstens eine Angabe der Ausrichtung je Objekt | Review: `#[repr(align)]` nur an Typen | — |
| R.9.1 (R), R.9.4 (R) | alle | nichts lesen, bevor es gesetzt ist; kein Element doppelt initialisieren | rustc: Initialisierungsanalyse | — |
| R.10.5 (A), R.10.8 (A), R.11.1 (R), R.11.3 (R), R.11.4 (R), R.11.8 (R) | alle | Umwandlungen in ungeeignete Typen, zwischen Zeigern, zwischen Zeigern und Zahlen, ohne `const` | Stufe 2: `as_conversions`, `fn_to_numeric_cast_any`; `From` und `TryFrom` statt `as`. Im Werkzeug Review | `tests/rule_catalog.rs` |
| R.11.11 (R) | alle | Zeiger nicht implizit gegen null vergleichen | rustc: Rust vergleicht nie implizit | — |
| R.12.1 (A) | alle | der Vorrang der Operatoren ist ausdrücklich | Clippy `precedence` (in `clippy::all`) | Clippy ohne Warnung |
| R.12.2 (A) | alle | die Schiebeweite liegt im Bereich des Typs | rustc `arithmetic_overflow` für konstante Weiten. Variable Weiten hält kein Lint — Clippy überlässt Schiebeoperationen der Konstantenauswertung (Lücke): im Review, in Stufe 2 durch Masken wie `% 64` oder feste Grenzen | — |
| R.13.1 (A) | alle | Initialisierer ohne bleibende Seiteneffekte | Sprache: Rust wertet strikt von links nach rechts aus | — |
| R.13.5 (A) | alle | die rechte Seite von `&&` und `||` ohne bleibende Seiteneffekte | Review (Lücke) | — |
| R.14.1 (R) | alle | kein Schleifenzähler in Gleitkomma | Review; `while_float` steht nur in Clippys `nursery` (Lücke) | — |
| R.14.3 (R) | alle | keine unveränderlichen Bedingungen | rustc `while_true`, Clippy `absurd_extreme_comparisons` (in `clippy::all`); der Rest im Review | Clippy ohne Warnung |
| R.14.4 (R) | alle | eine Bedingung ist boolesch | rustc | — |
| R.15.4 (A) | alle | höchstens ein `break` je Schleife | Review (Lücke) | — |
| R.15.7 (A) | alle | `if … else if` endet mit `else` | Stufe 2: `else_if_without_else` | `tests/rule_catalog.rs` |
| R.17.2 (R) | alle | keine Rekursion | Lücke: kein Lint über Crate-Grenzen. Für die Runtime begrenzt die gemessene Stackschranke je Board den Schaden (M11 Schritt 13). Abweichung: `takt_native::bytes` (`check`, `skip`, `repeat`) steigt rekursiv über die Gestalt eines Werts, höchstens `DEPTH` = 32 Ebenen; den Stack je Native misst `takt bench` auf beiden Boards | `tests/board_*.rs` (Stacks), `.hw` je Board (`native`-Zeilen) |
| R.17.11 (A) | alle | eine Funktion, die nie zurückkehrt, ist als `-> !` deklariert | Review (Lücke) | — |
| R.18.3 (R), R.18.5 (A) | alle | Zeigervergleiche nur im selben Objekt; höchstens zwei Ebenen Zeiger | Review; im Workspace Referenzen statt Zeiger | — |
| R.19.2 (A), R.19.3 (R) | alle | keine `union`; kein Lesen eines ungesetzten Glieds | Sprache: eine `union` liest nur `unsafe`; Prüfung: kein `union` in einem eigenen Crate | `tests/rule_catalog.rs` |
| R.20.4 (R), R.20.7 (A) | alle | Makronamen sind keine Schlüsselwörter; Makroparameter sind geklammert | rustc; `macro_rules!` bindet Ausdrücke als Ganzes; Prüfung: kein prozedurales Makro im eigenen Code | `tests/rule_catalog.rs` |
| R.21.25 (R) | alle | Synchronisation des Speichers sequentiell konsistent | Prüfung: Stufe 2 nutzt nur `Ordering::SeqCst` | `tests/rule_catalog.rs` |
| R.22.11 (R) | alle | einen Faden höchstens einmal verbinden oder lösen | Sprache: `JoinHandle::join` verbraucht den Griff | — |
| D.4.2 (R), D.4.3 (A) | `unsafe` | Assembler ist dokumentiert und gekapselt | Review: `asm!` nur in den Board-Crates, je Block mit `SAFETY` | Abschnitt 2 |
| R.8.3 (R), R.8.5 (A), R.8.6 (R), R.8.15 (R) | `unsafe` | Deklarationen über `extern "C"` passen zur C-Seite, stehen einmal; ein Symbol hat genau eine Definition | Header und Rust-Modul aus einer Quelle (`takt-frame`), `_Static_assert` je Feld; der Linker | `takt-frame` (Einheitentests), `takt-embed-c` (`the_header_lays_out_what_the_archive_reads`) |
| R.9.7 (R) | `unsafe` | atomare Objekte sind vor dem Zugriff initialisiert | Sprache: ein Atomic ist ein initialisierter Wert; Bereiche des Linkers im Review | — |
| R.11.2 (R), R.11.5 (R), R.11.6 (A) | `unsafe` | Zeiger auf unvollständige Typen und auf `void` | Review mit `SAFETY` (die Arena als `void *` in `takt-embed-c`); Miri (M11 Schritt 33c) | — |
| R.12.4 (A) | `unsafe` | Konstantenausdrücke laufen nicht über | rustc `arithmetic_overflow` (Fehler) | — |
| R.17.9 (R) | `unsafe` | eine Funktion mit `!` kehrt nicht zurück | rustc | — |
| R.18.1 (R), R.18.2 (R), R.18.4 (A), R.18.6 (R), R.19.1 (R) | `unsafe` | Zeigerarithmetik bleibt im Objekt; keine Adresse eines Lokalen nach außen; keine überlappende Kopie | Stufe 2 mit `unsafe`: `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`; Miri (M11 Schritt 33c) | `tests/rule_catalog.rs` |
| R.21.3 (R), R.21.4 (R), R.21.5 (R), R.21.6 (R), R.21.7 (R), R.21.9 (R), R.21.10 (R), R.21.12 (R), R.21.13 (R), R.21.14 (R), R.21.15 (R), R.21.16 (R), R.21.17 (R), R.21.18 (R), R.21.19 (R), R.21.20 (R), R.21.21 (R), R.21.24 (R), R.21.26 (R), R.22.1 (R), R.22.2 (R), R.22.3 (R), R.22.4 (R), R.22.5 (R), R.22.6 (R), R.22.7 (R), R.22.12 (R), R.22.16 (R), R.22.17 (R) | `unsafe` | Funktionen der C-Bibliothek: Speicher, Zeichenketten, Ein- und Ausgabe, Signale, Umgebung, Fäden | Prüfung: Die `extern`-Blöcke binden nur Symbole des eigenen Rahmens und des Linkers, aus der C-Bibliothek nur `abort` (R.21.8) | `tests/rule_catalog.rs` |
| R.21.8 (R) | `unsafe` | kein `abort` und kein `exit` | Abweichung: Die Panic-Handler von `takt-native-abi` und `takt-embed-c` enden mit `abort` des Wirts. Stufe 2 schließt Panics aus (D.4.1); der Handler ist der letzte Halt | Abschnitt 2 |

**MISRust.** Die Zuordnung von MISRA C++ 2023 zu Rust (Molz u. a.,
SAFECOMP 2026; Tabelle `misra_cpp_rust_comparison_rules.csv`, Stand
`dee238c`, CC-BY-4.0) teilt 110 Regeln als in Rust entfallend ein (C1 bis
C3) und 36 als nur mit `unsafe` geltend (C4); diese hält der Katalog über
die Spalte „`unsafe`“. Die 22 Regeln, die auch in sicherem Rust gelten (C6),
und die 11, die eine Anpassung brauchen (C5), stehen hier mit ihrer Zeile
im Katalog:

| MISRA C++ 2023 | Klasse | im Katalog |
|---|---|---|
| 0.0.2 | C6 | R.14.3 |
| 0.1.1 | C6 | R.2.2 |
| 0.1.2 | C6 | R.17.7 |
| 0.3.1 | C6 | D.4.15 |
| 0.3.2 | C6 | D.4.11 |
| 4.1.1 | C5 | R.1.1; Rust hat keinen Standard, die Edition 2024 ist festgelegt |
| 4.1.2 | C6 | R.1.5 |
| 5.7.2 | C6 | D.4.4 |
| 6.0.2 | C6 | Sprache: ein Array hat immer seine Größe |
| 6.0.3 | C5 | Sprache: Module statt eines globalen Namensraums |
| 6.0.4 | C6 | Review (Lücke) |
| 6.4.1 | C6 | R.5.3 |
| 6.4.2 | C5 | Sprache: Traits verdecken keine Methoden |
| 6.5.1 | C5 | R.8.5 |
| 7.0.1 | C6 | R.10.5 |
| 7.0.6 | C6 | R.10.5, D.4.1 |
| 8.0.1 | C6 | R.12.1 |
| 8.2.10 | C6 | R.17.2 |
| 8.14.1 | C6 | R.13.5 |
| 8.20.1 | C6 | R.12.4, D.4.1 |
| 9.4.1 | C6 | R.15.7 |
| 12.3.1 | C5 | R.19.2 |
| 13.3.3 | C6 | rustc: Parameter tragen Namen |
| 14.1.1 | C6 | Review: ein öffentliches Feld trägt keine Invariante; MISRA C kennt die Regel nicht, `partial_pub_fields` hiesse Zugriffsmethoden in jedem Port ohne Gewinn |
| 15.0.1 | C5 | Sprache: `Copy`, `Clone`, `Drop` |
| 18.5.1 | C5 | D.4.7 |
| 18.5.2 | C6 | D.4.1, R.21.8 |
| 19.0.1 | C5 | Sprache: kein Präprozessor |
| 19.0.3 | C6 | Review: `use` am Kopf des Moduls |
| 19.2.2 | C5 | Sprache: kein Präprozessor |
| 21.6.1 | C6 | D.4.12 |
| 21.6.2 | C5 | Sprache: Besitz und `Drop` |
| 22.3.1 | C5 | Review: `const _: () = assert!(…)` für Konstanten |

### Der C-Teil (M11 Schritt 33b)

Der C-Teil ist, was ein C-Wirt übersetzt: der Rahmen, den `takt build
--emit embed` je Programm schreibt (`P_frame.c`, `P_ops.c`, `P.h`; erzeugt
von `takt-frame`), der Kopf des SDK (`takt_sdk.h`), der POSIX-Port
(`crates/takt-embed-c/ports/posix`) und die Beispiele
(`examples/c-host/main.c`, `examples/posix/main.c`). Für ihn gelten CERT C
und MISRA C:2012 unmittelbar (FB-524).

`crates/takt-cli/tests/c_analysis.rs` erzeugt den Rahmen für drei
Programme — `examples/c-host/takt/valve.takt` mit dem Beispiel `c-host`,
`corpus-try/124_port_test.takt` mit dem Port und dem Beispiel `posix`,
`corpus-try/05_streams_and_protocol.takt` für Ströme, Records und den
Rand — und prüft jede Bindung, also die Quellen eines Programms des Wirts,
mit beiden Werkzeugen. Der Test liest Aufruf und Abweichungen aus diesem
Abschnitt; er läuft unter Linux (die Kopfdateien von POSIX), auf CI in
`.github/workflows/analysis.yml` bei jedem Push.

| Werkzeug | Fassung | Aufruf |
|---|---|---|
| clang-tidy | LLVM 22.1.8 | `--checks=-*,cert-*,bugprone-*,clang-analyzer-*,concurrency-*,misc-no-recursion,-bugprone-easily-swappable-parameters,-clang-analyzer-security.insecureAPI.DeprecatedOrUnsafeBufferHandling,-clang-analyzer-optin.performance.Padding` |
| cppcheck | 2.22.0 | `--enable=warning,style,performance,portability` `--addon=misra` `--library=posix` `--library=tools/c-analysis/takt.cfg` `--suppress=unusedStructMember` |

Das MISRA-Addon von cppcheck prüft einen Teil von MISRA C:2012 und läuft
ohne die Regeltexte: Es nennt nur die Nummer. `tools/c-analysis/takt.cfg`
macht ihm die Builtins des Rahmens (`__atomic_*`, `__builtin_ia32_*`) und
die POSIX-Namen bekannt, die `posix.cfg` nicht führt; ohne sie wäre jeder
Aufruf eine Funktion ohne Deklaration (17.3). Volle Konformität zu MISRA C
verlangt einen qualifizierten Prüfer; das entscheidet die Zertifizierung.

**Ausgeschaltet.** Prüfungen, die keine Regel tragen oder etwas verlangen,
das es im C-Teil nicht gibt:

| Ausgeschaltet | Regel | Grund |
|---|---|---|
| `bugprone-easily-swappable-parameters` | — | eine Heuristik ohne Regel aus CERT C oder MISRA C; die Parameter der Einstiege und Treiber legt die C-ABI fest (`takt_sdk.h`, `P.h`) |
| `clang-analyzer-security.insecureAPI.DeprecatedOrUnsafeBufferHandling` | CERT C MSC24-C | verlangt die Funktionen aus Anhang K von C11 (`memcpy_s`, `fprintf_s`); Anhang K ist optional und fehlt in glibc wie in jeder freistehenden Umgebung des Rahmens. MSC24-C ist eine Empfehlung; die Längen prüft der Code selbst |
| `clang-analyzer-optin.performance.Padding` | — | Die Lage der Strukturen gibt `takt-hal` vor, der Rahmen hält sie mit `_Static_assert` je Feld; Umordnen hieße die ABI ändern |
| `unusedStructMember` | — | keine Regel; die Typen des Randes teilt der Rahmen mit `takt-hal`, und ein Feld, das ein Programm nicht liest, liest die Bibliothek |

**Der Rahmen ist erzeugter Code.** Er entsteht je Programm aus derselben
Quelle in `takt-frame`, niemand bearbeitet ihn, und er läuft gegen den
Interpreter (Satz 9.4.4: `tests/differential.rs`, die Board-Suiten). Regeln,
die gegen Fehler beim Schreiben und Ändern von Hand schützen — Form,
Klammern, ein Ausgang je Funktion —, hält für ihn der Generator; so sieht
MISRA automatisch erzeugten Code (MISRA AC AGC). Was ein Werkzeug als
Fehler im Ablauf meldet, ist behoben: `memcpy` statt Zeiger-Casts im Trace
(CERT C EXP39-C), kein ungelesenes `now`, `NULL` statt `0`, ausdrücklich
breite Multiplikationen vor einem Versatz.

**Abweichungen.** Jeder Befund steht hier mit Teil und genauer Zahl;
`tests/c_analysis.rs` verlangt genau diese Zahlen. Ein neuer Befund
scheitert wie ein behobener, und die Zahl fällt nur. Der Grund einer
Fehlmeldung beginnt mit „Fehlmeldung“; von einer Regel der Kategorie
Mandatory (M) weicht nichts ab.

| Befund | Teil | Zahl | Regel | Grund |
|---|---|---|---|---|
| `misra-c2012-1.4` | SDK | 1 | MISRA C:2012 1.4 (R) | `_Alignas(8)` am Speicher des Kerns: Das Archiv legt dort einen Rust-Typ mit Ausrichtung 8 ab; ohne die Angabe hinge sie an der ABI des Wirts |
| `misra-c2012-2.5` | SDK | 17 | MISRA C:2012 2.5 (A) | Die Konstanten (`TAKT_NVM_*`, `TAKT_ERR_*`, `TAKT_NEXT_*`, `TAKT_PROFILE_*`) sind Schnittstelle für jeden Wirt; eine Bindung nutzt nur einige |
| `constParameterPointer` | Port | 1 | MISRA C:2012 8.13 (A) | Fehlmeldung: `pthread_attr_setstack` nimmt den Stack als `void *`, und der Faden schreibt hinein; `const` brauchte einen Cast, der es wieder entfernt (11.8) |
| `misra-c2012-11.5` | Port | 3 | MISRA C:2012 11.5 (A) | `pthread_create` reicht den Port als `void *` an den Faden, der Wirt den Stack als `void *` (`takt_posix_config`) |
| `misra-c2012-2.5` | Port | 1 | MISRA C:2012 2.5 (A) | `_POSIX_C_SOURCE` liest nur die C-Bibliothek: Es macht `clock_gettime` und `pthread_condattr_setclock` sichtbar |
| `misra-c2012-21.1` | Port | 1 | MISRA C:2012 21.1 (R) | `#define _POSIX_C_SOURCE` wählt die Schnittstelle, wie POSIX es vorschreibt (Feature Test Macro); kein Bezeichner der Bibliothek ändert sich |
| `misra-c2012-21.10` | Port | 1 | MISRA C:2012 21.10 (R) | Der Port ist die Uhr des Wirts: `CLOCK_MONOTONIC` und `struct timespec` aus `<time.h>`; Datum und Ortszeit benutzt er nicht |
| `misra-c2012-1.4` | Beispiele | 2 | MISRA C:2012 1.4 (R) | `_Alignas(64)` an den Stacks der Fäden: `pthread_attr_setstack` verlangt einen ausgerichteten Stack |
| `misra-c2012-11.5` | Beispiele | 4 | MISRA C:2012 11.5 (A) | Die Rückrufe der Leitung und der Treiber bekommen den Zeiger des Wirts als `void *` (`takt_sdk.h`); nur der Wirt weiß, was dahinter liegt |
| `misra-c2012-17.3` | Beispiele | 1 | MISRA C:2012 17.3 (M) | Fehlmeldung: `fcntl` ist in `<fcntl.h>` deklariert; `posix.cfg` kennt die Form mit drei Argumenten nicht |
| `misra-c2012-21.1` | Beispiele | 1 | MISRA C:2012 21.1 (R) | wie im Port: `_POSIX_C_SOURCE` |
| `misra-c2012-21.6` | Beispiele | 9 | MISRA C:2012 21.6 (R) | Ein Wirt auf dem Rechner schreibt den Trace nach stdout und Fehler nach stderr; auf einem Ziel ist die Leitung ein UART (`struct takt_line`). Die Ein- und Ausgabe gehört zum Beispiel, nicht zum SDK |
| `clang-analyzer-security.ArrayBound` | Rahmen | 1 | CERT C ARR30-C | Fehlmeldung: Der Kopf eines Rings liegt stets unter seiner Kapazität (`takt_int_push` und `takt_int_pop` halten es), der Index darum nach einer Subtraktion im Ring; die Invariante über Aufrufe sieht der Analyzer nicht |
| `constParameterPointer` | Rahmen | 49 | MISRA C:2012 8.13 (A) | Die Einstiege und Helfer nehmen die Arena veränderlich, weil der Text für jedes Programm derselbe ist; was in einem Programm nur liest, schreibt in einem anderen |
| `knownConditionTrueFalse` | Rahmen | 8 | MISRA C:2012 14.3 (R) | Der Rahmen ist für jedes Programm derselbe Text mit dessen Konstanten; eine Bedingung, die für ein Programm feststeht (keine Jobs, kein `next_run`), entscheidet in einem anderen, und der Übersetzer entfernt den toten Zweig |
| `misc-no-recursion` | Rahmen | 6 | MISRA C:2012 17.2 (R) | `takt_dump_field` und `takt_dump_fields` steigen nur im Trace in die Felder eines Records ab; die Tiefe ist die Schachtelung der Typen des Programms, eine Konstante, und der Stack des Ticks ist gemessen (12.3) |
| `misra-c2012-1.4` | Rahmen | 61 | MISRA C:2012 1.4 (R) | `_Alignas` und `_Alignof`: Arena und Puffer des Randes liegen ausgerichtet, wie der erzeugte LLVM-Code sie liest (12.11) |
| `misra-c2012-10.1` | Rahmen | 9 | MISRA C:2012 10.1 (R) | Das Modell der wesentlichen Typen: Der Rahmen rechnet in den Typen des erzeugten Codes und mischt sie, wo Takt es festlegt; was herauskommt, prüft der Vergleich mit dem Interpreter bitgleich (Satz 9.4.4) |
| `misra-c2012-10.4` | Rahmen | 321 | MISRA C:2012 10.4 (R) | wie 10.1 |
| `misra-c2012-10.7` | Rahmen | 6 | MISRA C:2012 10.7 (R) | wie 10.1 |
| `misra-c2012-10.8` | Rahmen | 11 | MISRA C:2012 10.8 (R) | wie 10.1 |
| `misra-c2012-11.3` | Rahmen | 7 | MISRA C:2012 11.3 (R) | Die Arena ist ein Byte-Feld mit festen Versätzen, die der erzeugte Code mit genau einem Typ je Versatz liest und schreibt; der Rahmen greift mit demselben Typ zu. Ohne `-fno-strict-aliasing` bleibt das gegen CERT C EXP39-C offen (FB-650) |
| `misra-c2012-11.5` | Rahmen | 69 | MISRA C:2012 11.5 (A) | Arena und Treiberobjekt kommen als `void *` über die C-ABI (`takt_sdk.h`, `P_ops`) |
| `misra-c2012-12.1` | Rahmen | 142 | MISRA C:2012 12.1 (A) | erzeugte Ausdrücke; den Vorrang legt der Generator fest, nicht ein Leser |
| `misra-c2012-12.3` | Rahmen | 88 | MISRA C:2012 12.3 (A) | Deklarationen mehrerer Felder und Variablen in einer Zeile des erzeugten Texts |
| `misra-c2012-13.3` | Rahmen | 8 | MISRA C:2012 13.3 (A) | `x++` in Vergleichen und Indizes der Ringe; erzeugt, nicht von Hand geändert |
| `misra-c2012-14.4` | Rahmen | 70 | MISRA C:2012 14.4 (R) | Bedingungen auf Zählern und Zeigern (`if (k)`, `if (o->variants)`) im erzeugten Text |
| `misra-c2012-15.5` | Rahmen | 237 | MISRA C:2012 15.5 (A) | früher Ausgang in erzeugten Funktionen; jeder Weg läuft gegen den Interpreter |
| `misra-c2012-15.6` | Rahmen | 238 | MISRA C:2012 15.6 (R) | einzeilige Rümpfe im erzeugten Text; die Gefahr der Regel, eine später eingefügte Anweisung, besteht nur von Hand |
| `misra-c2012-15.7` | Rahmen | 9 | MISRA C:2012 15.7 (R) | Ketten aus `if … else if` ohne `else`, wo der letzte Fall nichts tut |
| `misra-c2012-16.6` | Rahmen | 10 | MISRA C:2012 16.6 (R) | Ein `switch` über die Kanäle eines Programms hat für ein Programm ohne solche Kanäle nur `default` |
| `misra-c2012-17.3` | Rahmen | 27 | MISRA C:2012 17.3 (M) | Fehlmeldung: Das Addon hält `__asm__ volatile(…)` für den Aufruf einer Funktion ohne Deklaration; es ist Inline-Assembler für die Gleitkomma-Umgebung (4.2), gekapselt in je zwei Funktionen (D.4.3) |
| `misra-c2012-17.7` | Rahmen | 59 | MISRA C:2012 17.7 (R) | `memcpy`, `memset` und `memmove` liefern ihr Ziel zurück, das der Rahmen schon hat |
| `misra-c2012-17.8` | Rahmen | 10 | MISRA C:2012 17.8 (A) | Parameter als Zähler in erzeugten Schleifen |
| `misra-c2012-18.4` | Rahmen | 25 | MISRA C:2012 18.4 (A) | Zeiger in die Arena als Basis plus Versatz, die Versätze aus dem Layout (`takt-frame::layout`) |
| `misra-c2012-2.2` | Rahmen | 20 | MISRA C:2012 2.2 (R) | `+ 0` und `* 1`: Der Generator schreibt jeden Zugriff in derselben Form (Basis plus Versatz), auch beim Versatz 0; der Übersetzer faltet die Konstante |
| `misra-c2012-2.4` | Rahmen | 2 | MISRA C:2012 2.4 (A) | Tags der geteilten Typen des Randes, die nicht jedes Programm braucht |
| `misra-c2012-2.5` | Rahmen | 20 | MISRA C:2012 2.5 (A) | Makros des Kopfs und der geteilten Typen, die nicht jedes Programm nutzt |
| `misra-c2012-2.7` | Rahmen | 22 | MISRA C:2012 2.7 (A) | Die Einstiege haben für jedes Programm dieselbe Signatur (`P_ops`); ein Programm ohne Journal oder Tunables liest manche Parameter nicht |
| `misra-c2012-20.1` | Rahmen | 5 | MISRA C:2012 20.1 (A) | `#include <stddef.h>` im Block der geteilten Typen des Randes, den ein Wächter-Makro umschließt |
| `misra-c2012-21.15` | Rahmen | 2 | MISRA C:2012 21.15 (R) | `memcpy` zwischen einem Wert und dem Byte-Feld der Arena; die Bytes sind der Wert |
| `misra-c2012-21.2` | Rahmen | 12 | MISRA C:2012 21.2 (R) | Der Rahmen ist freistehend (`-ffreestanding`, keine C-Bibliothek) und deklariert `memcpy`, `memmove`, `memset` und `memcmp` selbst, mit den Signaturen des Standards |
| `misra-c2012-5.1` | Rahmen | 3 | MISRA C:2012 5.1 (R) | Das Symbol `P_logic_<Hash>` trägt den Logik-Hash, damit das Archiv eines anderen Programms nicht bindet; die Linker aller Ziele unterscheiden die volle Länge |
| `misra-c2012-5.6` | Rahmen | 2 | MISRA C:2012 5.6 (R) | wie 5.7, für den Typ der Jobs |
| `misra-c2012-5.7` | Rahmen | 64 | MISRA C:2012 5.7 (R) | Rahmen und `P.h` beschreiben dieselben Strukturen je für sich, aus derselben Quelle in `takt-frame`; `_Static_assert` hält Größe und Versätze gleich |
| `misra-c2012-7.3` | Rahmen | 45 | MISRA C:2012 7.3 (R) | Fehlmeldung: Das Addon hält den Parameter `all` für ein Literal mit dem Suffix `l` |
| `misra-c2012-8.2` | Rahmen | 155 | MISRA C:2012 8.2 (R) | Prototypen der Einstiege des erzeugten LLVM-Codes und der Funktionen des Randes ohne Namen der Parameter; die Typen prüft der Übersetzer |
| `misra-c2012-8.4` | Rahmen | 54 | MISRA C:2012 8.4 (R) | Der Rahmen definiert die Einstiege, die `P.h` deklariert, ohne `P.h` einzubinden; beide entstehen aus derselben Quelle, und die Lieferform bindet beide (`tests/embed_cmd.rs`) |
| `misra-c2012-8.5` | Rahmen | 88 | MISRA C:2012 8.5 (R) | wie 8.4: Funktionen und Objekte stehen im Rahmen und in `P.h` |
| `misra-c2012-8.7` | Rahmen | 14 | MISRA C:2012 8.7 (A) | Einstiege mit externer Bindung, die ein Programm nur in einer Einheit ruft; ein Wirt ruft sie von außen |
| `misra-c2012-8.9` | Rahmen | 38 | MISRA C:2012 8.9 (A) | konstante Tabellen des Programms auf Dateiebene, neben den Funktionen, die sie lesen |

## 7. Zielnormen

IEC 61508 (SIL 2–3), IEC 61513 (Kerntechnik), DO-178C (vom Ground
Support Equipment bis zum Flugsystem). Dieses Dokument ist kein
Sicherheitsnachweis für ein Produkt; es sagt, welche Belege Takt dafür
liefert und wo sie liegen.
