# Takt — Lexer-Spezifikation, Edition 1

Normative Beschreibung des Tokenizers. Sie ergänzt `grammar/takt.ebnf`: die Grammatik
beschreibt die Satzform über Tokens, dieses Dokument beschreibt, wie aus Zeichen Tokens
werden. Quellen in der Referenz: 2.1 (Lexik), 2.2 (Schlüsselwörter), 2.5 (reservierte
Wörter), 3.1 (Literale), 3.3 (Duration-Regel), 3.9 (Formatstrings), 8.1 (Adressen),
8.7 (Musterliterale).

**Testvektoren.** Jede Regel trägt Vektoren in Blöcken der Form

```
vectors
<Eingabe> => <Tokenfolge>
```

Die Eingabe steht in doppelten Anführungszeichen mit den Escapes `\n` (Zeilenende),
`\t` (Tabulator), `\"` und `\\`. Tokens werden als `ART(text)` geschrieben; Interpunktion
und Operatoren stehen nackt. `!` vor einem Vektor bedeutet: die Eingabe ist ein Fehler,
und der Text nach `=>` ist der Fehlercode aus Abschnitt L9. Ein Tokenizer gilt als
konform, wenn er alle Vektoren dieses Dokuments erfüllt (`takt-conformance`, 13.8).

Tokenarten: `NEWLINE INDENT DEDENT IDENT UPPER TYPE KW RESERVED INT HEX BIN OCT FLOAT DUR
STRING WILD` sowie die Operatoren und Interpunktion aus L6. `UPPER`, `TYPE` und `KW`
entsprechen `UPPER_IDENT`, `TYPE_IDENT` und `KEYWORD` der Grammatik; `DUR` entspricht
`DURATION`; `WILD` ist das Terminal `"_"`.

---

## L1 Quelltext

- **L1.1 Kodierung.** Die Datei ist UTF-8. Außerhalb von Stringliteralen und Kommentaren
  sind nur ASCII-Zeichen erlaubt (2.1: alles Tippbare ist ASCII). Eine Byte-Order-Mark
  am Dateianfang ist ein Fehler `E_BOM`; `takt fmt` entfernt sie.
- **L1.2 Zeilenenden.** `\n` und `\r\n` sind Zeilenenden; ein einzelnes `\r` ist ein
  Fehler `E_CR`. Positionen werden als (Zeile, Spalte) ab 1 gezählt; die Spalte zählt
  Zeichen, was außerhalb von Strings mit Bytes übereinstimmt.
- **L1.3 Dateiende.** Endet die letzte Zeile ohne Zeilenende, erzeugt der Tokenizer
  trotzdem `NEWLINE`. Danach folgen so viele `DEDENT`, wie Einrückungsstufen offen sind.
- **L1.4 Kommentare.** `#` beginnt einen Kommentar bis zum Zeilenende, außer innerhalb
  eines Stringliterals. Kommentare erzeugen kein Token. Es gibt keine Blockkommentare.
- **L1.5 Leerraum.** Leerzeichen trennen Tokens und sind sonst bedeutungslos, mit zwei
  Ausnahmen: der Einrückung am Zeilenanfang (L2) und der Bindung von Einheiten an
  Zahlen (L4.3). Ein Tabulator ist überall außerhalb von Strings und Kommentaren ein
  Fehler `E_TAB` (2.1).

```
vectors
"x = 1  # Kommentar\n"          => IDENT(x) = INT(1) NEWLINE
"x = 1"                         => IDENT(x) = INT(1) NEWLINE
"log \"a # b\"\n"               => KW(log) STRING(a # b) NEWLINE
!"\xEF\xBB\xBFsystem:\n"        => E_BOM
!"x =\t1\n"                     => E_TAB
!"x = 1\r"                      => E_CR
!"x = \xC3\xA4\n"               => E_NONASCII
```

---

## L2 Zeilen, Einrückung, Blöcke

Die Blockstruktur folgt CPython: `NEWLINE` beendet eine logische Zeile, `INDENT` und
`DEDENT` markieren Einrückungswechsel. Abweichend von Python ist die Schrittweite fest.

