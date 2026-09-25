/* takt bench (13.8): PID-Schleife bei 1 kHz, in f32 — die C-Referenz zu
 * `pid.takt`. Jedes zugewiesene Ergebnis wird auf Endlichkeit geprueft,
 * wie Takt es fuer Fliesskomma verlangt (4.1: nicht endlich ist ein
 * Fault); ein nicht endliches Zwischenergebnis setzt sich bis dorthin
 * fort. `faults` zaehlt, was Takt gemeldet haette; im Lauf bleibt es null. */
#include <stdint.h>

static float y, integ, e_last, sp = 1.0f;
static int32_t n, settled;
static volatile uint32_t faults;

static float checked(float x) {
    if (!__builtin_isfinite(x)) {
        faults++;
    }
    return x;
}

void takt_bench_reference(void) {
    if (n == 499) {
        n = 0;
        sp = checked(2.0f - sp);
    } else {
        n++;
    }
    float e = checked(sp - y);
    integ = checked(integ + e * 0.001f);
    if (integ > 10.0f) {
        integ = 10.0f;
    }
    if (integ < -10.0f) {
        integ = -10.0f;
    }
    float d = checked((e - e_last) * 1000.0f);
    e_last = e;
    float u = checked(2.0f * e + 0.5f * integ + 0.01f * d);
    if (u > 5.0f) {
        u = 5.0f;
    }
    if (u < -5.0f) {
        u = -5.0f;
    }
    y = checked(y + (u - y) * 0.05f);
    if (e < 0.01f && e > -0.01f && settled < 1000000) {
        settled++;
    }
}

unsigned long long takt_bench_reference_digest(void) {
    return (unsigned long long)settled;
}
