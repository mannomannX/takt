"""Tabellen und Referenz der korrekt gerundeten Mathematik in `libtaktm` (4.2).

Drei Aufrufe, alle nur mit der Standardbibliothek (`decimal`, `fractions`):

  python tools/libtaktm.py tables > crates/libtaktm/src/table.rs
      Die Konstanten und Tabellen der Implementierung, auf 256 Bit gerundet.

  python tools/libtaktm.py round < eingaben > vektoren
      Je Zeile `<breite> <funktion>: <argument...>` die Vektorzeile mit dem
      korrekt gerundeten Ergebnis, `... -> <ergebnis>`.

  python tools/libtaktm.py random SEED ANZAHL > crates/libtaktm/tests/random.txt
      ANZAHL Zufallsvektoren je Funktion und Breite.

Die Referenz ist von der Implementierung unabhaengig: Dezimalarithmetik statt
Ganzzahl-Gleitkomma, andere Reduktionen, keine Tabellen. Sie rundet erst, wenn
das Fehlerintervall um ihren Naeherungswert keine Rundungsgrenze enthaelt, und
rechnet sonst mit doppelter Stellenzahl weiter (Ziv). Exakte Ergebnisse von
`pow`, darunter Mittelpunkte zwischen zwei Gleitkommazahlen, rechnet sie mit
Bruechen exakt — dort endete die Schleife nie.
"""

import math
import random
import struct
import sys
from decimal import ROUND_HALF_EVEN, Decimal, localcontext
from fractions import Fraction

# Mantissenbits, kleinster und groesster Exponent, Breite des Bitmusters.
FORMATS = {"f64": (53, -1022, 1023, 64), "f32": (24, -126, 127, 32)}
FUNCTIONS = ["exp", "log", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "pow"]
BINARY = {"atan2", "pow"}

# --------------------------------------------------------------- Bitmuster


def from_bits(bits, width):
    if width == "f64":
        return struct.unpack("<d", struct.pack("<Q", bits))[0]
    return struct.unpack("<f", struct.pack("<I", bits))[0]


def to_bits(x, width):
    if width == "f64":
        return struct.unpack("<Q", struct.pack("<d", x))[0]
    return struct.unpack("<I", struct.pack("<f", x))[0]


def special_bits(kind, negative, width):
    _, _, _, n = FORMATS[width]
    sign = 1 << (n - 1) if negative else 0
    if kind == "zero":
        return sign
    exp_bits = 11 if width == "f64" else 8
    return sign | (((1 << exp_bits) - 1) << (n - 1 - exp_bits))


def round_exact(q, width):
    """Rundet einen Bruch zur naechsten Zahl der Breite, Gleichstand gerade."""
    p, emin, emax, n = FORMATS[width]
    negative = q < 0
    a = -q if negative else q
    if a == 0:
        return special_bits("zero", negative, width)
    e = a.numerator.bit_length() - a.denominator.bit_length()
    if Fraction(2) ** e > a:
        e -= 1
    elif Fraction(2) ** (e + 1) <= a:
        e += 1
    e = max(e, emin)
    quantum = Fraction(2) ** (e - p + 1)
    scaled = a / quantum
    m = scaled.numerator // scaled.denominator
    rest = scaled - m
    if rest > Fraction(1, 2) or (rest == Fraction(1, 2) and m % 2 == 1):
        m += 1
    if m == 1 << p:
        m >>= 1
        e += 1
    if e > emax:
        return special_bits("inf", negative, width)
    sign = 1 << (n - 1) if negative else 0
    if m < 1 << (p - 1):
        return sign | m
    return sign | ((e + (1 << (n - p - 1)) - 1) << (p - 1)) | (m - (1 << (p - 1)))


def settle(value, eps, width):
    """Das korrekt gerundete Ergebnis, wenn `value` mit relativem Fehler
    hoechstens `eps` es eindeutig bestimmt; sonst None."""
    v = Fraction(value)
    slack = abs(v) * Fraction(eps)
    lo, hi = round_exact(v - slack, width), round_exact(v + slack, width)
    return lo if lo == hi else None


# ------------------------------------------------------- Dezimalfunktionen
#
# Jede rechnet mit `prec` Stellen und liefert (Wert, relativer Fehler). Die
# Fehler sind grosszuegig: Eine zu grosse Schranke kostet nur eine weitere
# Runde der Ziv-Schleife, eine zu kleine waere falsch.

_PI = {}