- **L2.1 Logische Zeilen.** Eine physische Zeile, die nur Leerraum oder einen Kommentar
  enthält, erzeugt kein Token und beeinflusst die Einrückung nicht.
- **L2.2 Klammerfortsetzung.** Innerhalb offener `(`, `[` oder `{` erzeugen Zeilenenden
  weder `NEWLINE` noch `INDENT`/`DEDENT`; die Einrückung der Folgezeilen ist frei (2.1:
  Zeilenfortsetzung innerhalb offener Klammern). Eine andere Fortsetzung, etwa per
  Backslash, gibt es nicht. Ein Dateiende innerhalb offener Klammern ist `E_UNCLOSED`.
- **L2.3 Schrittweite.** Die Einrückung einer logischen Zeile ist die Zahl führender
  Leerzeichen. Sie muss ein Vielfaches von 4 sein und darf gegenüber der vorigen
  logischen Zeile um höchstens eine Stufe (4 Leerzeichen) steigen. Sonst `E_INDENT`.
- **L2.4 Stapel.** Der Tokenizer führt einen Stapel von Einrückungen, Start `[0]`.
  Steigt die Einrückung, wird sie auf den Stapel gelegt und `INDENT` erzeugt. Fällt sie,
  werden Stufen abgebaut und je Stufe ein `DEDENT` erzeugt; die Zieleinrückung muss auf
  dem Stapel liegen, sonst `E_DEDENT`.
- **L2.5 Reihenfolge.** Am Ende einer logischen Zeile kommt erst `NEWLINE`, dann bei der
  nächsten logischen Zeile `INDENT` oder die `DEDENT`s. Nach einem `DEDENT` folgt kein
  weiteres `NEWLINE` (die Grammatik verlässt sich darauf, `record_field`).

```
vectors
"a:\n    b\nc\n"                          => IDENT(a) : NEWLINE INDENT IDENT(b) NEWLINE DEDENT IDENT(c) NEWLINE
"a:\n    b:\n        c\nd\n"              => IDENT(a) : NEWLINE INDENT IDENT(b) : NEWLINE INDENT IDENT(c) NEWLINE DEDENT DEDENT IDENT(d) NEWLINE
"a:\n    b\n"                             => IDENT(a) : NEWLINE INDENT IDENT(b) NEWLINE DEDENT
"a:\n\n    # nur Kommentar\n    b\n"      => IDENT(a) : NEWLINE INDENT IDENT(b) NEWLINE DEDENT
"f(1,\n  2,\n        3)\n"                => IDENT(f) ( INT(1) , INT(2) , INT(3) ) NEWLINE
"x = [1,\n2]\n"                           => IDENT(x) = [ INT(1) , INT(2) ] NEWLINE
!"a:\n  b\n"                              => E_INDENT
!"a:\n        b\n"                        => E_INDENT
!"a:\n    b:\n        c\n  d\n"           => E_DEDENT
!"f(1,\n"                                 => E_UNCLOSED
```

---

## L3 Wörter

Ein Wort ist eine maximale Folge aus `[A-Za-z0-9_]`, die nicht mit einer Ziffer beginnt.
Die Tokenart ergibt sich aus Liste und Form, in dieser Reihenfolge:

- **L3.1 Schlüsselwörter.** Steht das Wort in der Liste aus 2.2, ist es `KW`. Die Liste
  ist je Edition fest und lebt in 2.2; `check_grammar.py` prüft, dass jedes Wort daraus
  in der Grammatik vorkommt, und weist jedes weitere Wort-Terminal der Grammatik als
  kontextuell aus. `true`, `false`, `none` und `default` sind Schlüsselwörter, keine
  Literaltokens.
- **L3.2 Reservierte Wörter.** Steht das Wort in der Liste `RESERVED` (2.5), ist es
  `RESERVED`. Der Parser meldet es als Fehler `E_RESERVED` mit dem Wort; für `while`
  lautet der Vorschlag „`for` mit Schranke oder `sequence` mit `until`".
