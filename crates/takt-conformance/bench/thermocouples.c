/* takt bench (13.8): Pruefung von 16 Thermoelementen je Tick — die
 * C-Referenz zu `thermocouples.takt`. Der Alert je Kanal fuehrt seine
 * Flanke wie Takt (5.6: je Durchlauf einer Schleife eine eigene), die
 * Ergebnisse werden auf Endlichkeit geprueft. */
#include <stdint.h>

static double t[16];
static uint32_t seed = 7u;
static double lo, hi, mean;
static int32_t warm;
static uint8_t active[16];
static volatile uint32_t edges, faults;

static double checked(double x) {
    if (!__builtin_isfinite(x)) {
        faults++;
    }
    return x;
}

void takt_bench_reference(void) {
    for (int i = 0; i < 16; i++) {
        seed = (uint32_t)((uint64_t)seed * 1664525u + 1013904223u);
        t[i] = checked(300.0 + (double)(seed >> 20) * 0.025);
    }
    lo = t[0];
    hi = t[0];
    double sum = 0.0;
    for (int i = 0; i < 16; i++) {
        uint8_t now = t[i] > 350.0;
        if (now != active[i]) {
            active[i] = now;
            edges++;
        }
        if (now && warm < 1000000) {
            warm++;
        }
        if (t[i] < lo) {
            lo = t[i];
        }
        if (t[i] > hi) {
            hi = t[i];
        }
        sum = checked(sum + t[i]);
    }
    mean = checked(sum / 16.0);
}

unsigned long long takt_bench_reference_digest(void) {
    return (unsigned long long)warm;
}
