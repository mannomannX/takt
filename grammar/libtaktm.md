# libtaktm — Vektoren der Mathematikbibliothek

Normativ für `crates/libtaktm`. Der Test `crates/libtaktm/tests/vectors.rs`
prüft jede Zeile dieses Dokuments; eine Abweichung ist ein Testfehler.

Satz 9.4.4 verlangt bitidentische Ergebnisse auf jedem Target (Referenz
4.2). Die Standardbibliothek der Plattform kann das nicht zusagen —
glibc, musl und die ARM-Bibliotheken unterscheiden sich im letzten Bit.
Diese Vektoren sind der Beleg, dass Takts eigene Mathematik es kann, und
zugleich das **Aufnahmekriterium**: 13.8 nimmt eine Funktion in die
kuratierte Menge auf, *nachdem* ihre Bit-Gleichheit belegt ist.

## Notation

Der Vektorblock ist mit `libtaktm` ausgezeichnet und enthält je Zeile
einen Vektor der Form

    <breite> <funktion>: <argument…> -> <ergebnis>

Breite ist `f64` oder `f32`, Argumente und Ergebnis stehen als
**Bitmuster** in Hexadezimal (16 beziehungsweise 8 Stellen). Bitmuster
statt Dezimalzahlen, damit nicht die Dezimalkonversion mitgeprüft wird —
sie ist nicht Teil der Zusage. Zeilen mit `#` sind Kommentare.

## Welche Funktionen hier stehen

Stufe 1: die Operationen, deren Ergebnis IEEE-754 vorschreibt. Sie
brauchen keine eigene Approximation und sind damit auf jedem konformen
Target dasselbe.

| Funktion | Grundlage |
|---|---|
| `sqrt` | IEEE-754-Operation, korrekt gerundet |
| `fma` | IEEE-754-2008 fusedMultiplyAdd, eine Rundung |
| `round`, `floor`, `ceil`, `trunc` | exakt, nur der Rundungsmodus unterscheidet sie |
| `abs`, `copysign` | exakt, Vorzeichenbits |

Die transzendenten Funktionen (`sin`, `cos`, `tan`, `asin`, `acos`,
`atan`, `atan2`, `exp`, `log`, `pow`) sind **noch nicht kuratiert**. Ihre
Vektoren entstehen mit ihrer Implementierung aus MPFR in hoher Präzision;
bis dahin lehnt der Compiler sie mit einer Stufenmeldung ab, statt ein
Ergebnis zu erfinden, das er nicht garantieren kann (plan/m4.md 2.3b).

## Pflichtfälle

Jede Funktion trägt die Ränder, nicht nur die Mitte: ±0 (das Vorzeichen
überlebt), ±∞, der kleinste und der größte Subnormale, der größte
endliche Wert. Bei `fma` kommen die Fälle hinzu, an denen sich eine
Rundung von zweien unterscheidet — sonst prüfte der Vektor nur, dass
`a * b + c` gerechnet wurde.

NaN steht nicht in den Vektoren: Sein Bitmuster ist nicht eindeutig
(IEEE-754 lässt die Nutzlast offen), und ein Vektor über einem
mehrdeutigen Wert prüft nichts. Dass NaN nicht panickt, prüft der Fuzzer.