- **L3.3 Wildcard.** Das Wort `_` allein ist `WILD`.
- **L3.4 Form.** Sonst entscheidet die Form: beginnt das Wort mit einem Kleinbuchstaben
  oder `_`, ist es `IDENT`; besteht es nur aus Großbuchstaben, Ziffern und `_`, ist es
  `UPPER`; beginnt es mit einem Großbuchstaben und enthält einen Kleinbuchstaben, ist
  es `TYPE`. Ein einzelner Großbuchstabe wie `U`, `T`, `N` oder `B` ist `UPPER`.
- **L3.5 Konventionen.** Die Form bestimmt nur die Tokenart. Ob ein Name der Konvention
  aus 2.1 folgt, prüft die Semantik (Prüfung 2), weil sie die Art des Namens kennt:
  `myVar` als Variable verdient eine Warnung, `degC`, `kHz` und `mV` als Einheiten nicht.
  Der Tokenizer warnt deshalb nie.
- **L3.6 Kontextuelle Terminale.** Wörter wie `bool`, `bytes`, `layout`, `offset`,
  `timeout`, `asap`, `Duration`, `Edge` erkennt der Tokenizer nicht, ebenso wenig eingebaute
  Bezeichner wie `now`, `tick`, `last_fault` oder `event` in Triggern. Sie sind `IDENT`,
  `UPPER` oder `TYPE`, und der Parser vergleicht an der jeweiligen Stelle den Text.
- **L3.7 Membernamen.** Nach `.` darf ein Schlüsselwort stehen (`.as`, `.or`, `.state`,
  `.len`); der Tokenizer liefert dennoch `KW`, der Parser akzeptiert es dort (2.5,
  Produktion `member`).

```
vectors
"machine state check"     => KW(machine) KW(state) KW(check)
"true false none default" => KW(true) KW(false) KW(none) KW(default)
"tank_p now _x x1"        => IDENT(tank_p) IDENT(now) IDENT(_x) IDENT(x1)
"PEAK_TEMP S0 OK U N B"   => UPPER(PEAK_TEMP) UPPER(S0) UPPER(OK) UPPER(U) UPPER(N) UPPER(B)
"ValveCmd Hz KiB Sha256Ctx" => TYPE(ValveCmd) TYPE(Hz) TYPE(KiB) TYPE(Sha256Ctx)
"bool bytes layout asap"  => IDENT(bool) IDENT(bytes) IDENT(layout) IDENT(asap)
"case _:"                 => KW(case) WILD :
"d.as(s)"                 => IDENT(d) . KW(as) ( IDENT(s) )
"x.or(1)"                 => IDENT(x) . KW(or) ( INT(1) )
"myVar degC kHz mV"       => IDENT(myVar) IDENT(degC) IDENT(kHz) IDENT(mV)
!"while x:"               => E_RESERVED(while)
!"var struct = 1"         => E_RESERVED(struct)
```

---

## L4 Zahlen, Einheiten, Dauern

### L4.1 Ganze Zahlen

- Dezimal `[0-9][0-9_]*` → `INT`; `0x[0-9a-fA-F_]+` → `HEX`; `0b[01_]+` → `BIN`;
  `0o[0-7_]+` → `OCT`. Das Präfix ist klein geschrieben; `0X`, `0B`, `0O` sind `E_NUMBER`.
- Der Unterstrich ist Trennzeichen und bedeutungslos; er darf nicht direkt auf das
  Präfix folgen und nicht am Ende stehen (`E_NUMBER`).
- Führende Nullen sind erlaubt und ohne Bedeutung (`007` ist 7); ein Oktalpräfix ist nur `0o`.
- Der Tokenizer speichert die Ziffernfolge unverändert. Ob der Wert in `int` (i64) oder in
  einen schmaleren Typ passt, prüft die Semantik (3.1, 4.1); `0xFFFF_FFFF_FFFF_FFFF` ist
  als Literal gültig und passt in `u64`.

