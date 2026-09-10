# M0-Rest: Diagnoserahmen, Edition, Prüfungen 49–51 — Entwurf

Stand: umgesetzt (`crates/takt-diag`, `crates/takt-sema`, `crates/takt-cli`, `corpus-try/checks/`);
Abschnitt 8 nennt die Abweichungen vom Entwurf. Entwurf nach Parser, Formatter und Fuzzer (Commit 9850ef3). Offen aus M0 waren laut
plan.md: Fehlerrahmen mit Positionen und Vorschlägen, Edition (`language`, Warnung bei
Fehlen, Hash-Anteil), Prüfung reservierter Wörter und Membernamen, Regel der offenen Enums,
dann der MIR-Entwurf (eigenes Dokument, `plan/mir.md`). Dieses Dokument legt fest, wie die
ersten semantischen Prüfungen gebaut werden, damit die 55 folgenden denselben Rahmen nutzen.

## 1. Ziel

Nach diesem Schritt gibt es einen Diagnoserahmen, der für Tokenizer, Parser, Sema, MIR und
Runtime derselbe ist; die Sprachedition als Datenstruktur, aus der Schlüsselwörter,
reservierte Wörter, reservierte Membernamen und offene Enums je Edition kommen; ein
`takt-sema`-Crate mit Symbolsammlung und den Prüfungen 49, 50 und 51 samt Testrahmen, der
für jede der 58 Prüfungen positive und negative Fälle mit Zeilenanmerkungen führt; und ein
`takt-cli` mit `check` und `fmt`, weil Diagnosen ohne Ausgabe niemandem helfen.

## 2. Diagnoserahmen

### 2.1 Abwägung: eigenes Crate oder in `takt-syntax`?

Die Diagnose ist der eine Typ, den *jede* Schicht erzeugt und die Kommandozeile, der
Editor (LSP) und die Testrahmen lesen. Liegt er in `takt-syntax`, hängen Runtime-Berichte
und `takt size` an der Syntax; liegt er in einem winzigen Crate ohne Abhängigkeiten, hängt
alles nur an ihm. **Entscheidung: `crates/takt-diag`**, keine Abhängigkeiten, `no_std`-fähig
(die Runtime meldet Randverletzungen im selben Format, 12.6).

Die vorhandenen Typen `LexError` und `ParseError` in `takt-syntax` werden durch die
Diagnose ersetzt (kein zweiter Fehlertyp, der später übersetzt werden müsste); `ErrorCode`
bleibt als Code-Quelle des Tokenizers. Das ist eine Umstellung von etwa 20 Stellen und
sechs Testdateien, jetzt billig, später teuer.

### 2.2 Form

```
Severity   := Error | Warning | Note
Span       := { file: FileId, start: u32, end: u32 }        Byte-Bereich; Zeile/Spalte rechnet die SourceMap
Diagnostic := { severity, code: &'static str, message: String, span: Span,
                suggestion: Option<String>, notes: Vec<(Span, String)>, stage: Option<Stage> }
SourceMap  := Dateien mit Text und Zeilenanfängen; liefert (Zeile, Spalte), Zeilentext
Stage      := V1_1 | V1_2 | V2                              "ab v1.1": geparst, aber noch nicht umgesetzt
```

