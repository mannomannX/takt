"""Erzeugt die Twiddle-Tabelle von `fft256` (takt-native/src/fft_table.rs).

cos(2*pi*k/256) und sin(2*pi*k/256) fuer k = 0..128, mit 60 Dezimalstellen
gerechnet und korrekt nach binary64 und binary32 gerundet (round to nearest,
ties to even). Die Tabelle steht als Bitmuster im Quelltext: Kein Ziel rechnet
sie zur Laufzeit, also ist sie ueberall dieselbe (4.2, 4.5).

Aufruf: python tools/fft_twiddles.py > crates/takt-native/src/fft_table.rs
"""

import struct
from decimal import Decimal, getcontext

getcontext().prec = 80
N = 256


def pi():
    # Machin: pi = 16 atan(1/5) - 4 atan(1/239).
    def atan_inv(x):
        x = Decimal(x)
        term = 1 / x
        total, k, sign = term, 1, -1
        while True:
            term /= x * x
            step = term / (2 * k + 1)
            if step == 0:
                return total
            total += sign * step
            sign, k = -sign, k + 1

    return 16 * atan_inv(5) - 4 * atan_inv(239)


def cos_sin(theta):
    # Taylor-Reihen; |theta| <= pi konvergiert bei 80 Stellen schnell.
    c, s = Decimal(0), Decimal(0)
    term, n = Decimal(1), 0
    while True:
        if n % 4 == 0:
            c += term
        elif n % 4 == 1:
            s += term
        elif n % 4 == 2:
            c -= term
        else:
            s -= term
        n += 1
        term = term * theta / n
        if abs(term) < Decimal(10) ** -75:
            return c, s


def exact(x):
    # cos(pi/2), sin(0) und sin(pi) sind exakt null; die Reihe laesst einen
    # Rest weit unter jeder Rundung.
    return Decimal(0) if abs(x) < Decimal(10) ** -60 else x


def f64_bits(x):
    # float(Decimal) ist korrekt gerundet.
    return struct.unpack("<Q", struct.pack("<d", float(exact(x))))[0]


def f32_bits(x):
    # Kandidaten um den nach binary32 gerundeten Betrag; gewaehlt wird der
    # naechste zum exakten Wert, bei Gleichstand der mit gerader Mantisse.
    # Das Vorzeichen kommt danach: Die Nachbarn eines Betrags bleiben Betraege.
    x = exact(x)
    sign = 0x8000_0000 if x < 0 else 0
    magnitude = abs(x)
    near = struct.unpack("<I", struct.pack("<f", float(magnitude)))[0]
    best = None
    for bits in (near - 1, near, near + 1):
        if bits < 0:
            continue
        value = Decimal(struct.unpack("<f", struct.pack("<I", bits))[0])
        key = (abs(value - magnitude), bits & 1)
        if best is None or key < best[0]:
            best = (key, bits)
    return best[1] | sign


def main():
    p = pi()
    rows = [cos_sin(2 * p * k / N) for k in range(N // 2 + 1)]
    print("//! Twiddle-Faktoren von `fft256`: cos und sin von 2*pi*k/256 fuer")
    print("//! k = 0..=128, korrekt gerundet. Erzeugt von `tools/fft_twiddles.py`;")
    print("//! nicht von Hand aendern.")
    print()
    print("/// cos(2*pi*k/256) als binary64.")
    print(f"pub const COS64: [u64; {len(rows)}] = [")
    for c, _ in rows:
        print(f"    0x{f64_bits(c):016x},")
    print("];")
    print()
    print("/// sin(2*pi*k/256) als binary64.")
    print(f"pub const SIN64: [u64; {len(rows)}] = [")
    for _, s in rows:
        print(f"    0x{f64_bits(s):016x},")
    print("];")
    print()
    print("/// cos(2*pi*k/256) als binary32.")
    print(f"pub const COS32: [u32; {len(rows)}] = [")
    for c, _ in rows:
        print(f"    0x{f32_bits(c):08x},")
    print("];")
    print()
    print("/// sin(2*pi*k/256) als binary32.")
    print(f"pub const SIN32: [u32; {len(rows)}] = [")
    for _, s in rows:
        print(f"    0x{f32_bits(s):08x},")
    print("];")


if __name__ == "__main__":
    main()