```
vectors
"42 1_000_000 007"        => INT(42) INT(1_000_000) INT(007)
"0x1F 0xFFFF_FFFF 0x7E8"  => HEX(0x1F) HEX(0xFFFF_FFFF) HEX(0x7E8)
"0b1010 0o17"             => BIN(0b1010) OCT(0o17)
!"0X1F"                   => E_NUMBER
!"1_"                     => E_NUMBER
!"0x_1"                   => E_NUMBER
!"0b102"                  => E_NUMBER
```

### L4.2 Fließkommazahlen

- `[0-9][0-9_]*\.[0-9][0-9_]*([eE][+-]?[0-9]+)?` oder `[0-9][0-9_]*[eE][+-]?[0-9]+` → `FLOAT`.
- Es gibt keine Form ohne Ziffer vor oder nach dem Punkt: `1.` und `.5` sind nicht
  erlaubt. Darum ist `1..3` eindeutig `INT .. INT` und `2.0..4.5` ist `FLOAT .. FLOAT`.
- Der Tokenizer speichert den Dezimaltext. Die Rundung in die Breite von `float`
  (`system: float`, 4.2) geschieht in der Semantik, damit Compile-Zeit und Laufzeit
  dieselbe Zahl sehen (11.3).
- Ein Vorzeichen gehört nie zum Literal; `-4.25` ist `- FLOAT(4.25)` (Grammatik `unary`).

```
vectors
"4.25 1e-3 6894.757293168 1e-30 3.0" => FLOAT(4.25) FLOAT(1e-3) FLOAT(6894.757293168) FLOAT(1e-30) FLOAT(3.0)
"1..3"                    => INT(1) .. INT(3)
"2.0..4.5"                => FLOAT(2.0) .. FLOAT(4.5)
"-4.25"                   => - FLOAT(4.25)
!"1."                     => E_NUMBER
!".5"                     => E_NUMBER
!"1e"                     => E_NUMBER
```

### L4.3 Einheiten nach Zahlen

Ein Einheitenausdruck darf nur direkt auf ein Zahlenliteral folgen (Grammatik
`primary := number [ unit_expr ]`). Die Zuordnung ist lexikalisch, nach diesen Regeln:

- **Abstand.** Zwischen Zahl und Einheit steht mindestens ein Leerzeichen. `85degC` ist
  `E_UNIT_SPACE`; dadurch bleiben `1e3` und `0x1F` eindeutig.
- **Kompakte Schreibweise.** Der Einheitenausdruck ist die maximale Folge aus
  Einheitennamen, `*`, `/`, `^` und Ziffern nach `^` **ohne Leerraum**: `K/min`, `m/s^2`,
  `A*s`, `1/s`, `pct/bar`. Ein Leerzeichen beendet den Einheitenausdruck. So ist
  `200 ms * 2` eine Dauer mal zwei, und `5 K/min` ist ein Literal in `K/min`.
- **Einheitennamen** sind Wörter beliebiger Form (`bar`, `mV`, `V`, `K`, `B`, `Hz`, `Pa`,
  `KiB`, `degC`) oder die Ziffer `1` für dimensionslos (`1/s`). Der Tokenizer prüft nicht,
  ob die Einheit existiert; das tut die Semantik (3.2).
- Der Tokenizer erzeugt für den Einheitenausdruck gewöhnliche Tokens (`IDENT`, `UPPER`,
  `TYPE`, `INT`, `*`, `/`, `^`) und markiert das erste davon als *an die Zahl gebunden*.
  Der Parser liest dann `unit_expr` nach der Grammatik. Ein Einheitenname, der zugleich
  Schlüsselwort ist, kommt nicht vor (2.2 enthält keine Einheiten).

