# Formatter (`takt fmt`) — Entwurf

Stand: umgesetzt in `crates/takt-syntax/src/fmt/` (Entwurf nach dem Parser, Commit d3d18db).
Dieses Dokument legt fest, was der Formatter tut, warum so und nicht anders, wie er gebaut und
wie er geprüft wird. Die kanonische Form selbst steht mit Testvektoren in `grammar/format.md`
(wie `lexer.md`); Abschnitt L7 von `lexer.md` verweist dorthin. Wo dieses Dokument und
`format.md` abweichen, gilt `format.md`.

## 1. Ziel

Ein Formatter, der jede syntaktisch gültige Takt-Datei in genau eine kanonische Form bringt,
in der Referenz (2.1: „der Formatter ist kanonisch") und im Plan (M0: jede Grammatik-ID besteht
den Roundtrip) gefordert. Vier Eigenschaften sind nicht verhandelbar:

1. **Bedeutungstreu.** `parse(format(s))` liefert denselben Baum wie `parse(s)`; kein Wert, kein
   Literaltext, kein Kommentar geht verloren oder ändert sich (L7: „Der Formatter ändert nie einen
   Wert").
2. **Idempotent.** `format(format(s)) == format(s)`. Eine Datei, die der Formatter unverändert
   lässt, heißt kanonisch.
3. **Vollständig.** Jede Produktion der Grammatik wird gedruckt; ein Konstrukt ohne Druckregel
   ist ein Fehler im Formatter, nicht ein Sonderfall. Der Test `productions.rs` wird dafür um
   `fmt_<produktion>`-Funktionen erweitert (Nachverfolgbarkeit wie beim Parser).
4. **Nur gültigen Code.** Eine Datei mit Syntaxfehlern wird nicht umgeschrieben; der Formatter
   gibt die Parserfehler zurück und lässt die Datei unberührt.

## 2. Abwägung: welche Schule?

Es gibt zwei erprobte Arten, einen Formatter zu bauen.

| | Umbruch-Formatter (black, prettier, rustfmt) | Struktur-erhaltender Formatter (gofmt) |
|---|---|---|
| Zeilenumbrüche | der Formatter setzt sie nach einer Breite | der Autor setzt sie, der Formatter normiert Leerraum und Einrückung |
| Kanonizität | eindeutig: eine Eingabe, eine Ausgabe, unabhängig vom Layout der Eingabe | eindeutig bis auf die Umbrüche des Autors |
| Ausrichtung in Tabellen | keine (prettier, black) oder nur begrenzt | ja, elastische Spalten je Laufgruppe (tabwriter) |
| Diff-Stabilität | gut in Fließtext, schlecht bei Tabellen | gut, außer eine Spalte wird breiter |
| Aufwand | Wadler/Prettier-Dokumentmodell mit Gruppen und `fits` | Druck aus dem Baum plus Spaltenpass |

Die Referenz entscheidet das. Ihre 1422 Beispielzeilen sind Tabellen, keine Fließtexte:

- 259 Zeilen sind länger als 100 Zeichen, 406 länger als 80 — aber nur 4 Zeilen sind
  Fortsetzungszeilen in Klammern. Die Referenz will lange Zeilen, nicht Umbrüche.
- 234 Zeilen richten `:` in Spalten aus; alle 139 Endkommentare stehen mit zwei oder mehr
  Leerzeichen, in Blöcken untereinander. Kanäle, Parameter, Records, Variablen werden als
  Tabellen gelesen — das ist die Domäne (Register- und Kanallisten).
- 37 Übergänge stehen einzeilig (`when x: -> X`), 7 als Block; `if h.size > 1 MiB: return
  ERR(SIZE)` und `enter: led = 0` sind bewusst einzeilig. Die Wahl ist inhaltlich.

Ein Umbruch-Formatter müsste entweder gegen die Referenz arbeiten (259 Zeilen umbrechen, alle
Tabellen auflösen) oder eine Breite von 200 setzen, was ihn wirkungslos macht. Ein
Struktur-erhaltender Formatter mit Spaltenausrichtung macht genau das, was die Referenz
vorlebt. **Entscheidung: gofmt-Schule.** Konkret:

- Der Formatter bricht **nie** eine Zeile um und fügt **nie** eine zusammen. Zeilenumbrüche in
  Klammern (Fortsetzungszeilen) bleiben, wo der Autor sie gesetzt hat; ihre Einrückung wird
  normiert. Es gibt keine Zeilenbreite. Ein späterer `takt lint` darf lange Zeilen melden.
- Strukturelle Wahlen des Autors bleiben: einzeiliger oder Block-Körper bei einem einzelnen
  einfachen Statement, einzeilige oder mehrzeilige `enum`-Form, Klammern um Ausdrücke,
  Reihenfolge von allem.
- Alles andere ist normiert: Leerraum, Einrückung, Spaltenausrichtung in Laufgruppen,
  Kommentarabstand, Leerzeilen.

Damit ist die Ausgabe eindeutig bestimmt durch den Tokenstrom (Art, Text, Zeilenstruktur) und
das Beiwerk — und nur dadurch.

Gegen die Alternative „Token-Reformatter ohne Baum" spricht, dass fast jede Leerraumregel vom
Kontext abhängt (`-` unär oder binär, `[` Index, Generik oder Einheit, `:` Typ oder
Blockkopf, `>` Vergleich oder Typklammer, `K/min` Einheit oder Division). Der Baum hat diese
Unterscheidungen schon getroffen. Gedruckt wird deshalb **aus dem AST**, mit Rückgriff auf
Quelltext und Tokens für alles, was der AST nicht trägt (Literaltext, Zeilenstruktur,
Beiwerk).

## 3. Architektur

Ort: `crates/takt-syntax/src/fmt/` (Modul `fmt`), API `takt_syntax::fmt::format(src: &str) ->
Result<String, Vec<ParseError>>`. Die Kommandozeile (`takt fmt`, `--check`, `--diff`, stdin)
kommt mit `takt-cli`; bis dahin `examples/fmt.rs` wie `parse.rs`.

```
Quelltext ──tokenize──▶ Tokens (mit Beiwerk, Zeilen, Anliegen)
                │
                └──parse_file──▶ AST ──┐
                                       ▼
   (1) Beiwerk zuordnen: Kommentare und Leerzeilen an Knoten (führend / nachgestellt / hängend)
   (2) Drucken: AST → Zeilen aus Zellen; Leerraumregeln je Produktion; Literale aus dem Quelltext
   (3) Ausrichten: Laufgruppen gleichartiger Zeilen → Spaltenbreiten (elastische Tabulatoren)
   (4) Normieren: LF, keine Tabs, kein Leerraum am Zeilenende, genau ein Zeilenende am Dateiende
```

**Stufe 1 — Beiwerk.** Der Tokenizer hängt Kommentare und Leerzeilen an das folgende Token.
Der Formatter ordnet jedes Beiwerk nach Position einem Knoten zu:

- *führend*: Kommentar auf eigener Zeile vor einem Statement, einer Deklaration, einer
  Variante, einem Feld, einem `case`, einem Zustand — steht davor auf dessen Einrückung;
- *nachgestellt*: Kommentar auf der Zeile eines Knotens hinter dessen letztem Token — bleibt
  auf der Zeile, zwei Leerzeichen Abstand, in Laufgruppen ausgerichtet;
- *hängend*: Kommentar am Ende eines Blocks (vor `DEDENT`) — bleibt im Block auf dessen
  Einrückung; Kommentar in Klammern vor einem Element — steht vor diesem Element auf der
  Fortsetzungseinrückung, vor der schließenden Klammer sonst;
- *Leerzeilen*: höchstens eine, wo der Autor eine oder mehr hatte; keine am Anfang und Ende
  eines Blocks; keine vor dem ersten und nach dem letzten Element einer Klammer. Der
  Formatter fügt keine Leerzeilen hinzu.

`#Text` wird `# Text`; `#` ohne Text bleibt `#`. Kommentare werden nie umgebrochen oder
verschoben.

**Stufe 2 — Drucken.** Ein Drucker je Produktion (`fmt_<name>`), Struktur wie `sexpr.rs`.
Ausdrücke, Typen, Einheiten liefern Strings; Statements und Deklarationen liefern Zeilen mit
Einrückungstiefe. Eine Zeile ist eine Folge von *Zellen*; Zellgrenzen sind die
Ausrichtungspunkte (Abschnitt 4.6). Was der AST nicht kennt, kommt über die Spanne aus dem
Quelltext: Zahlen (`0xFF_FF`, `1e-3`), Strings mit ihren Escapes, Dauern in ihrer Schreibweise
(`200 ms`, nicht `0.2 s`).

**Stufe 3 — Ausrichten.** Wie gofmts tabwriter: aufeinanderfolgende Zeilen derselben Art
(Abschnitt 4.6) mit gleicher Einrückung bilden eine Laufgruppe; je Spalte wird die maximale
Zellbreite bestimmt und aufgefüllt. Eine Leerzeile, eine eigene Kommentarzeile oder eine Zeile
anderer Art beendet die Gruppe. Nachgestellte Kommentare sind die letzte Zelle jeder Zeile,
auch in Gruppen, deren übrige Zellen nicht ausgerichtet werden (Statements).

**Stufe 4 — Normieren.** Zeilenenden `\n`, BOM entfernt, Tabs kommen nicht vor (die
Einrückung wird neu erzeugt), Leerraum am Zeilenende entfernt (auch nach Auffüllung), genau ein
Zeilenende am Dateiende, keine Leerzeile am Dateianfang.

## 4. Kanonische Form (Katalog; vollständig in `grammar/format.md`)

### 4.1 Einrückung und Zeilen
- 4 Leerzeichen je Stufe.
- Fortsetzungszeilen in Klammern: Umbruch des Autors bleibt; die Fortsetzung wird auf die
  Spalte nach der öffnenden Klammer ausgerichtet (L7). Eine schließende Klammer auf eigener
  Zeile steht auf der Einrückung der Zeile, die die Klammer öffnete.
- Eine Zeile pro Statement, Deklaration, Variante, Feld, Systemeintrag, Kampagneneintrag.

### 4.2 Leerraum in Deklarationen
- Genau ein Leerzeichen zwischen Wörtern: `tunable param`, `native fn`, `persist var`,
  `driver machine`, `state X idle resume`.
- Typangabe in Deklarationszeilen mit Leerzeichen vor und nach dem Doppelpunkt: `var x :
  float[V] = 0 V`, `input tank_p : float[bar]`, `param P : int = 3`, Felder `size : u32[B]`
  (Referenz: 133 zu 3 bei Deklarationen, 32 zu 0 bei `var`; Felder gemischt 15 zu 11).
  Parameter in Signaturen dagegen `x: float[U]` (Referenz: 24 zu 0) — Tabellenzeilen
  bekommen den ausgerichteten Doppelpunkt, Aufzählungen in Klammern nicht.
- Blockköpfe ohne Leerzeichen vor dem Doppelpunkt: `machine m every 50 us:`, `state A:`,
  `if x > 0:`, `when c:`, `case OK(v):`, `enter:`.
- `=` mit Leerzeichen; `->` mit Leerzeichen (`-> float[U]`, `-> SAFE`); `@` mit Leerzeichen;
  `with` und Attribute `a = 1, b = 2`.
- Signaturen: `fn clamp[U](x: float[U], lo: float[U]) -> float[U]:`; Generik-Klammer
  direkt am Namen, Parameterklammer direkt an der Generik.

### 4.3 Leerraum in Ausdrücken
- Binäre Operatoren mit einem Leerzeichen: `a + b`, `a and b`, `x matches P as m`, `a if c
  else b`, `x as u16`, `a implies b`, `a << 2`, `a >> 2`.
- Unäre ohne: `-x`, `~x`, `not x` (Wort, ein Leerzeichen).
- Klammern innen ohne Leerraum, Kommas mit einem Leerzeichen danach: `f(a, b)`, `[1, 2]`,
  `(a, b)`, `x[i, j]`, `x[a..b]`, `f[U, 1/s](a = 1)`.
- Member ohne Leerraum: `x.y`, `x.step(e, 1 ms)`.
- Ranges ohne Leerraum: `0..100`, `-45..45 A`.
- Zahl und Einheit: genau ein Leerzeichen; die Einheit kompakt (`5 K/min`, `9.81 m/s^2`);
  Dauern wie geschrieben (`200 ms`, `1 h`).
- Literaltext unverändert: `0xFF`, `1_000`, `4.25`, `1e-3`, Strings samt Escapes.

### 4.4 Typen
- `float[V]`, `u16[mV]`, `[16] float[degC]` (Leerzeichen nach `]` des Arrays), `bytes<64>`,
  `vec<u8, 4>`, `map<u16, int, 8>`, `mat<2, 2>[K]`, `mat[X, 1/X]`, `T?`, `Header!ParseErr`,
  `float in 0..1`, `Duration in tick..1 h`.

### 4.5 Blöcke und Formen des Autors
- Ein Körper mit genau einem einfachen Statement darf einzeilig stehen (`when c: -> X`,
  `enter: led = 0`, `if x: return y`); der Formatter behält die Wahl bei. Ein Körper mit
  Kommentar, mehreren Statements oder einem zusammengesetzten Statement steht immer als Block.
- `enum` einzeilig oder als Block: Wahl des Autors; ein Block mit Feldern oder Diskriminanten
  bleibt Block.
- Klammern um Ausdrücke bleiben (sie sind im AST).

### 4.6 Ausrichtung (Laufgruppen)
Zeilen derselben Art direkt untereinander mit gleicher Einrückung bilden eine Gruppe. Zellen:

| Art | Zellen |
|---|---|
| Kanäle (`input`/`output`) | Richtung · Name · `: Typ` · `@ Bindung` · `with …` · Kommentar |
| `var`, `pub var`, `persist var`, `param`, `tunable param`, `const` | Schlüsselwort und Name · Rest (`: Typ = Wert …`) · Kommentar |
| Record-Felder, Bitfelder | Name · Rest · Kommentar |
| Varianten im Block, Systemeinträge, Profileinträge | Name · Rest · Kommentar |
| Statements, Übergänge, Kampagneneinträge, alles andere | Zeile · Kommentar |

Bei Variablen, Parametern und Feldern gibt es nur einen Ausrichtungspunkt nach dem Namen
(die Referenz richtet `:` aus, nicht `=`); bei Kanälen alle Spalten, wie in der Referenz.
Nachgestellte Kommentare stehen in aufeinanderfolgenden Zeilen in einer Spalte; der
Mindestabstand ist zwei Leerzeichen. Ein `input` in einer Gruppe mit `output` wird auf
`output` aufgefüllt (`input  x`), wie in der Referenz.

## 5. Tests

1. **Vektoren** in `grammar/format.md`: Paare Eingabe → Ausgabe je Regel, gelesen von
   `tests/format_vectors.rs` (wie `tests/vectors.rs` für den Lexer). Jede Regel aus Abschnitt
   4 hat mindestens einen Vektor; jede Produktion einen.
2. **Roundtrip** über Korpus und Referenzschnipsel: Tokens von `format(s)` gleich Tokens von
   `s` in Art und Text (Anliegen dort gleich, wo es Bedeutung trägt: Einheiten, `>>`), und
   `sexpr(parse(format(s))) == sexpr(parse(s))`.
3. **Idempotenz**: `format(format(s)) == format(s)` für alles.
4. **Beiwerk**: Multimenge der Kommentartexte vor und nach dem Formatieren gleich; Anzahl der
   Leerzeilen nach dem Formatieren nie größer als davor.
5. **Normierung**: Aus jeder Korpusdatei wird eine „zerknitterte" Variante erzeugt (Leerraum
   verdoppelt oder entfernt, wo die Lexik es zulässt; Fortsetzungseinrückung verschoben;
   Kommentare ohne Leerzeichen nach `#`); ihre Ausgabe muss gleich der Ausgabe der Vorlage
   sein. Das ist der Test, dass die Ausgabe nur von Tokens und Zeilenstruktur abhängt.
