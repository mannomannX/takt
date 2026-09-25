/* takt bench (13.8): 4×4-Matrix-Update je Tick, `P = A·P·Aᵀ + Q` in f32 —
 * die C-Referenz zu `matrix.takt`. Die Produkte sind `fma`-Ketten in der
 * Reihenfolge von `libtaktm::mat::mul` (4.2: korrekt gerundet, bitgleich),
 * die Summe elementweise. Geprueft wird, was Takt prueft: die Endlichkeit
 * des Ergebnisses (4.1), der Range der Konversion nach `int` und der des
 * Digests. `faults` zaehlt, was Takt gemeldet haette; im Lauf bleibt es
 * null. */
#include <stdint.h>

static const float A[16] = {
    0.95f, 0.1f, 0.0f, 0.0f, 0.0f, 0.95f, 0.1f, 0.0f, 0.0f, 0.0f, 0.95f, 0.1f, 0.0f, 0.0f, 0.0f, 0.95f,
};
static const float Q[16] = {
    0.01f, 0.0f, 0.0f, 0.0f, 0.0f, 0.01f, 0.0f, 0.0f, 0.0f, 0.0f, 0.01f, 0.0f, 0.0f, 0.0f, 0.0f, 0.01f,
};
static float p[16] = {
    1.0f, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f,
};
static int64_t digest;
static volatile uint32_t faults;

/* `out = a · b`, je Element `acc = fma(a[i][l], b[l][j], acc)` ueber `l`. */
static void mul(const float *a, const float *b, float *out) {
    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 4; j++) {
            float acc = 0.0f;
            for (int l = 0; l < 4; l++) {
                acc = __builtin_fmaf(a[i * 4 + l], b[l * 4 + j], acc);
            }
            out[i * 4 + j] = acc;
        }
    }
}

void takt_bench_reference(void) {
    float ap[16], at[16], apa[16], next[16];
    mul(A, p, ap);
    for (int i = 0; i < 4; i++) {
        for (int j = 0; j < 4; j++) {
            at[j * 4 + i] = A[i * 4 + j];
        }
    }
    mul(ap, at, apa);
    for (int k = 0; k < 16; k++) {
        next[k] = apa[k] + Q[k];
        if (!__builtin_isfinite(next[k])) {
            faults++;
            return;
        }
    }
    for (int k = 0; k < 16; k++) {
        p[k] = next[k];
    }
    /* `floor` nach `int` (4.1): ausserhalb des Ranges ein Fault. Fuer
     * |x| < 2^63 schneidet die Konversion ab; eine negative Zahl mit Rest
     * liegt dann eins zu hoch. */
    float x = p[0] * 1000000.0f;
    if (!(x >= -9223372036854775808.0f && x < 9223372036854775808.0f)) {
        faults++;
        return;
    }
    int64_t f = (int64_t)x;
    if ((float)f > x) {
        f--;
    }
    int64_t d = f % 65536;
    if (d < 0) {
        faults++;
        return;
    }
    digest = d;
}

unsigned long long takt_bench_reference_digest(void) {
    return (unsigned long long)digest;
}