```
vectors
"85 degC"                 => INT(85) IDENT(degC)
"4.25 V"                  => FLOAT(4.25) UPPER(V)
"5 K/min"                 => INT(5) UPPER(K) / IDENT(min)
"9.81 m/s^2"              => FLOAT(9.81) IDENT(m) / IDENT(s) ^ INT(2)
"0.0005 1/s"              => FLOAT(0.0005) INT(1) / IDENT(s)
"72000 A*s"               => INT(72000) UPPER(A) * IDENT(s)
"0.5 pct/bar"             => FLOAT(0.5) IDENT(pct) / IDENT(bar)
"4 KiB"                   => INT(4) TYPE(KiB)
"1 B..4096 B"             => INT(1) UPPER(B) .. INT(4096) UPPER(B)
"5 * n"                   => INT(5) * IDENT(n)
!"85degC"                 => E_UNIT_SPACE
```

### L4.4 Dauern (3.3)

- Folgt auf ein Zahlenliteral, nach Leerraum, **genau ein** Zeitsuffix aus
  `ns us ms s min h d`, und ist das Zeichen unmittelbar nach dem Suffix weder `*` noch
  `/` noch `^`, dann bilden Zahl und Suffix zusammen ein Token `DUR`.
- Der Wert ist die exakte Zahl von Nanosekunden als i64: `ns` 1, `us` 10³, `ms` 10⁶,
  `s` 10⁹, `min` 60·10⁹, `h` 3600·10⁹, `d` 86400·10⁹. Fließkommazahlen werden exakt
  als Dezimalbruch multipliziert; ist das Ergebnis nicht ganzzahlig, ist das Literal
  `E_DURATION` (`0.1 ns`). Übersteigt es i64, ebenfalls `E_DURATION`.
- Folgt dem Suffix `*`, `/` oder `^`, ist es ein Einheitenausdruck nach L4.3
  (`3 s/m` ist ein `float[s/m]`).
- Die Suffixe sind an jeder anderen Stelle gewöhnliche Einheitennamen (`float[s]`,
  `d.as(min)`, `.as(s)`), also `IDENT`. Ein Fließkommaliteral in einer reinen Zeiteinheit
  gibt es nicht: `0.01 s` ist eine Dauer; ein `float[s]` schreibt man `(10 ms).as(s)` (3.3).
- `tick` ist kein Literal, sondern ein eingebauter Bezeichner (3.3, L3.6).

```
vectors
"200 ms"                  => DUR(200000000)
"1.5 s"                   => DUR(1500000000)
"30 min"                  => DUR(1800000000000)
"7 d"                     => DUR(604800000000000)
"1e-3 s"                  => DUR(1000000)
"200 ms * 2"              => DUR(200000000) * INT(2)
"3 s/m"                   => INT(3) IDENT(s) / IDENT(m)
"tick..1 h"               => IDENT(tick) .. DUR(3600000000000)
"d.as(min)"               => IDENT(d) . KW(as) ( IDENT(min) )
"float[s]"                => IDENT(float) [ IDENT(s) ]
"300 d * 1"               => DUR(25920000000000000) * INT(1)
!"0.1 ns"                 => E_DURATION
!"99999999999 d"          => E_DURATION
```

---

## L5 Strings

- **L5.1 Form.** Ein String beginnt und endet mit `"` auf derselben logischen Zeile.
  Der Inhalt darf UTF-8 enthalten. Ein Zeilenende im String ist `E_STRING`.