def pi(prec):
    if prec not in _PI:
        with localcontext() as c:
            c.prec = prec + 10

            limit = Decimal(10) ** -(prec + 12)

            def atan_inv(x):
                x = Decimal(x)
                term = 1 / x
                total, k, sign = term, 1, -1
                while True:
                    term /= x * x
                    step = term / (2 * k + 1)
                    if step < limit:
                        return total
                    total += sign * step
                    sign, k = -sign, k + 1

            _PI[prec] = +(16 * atan_inv(5) - 4 * atan_inv(239))
    return _PI[prec]


def d_exp(x, prec):
    with localcontext() as c:
        c.prec = prec
        return x.exp(), Decimal(10) ** (2 - prec)


def d_log(x, prec):
    with localcontext() as c:
        c.prec = prec
        return x.ln(), Decimal(10) ** (2 - prec)


def taylor_sin_cos(r, prec):
    """sin und cos fuer |r| <= 1; der Abbruch liegt relativ zu r und zu 1
    unter 10^-(prec+15)."""
    if r == 0:
        return Decimal(0), Decimal(1)
    with localcontext() as c:
        c.prec = prec + 10
        s, co = Decimal(0), Decimal(0)
        term, n = Decimal(1), 0
        limit = Decimal(10) ** -(prec + 15) * min(1, abs(r))
        while True:
            if n % 4 == 0:
                co += term
            elif n % 4 == 1:
                s += term
            elif n % 4 == 2:
                co -= term
            else:
                s -= term
            n += 1
            term = term * r / n
            if abs(term) < limit and n > 2:
                return s, co


def reduce_half_pi(x, prec):
    """x = k * pi/2 + r mit |r| <= pi/4; liefert (k mod 4, r, Fehler von r)."""
    work = max(0, x.adjusted()) + prec + 30
    with localcontext() as c:
        c.prec = work
        half = pi(work) / 2
        k = (x / half).to_integral_value(rounding=ROUND_HALF_EVEN)
        if k == 0:
            return 0, +x, abs(x) * Decimal(10) ** (1 - work)
        r = x - k * half
        err = (abs(k) + 1) * Decimal(10) ** (2 - work)
    return int(k) % 4, r, err


def d_sin_cos_tan(fun, x, prec):
    k, r, err_r = reduce_half_pi(x, prec)
    s, co = taylor_sin_cos(r, prec)
    with localcontext() as c:
        c.prec = prec + 10
        # Relative Fehler von sin r und cos r: Der Fehler von r geht absolut
        # ein, |sin r| >= 2|r|/pi und cos r >= 0.7 fuer |r| <= pi/4.
        rel_s = 2 * err_r / abs(r) + Decimal(10) ** -(prec + 12)
        rel_c = 2 * err_r + Decimal(10) ** -(prec + 12)
        sin_x, rel_sin = [(s, rel_s), (co, rel_c), (-s, rel_s), (-co, rel_c)][k]
        cos_x, rel_cos = [(co, rel_c), (-s, rel_s), (-co, rel_c), (s, rel_s)][k]
        if fun == "sin":
            value, eps = sin_x, rel_sin
        elif fun == "cos":
            value, eps = cos_x, rel_cos
        else:
            value, eps = sin_x / cos_x, rel_sin + rel_cos
    return value, 2 * eps + Decimal(10) ** (2 - prec)


def d_atan(x, prec):
    """atan einer Dezimalzahl; |x| beliebig."""
    with localcontext() as c:
        c.prec = prec + 20
        if x == 0:
            return Decimal(0), Decimal(0)
        if abs(x) > 1:
            inner, _ = d_atan(1 / x, prec + 10)
            half = pi(prec + 20) / 2
            value = (half if x > 0 else -half) - inner
            return value, Decimal(10) ** (2 - prec)
        # Dreimal halbieren: atan x = 2 atan(x / (1 + sqrt(1 + x^2))).
        a = x
        for _ in range(3):
            a = a / (1 + (1 + a * a).sqrt())
        total, power, n = Decimal(0), a, 0
        a2 = a * a
        limit = abs(a) * Decimal(10) ** -(prec + 15)
        while True:
            step = power / (2 * n + 1)
            total += -step if n % 2 else step
            if abs(step) < limit:
                break
            power *= a2
            n += 1
        return 8 * total, Decimal(10) ** (2 - prec)


