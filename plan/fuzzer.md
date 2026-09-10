# Grammatik-Fuzzer — Entwurf

Stand: umgesetzt in `grammar/fuzz_grammar.py` (Abschnitt 8 nennt die Funde des ersten Laufs). Der Fuzzer ist die positive Hälfte des
differenziellen Testens: `grammar/diff_parse.py` prüft an *verdorbenen* Eingaben, dass Parser
und Orakel gleich ablehnen; der Fuzzer prüft an *wohlgeformten, erzeugten* Programmen, dass
alles, was die Grammatik erlaubt, vom Parser angenommen, vom Orakel bestätigt und vom
Formatter unversehrt durchgereicht wird. plan.md nennt ihn in Prinzip 3 und in `takt-syntax`.

## 1. Was geprüft wird — und was nicht

Es gibt keinen Rückweg „durch den Parser zurück". Der Weg ist:

```
Ableitung aus takt.ebnf ──▶ Programmtext ──▶ Parser (Rust)       muss annehmen
                                       ├──▶ Orakel (Python)      muss annehmen
                                       └──▶ format(text)         Garantien aus format.md:
                                                 gleiche Tokens, gleicher Baum, Kommentare
                                                 erhalten, idempotent
```

„Inhaltlich das Gleiche" heißt hier: derselbe Tokenstrom und derselbe Syntaxbaum
(S-Expression), nicht dieselbe Bedeutung. Die erzeugten Programme sind syntaktisch korrekt
und meist semantisch Unsinn (Variablen ohne Deklaration, Einheiten ohne Definition); das ist
gewollt, denn geprüft werden Parser, Formatter und Grammatik.

**Sim-Modus (seit M1).** Mit `--sim` durchläuft jede kanonische Fassung zusätzlich `takt
check`, und jedes Programm, das dabei fehlerfrei bleibt, läuft `--sim-ticks` Ticks unter
`takt sim`. Der Maßstab ist Satz 9.4.2: ein abgelehntes Programm liefert Diagnosen, ein
angenommenes läuft ohne Absturz und ohne internen Fehler. Ein Verdikt `FAIL` ist kein
Fehlschlag, ein Abbruch des Prozesses oder ein `Bug(…)` in der Ausgabe schon. Nur die
kanonische Fassung wird geprüft, weil sich die Leerraumfassungen nach der Formatinvarianz
nicht in der Bedeutung unterscheiden.

```
python -X utf8 grammar/fuzz_grammar.py --count 40 --seed 31 --sim --sim-ticks 50
```

Zusätzlich zur Annahme wird die *Struktur* geprüft, ohne den Baum in Python nachzubauen:

- **Klammer-Zwilling.** Jeder Ausdruck wird ein zweites Mal gerendert, jede binäre
  Teiloperation vollständig eingeklammert. Beide Fassungen müssen denselben Baum liefern
  (`sexpr` druckt Klammern transparent). Das prüft Vorrang und Assoziativität des Parsers
  gegen die Absicht des Generators, ohne dass der Generator einen Drucker für Bäume braucht.
- **Leerraum-Zwillinge.** Dieselbe Ableitung mit minimalem und mit maximalem erlaubtem
  Leerraum (zusätzliche Leerzeichen, Fortsetzungszeilen in Klammern, Kommentare auf eigener
  Zeile und nachgestellt, Leerzeilen) muss denselben Baum und dieselbe Formatterausgabe
  ergeben. Das verallgemeinert den Zerknitter-Test aus `format_roundtrip.rs` auf beliebige
  Programme.
- **Abdeckung.** Jede Alternative jeder Produktion und jedes `[ … ]`/`{ … }` muss in der
  erzeugten Menge vorkommen; der Lauf meldet, was fehlt. Damit ist belegt, dass wirklich die
  ganze Grammatik durch Parser und Formatter gegangen ist — dieselbe Idee wie
  `corpus-try/coverage.py`, nur ohne Handarbeit.

## 2. Abwägung

**Generator aus der EBNF oder handgeschrieben?** Ein handgeschriebener Generator (über
`ast::*` in Rust oder als Python-Funktionen je Konstrukt) veraltet mit jeder
Grammatikänderung und deckt nur ab, woran der Autor gedacht hat. Ein Generator, der
`takt.ebnf` liest, folgt der Grammatik automatisch und trifft jede Alternative — genau der
Wert, den der Fuzzer haben soll (Prinzip 4: eine Quelle je Artefakt). **Entscheidung:
EBNF-getrieben.**

