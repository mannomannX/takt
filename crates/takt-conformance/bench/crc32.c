/* takt bench (13.8): CRC32 ueber 256 Byte je Tick — die C-Referenz zu
 * `crc32.takt`, mit denselben Pruefungen: Index gegen die Laenge des
 * Puffers, Anfuegen gegen seine Kapazitaet. `faults` zaehlt, was Takt als
 * Fault gemeldet haette; im Lauf bleibt es null. */
#include <stdint.h>

static uint8_t buf[256];
static uint32_t len;
static uint32_t seed = 12345u;
static uint32_t acc;
static volatile uint32_t faults;

static uint32_t crc32(const uint8_t *b, uint32_t n) {
    uint32_t crc = 0xFFFFFFFFu;
    for (uint32_t i = 0; i < 256u; i++) {
        if (i >= n) {
            faults++;
            return 0;
        }
        crc ^= b[i];
        for (int bit = 0; bit < 8; bit++) {
            crc = (crc & 1u) ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
        }
    }
    return crc ^ 0xFFFFFFFFu;
}

void takt_bench_reference(void) {
    len = 0;
    for (int i = 0; i < 256; i++) {
        seed = (uint32_t)((uint64_t)seed * 1664525u + 1013904223u);
        if (len < 256u) {
            buf[len++] = (uint8_t)(seed >> 24);
        }
    }
    acc ^= crc32(buf, len);
}

unsigned long long takt_bench_reference_digest(void) {
    return acc;
}
