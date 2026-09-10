# Takt — Kanonische Form (`takt fmt`)

Normative Spezifikation des Formatters in `crates/takt-syntax/src/fmt/`. Sie ergänzt
`lexer.md` (Abschnitt L7 verweist hierher) und `takt.ebnf`; die Entwurfsentscheidungen mit
Begründung stehen in `plan/formatter.md`.

Der Formatter erzeugt aus einer syntaktisch gültigen Datei genau einen Text, die *kanonische
Form*. Er bricht keine Zeile um, fügt keine zusammen und behält die strukturellen Wahlen des
Autors; er normiert Leerraum, Einrückung, Spaltenausrichtung, Kommentarabstand und Leerzeilen.
Eine Datei mit Tokenizer- oder Parserfehlern bleibt unverändert.

Garantien: `tokens(format(s))` gleicht `tokens(s)` in Art und Text (Dauern mit einem
Leerzeichen), `parse(format(s))` gleicht `parse(s)`, `format(format(s)) == format(s)`, und die
Kommentare bleiben als Multimenge erhalten. Die Tests dazu: `tests/format_vectors.rs` (die
Vektoren dieser Datei), `tests/format_roundtrip.rs`, `tests/format_canonical.rs`.

## Notation der Vektoren

Ein Block ```` ```fmt ```` enthält die Eingabe, eine Zeile `---`, die erwartete Ausgabe. Beide
werden als Schnipsel gelesen (`format_snippet`); die Ausgabe muss zusätzlich kanonisch sein.

## F1 Zeilen und Einrückung

- Zeilenenden `\n`; keine BOM; keine Tabulatoren (die Einrückung wird neu erzeugt); kein
  Leerraum am Zeilenende; genau ein Zeilenende am Dateiende; keine Leerzeile am Dateianfang.
- 4 Leerzeichen je Blockstufe.
- Zeilenumbrüche innerhalb von Klammern sind Fortsetzungszeilen des Autors und bleiben. Ihre
  Einrückung: die Spalte hinter der öffnenden Klammer; stand die Klammer am Zeilenende, die
  Einrückung der öffnenden Zeile plus 4; eine schließende Klammer auf eigener Zeile steht auf
  der Einrückung der öffnenden Zeile.
- Eine Zeile mit Fortsetzung nimmt an keiner Spaltenausrichtung teil.

```fmt
var t = f(a,
    b)
var u = g(
  a, b
  )
---
var t = f(a,
          b)
var u = g(
    a, b
)
```

## F2 Leerraum in Deklarationen und Köpfen

- Ein Leerzeichen zwischen Wörtern (`tunable param`, `native fn`, `persist var`).
- Typangabe in Deklarationszeilen (`var`, `input`/`output`, `param`, `const`, `persist var`,
  Record- und Bitfelder) mit Leerzeichen vor und nach dem Doppelpunkt: `var x : float[V]`.
- Parameter in Signaturen und `type T: pod` ohne Leerzeichen vor dem Doppelpunkt:
  `fn f(x: float[U], lo: float[U]) -> float[U]:`.
- Blockköpfe ohne Leerzeichen vor dem Doppelpunkt: `state A:`, `if x > 0:`, `when c:`.
- `=`, `->`, `@`, `with` und Attribute mit einem Leerzeichen; Attribute durch `, ` getrennt.
- Generik- und Parameterklammer direkt am Namen: `clamp[U](x: float[U])`.

```fmt
var  x:float[V]=0 V
input   i_u:float[A] in -45..45 A@hw("adc1/i_u")with max_age=100 us,safe=0 A
fn clamp[U](x:float[U],lo:float[U],hi:float[U])->float[U]:
    return lo if x<lo else x
block win[U,const N in 1..8](k:float[U]=1 V):
    var s:float[U]=0 V
    step(x:float[U])->float[U]:
        return x