6. **Kanonischer Korpus**: Korpus, Referenzschnipsel und Golden-Bäume werden einmal
   kanonisiert; danach prüft ein Test `format(s) == s` für jede Datei — der Korpus ist damit
   zugleich Golden-Test des Formatters. `corpus-try/check_examples.py` bekommt `--fmt`, damit
   die Codeblöcke der Referenz kanonisch bleiben (Dokumentation als Test).
7. **Mutanten und Fuzzer**: `diff_parse.py` liefert angenommene Mutanten; jede angenommene
   Variante durchläuft Roundtrip und Idempotenz. Der spätere Grammatik-Fuzzer nutzt dieselben
   Prüfungen.
8. **Inventur**: Mit grünem Roundtrip gehen die 233 Zeilen aus 2.3 von `teilweise` auf
   `fertig` (plan.md, Abschnitt 6 Punkt 4).

## 6. Umsetzung in Schritten (erledigt bis Schritt 6, Referenzblöcke offen)

1. `grammar/format.md` schreiben: Regeln aus Abschnitt 4 mit Vektoren; L7 in `lexer.md`
   auf den Verweis kürzen. Offene Punkte aus Abschnitt 7 vorher entscheiden.
2. `fmt/trivia.rs`: Beiwerk-Zuordnung (führend, nachgestellt, hängend, Leerzeilen) mit
   eigenen Tests an kleinen Schnipseln.