Codes: Tokenizer `E_BOM` … `E_CHAR` (lexer.md L9), Parser `P` (Prüfung 1), Sema `SC-2` …
`SC-58` (Nummern aus Abschnitt 10; ein Code je Prüfung, nicht je Meldung, damit Inventur,
Tests und Meldungen dieselbe Nummer tragen), MIR und Runtime `M-…`, `R-…`. Jede Diagnose
hat einen Vorschlag, wo einer möglich ist (10: „Position, Ursache und ein konkreter
Vorschlag"); der Parser hat das schon.

Ausgabe: Text mit Quellauszug und Markierung, ein Format für alle Werkzeuge:

```
error[SC-50]: `valid` ist als Feldname verboten
  --> ctrl.takt:12:5
   |
12 |     valid : bool
   |     ^^^^^ Zugriff eines Wrappers (T?, T!E, Channel); auf `x.valid` ist immer der Wrapper gemeint
   = Vorschlag: Feld umbenennen, etwa `is_valid`
```

Dazu eine einzeilige Form für Tests und Skripte (`datei:zeile:spalte: severity[code]:
message`) und später JSON für den Editor (kein Serde in `takt-diag`; ein Schreiber von
Hand, das Format ist klein).

### 2.3 Politik

Warnungen sind projektweise zu Fehlern eskalierbar, und der Zertifizierungsmodus macht aus
SC-49 einen Fehler (10, 2.5). Das ist keine Eigenschaft der Prüfung, sondern der
Auswertung: `Policy { warnings_as_errors, certification }` in `takt-diag`, angewandt beim
Sammeln (`DiagnosticSink::report`). Prüfungen melden immer ihre Grundschwere aus Abschnitt
10. Die Konfiguration (`takt.toml`) kommt mit der CLI; bis dahin Kommandozeilenschalter
`--warnings-as-errors`, `--certification`.

## 3. Edition

### 3.1 Abwägung: wann ist die Edition bekannt?

`system: language = N` steht *in* der Datei, aber der Tokenizer braucht die Schlüsselwörter
der Edition schon beim Lesen (neue Schlüsselwörter kommen nur mit einer Edition, 2.5).
Drei Wege: (a) alle Editionen teilen einen Tokenizer, neue Schlüsselwörter sind in alten
Editionen kontextuell — das verwässert 2.2 und bricht, sobald ein neues Schlüsselwort am
Zeilenanfang steht; (b) die Edition steht außerhalb der Datei (Cargo-Modell) — 2.5 legt
sie in die Datei; (c) ein Vorlauf liest `language = N` aus dem `system:`-Block mit einem
winzigen Scanner, dann tokenisiert der Tokenizer mit dem Wortschatz dieser Edition.
**Entscheidung: (c).** Der Vorlauf ist eine Funktion über den Text (die ersten Zeilen bis
zur ersten Deklaration außer `system`), robust gegen Kommentare und Leerzeilen; fehlt die
Angabe, gilt die neueste Edition (mit SC-49).

### 3.2 Datenstruktur

```
Edition::E1  (heute die einzige; `latest()`)
edition.keywords()          -> &[&str]      2.2
edition.reserved_words()    -> &[&str]      2.5
edition.reserved_members()  -> &[&str]      2.5 (alle 60), davon wrapper_accessors() (8) und capture_names() (4)
edition.open_enums()        -> &[&str]      2.5: FaultKind BootReason ImageState RebootCmd Quality JobErr, Wertebereich von `.reason`
edition.predefined_units()  -> …            3.2 (kommt mit M1)
```

Ort: `takt-syntax::edition` (der Tokenizer braucht sie; `keywords.rs` geht darin auf). Die
Paritätstests gegen 2.2 bleiben und laufen je Edition. `tokenize(src)` nimmt die neueste,
`tokenize_in(src, edition)` eine bestimmte; `takt check` und `takt fmt` rufen erst den
Vorlauf. Die Edition ist Teil des Logik-Hashs (MIR, `plan/mir.md`).

### 3.3 SC-49 und `takt fmt`

Fehlt `language`, meldet Sema `SC-49` als Warnung mit Vorschlag `language = 1 in system:
eintragen (takt fmt --edition)`. `takt fmt --edition` trägt sie ein: ein eigener Schritt
außerhalb von `format()`, weil er den Tokenstrom ändert und deshalb nicht unter den
Roundtrip fällt (plan/formatter.md, Schritt 7). Einfügung: erste Zeile des vorhandenen
`system:`-Blocks, sonst ein neuer Block am Dateianfang nach führenden Kommentaren.
Unbekannte Edition (`language = 7`) ist ein Fehler `SC-49` mit Nennung der bekannten.

## 4. `takt-sema`: Rahmen und die Prüfungen 50 und 51

### 4.1 Abwägung: wie viel Sema jetzt?

Prüfung 50 braucht nur die Deklarationen (Felder, Varianten, Capture-Namen). Prüfung 51
braucht den *Typ des Match-Subjekts* — das ist Typinferenz, also M1. Zwei Wege: 51
verschieben, oder eine typfreie Näherung: das `match` nennt in seinen `case`-Zweigen
Variantennamen; gehören alle Varianten mit diesem Namen zu offenen Enums (oder ist das
Subjekt ein Name mit deklariertem Enum-Typ, oder ein eingebauter Wert wie `x.reason`,
`last_fault.kind`), ist das Enum bestimmt. **Entscheidung: Näherung jetzt, vollständige
Form mit der Typinferenz in M1**, und der Test hält beide auseinander (Fälle, die die
Näherung entscheidet, und Fälle, die als „ab M1" markiert sind). Der Rahmen von
`takt-sema` wird so gebaut, dass M1 nur ergänzt:

```
takt-sema
  symbols.rs    Symbolsammlung: Deklarationen je Datei, Maschine, Zustand (Name -> Art, Span, Deklaration);
                Doppelte Namen = SC-2 (Teilaspekt; vollständig in M1)
  edition.rs    Vorlauf, SC-49
  names.rs      SC-50: reservierte Membernamen (Felder, Varianten, Bitfelder, Capture-Namen aus pattern_text)
  enums.rs      SC-51: offene Enums (Deklarationen `open`, eingebaute Liste der Edition), Näherung wie oben
  lib.rs        check(file, edition, policy) -> Vec<Diagnostic>; ein Durchlauf ruft alle Prüfungen
```

Reservierte *Wörter* als Bezeichner meldet schon der Tokenizer (`E_RESERVED`); Sema zählt
das unter SC-50 mit, damit die Inventur eine Nummer hat, und `while` bekommt die eigene
Meldung aus 2.5 („nicht erlaubt: `for` mit Schranke oder `sequence` mit `until`").

### 4.2 Testrahmen der Prüfungen

Jede Prüfung SC-n bekommt ein Verzeichnis `corpus-try/checks/SC-n/` mit `ok_*.takt`
(müssen ohne Diagnose dieses Codes durchlaufen) und `bad_*.takt` mit Zeilenanmerkungen im
Quelltext, wie die UI-Tests von rustc:

```
record Header:
    valid : bool        #~ SC-50
    data  : bytes<8>
```

`#~ CODE` erwartet die Diagnose auf derselben Zeile, `#~^ CODE` auf der vorigen. Der
Rahmen (`crates/takt-sema/tests/checks.rs`) vergleicht die Menge (Zeile, Code) und
meldet fehlende wie überzählige Diagnosen. Anmerkungen sind Kommentare, also für Parser
und Formatter unsichtbar; die Dateien bleiben kanonisch formatiert (`format_canonical`
deckt sie mit ab). Eine Inventurzeile SC-n wird `fertig`, wenn beide Dateiarten existieren
und grün sind (plan.md, Abschnitt 6 Punkt 2) — für 49 bis 51 nach diesem Schritt; für 51
mit dem Vermerk der Näherung, `fertig` erst in M1.

Die 60 MEM-Zeilen: die acht Wrapper-Zugriffe und die vier Capture-Namen sind mit SC-50
getestet und werden `fertig`; die übrigen 48 sind reservierte Namen eingebauter Typen,
deren Bedeutung mit M1/M2 kommt — sie werden `teilweise` (Liste je Edition vorhanden,
Bedeutung fehlt).

## 5. `takt-cli`

Minimal, weil Diagnosen eine Ausgabe brauchen und die Referenz `takt check` und `takt fmt`
nennt: `takt check DATEI… [--warnings-as-errors] [--certification] [--format text|line]`,
`takt fmt DATEI… [--check] [--stdout] [--edition]`, `takt parse DATEI [--ast|--debug]`
(ersetzt die Beispiele; die Python-Werkzeuge rufen danach die CLI). Kein
Argument-Parser-Crate: drei Kommandos mit wenigen Schaltern, von Hand.

## 6. Umsetzung in Schritten

1. `takt-diag`: Typen, SourceMap, Textausgabe, Policy, Sink; Tests mit Quellauszügen.
2. Umstellung von `takt-syntax` auf `Diagnostic` (Tokenizer, Parser, Formatter, Tests).
3. `edition.rs` in `takt-syntax` mit Tabellen aus 2.2 und 2.5, Paritätstests, Vorlauf,
   `tokenize_in`.
4. `takt-sema`: Symbolsammlung, SC-49, SC-50, SC-51 (Näherung), Testrahmen mit `#~`,
   Verzeichnisse `corpus-try/checks/SC-49..51` mit `ok_`/`bad_`-Dateien; `n03` aus dem
   Korpus wird die erste `bad_`-Datei von SC-50.
5. `takt-cli` mit `check`, `fmt`, `parse`; `fmt --edition`; Python-Werkzeuge auf die CLI.
6. Inventur: SC-49, SC-50 `fertig`, SC-51 `teilweise`, MEM-Zeilen wie in 4.2; plan.md und
   Anhang A nachziehen (Vorlauf für die Edition ist eine Festlegung zu 2.5).

Größenordnung: `takt-diag` 300 Zeilen, Umstellung 200, Edition 150, Sema 500, CLI 250,
Tests 300.

## 7. Offene Entscheidungen (mit Empfehlung)

| Nr. | Frage | Empfehlung | Grund |
|---|---|---|---|
| D1 | Eigenes Diagnose-Crate? | ja, `takt-diag` ohne Abhängigkeiten | ein Typ für alle Schichten bis zur Runtime |
| D2 | `LexError`/`ParseError` ersetzen? | ja, jetzt | zwei Fehlertypen wären dauerhafte Übersetzungsarbeit |
| D3 | Edition vor dem Tokenizer per Vorlauf? | ja | einziger Weg, der 2.2 (Schlüsselwörter je Edition) und 2.5 (Angabe in der Datei) vereinbart |
| D4 | SC-51 als Näherung in M0? | ja, mit Kennzeichnung | die Regel steht seit M0, die Typinferenz kommt in M1 |
| D5 | Zeilenanmerkungen `#~` als Testform? | ja | selbstdokumentierend, positionsstabil, ein Rahmen für alle 58 Prüfungen |
| D6 | CLI jetzt? | ja, minimal | Diagnosen brauchen eine Ausgabe; die Referenz nennt `takt check` |

## 8. Stand nach der Umsetzung

- `takt-diag` wie entworfen; `LexError` und `ParseError` sind ersetzt, `ast::Span` ist
  derselbe Typ wie der Diagnose-Span (ein Spannentyp fuer alles, `Span::new` bleibt
  dateilos, die `SourceMap` des Aufrufers ordnet zu).
- Reservierte Woerter meldet weiter der Tokenizer (`E_RESERVED`, Vektoren in lexer.md);
  `takt-sema` fuehrt die Diagnose unter `SC-50`, damit Inventur und Testrahmen eine Nummer
  haben.
- SC-51 ist die Naeherung aus 4.1: deklarierte Art des Subjekts (`var`, `input`, Parameter,
  `persist`), `x.reason`, `last_fault.kind`, sonst die Variantennamen der Zweige, wenn sie
  genau ein bekanntes Enum bestimmen. Alles andere wartet auf die Typinferenz (M1);
  die Inventurzeile bleibt `teilweise`.
- Die Kommandozeile ersetzt die Beispiele; die Python-Werkzeuge rufen `takt parse`,
  `takt fmt` und `takt tokens`.