def d_atan2(y, x, prec):
    """atan2 fuer endliche, nicht beide null."""
    with localcontext() as c:
        c.prec = prec + 20
        p = pi(prec + 20)
        if x == 0:
            return (p / 2 if y > 0 else -p / 2), Decimal(10) ** (2 - prec)
        a, _ = d_atan(y / x, prec + 10)
        if x > 0:
            return a, Decimal(10) ** (2 - prec)
        return (a + p if y >= 0 else a - p), Decimal(10) ** (2 - prec)


def d_asin_acos(fun, x, prec):
    with localcontext() as c:
        c.prec = prec + 1600  # 1 - x exakt, auch fuer subnormale x
        root = ((1 - x) * (1 + x)).sqrt()
        c.prec = prec + 20
        if fun == "asin":
            return d_atan2(x, +root, prec + 10)
        return d_atan2(+root, x, prec + 10)


def d_pow(x, y, prec):
    with localcontext() as c:
        c.prec = prec + 20
        lg, _ = d_log(abs(x), prec + 20)
        t = y * lg
        value, _ = d_exp(t, prec + 20)
        eps = (abs(t) + 1) * Decimal(10) ** (2 - prec)
        return (-value if x < 0 and is_odd_integer(y) else value), eps


def is_integer(y):
    return y == y.to_integral_value()


def is_odd_integer(y):
    return is_integer(y) and int(y) % 2 == 1


# ------------------------------------------------------ Sonderfaelle, pow


def odd_part(q):
    """q = X * 2^e mit X ungerade (Zaehler) — fuer dyadische q."""
    num, den = q.numerator, q.denominator
    e = 0
    while num % 2 == 0:
        num //= 2
        e += 1
    while den % 2 == 0:
        den //= 2
        e -= 1
    assert den == 1
    return num, e


def iroot(n, k):
    """Die ganze k-te Wurzel von n, wenn sie exakt ist."""
    lo, hi = 0, 1
    while hi**k <= n:
        hi *= 2
    while hi - lo > 1:
        mid = (lo + hi) // 2
        if mid**k <= n:
            lo = mid
        else:
            hi = mid
    return lo if lo**k == n else None


def pow_exact(x, y):
    """|x|^y als Bruch, wenn das Ergebnis dyadisch und kurz ist; sonst None."""
    xq, yq = abs(Fraction(x)), Fraction(y)
    big_x, ex = odd_part(xq)
    big_y, ey = odd_part(abs(yq))
    if yq < 0:
        big_y = -big_y
    if ey >= 0:
        n = big_y << ey
        if big_x == 1:
            if abs(n * ex) > 4096:
                return None
            return Fraction(2) ** (ex * n)
        if abs(n) > 128:
            return None
        return xq**n
    k = -ey
    if big_x != 1 and k > 5:
        # Eine 2^k-te Wurzel einer ungeraden Zahl unter 2^53 ist ab k = 6
        # nur fuer 1 ganz.
        return None
    if (ex * big_y) % (1 << k):
        return None
    shift = (ex * big_y) >> k
    if big_x == 1:
        return Fraction(2) ** shift
    root = iroot(big_x, 1 << k)
    if root is None or big_y < 0 or big_y > 128:
        return None
    return Fraction(root**big_y) * Fraction(2) ** shift