- **L5.2 Escapes.** `\\`, `\"`, `\n`, `\t`, `\r`, `\0`. Jedes andere `\` ist `E_ESCAPE`.
  Escapes werden beim Tokenisieren aufgelöst; `STRING(text)` in den Vektoren zeigt den
  aufgelösten Text.
- **L5.3 Teilsprachen.** Der Inhalt eines Strings wird je nach Stelle in der Grammatik
  weiter zerlegt. Das geschieht nach dem Parsen, mit den Teilgrammatiken aus
  `takt.ebnf` (`@start`):

| Stelle in der Grammatik | Teilsprache | Inhalt |
|---|---|---|
| `log`, Nachricht von `check`, `expect`, `alert`, `verify`, `verdict`; `send` mit String; `STRING` als `primary` | `format_text` | Text mit `{ausdruck[:spec]}`, Escapes `{{` `}}` |
| `pattern` nach `matches`, `has`, `on`, `until` | `pattern_text` | Text mit `{name:kind}`, `{_}`, Escapes `{{` `}}` |
| `hw(…)`, `sim(…)`, `tick_source`, `node … @ hw(…)` | `address_text` | Segmente mit `/`, Bereich `[a:b]` |
| `import channels from`, `program`, `scenario`-Name, `step`-Name, `label`, `group`, `doc`, `native … from` | keine | reiner Text, `{` ist gewöhnlich |

- **L5.4 Formatstrings.** Innerhalb `{ … }` wird der Ausdruck mit diesem Tokenizer im
  Ausdrucksmodus gelesen; er endet am ersten `:` oder `}` auf Klammertiefe 0. Der
  Ausdruck darf keine Stringliterale und keine `{`/`}` enthalten (`E_FORMAT`). Nach `:`
  folgt `hex`, `.INT` oder `INT` (führende Nullen erlaubt, `{x:08}`). Ein einzelnes `{`
  oder `}` ohne Gegenstück ist `E_FORMAT`.
- **L5.5 Musterliterale.** `{name:kind}` mit `name` als `IDENT` und `kind` aus
  `int hex float word str str<N>`; `{_}` ohne Bindung; `{{` und `}}` sind Escapes.
  Alles andere in geschweiften Klammern ist `E_PATTERN`. Die Zeichenklassen der Arten
  (8.7) gehören zur DFA-Konstruktion, nicht zum Tokenizer:

| Art | Zeichenklasse |
|---|---|
| `int` | `[+-]?[0-9]{1,19}` |
| `hex` | `(0x)?[0-9a-fA-F]{1,16}` |
| `float` | `[+-]?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?` |
| `word` | `[A-Za-z0-9_]{1,64}` |
| `str`, `str<N>` | beliebige Zeichen, höchstens N |
| `_` | wie `str`, ohne Bindung |

- **L5.6 Adressen.** `ADDR_WORD` ist `[A-Za-z0-9_][A-Za-z0-9_.-]*`; Segmente sind durch
  `/` getrennt; ein Segment darf einen Bereich `[INT:INT]` tragen. Alles andere ist
  `E_ADDRESS`.

```
vectors
"\"UPDATE {size} {crc:hex}\\n\""      => STRING(UPDATE {size} {crc:hex}\n)
"\"a\\\"b\""                            => STRING(a"b)
"\"Grüße\""                             => STRING(Grüße)
!"\"abc"                                => E_STRING
!"\"a\\qb\""                            => E_ESCAPE
```

Teilsprachen (Eingabe ist der Stringinhalt, Tokenarten der Teilsprache):

```
vectors-format
"TC {i} over limit: {tcs[i]}"   => TEXT(TC ) EXPR(i) TEXT( over limit: ) EXPR(tcs[i])
"{x:hex} {y:.3} {z:08}"         => EXPR(x) SPEC(hex) TEXT( ) EXPR(y) SPEC(.3) TEXT( ) EXPR(z) SPEC(08)
"{{literal}}"                    => TEXT({literal})
"{i_dut.max()}"                  => EXPR(i_dut.max())
!"{x"                            => E_FORMAT
!"}"                             => E_FORMAT
!"{f(\"a\")}"                    => E_FORMAT
```

```
vectors-pattern
"Erasing sector {n:int}"         => TEXT(Erasing sector ) CAP(n:int)
"Boot v{major:int}.{minor:int}"  => TEXT(Boot v) CAP(major:int) TEXT(.) CAP(minor:int)
"{_}CRC mismatch{_}"             => ANY TEXT(CRC mismatch) ANY
"{name:str<32>}"                 => CAP(name:str<32>)
"{{x}}"                          => TEXT({x})
!"{n}"                           => E_PATTERN
!"{n:bytes}"                     => E_PATTERN
```

```
vectors-address
"daq1/ai0"                       => SEG(daq1) SEG(ai0)
"daq1/tc[0:16]"                  => SEG(daq1) SEG(tc) RANGE(0:16)
"i2c1/0x36/0x0C"                 => SEG(i2c1) SEG(0x36) SEG(0x0C)
"sys/boot_reason"                => SEG(sys) SEG(boot_reason)
"can0/pdo/0x181/0"               => SEG(can0) SEG(pdo) SEG(0x181) SEG(0)
!"daq1//ai0"                     => E_ADDRESS
!"daq1/tc[0-16]"                 => E_ADDRESS
```

---

## L6 Operatoren und Interpunktion

Die Tokens sind genau die nicht-wörtlichen Terminale der Grammatik. Es gilt die längste
Übereinstimmung.

```
->  ..  +=  -=  *=  /=  ==  !=  <=  >=  <<  >>
+  -  *  /  %  &  |  ^  ~  <  >  =  .  ,  :  (  )  [  ]  {  }  @  ?  !
```

- **L6.1 Spitze Klammern.** `<` und `>` sind sowohl Vergleich als auch Typklammern
  (`vec<u8, 4>`). Der Tokenizer unterscheidet das nicht. Ein `>>` ist ein Token; wenn
  der Parser in einem Typausdruck ein einzelnes `>` erwartet, teilt er `>>` in zwei `>`
  (`vec<vec<u8, 4>, 2>`). Dasselbe gilt für `>=` und `>>`-Kombinationen nicht, weil sie
  in Typen nicht vorkommen.
- **L6.2 Ausrufezeichen.** `!` steht in `T!E`; `!=` ist ein eigenes Token. `x!=y` ist
  Vergleich, `ImageHeader!HeaderErr` ist ein Ergebnistyp. Da nach `!` in einem Typ nie
  `=` folgt, gibt es keinen Konflikt.
- **L6.3 Nicht vorhanden.** `=>`, `;`, `\`, `$`, `` ` `` und `'` sind keine Tokens; sie
  erzeugen `E_CHAR`. Einfache Anführungszeichen sind keine Stringbegrenzer.

```
vectors
"a -> B"                  => IDENT(a) -> UPPER(B)
"x += 1"                  => IDENT(x) += INT(1)
"a <= b != c"             => IDENT(a) <= IDENT(b) != IDENT(c)
"x >> 2 << 1"             => IDENT(x) >> INT(2) << INT(1)
"vec<vec<u8, 4>, 2>"      => IDENT(vec) < IDENT(vec) < IDENT(u8) , INT(4) >> , INT(2) >
"T!E T?"                  => UPPER(T) ! UPPER(E) UPPER(T) ?
"x != y"                  => IDENT(x) != IDENT(y)
"@ hw(\"a/b\")"           => @ IDENT(hw) ( STRING(a/b) )
"b[a..c]"                 => IDENT(b) [ IDENT(a) .. IDENT(c) ]
"P[0, 1]"                 => UPPER(P) [ INT(0) , INT(1) ]
"~x & 0xFF"               => ~ IDENT(x) & HEX(0xFF)
!"a => b"                 => E_CHAR
!"x = 'a'"                => E_CHAR
!"a; b"                   => E_CHAR
```

---

## L7 Kanonische Form (`takt fmt`)

Der Formatter erzeugt aus einem Tokenstrom mit Positionen den kanonischen Text (2.1).
Lexikalisch gilt:

- Zeilenenden `\n`, keine BOM, keine Tabulatoren, kein Leerraum am Zeilenende, genau ein
  Zeilenende am Dateiende.
- Einrückung mit 4 Leerzeichen je Stufe; Fortsetzungszeilen in Klammern werden auf die
  öffnende Klammer ausgerichtet.
- Ein Leerzeichen um binäre Operatoren und nach `,` und `:` in Typen und Argumenten;
  kein Leerzeichen innerhalb von Einheitenausdrücken (`K/min`, nicht `K / min`); genau
  ein Leerzeichen zwischen Zahl und Einheit oder Zeitsuffix.
- Zahlenliterale bleiben, wie geschrieben (Unterstriche, Hex-Groß-/Kleinschreibung,
  Dezimaltext). Der Formatter ändert nie einen Wert.
- Kommentare bleiben an ihrer Zeile; ein Leerzeichen nach `#` wird ergänzt.

Ein Programm, das der Formatter unverändert lässt, heißt kanonisch. Der Roundtrip
Parse → Format → Parse muss denselben Tokenstrom ergeben; das ist der Test der
Grammatikzeilen der Inventur (plan.md, Abschnitt 6).

---

## L8 Modi

Der Tokenizer hat zwei Modi. Im **Dateimodus** gelten L1 bis L6 vollständig. Im
**Ausdrucksmodus** (L5.4) werden `NEWLINE`, `INDENT` und `DEDENT` nicht erzeugt und
Stringliterale sind verboten; er endet an `:` oder `}` auf Tiefe 0. Die Teilsprachen
`pattern_text` und `address_text` sind eigene kleine Tokenizer ohne Modi.

---

## L9 Fehler

Jeder Fehler nennt Position, Ursache und einen Vorschlag (0.1). Der Tokenizer bricht beim
ersten Fehler nicht ab, sondern liefert ein Fehlertoken und setzt am nächsten Leerraum
fort, damit der Parser mehrere Fehler melden kann; die Ausgabe ist dann kein gültiges
Programm.

| Code | Ursache | Vorschlag |
|---|---|---|
| `E_BOM` | Byte-Order-Mark am Dateianfang | `takt fmt` entfernt sie |
| `E_CR` | einzelnes `\r` | Zeilenenden `\n` oder `\r\n` verwenden |
| `E_NONASCII` | Nicht-ASCII außerhalb von String und Kommentar | Bezeichner und Einheiten in ASCII schreiben (`degC`, `uA`, `ohm`) |
| `E_TAB` | Tabulator | 4 Leerzeichen je Stufe; `takt fmt` ersetzt |
| `E_INDENT` | Einrückung kein Vielfaches von 4 oder Sprung um mehr als eine Stufe | Block um genau 4 Leerzeichen einrücken |
| `E_DEDENT` | Rückkehr auf eine Einrückung, die nicht offen ist | Einrückung an den umgebenden Block angleichen |
| `E_UNCLOSED` | Dateiende innerhalb offener Klammern | schließende Klammer ergänzen |
| `E_RESERVED` | reserviertes Wort als Bezeichner | anderes Wort wählen; bei `while`: `for` mit Schranke oder `sequence` mit `until` |
| `E_NUMBER` | ungültiges Zahlenliteral | Formen: `42`, `1_000`, `0x1F`, `0b1010`, `0o17`, `4.25`, `1e-3` |
| `E_UNIT_SPACE` | Einheit ohne Leerzeichen nach der Zahl | `85 degC` statt `85degC` |
| `E_DURATION` | Dauer nicht ganzzahlig in ns oder außerhalb i64 | kleinere Einheit wählen (`100 ps` gibt es nicht; `0.1 ns` ist nicht darstellbar) |
| `E_STRING` | Zeilenende oder Dateiende im String | schließendes `"` ergänzen; für mehrzeiligen Text mehrere `log`-Aufrufe |
| `E_ESCAPE` | unbekanntes Escape | erlaubt sind `\\ \" \n \t \r \0` |
| `E_FORMAT` | fehlerhafter Platzhalter im Formatstring | `{ausdruck}` oder `{ausdruck:hex}`; `{{` für ein geschweiftes Zeichen |
| `E_PATTERN` | fehlerhafter Platzhalter im Muster | `{name:int}`, `{name:word}`, `{_}`; `{{` für ein geschweiftes Zeichen |
| `E_ADDRESS` | fehlerhafte Hardware-Adresse | `geraet/kanal`, Bereich als `kanal[0:16]` |
| `E_CHAR` | Zeichen ohne Bedeutung | siehe L6.3 |

Namenskonventionen (2.1) meldet nicht der Tokenizer, sondern die Semantik (Prüfung 2), siehe L3.5.