---
var x : float[V] = 0 V
input  i_u : float[A] in -45..45 A @ hw("adc1/i_u") with max_age = 100 us, safe = 0 A
fn clamp[U](x: float[U], lo: float[U], hi: float[U]) -> float[U]:
    return lo if x < lo else x
block win[U, const N in 1..8](k: float[U] = 1 V):
    var s : float[U] = 0 V
    step(x: float[U]) -> float[U]:
        return x
```

## F3 Leerraum in Ausdrücken

- Binäre Operatoren mit einem Leerzeichen: `a + b`, `a and b`, `a << 2`, `a >> 2`,
  `x matches P as m`, `x as u16`, `a if c else b`, `a implies b`.
- Unäre Operatoren ohne: `-x`, `~x`; `not x` mit Leerzeichen.
- Klammern innen ohne Leerraum, Kommas mit einem Leerzeichen danach: `f(a, b)`, `[1, 2]`,
  `(a, b)`, `x[i, j]`, `x[a..b]`, `f[U, 1/s](k = 1)`. Benannte Argumente `k = 1`.
- Member ohne Leerraum: `x.y`, `x.step(e, 1 ms)`. Ranges ohne Leerraum: `0..100`.
- Zahl und Einheit mit genau einem Leerzeichen, die Einheit kompakt (`5 K/min`,
  `9.81 m/s^2`); Dauern mit genau einem Leerzeichen (`200 ms`).
- Literaltext unverändert: `0xFF_FF`, `1_000`, `4.25`, `1e-3`, Strings samt Escapes.

```fmt
x=a+b*(c-d)- -e
y=not a and(b or c)
z=(hi if x>hi else x)as u16
w=f[U,1/s](k=1,m=[1,2])+v.step(e,1 ms)+t[i,j]+t[a..b]+p.q
v=5   K/min+9.81 m/s^2
d=200    ms
s = a<<2|b>>2
---
x = a + b * (c - d) - -e
y = not a and (b or c)
z = (hi if x > hi else x) as u16
w = f[U, 1/s](k = 1, m = [1, 2]) + v.step(e, 1 ms) + t[i, j] + t[a..b] + p.q
v = 5 K/min + 9.81 m/s^2
d = 200 ms
s = a << 2 | b >> 2
```

## F4 Typen

`float[V]`, `u16[mV]`, `[16] float[degC]` (Leerzeichen nach dem Array-Präfix), `bytes<64>`,
`vec<u8, 4>`, `map<u16, int, 8>`, `mat<2, 2>[K]`, `mat[X, 1/X]`, `T?`, `Header!ParseErr`,
`float in 0..1`, `Duration in tick..1 h`.

```fmt
var a:[16]float[degC]=default
var b:vec<u8,4> =default
var c:mat<2,2>[K]=default
var d:Header!ParseErr=default
var e:map<u16,int,8> =default
var f:float in 0..1=0
---
var a : [16] float[degC] = default
var b : vec<u8, 4> = default
var c : mat<2, 2>[K] = default
var d : Header!ParseErr = default
var e : map<u16, int, 8> = default
var f : float in 0..1 = 0
```

## F5 Formen des Autors bleiben

- Ein Körper mit genau einem einfachen Statement darf einzeilig stehen (`when c: -> X`,
  `enter: led = 0`); der Formatter behält die Wahl bei.
- `enum` einzeilig oder als Block bleibt, wie geschrieben.
- Klammern um Ausdrücke bleiben.

```fmt
enum Mode:   A,B,C
enum Kind:
    ON
    OFF
state A:
    enter:   led=0
    when go:->B
    when stop:
        led=1
        ->C
---
enum Mode: A, B, C
enum Kind:
    ON
    OFF
state A:
    enter: led = 0
    when go: -> B
    when stop:
        led = 1
        -> C