**Python oder Rust?** Der EBNF-Lader existiert in Python (`check_grammar.parse_rhs`,
vom Orakel benutzt); ein zweiter in Rust wäre Doppelung. Erzeugungsgeschwindigkeit ist
unkritisch (Tausende Programme pro Minute), und das Orakel ist ohnehin Python. Die späteren
*semantischen* Fuzzer (Interpreter gegen nativen Code, M4) brauchen dagegen wohlgetypte,
ausführbare Programme mit Symboltabellen; das ist ein eigener, typgeführter Generator über
dem AST in Rust (`takt-conformance`) und kein Ausbau dieses Werkzeugs. **Entscheidung:
Python, `grammar/fuzz_grammar.py`, mit klarer Grenze zum späteren typgeführten Generator.**

**Zufall oder Aufzählung?** Reine Aufzählung aller Ableitungen bis Tiefe n explodiert;
reiner Zufall trifft seltene Alternativen nie. **Entscheidung: gewichteter Zufall mit
Abdeckungssteuerung** — die Wahl einer Alternative wird zugunsten noch nicht getroffener
Alternativen verzerrt, bis alles mindestens k-mal vorkam; danach gleichverteilt mit
tiefenabhängiger Dämpfung der Rekursion.

**Was mit Fehlern?** Ein Fehlschlag ohne Verkleinerung ist wertlos. **Entscheidung:
Delta-Debugging auf dem Text** (Zeilen und Elemente in Klammern entfernen, solange der
Fehler bleibt, mit dem Parser als Orakel), Ausgabe als Datei mit Seed und Kommandozeile zum
Nachstellen; ein bestätigter Fund wandert von Hand in `corpus-try/` als Regressionsfall.

## 3. Aufbau

`grammar/fuzz_grammar.py`, drei Schichten:

**Ableitung.** Läuft die Produktionen aus `takt.ebnf` als Baum (`parse_rhs`). Ein Knoten je
Terminal, Token oder Nichtterminal; `{ }` und `[ ]` mit gewichteten Wiederholungen (0 bis 3,
in Ausdrücken tiefenabhängig). Tiefenbudget je Nichtterminal (Ausdrücke 6, Typen 3,
Zustände 3, Datei 1 bis 8 Deklarationen). Startsymbole: `file` und `snippet` (dieselbe
Erweiterung wie im Orakel).

**Lexikalische Realisierung.** Aus Tokenklassen werden Texte, nach `lexer.md`; hier sitzen
die Regeln, die die EBNF nicht ausdrückt:

| Token | Erzeugung |
|---|---|
| `IDENT`, `UPPER_IDENT`, `TYPE_IDENT` | aus Wortlisten, geprüft mit `parse_corpus.classify`; nie Schlüsselwort oder reserviert; kontextuelle Wörter mit kleiner Wahrscheinlichkeit *absichtlich* als Bezeichner (Abschnitt 5) |
| `INT`, `HEX`, `BIN`, `OCT`, `FLOAT` | alle Schreibweisen mit Unterstrichen, Exponenten, Groß-/Kleinschreibung der Präfixe; nie `.5` oder `1.` |
| `DURATION` | Zahl, ein oder mehr Leerzeichen, genau ein Zeitsuffix; danach Leerraum (L4.4) |
| `unit_lit` nach `number` | kompakt und anliegend; nie ein Zeitsuffix allein (sonst `DURATION`), nie ein kontextuelles Wort, `1` nur als Zähler |
| `STRING` | je Stelle die passende Teilsprache aus ihrer Produktion: `pattern_text` in `pattern`, `address_text` in `hw()`/`sim()`/`tick_source`/`node`, `format_text` in Meldungen (`log`, `check`, `alert`, `verify`, `expect`, `abort`, `verdict`, `send`), sonst beliebiger Text mit Escapes |
| `>>` | zwei anliegende `>`; `> >` nie |
| Einrückung | `INDENT`/`DEDENT` als Tiefe, 4 Leerzeichen; Zeilenumbrüche nur in Klammern |

**Rendering und Zwillinge.** Ein Tokenstrom wird zu Text: kanonisch (ein Leerzeichen),
minimal (kein Leerraum, wo die Lexik ihn nicht braucht), maximal (zusätzlicher Leerraum,
Fortsetzungszeilen an zufälligen Kommas und Operatoren in Klammern, Kommentare, Leerzeilen)
und geklammert (Abschnitt 1). Alle Fassungen einer Ableitung tragen denselben Namen und
Seed.

**Läufer.** Erzeugt n Programme in ein Arbeitsverzeichnis, ruft in Stapeln
`takt parse --ast`, `parse_corpus.py` und `takt fmt --verify`
(wie `diff_parse.py`), vergleicht die S-Expressions der Zwillinge über
`parse --ast`, meldet Abdeckung, verkleinert Fehlschläge, Exit-Code ungleich 0 bei jedem
Fund. Optionen: `--seed`, `--count`, `--start file|snippet`, `--depth`, `--cover k`,
`--keep DIR`.

