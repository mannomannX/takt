# Takt — Dateiformat `TAKT-MIR`

Normative Spezifikation der Serialisierung der MIR in `crates/takt-mir/src/format/`
(Referenz 11.3 „Versionierte Formate", plan/mir.md Abschnitt 4). Sie ist der Vertrag für die
Bytecode-VM (v2), das Replay und den Logik-Hash: gleiche MIR, gleiche Bytes, gleiche Semantik.

Regeln: Leser akzeptieren jede Formatversion bis zu ihrer eigenen; Schreiber schreiben die
neueste. Unbekannte Feldnummern werden überlesen. Ein Feld, das eine spätere Version hinzufügt,
ist optional oder eine Liste, damit alte Dateien lesbar bleiben; Nummern werden nie umvergeben.
Das Format braucht keine Abhängigkeit und keinen `unsafe`-Code; der Leser ist ohne Allokation
über die Stringtabelle hinaus für `no_std` geeignet.

## Notation der Vektoren

Ein Block ```` ```mir ```` enthält je Zeile einen Vektor: `art: eingabe -> hex`. Arten:
`varint` (Zahl → Bytes), `zigzag` (vorzeichenbehaftete Zahl → Varint), `f64` (Dezimalzahl → 8
Bytes), `key` (`nummer/drahtart` → Varint des Schlüssels), `node` (Feldliste → Bytes des
Knotens), `sha256` (ASCII-Text → Hash). Der Test `tests/format_vectors.rs` prüft jede Zeile.

## W1 Zahlen

- **Varint** ist LEB128: sieben Bits je Byte, niederwertige zuerst, Fortsetzungsbit 0x80;
  höchstens 10 Bytes.
- **Zickzack** für vorzeichenbehaftete Zahlen: `(v << 1) ^ (v >> 63)`, dann Varint.
- **f64** als Bitmuster in 8 Bytes Little-Endian (bitidentisch, 4.2); `f32`-Programme tragen
  ihre Werte in `f64` ohne Verlust.
- **bool** ist Varint 0 oder 1; jeder andere Wert ist ein Fehler.

```mir
varint: 0 -> 00
varint: 1 -> 01
varint: 127 -> 7f
varint: 128 -> 80 01
varint: 300 -> ac 02
varint: 18446744073709551615 -> ff ff ff ff ff ff ff ff ff 01
zigzag: 0 -> 00
zigzag: -1 -> 01
zigzag: 1 -> 02
zigzag: -2 -> 03
zigzag: 2147483647 -> fe ff ff ff 0f
zigzag: -9223372036854775808 -> ff ff ff ff ff ff ff ff ff 01
f64: 1.0 -> 00 00 00 00 00 00 f0 3f
f64: -2.5 -> 00 00 00 00 00 00 04 c0
f64: 0.1 -> 9a 99 99 99 99 99 b9 3f
```

## W2 Schlüssel und Drahtarten

Ein Feld beginnt mit dem Schlüssel `(nummer << 2) | drahtart` als Varint. Drahtarten:

| Wert | Name | Inhalt |
|---|---|---|
| 0 | `varint` | Varint: Bool, Ganzzahlen (Zickzack), Indizes, Stringnummern, Enums ohne Nutzlast |
| 1 | `fixed64` | 8 Bytes: `f64` |
| 2 | `bytes` | Varint-Länge, dann Bytes: Knoten, Bytefolgen |

Eine unbekannte Drahtart (3) ist ein Fehler, weil ihre Länge nicht bekannt wäre.

```mir
key: 1/varint -> 04
key: 1/fixed64 -> 05
key: 2/bytes -> 0a
key: 16/varint -> 40
key: 99/bytes -> 8e 03
```

## W3 Knoten

Ein Knoten ist die Folge seiner Felder in aufsteigender Nummer, wie der Schreiber sie erzeugt;
der Leser verlangt keine Reihenfolge. Multiplizität je Feld laut Schema: `one` genau einmal
(fehlt es, Fehler `Missing`; doppelt, `Duplicate`), `dflt` wie `one`, aber fehlt es, gilt beim
Lesen der Default — für Zahlen, die eine spätere Version einem Knoten hinzufügt und deren
Default sagt, was ältere Dateien meinten —, `opt` höchstens einmal, `rep` beliebig oft
(jedes Element ein eigenes Feld derselben Nummer), `meta` wie `opt` mit Default beim Lesen —
Positionen, Bindungen und Metadaten, die in der Logikform fehlen (Abschnitt H).

- **Strings** sind Varint-Nummern in die Stringtabelle des Kopfes (W5); gleiche Strings stehen
  einmal.
- **Enums mit Nutzlast** sind Knoten mit Feld 0 = Variantennummer (Varint) und den Feldern der
  Variante ab 1. **Enums ohne Nutzlast** sind bloße Varints.
- **Paare** `(a, b)` sind Knoten mit den Feldern 1 und 2.
- **Positionen** (`Span`) sind Knoten `{1: Datei, 2: Anfang, 3: Ende}`.
- **Dimensionsvektoren** sind 7 Bytes (Zweierkomplement je Exponent).

Vektoren (Feldliste `nummer:art=wert`, Knoten ohne umgebenden Schlüssel):

```mir
node: 1:varint=5 -> 04 05
node: 1:varint=5 2:zigzag=-3 -> 04 05 08 05
node: 1:bytes=0102 -> 06 02 01 02
node: 3:fixed64=1.0 -> 0d 00 00 00 00 00 00 f0 3f
node: 0:varint=2 1:varint=1 -> 00 02 04 01
node: 1:node(1:varint=7) -> 06 02 04 07
```

## W4 Schema

Die Feldnummern aller Knoten stehen in `crates/takt-mir/src/format/schema.rs`; die Makros
`codec_struct!`, `codec_enum!` und `codec_unit_enum!` sind das Schema selbst (Name, Nummer,
Multiplizität je Feld). Beim Freeze (plan/mir.md, Abschnitt 6) wird jede Änderung dort mit
einem Versionssprung eingetragen.

## W5 Datei

```
magic             8 Bytes  "TAKT-MIR"
format_version    u16 LE   (9)
edition           u32 LE   (2.5; auch in Config.edition)
compiler_version  Varint-Länge + UTF-8
strings           Varint-Anzahl, je String Varint-Länge + UTF-8
body              Varint-Länge + Bytes eines Wurzelknotens mit Feld 1 = Program
```

Versionen: 1 (Freeze), 2 (`ExprKind::Lift`, `Ok`, `Err`, `Intrinsic`; neue Varianten kann ein
Leser der Version 1 nicht überspringen, daher der Sprung), 3 bis 6 (weitere Felder und
Varianten, jeweils überspringbar), 7 (`DeclaredBudget.wcet_ns`, Feld 3 — optional, also für
ältere Leser überspringbar; die Versionsnummer steigt trotzdem, weil ein Leser wissen muss,
ob ein fehlendes `wcet` bedeutet „nicht deklariert“ oder „aus einer Datei, die es nicht
kannte“), 8 (`Config.overrun`, Feld 10 — Metadatum ohne Logikanteil, 7.3; fehlt es, gilt
`fault`), 9 (`CostVec` Felder 8 bis 15, `dflt`: je Zahlklasse die Divisionen, `fma` und
Wurzeln unter ihren Operationen, 7.2; fehlen sie, gilt null — so rechneten die Dateien, die sie
nicht kannten). Ein Leser mit kleinerer
`format_version` als die Datei lehnt sie ab (`UnsupportedVersion`);
alles andere liest er, Unbekanntes überspringend. Der Kopf ist ohne Stringtabelle lesbar
(`read_header`), damit Werkzeuge Edition und Compiler-Version ohne Vollparse zeigen.

## H Logik-Hash und Programm-Hash

`logic_hash` ist SHA-256 über `format_version (u16 LE) ‖ Stringtabelle (je u32-LE-Länge +
UTF-8) ‖ u32-LE-Länge des Rumpfes ‖ Rumpf`, wobei der Rumpf in der **Logikform** geschrieben
ist: Felder der Art `meta` fehlen — Positionen, Bindungen (`hw`/`sim`, `tick_source`,
`target`), Metadaten (`label`, `display`, `group`, `doc`). Edition und `float`-Breite sind
Felder von `Config` und damit enthalten (2.5, 4.2). `program_hash` nimmt dieselben Bytes mit
allen Feldern. Sim- und HW-Build desselben Programms haben denselben Logik-Hash (8.3).

```mir
sha256:  -> e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
sha256: abc -> ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
```