```libtaktm
# sqrt: IEEE-754-Operation. Null behaelt ihr Vorzeichen, Unendlich bleibt.
f64 sqrt: 0000000000000000 -> 0000000000000000
f64 sqrt: 8000000000000000 -> 8000000000000000
f64 sqrt: 3ff0000000000000 -> 3ff0000000000000
f64 sqrt: 4000000000000000 -> 3ff6a09e667f3bcd
f64 sqrt: 4010000000000000 -> 4000000000000000
f64 sqrt: 3fe0000000000000 -> 3fe6a09e667f3bcd
f64 sqrt: 7e37e43c8800759c -> 5f138d352e5096af
f64 sqrt: 0000000000000001 -> 1e60000000000000
f64 sqrt: 000fffffffffffff -> 1fffffffffffffff
f64 sqrt: 7fefffffffffffff -> 5fefffffffffffff
f64 sqrt: 7ff0000000000000 -> 7ff0000000000000
f32 sqrt: 00000000 -> 00000000
f32 sqrt: 80000000 -> 80000000
f32 sqrt: 3f800000 -> 3f800000
f32 sqrt: 40000000 -> 3fb504f3
f32 sqrt: 40800000 -> 40000000
f32 sqrt: 3f000000 -> 3f3504f3
f32 sqrt: 00000001 -> 1a3504f3
f32 sqrt: 7f7fffff -> 5f7fffff
f32 sqrt: 7f800000 -> 7f800000

# fma: eine Rundung statt zweier. Zeile 4 und 5 zeigen den Unterschied
# zu `a * b + c` — dort loescht die Summe die fuehrenden Bits aus.
f64 fma: 3ff0000000000000 4000000000000000 4008000000000000 -> 4014000000000000
f64 fma: 0000000000000000 0000000000000000 0000000000000000 -> 0000000000000000
f64 fma: 8000000000000000 3ff0000000000000 8000000000000000 -> 8000000000000000
f64 fma: 3ff0000000000001 3feffffffffffffe bff0000000000000 -> b970000000000000
f64 fma: 0000000000000001 3fe0000000000000 0000000000000000 -> 0000000000000000
f64 fma: 7e37e43c8800759c 7e37e43c8800759c fff0000000000000 -> fff0000000000000
f32 fma: 3f800000 40000000 40400000 -> 40a00000
f32 fma: 00000000 00000000 00000000 -> 00000000

# round: halbe Werte vom Nullpunkt weg (Rusts `f64::round`).
f64 round: 0000000000000000 -> 0000000000000000
f64 round: 8000000000000000 -> 8000000000000000
f64 round: 3fe0000000000000 -> 3ff0000000000000
f64 round: bfe0000000000000 -> bff0000000000000
f64 round: 3ff8000000000000 -> 4000000000000000
f64 round: bff8000000000000 -> c000000000000000
f64 round: 4004000000000000 -> 4008000000000000
f64 round: c004000000000000 -> c008000000000000
f64 round: 3ff6666666666666 -> 3ff0000000000000
f64 round: bff6666666666666 -> bff0000000000000
f64 round: 7e37e43c8800759c -> 7e37e43c8800759c
f64 round: 7ff0000000000000 -> 7ff0000000000000
f64 round: fff0000000000000 -> fff0000000000000
f32 round: 3f000000 -> 3f800000
f32 round: bf000000 -> bf800000
f32 round: 3fc00000 -> 40000000
f32 round: 40200000 -> 40400000

# floor, ceil, trunc: die drei uebrigen Modi.
f64 floor: 0000000000000000 -> 0000000000000000
f64 floor: 8000000000000000 -> 8000000000000000
f64 floor: 3fe0000000000000 -> 0000000000000000
f64 floor: bfe0000000000000 -> bff0000000000000
f64 floor: 3ff8000000000000 -> 3ff0000000000000
f64 floor: bff8000000000000 -> c000000000000000
f64 floor: bfeccccccccccccd -> bff0000000000000
f64 floor: 7e37e43c8800759c -> 7e37e43c8800759c
f64 floor: 7ff0000000000000 -> 7ff0000000000000
f64 floor: fff0000000000000 -> fff0000000000000
f64 ceil: 0000000000000000 -> 0000000000000000
f64 ceil: 8000000000000000 -> 8000000000000000
f64 ceil: 3fe0000000000000 -> 3ff0000000000000
f64 ceil: bfe0000000000000 -> 8000000000000000
f64 ceil: 3ff8000000000000 -> 4000000000000000
f64 ceil: bff8000000000000 -> bff0000000000000
f64 ceil: bfeccccccccccccd -> 8000000000000000
f64 ceil: 7e37e43c8800759c -> 7e37e43c8800759c
f64 ceil: 7ff0000000000000 -> 7ff0000000000000
f64 ceil: fff0000000000000 -> fff0000000000000
f64 trunc: 0000000000000000 -> 0000000000000000
f64 trunc: 8000000000000000 -> 8000000000000000
f64 trunc: 3fe0000000000000 -> 0000000000000000
f64 trunc: bfe0000000000000 -> 8000000000000000
f64 trunc: 3ff8000000000000 -> 3ff0000000000000
f64 trunc: bff8000000000000 -> bff0000000000000
f64 trunc: bfeccccccccccccd -> 8000000000000000
f64 trunc: 7e37e43c8800759c -> 7e37e43c8800759c
f64 trunc: 7ff0000000000000 -> 7ff0000000000000
f64 trunc: fff0000000000000 -> fff0000000000000
f32 floor: 3f000000 -> 00000000
f32 ceil: 3f000000 -> 3f800000
f32 trunc: 3f000000 -> 00000000
f32 floor: bf000000 -> bf800000
f32 ceil: bf000000 -> 80000000
f32 trunc: bf000000 -> 80000000
f32 floor: 3fc00000 -> 3f800000
f32 ceil: 3fc00000 -> 40000000
f32 trunc: 3fc00000 -> 3f800000
f32 floor: bfc00000 -> c0000000
f32 ceil: bfc00000 -> bf800000
f32 trunc: bfc00000 -> bf800000

# abs und copysign: Vorzeichenbits, ohne `std` verfuegbar.
f64 abs: 0000000000000000 -> 0000000000000000
f64 abs: 8000000000000000 -> 0000000000000000
f64 abs: 3ff0000000000000 -> 3ff0000000000000
f64 abs: bff0000000000000 -> 3ff0000000000000
f64 abs: 7e37e43c8800759c -> 7e37e43c8800759c
f64 abs: fe37e43c8800759c -> 7e37e43c8800759c
f64 abs: 0000000000000001 -> 0000000000000001
f64 abs: 7ff0000000000000 -> 7ff0000000000000
f64 abs: fff0000000000000 -> 7ff0000000000000
f64 copysign: 3ff0000000000000 bff0000000000000 -> bff0000000000000
f64 copysign: bff0000000000000 3ff0000000000000 -> 3ff0000000000000
f64 copysign: 0000000000000000 8000000000000000 -> 8000000000000000
f64 copysign: 8000000000000000 0000000000000000 -> 0000000000000000
f64 copysign: 7ff0000000000000 bff0000000000000 -> fff0000000000000
f64 copysign: fff0000000000000 3ff0000000000000 -> 7ff0000000000000
f32 abs: 00000000 -> 00000000
f32 abs: 80000000 -> 00000000
f32 abs: 3f800000 -> 3f800000
f32 abs: bf800000 -> 3f800000
f32 abs: 7f800000 -> 7f800000
f32 abs: ff800000 -> 7f800000
f32 copysign: 3f800000 bf800000 -> bf800000
f32 copysign: bf800000 3f800000 -> 3f800000
```