## 4. Regeln, die der Generator kennen muss (Determinisierungen der Grammatik)

Die EBNF ist an einigen Stellen bewusst weiter als die Sprache; der Parser trifft die
Entscheidung, und der Generator muss auf seiner Seite bleiben, sonst meldet der Fuzzer
Scheinfunde:

1. Kontextuelle Wörter sind keine Einheitennamen (2.2, lexer.md L4.3).
2. Die `1` eines Einheitenausdrucks nur als Zähler (`unit_expr`).
3. In `<…>` eines Typs schließt `>`; ein Vergleich als `const_expr` dort steht in Klammern
   (`cmp_expr`-Kommentar).
4. `unit_lit` reicht so weit, wie die Tokens anliegen; `5 K / min` ist eine Division.
5. `IDENT generic_args` nur mit folgendem `(` (Instanziierung), sonst Index (`primary`).
6. `as` gefolgt von einem Skalartypwort ist ein Cast; sonst eine Bindung (`cast_expr`).
7. `fn` ohne `->` nur mit einem `inout`-Parameter (3.9, Parserprüfung).
8. Eine Zeile ist eine Anweisung; Umbrüche nur in Klammern; Fortsetzungszeilen ohne
   Einrückungsregel.

Jede dieser Regeln steht als Kommentar in `takt.ebnf` oder in `lexer.md`; der Generator
liest sie nicht, er verkörpert sie. Kommt eine neue Determinisierung hinzu, sind Grammatik,
Parser, Orakel und Generator gemeinsam zu ändern — der Fuzzer erzwingt das, weil er sonst
Abweichungen meldet.

## 5. Erwartete Funde