def special(fun, args, width):
    """Ergebnis der Sonderfaelle (Null, Unendlich, Rand); sonst None."""
    inf = float("inf")
    zero = lambda neg: special_bits("zero", neg, width)  # noqa: E731
    infinity = lambda neg: special_bits("inf", neg, width)  # noqa: E731
    x = args[0]
    neg = struct.pack("<d", x)[7] >= 0x80
    if fun == "exp":
        # Jenseits von 1000 und -1100 liegt das Ergebnis weit hinter dem
        # groessten und unter dem halben kleinsten Wert beider Breiten.
        if x > 1000:
            return infinity(False)
        if x < -1100:
            return zero(False)
        if x == 0:
            return round_exact(Fraction(1), width)
    elif fun == "log":
        if x == 1:
            return zero(False)
        if x == 0:
            return infinity(True)
        if x == inf:
            return infinity(False)
    elif fun in ("sin", "tan", "atan", "asin"):
        if x == 0:
            return zero(neg)
        if fun == "atan" and abs(x) == inf:
            return settle_pi(Fraction(1, 2), neg, width)
        if fun == "asin" and abs(x) == 1:
            return settle_pi(Fraction(1, 2), neg, width)
    elif fun == "cos":
        if x == 0:
            return round_exact(Fraction(1), width)
    elif fun == "acos":
        if x == 1:
            return zero(False)
        if x == -1:
            return settle_pi(Fraction(1), False, width)
    elif fun == "atan2":
        y, xx = args
        y_neg = neg
        x_neg = struct.pack("<d", xx)[7] >= 0x80
        if y == 0:
            if x_neg:
                return settle_pi(Fraction(1), y_neg, width)
            return zero(y_neg)
        if abs(y) == inf and abs(xx) == inf:
            return settle_pi(Fraction(3, 4) if x_neg else Fraction(1, 4), y_neg, width)
        if abs(y) == inf or xx == 0:
            return settle_pi(Fraction(1, 2), y_neg, width)
        if abs(xx) == inf:
            return settle_pi(Fraction(1), y_neg, width) if x_neg else zero(y_neg)
    elif fun == "pow":
        y = args[1]
        if y == 0 or x == 1:
            return round_exact(Fraction(1), width)
        if abs(y) == inf:
            if x == -1:
                return round_exact(Fraction(1), width)
            return infinity(False) if (abs(x) > 1) == (y > 0) else zero(False)
        odd = y == int(y) and int(y) % 2 == 1
        negative = neg and odd
        if abs(x) == inf:
            return infinity(negative) if y > 0 else zero(negative)
        if x == 0:
            return infinity(negative) if y < 0 else zero(negative)
        exact = pow_exact(x, y)
        if exact is not None:
            return round_exact(-exact if negative else exact, width)
        # |y ln|x|| ueber 2000: weit jenseits beider Breiten.
        t = y * math.log(abs(x))
        if t > 2000:
            return infinity(negative)
        if t < -2000:
            return zero(negative)
    return None


def settle_pi(factor, negative, width):
    """factor * pi, korrekt gerundet, mit Vorzeichen."""
    prec = 60
    while True:
        value = pi(prec) * Decimal(factor.numerator) / Decimal(factor.denominator)
        bits = settle(-value if negative else value, Decimal(10) ** (2 - prec), width)
        if bits is not None:
            return bits
        prec *= 2


def evaluate(fun, args, prec):
    xs = [Decimal(a) for a in args]
    x = xs[0]
    if fun == "exp":
        return d_exp(x, prec)
    if fun == "log":
        return d_log(x, prec)
    if fun in ("sin", "cos", "tan"):
        return d_sin_cos_tan(fun, x, prec)
    if fun == "atan":
        return d_atan(x, prec)
    if fun in ("asin", "acos"):
        return d_asin_acos(fun, x, prec)
    if fun == "atan2":
        return d_atan2(x, xs[1], prec)
    return d_pow(x, xs[1], prec)


def domain_error(fun, args):
    """NaN-Faelle: Sie stehen nicht in den Vektoren (das Bitmuster von NaN
    ist nicht eindeutig)."""
    x = args[0]
    if any(a != a for a in args):
        return True
    inf = float("inf")
    if fun in ("sin", "cos", "tan") and abs(x) == inf:
        return True
    if fun == "log" and x < 0:
        return True
    if fun in ("asin", "acos") and abs(x) > 1:
        return True
    if fun == "pow":
        y = args[1]
        if x < 0 and abs(x) != inf and abs(y) != inf and y != int(y):
            return True
    return False


def reference(fun, args, width):
    """Das korrekt gerundete Ergebnis als Bitmuster; None bei NaN."""
    if domain_error(fun, args):
        return None
    bits = special(fun, args, width)
    if bits is not None:
        return bits
    prec = 60
    while True:
        value, eps = evaluate(fun, args, prec)
        bits = settle(value, eps, width)
        if bits is not None:
            return bits
        prec *= 2
        if prec > 20000:
            raise RuntimeError(f"keine Entscheidung: {width} {fun} {args}")


# ---------------------------------------------------------------- Tabellen

PREC_TABLE = 130  # Stellen, gut 430 Bit


def const_256(value):
    """(neg, exp, [4 Woerter]) mit value = 0.m * 2^exp, auf 256 Bit gerundet."""
    q = Fraction(value)
    if q == 0:
        return False, 0, [0, 0, 0, 0]
    neg = q < 0
    a = -q if neg else q
    e = a.numerator.bit_length() - a.denominator.bit_length() + 1
    while Fraction(2) ** (e - 1) > a:
        e -= 1
    while Fraction(2) ** e <= a:
        e += 1
    scaled = a * Fraction(2) ** (256 - e)
    m = (scaled.numerator * 2 + scaled.denominator) // (2 * scaled.denominator)
    if m == 1 << 256:
        m >>= 1
        e += 1
    words = [(m >> (64 * i)) & (2**64 - 1) for i in range(4)]
    return neg, e, words