```

## F6 Spaltenausrichtung

Direkt aufeinanderfolgende Zeilen derselben Art auf gleicher Einrückung bilden eine
Laufgruppe; eine Leerzeile, eine Kommentarzeile, eine Zeile anderer Art oder eine
Fortsetzungszeile beendet sie. Innerhalb der Gruppe werden die Zellen auf gemeinsame Breite
aufgefüllt. Die letzte Zelle einer Zeile wird nie aufgefüllt und zählt nicht für die
Spaltenbreite (wie bei gofmts tabwriter): eine Variante `B(x: u8)` ohne Diskriminante
verbreitert die Namensspalte nicht.

| Art | Zellen |
|---|---|
| `input`/`output` | Richtung (`input` auf die Breite von `output` aufgefüllt) · Name · `: Typ` · `@ Bindung` · `with …` |
| `var`, `pub var`, `persist var`, `param`, `tunable param`, `const` | Schlüsselwort und Name · Rest (`: Typ = Wert …`) |
| Record-Felder, Bitfelder | Name · Rest |
| Varianten im Block (nur vor `= Diskriminante`), Systemeinträge, Profileinträge | Name · Rest |
| `unit`, `type` | Schlüsselwort und Name · `= …` |

```fmt
input i_u:float[A] in -45..45 A @ hw("adc1/i_u") with max_age = 100 us
input v_dc:float[V] in 0..60 V @ hw("adc1/v_dc") with max_age = 200 us
output gate_en:bool @ hw("drv/gate_en") with safe = false

var integral:float[V] = 0 V
var n = 0
pub var soc:float[pct] = 0 pct
record Header:
    magic:u16 = 0xA55A
    kind:FrameKind
    flags:u16 with bits:
        retry:bool at 0
        prio:u8 at 4..7
---
input  i_u     : float[A] in -45..45 A @ hw("adc1/i_u")    with max_age = 100 us
input  v_dc    : float[V] in 0..60 V   @ hw("adc1/v_dc")   with max_age = 200 us
output gate_en : bool                  @ hw("drv/gate_en") with safe = false

var integral : float[V] = 0 V
var n        = 0
pub var soc  : float[pct] = 0 pct
record Header:
    magic : u16 = 0xA55A
    kind  : FrameKind
    flags : u16 with bits:
        retry : bool at 0
        prio  : u8 at 4..7
```

## F7 Kommentare

- `#Text` wird `# Text`; ein `#` ohne Text bleibt. Kommentare werden nie umgebrochen oder
  verschoben.
- Ein Kommentar auf eigener Zeile steht auf der Einrückung der folgenden Codezeile. Vor einem
  Blockende (`DEDENT`) entscheidet seine eigene Spalte, ob er noch zum Block gehört.
- Ein nachgestellter Kommentar steht mit mindestens zwei Leerzeichen hinter dem Code;
  aufeinanderfolgende Zeilen mit nachgestelltem Kommentar (gleiche Einrückung) richten ihn in
  einer Spalte aus.
- Ein Kommentar in Klammern steht auf einer eigenen Fortsetzungszeile.

```fmt
#Kopf
machine m:
    initial A
    state A:
        loop:
            x = 1 # kurz
            long_name = 2   #laenger
                # noch im Block
        # im Zustand
    # in der Maschine
    state B:
        loop: pass
---
# Kopf
machine m:
    initial A
    state A:
        loop:
            x = 1          # kurz
            long_name = 2  # laenger
            # noch im Block
        # im Zustand
    # in der Maschine
    state B:
        loop: pass
```

## F8 Leerzeilen

Höchstens eine Leerzeile, wo der Autor eine oder mehr hatte; keine am Anfang eines Blocks,
keine am Dateianfang; eine Leerzeile vor einem Blockende gehört hinter den Block. Der
Formatter fügt keine Leerzeilen hinzu.

```fmt


const A = 1


const B = 2
machine m:

    initial A
    state A:
        loop: pass

    state B:
        loop: pass

---
const A = 1

const B = 2
machine m:
    initial A
    state A:
        loop: pass

    state B:
        loop: pass
```
