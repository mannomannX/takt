# Konformitaetsvektoren der nativen Funktionen

Diese Datei ist **normativ** (4.5, 13.8). Eine native Funktion kommt in
die kuratierte Menge, *nachdem* ihre Vektoren gruen sind — dieselbe
Huerde wie bei `libtaktm` (`grammar/libtaktm.md`).

## Warum diese Form

Die Vektoren stehen als Text und nicht als Rust-Tabelle, aus denselben
Gruenden wie bei `libtaktm`: Sie sind ohne Werkzeug lesbar, sie sind eine
Quelle fuer Host und Target, und das Projekt hat die Konvention bereits
dreimal (`lexer.md`, `mir-format.md`, `trace.md`).

Die Eingabe steht **als Hexadezimalfolge**, nicht als Text: `"abc"` waere
eine Frage nach der Kodierung, `616263` ist keine. Ein Bindestrich ist
die leere Eingabe — auch sie hat ein definiertes Ergebnis, und gerade
dort sitzen Fehler.

## Herkunft

Die Werte fuer `crc32` kommen aus `zlib.crc32` (IEEE 802.3), die uebrigen
sind nach ihren Normen nachgerechnet (CRC-32C: Castagnoli, iSCSI; CRC-16:
IBM/ARC, Modbus RTU; `sum8`: Summe modulo 256). Keiner stammt aus der
Implementierung dieses Projekts — sonst pruefte der Test sie gegen sich
selbst.

Der bekannteste Pruefwert steht mit dabei: `123456789` ist der
Check-Wert, den die CRC-Kataloge fuer jedes Polynom nennen.

## Format

Je Zeile `<funktion> <eingabe als hex>: <ergebnis als hex>`.

```takt-native
crc32    -: 00000000
crc32c   -: 00000000
crc16    -: 0000
sum8     -: 00
crc32    00: d202ef8d
crc32c   00: 527d5351
crc16    00: 0000
sum8     00: 00
crc32    ff: ff000000
crc32c   ff: ff000000
crc16    ff: 4040
sum8     ff: ff
crc32    61: e8b7be43
crc32c   61: c1d04330
crc16    61: e8c1
sum8     61: 61
crc32    616263: 352441c2
crc32c   616263: 364b3fb7
crc16    616263: 9738
sum8     616263: 26
crc32    313233343536373839: cbf43926
crc32c   313233343536373839: e3069283
crc16    313233343536373839: bb3d
sum8     313233343536373839: dd
crc32    00010203040506070809: 456cd746
crc32c   00010203040506070809: 022c2131
crc16    00010203040506070809: 4204
sum8     00010203040506070809: 2d
crc32    deadbeef: 7c9ca35a
crc32c   deadbeef: f1dc778e
crc16    deadbeef: e59b
sum8     deadbeef: 38
crc32    ffffffffffffffff: 2144df1c
crc32c   ffffffffffffffff: 48674bc7
crc16    ffffffffffffffff: 8441
sum8     ffffffffffffffff: f8
crc32    00000000000000000000000000000000000000000000000000000000000000: 6909acb0
crc32c   00000000000000000000000000000000000000000000000000000000000000: 1ed37c4b
crc16    00000000000000000000000000000000000000000000000000000000000000: 0000
sum8     00000000000000000000000000000000000000000000000000000000000000: 00
```