3. `fmt/print.rs`: Drucker für Einheiten, Typen, Ausdrücke (Strings), dann Statements,
   Maschinen, Deklarationen (Zeilen mit Zellen); Fortsetzungszeilen aus den Token-Zeilen.
4. `fmt/align.rs`: Laufgruppen und Spaltenpass; `fmt/mod.rs`: Normierung und `format()`.
5. Tests 1 bis 5, `examples/fmt.rs`, Erweiterung von `productions.rs` um `fmt_<name>`.
6. Korpus, Schnipsel und Referenzblöcke kanonisieren (Test 6), Inventur nachziehen. Diese
   Umschreibung der Referenz geschieht nur nach Freigabe.
7. Später: `takt fmt` in `takt-cli` mit `--check`/`--diff`/stdin und dem Eintragen der
   Edition (`language = N`, 2.5) als eigenem Schritt außerhalb von `format()`, weil er den
   Tokenstrom ändert und deshalb nicht unter den Roundtrip fällt.

Größenordnung: Drucker etwa so groß wie `sexpr.rs` (900 Zeilen), Beiwerk und Ausrichtung
je 150 bis 250 Zeilen, Vektoren einige hundert Zeilen.

## 7. Offene Entscheidungen (mit Empfehlung)

| Nr. | Frage | Empfehlung | Grund |
|---|---|---|---|
| D1 | Zeilenbreite und automatischer Umbruch? | keine Breite, kein Umbruch | Referenz: 259 lange Zeilen, 4 Fortsetzungen; Tabellen statt Fließtext |
| D2 | Spaltenausrichtung in Laufgruppen? | ja, wie gofmt, Zellen nach 4.6 | 234 ausgerichtete Zeilen in der Referenz; Kanal- und Registerlisten sind Tabellen |
| D3 | Leerzeichen vor `:` bei Typangaben, auch bei Feldern? | ja, überall ` : ` | 133 zu 3 in der Referenz; eine Regel statt zwei |
| D4 | Einzeilige Körper (`when c: -> X`, `enter: x = 1`) erhalten oder auflösen? | erhalten | 37 zu 7 in der Referenz; die Wahl trägt Bedeutung für den Leser |
| D5 | Leerzeilen hinzufügen (z. B. vor jedem `state`)? | nein, nur auf eine begrenzen | gofmt-Prinzip: der Formatter fügt keine Struktur hinzu |
| D6 | Fortsetzungszeilen: an die öffnende Klammer ausrichten (L7) oder hängend +4? | L7 beibehalten | selten (4 Zeilen); kein Grund, die Referenz zu ändern |
| D7 | Abstand vor Endkommentaren | mindestens zwei Leerzeichen, in Gruppen ausgerichtet | alle 139 Endkommentare der Referenz |
| D8 | Formatieren trotz Syntaxfehler? | nein | nie Code umschreiben, den man nicht versteht |

Nicht Teil des Formatters: Umbenennen, Sortieren, Entfernen unbenutzter Teile, Umstellen von
Attributen, Vereinheitlichen von Literalschreibweisen. Das sind Aufgaben eines `lint` oder
`migrate`.