def rust_const(value):
    neg, e, w = const_256(value)
    words = ", ".join(f"0x{x:016x}" for x in w)
    return f"Const {{ neg: {'true' if neg else 'false'}, exp: {e}, m: [{words}] }}"


def two_over_pi_words(count):
    """Die ersten 64*count Nachkommabits von 2/pi, ganzzahlig gerechnet."""
    bits = 64 * count

    def attempt(digits):
        with localcontext() as c:
            c.prec = digits + 10
            p = pi(digits + 10)
            scale = Decimal(2) ** (bits + 1)
            return int((scale / p).to_integral_value(rounding="ROUND_FLOOR"))

    a, b = attempt(bits // 3 + 60), attempt(bits // 3 + 120)
    assert a == b, "2/pi nicht stabil"
    return [(a >> (64 * (count - 1 - i))) & (2**64 - 1) for i in range(count)]


def tables():
    out = []
    w = out.append
    w("//! Konstanten und Tabellen der korrekt gerundeten Funktionen (4.2).")
    w("//!")
    w("//! Erzeugt von `tools/libtaktm.py tables` aus Dezimalarithmetik mit")
    w(f"//! {PREC_TABLE} Stellen; jeder Wert ist auf 256 Bit Mantisse gerundet. Von")
    w("//! Hand nicht aendern — der Generator ist die Quelle.")
    w("")
    w("use crate::big::Const;")
    w("")
    with localcontext() as c:
        c.prec = PREC_TABLE
        ln2 = Decimal(2).ln()
        p = pi(PREC_TABLE)
        w("/// ln 2.")
        w(f"pub(crate) const LN2: Const = {rust_const(ln2)};")
        w("/// 1 / ln 2.")
        w(f"pub(crate) const INV_LN2: Const = {rust_const(1 / ln2)};")
        w("/// pi.")
        w(f"pub(crate) const PI: Const = {rust_const(p)};")
        w("")
        words = two_over_pi_words(23)
        w("/// Die ersten 1472 Nachkommabits von 2/pi, das hoechstwertige Wort zuerst")
        w("/// (Reduktion nach Payne und Hanek).")
        w("pub(crate) const TWO_OVER_PI: [u64; 23] = [")
        for i in range(0, 23, 4):
            w("    " + " ".join(f"0x{x:016x}," for x in words[i : i + 4]))
        w("];")
        w("")
        fact = 1
        w("/// 1 / n! fuer n = 0..=27.")
        w("pub(crate) const INV_FACT: [Const; 28] = [")
        for n in range(28):
            if n:
                fact *= n
            w(f"    {rust_const(Fraction(1, fact))},")
        w("];")
        w("")
        w("/// 1 / (2m + 1) fuer m = 0..=18.")
        w("pub(crate) const INV_ODD: [Const; 19] = [")
        for m in range(19):
            w(f"    {rust_const(Fraction(1, 2 * m + 1))},")
        w("];")
        w("")
        w("/// sin(j / 64) fuer j = 0..=50.")
        w("pub(crate) const SIN64: [Const; 51] = [")
        cos_rows = []
        for j in range(51):
            s, co = taylor_sin_cos(Decimal(j) / 64, PREC_TABLE)
            w(f"    {rust_const(s)},")
            cos_rows.append(co)
        w("];")
        w("")
        w("/// cos(j / 64) fuer j = 0..=50.")
        w("pub(crate) const COS64: [Const; 51] = [")
        for co in cos_rows:
            w(f"    {rust_const(co)},")
        w("];")
        w("")
        w("/// atan(j / 64) fuer j = 0..=64.")
        w("pub(crate) const ATAN64: [Const; 65] = [")
        for j in range(65):
            a, _ = d_atan(Decimal(j) / 64, PREC_TABLE)
            w(f"    {rust_const(a)},")
        w("];")
        w("")
        w("/// ln(1 + j / 128) fuer j = -43..=43, Index j + 43.")
        w("pub(crate) const LOG128: [Const; 87] = [")
        for j in range(-43, 44):
            w(f"    {rust_const((1 + Decimal(j) / 128).ln())},")
        w("];")
    return "\n".join(out) + "\n"


# ---------------------------------------------------------- Zufallsvektoren


def random_float(rng, width, lo_exp, hi_exp, negative=None):
    p, _, _, n = FORMATS[width]
    e = rng.randint(lo_exp, hi_exp)
    m = rng.getrandbits(p - 1) | (1 << (p - 1))
    value = Fraction(m, 1 << (p - 1)) * Fraction(2) ** e
    bits = round_exact(value, width)
    if negative is None:
        negative = rng.random() < 0.5
    return bits | ((1 << (n - 1)) if negative else 0)


def random_args(rng, fun, width):
    big = 1023 if width == "f64" else 127
    tiny = -1074 if width == "f64" else -149
    r = lambda lo, hi, neg=None: random_float(rng, width, lo, hi, neg)  # noqa: E731
    if fun == "exp":
        if rng.random() < 0.2:
            return [r(-60, -1)]
        span = 709 if width == "f64" else 88
        x = Fraction(rng.randint(-1000 * (span + 36), 1000 * span), 1000) + Fraction(rng.getrandbits(40), 1 << 52)
        return [round_exact(x, width)]
    if fun == "log":
        if rng.random() < 0.2:
            return [round_exact(1 + Fraction(rng.randint(-(1 << 20), 1 << 20), 1 << 30), width)]
        return [r(tiny + 30, big, False)]
    if fun in ("sin", "cos", "tan"):
        return [r(-40, big if rng.random() < 0.3 else 20)]
    if fun == "atan":
        return [r(-60, 80)]
    if fun in ("asin", "acos"):
        if rng.random() < 0.3:
            x = 1 - Fraction(rng.getrandbits(30) + 1, 1 << rng.randint(31, 60))
            return [round_exact(x if rng.random() < 0.5 else -x, width)]
        return [r(-40, -1)]
    if fun == "atan2":
        return [r(-60, 60), r(-60, 60)]
    # pow: Basis nahe eins mit grossem Exponenten, sonst gemischt; negative
    # Basen mit ganzzahligem Exponenten.
    choice = rng.random()
    if choice < 0.25:
        x = 1 + Fraction(rng.randint(-(1 << 20), 1 << 20), 1 << 40)
        return [round_exact(x, width), r(10, 30)]
    if choice < 0.4:
        return [r(-5, 5, True), round_exact(Fraction(rng.randint(-60, 60)), width)]
    return [r(-20, 20, False), r(-6, 6)]


def random_vectors(seed, count):
    rng = random.Random(seed)
    lines = [
        "# Zufallsvektoren der korrekt gerundeten Funktionen, erzeugt mit",
        f"# `python tools/libtaktm.py random {seed} {count}`; Referenz in Dezimalarithmetik.",
    ]
    for width in ("f64", "f32"):
        digits = 16 if width == "f64" else 8
        for fun in FUNCTIONS:
            made = 0
            while made < count:
                args = random_args(rng, fun, width)
                floats = [from_bits(a, width) for a in args]
                want = reference(fun, floats, width)
                if want is None:
                    continue
                text = " ".join(f"{a:0{digits}x}" for a in args)
                lines.append(f"{width} {fun}: {text} -> {want:0{digits}x}")
                made += 1
    return "\n".join(lines) + "\n"


def round_lines(stream):
    out = []
    for raw in stream:
        line = raw.strip()
        if not line or line.startswith("#"):
            out.append(line)
            continue
        head, tail = line.split(":", 1)
        width, fun = head.split()
        tail = tail.split("->")[0]
        args = [int(t, 16) for t in tail.split()]
        digits = 16 if width == "f64" else 8
        want = reference(fun, [from_bits(a, width) for a in args], width)
        text = " ".join(f"{a:0{digits}x}" for a in args)
        if want is None:
            out.append(f"# {width} {fun}: {text} -> NaN")
        else:
            out.append(f"{width} {fun}: {text} -> {want:0{digits}x}")
    return "\n".join(out) + "\n"


def main():
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    if len(sys.argv) >= 2 and sys.argv[1] == "tables":
        sys.stdout.write(tables())
    elif len(sys.argv) >= 2 and sys.argv[1] == "round":
        sys.stdout.write(round_lines(sys.stdin))
    elif len(sys.argv) == 4 and sys.argv[1] == "random":
        sys.stdout.write(random_vectors(int(sys.argv[2]), int(sys.argv[3])))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