Der Generator soll kontextuelle Wörter mit kleiner Wahrscheinlichkeit als gewöhnliche
Bezeichner setzen (2.2 erlaubt das ausdrücklich: „Damit darf ein Record ein Feld `offset`
… haben"). Dort sind echte Mehrdeutigkeiten zu erwarten, die dann als Regel festgelegt
werden müssen, zum Beispiel:

- Eine Variable namens `int` oder `float`: `x as int` ist per Regel 6 ein Cast, nie eine
  Bindung. Vorschlag: Skalartypwörter sind als Bindungsnamen nach `as` ausgeschlossen
  (Grammatikkommentar, Parserfehler mit Vorschlag).
- Ein Feld oder eine Variable namens `bits`, `len`, `offset` in `record_field`
  (`x : u16 with bits:` gegen `with len = bits`) — die Grammatik unterscheidet über die
  Folgetokens; der Fuzzer bestätigt es oder findet den Fall.
- Ein Block namens `range` in `for x in range(3)`: `range` ist Schlüsselwort, also kein
  Fund; wohl aber `hw`, `sim`, `none` als Argumentnamen (`f(hw = 1)`).

Solche Funde landen als Festlegung in Anhang A der Referenz, wie die vier aus dem
Mutationstest.

## 6. Umsetzung in Schritten

1. Ableitung mit Abdeckungszähler und Tiefenbudget über `parse_rhs`; Ausgabe zunächst als
   Tokenliste; Test: jede Alternative erreichbar (sonst ist die Grammatik tot oder das
   Budget zu klein).
2. Lexikalische Realisierung nach Abschnitt 3 und 4; Rendering kanonisch.
3. Läufer mit Parser, Orakel, `fmt --verify`; erste Läufe mit `--count 200`; Funde
   sichten — hier ist mit Parser- und Grammatikfehlern zu rechnen, das ist der Zweck.
4. Zwillinge (Klammern, Leerraum) und Vergleich der S-Expressions.
5. Verkleinerung und Ablage von Fehlschlägen; CI-Lauf mit festem Seed und kleiner Zahl,
   nächtlich groß.
6. Kontextuelle Wörter als Bezeichner einschalten; Funde nach Abschnitt 5 festlegen.

Größenordnung: 800 bis 1000 Zeilen Python; der Läufer übernimmt Stapelung und
Berichtsformat von `diff_parse.py`.

## 7. Nicht Teil des Fuzzers

Semantisch sinnvolle Programme (dafür der typgeführte Generator ab M1/M4), Leistungsmessung,
Fuzzing der Teilsprachen mit ungültigen Strings (dafür die Vektoren in `lexer.md`), negative
Grammatikfälle (dafür `diff_parse.py --mutate`).

## 8. Funde der ersten Läufe

Aus etwa 1000 erzeugten Programmen in fünf Fassungen (Seeds 1 bis 23, `file` und `snippet`,
mit und ohne kontextuelle Wörter als Bezeichner):

1. **Stapelüberlauf im Parser.** Zwanzig verschachtelte Klammern brachten den Debug-Parser
   auf dem 1-MB-Hauptthread zum Absturz. Jetzt gilt eine Sprachregel (Verschachtelung
   höchstens 64 Ebenen, `MAX_DEPTH`, Fehler mit Vorschlag), und Parser wie Formatter laufen
   auf einem Thread mit 64 MB Stapel, damit die Regel und nicht der Stapel entscheidet.
2. **`>` in eckigen Klammern innerhalb von Typklammern.** `bytes<[8 >> 1][0]>` wurde
   abgelehnt: der Parser setzte die Typklammertiefe nur in runden Klammern zurück. Jetzt
   auch in Array-Literalen, Indizes, Generik-Argumenten und Array-Typen; das Orakel stellt
   die Tiefe nach der schließenden Klammer wieder her (vorher blieb sie auf 0).
3. **Mehrdeutigkeit in `property`.** Der Klammer-Zwilling zeigte, dass `a or b if c else d`
   zwei Ableitungen hatte: `or` auf Formelebene mit der Bedingung im letzten Atom, oder ein
   gewöhnlicher Ausdruck mit der Bedingung zuunterst — mit verschiedenen Bäumen. Festlegung:
   `tprop_atom := … | cmp_expr`, die Bedingungsform steht in Eigenschaften nur innerhalb
   eines Aufrufs; der Parser meldet `property q: x if a else b` mit Vorschlag.
4. **Exponent ohne Anliegen.** `float[K ^ 2]` war für den Parser kein Exponent, weil er das
   Anliegen aus `unit_lit` auch außerhalb verlangte; die Grammatik kennt die Regel nur für
   Literale nach Zahlen.
5. **Einheitenklammer ohne Anliegen.** `u16 [mV]` verlangte der Parser anliegend; weder
   Grammatik noch lexer.md kennen die Regel. Entfernt.
6. **Klassen der Generik-Argumente.** `f[KiB/s]` wurde als Typ `KiB` gelesen, `f[N + 1]`
   und `f[n * 2]` als Einheit `N` beziehungsweise als ungültige Einheit `n*2`, `f[T?]` als
   Einheit `T`. Jetzt entscheiden Namensform und Folgetoken, mit Rückzug auf einen
   Konstantenausdruck, wenn die Einheit nicht bis zum `,` oder `]` reicht.
9. **Anliegender Operator hinter einem Einheitenliteral.** `0o17 degC^2^0.5 ms` nahm der
   Parser an (Einheit `degC^2`, dann XOR), das Orakel lehnte nach L4.3 ab, weil die
   anliegende Folge kein Einheitenausdruck ist. Der Parser meldet das jetzt mit Vorschlag.
8. **Exponent mit Anliegen nur nach vorn.** `3 B ^N()` las der Parser als Exponent, weil er
   nur prüfte, ob auf `^` etwas anliegt, nicht, ob `^` an der Einheit anliegt.
7. **`3 s*2` im Orakel.** Das Orakel las die Einheit nur bis `s` und `*2` als Multiplikation,
   der Parser liest die anliegende Folge ganz (L4.3) und lehnt ab. Das Orakel folgt jetzt
   L4.3: ein anliegendes `*`, `/`, `^` hinter dem Ende gehört zur Einheit.

Kontextuelle Wörter als Bezeichner (`--contextual`) ergaben einen weiteren Fall: `rx[i8 < 4.25]`
mit einer Variablen `i8` liest der Parser als Typ. Festlegung: Typwörter (`bool`, `int`, …,
`bytes`, `mat`, …) sind überall, wo ein Typ stehen kann (Generik-Argument, nach `as`), Typen;
der Generator setzt sie deshalb nicht als Bezeichner ein, und `x as int` ist ein Cast.

**Abdeckung.** 300 Programme je Startsymbol treffen zusammen alle 394 Alternativen und
Gruppen bis auf wenige, die im jeweils anderen Modus liegen (Sequenzen und Übergänge nur
in `snippet`, weil `file` das Budget in Deklarationen ausgibt). Ein CI-Lauf sollte deshalb
beide Modi mit festem Seed fahren; die Ausgabe nennt, was fehlt.

**Funde des Sim-Modus (M1).** Der erste Lauf mit `--sim` fand einen Absturz von `takt check`:
enthielt der `system:`-Block der Nutzerdatei einen Fehler (etwa ein `tick_tolerance` ohne
`pct`), zählte die Prüfung „Prelude fehlerfrei" diese Diagnose dem Prelude zu und beendete
den Prozess mit einer Assertion. Die Zählung betrachtet jetzt nur die Diagnosen des Preludes,
und ein fehlerhaftes Prelude wird als interner Fehler gemeldet statt als Absturz. Die Läufe
mit den Seeds 31, 101 und 202 (100 Programme) sind seitdem ohne Fund.
