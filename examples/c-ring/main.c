/* Der Ring fuer Stream-Treiber aus dem C-SDK (`takt_ring.h`, 12.6,
 * plan/m11.md 2.7), als Pruefprogramm auf dem Wirt.
 *
 * Erst die Grenzen auf einem Faden: Reihenfolge, voller Ring, viele
 * Umlaeufe mit einer Groesse, die keine Zweierpotenz ist. Dann wie im
 * Treiber: Ein Faden schreibt wie eine ISR, der Hauptfaden liest wie
 * `poll`; jedes Byte kommt genau einmal und in seiner Reihenfolge an. Der
 * Rueckgabewert ist 0, wenn alles stimmt, sonst die Nummer der ersten
 * Pruefung, die scheiterte.
 *
 *     clang -std=c11 -O2 -pthread -I crates/takt-embed-c/include examples/c-ring/main.c
 */
#define _POSIX_C_SOURCE 200809L

#include <pthread.h>
#include <stdint.h>

#include "takt_ring.h"

/* So viele Bytes schreibt der Faden der ISR. */
#define COUNT 200000u

static uint8_t g_small[3];
static uint8_t g_big[16];
static struct takt_ring g_ring;

/* Das `i`-te Byte der Folge. */
static uint8_t byte_of(uint32_t i)
{
    return (uint8_t)(i % 251u);
}

/* Reihenfolge und voller Ring: 0, wenn beides stimmt. */
static int order_and_full(void)
{
    struct takt_ring r;
    uint8_t b = 0u;
    int failed = 0;
    if (takt_ring_init(&r, g_small, 3u) == 0u) {
        failed = 1;
    } else if ((takt_ring_push(&r, 1u) == 0u) || (takt_ring_push(&r, 2u) == 0u) || (takt_ring_push(&r, 3u) == 0u)) {
        failed = 2;
    } else if ((takt_ring_push(&r, 4u) != 0u) || (takt_ring_lost(&r) != 1u) || (takt_ring_len(&r) != 3u)) {
        failed = 3;
    } else if ((takt_ring_pop(&r, &b) == 0u) || (b != 1u)) {
        failed = 4;
    } else {
        failed = 0;
    }
    return failed;
}

/* Viele Umlaeufe mit drei Plaetzen: 0, wenn nichts verloren geht oder
 * doppelt kommt. */
static int many_laps(void)
{
    struct takt_ring r;
    uint8_t next = 0u;
    uint8_t expected = 0u;
    uint8_t b = 0u;
    int failed = 0;
    (void)takt_ring_init(&r, g_small, 3u);
    for (uint32_t lap = 0u; (lap < 300u) && (failed == 0); lap++) {
        for (uint32_t k = 0u; k <= (lap % 4u); k++) {
            if (takt_ring_push(&r, next) != 0u) {
                next++;
            }
        }
        for (uint32_t k = 0u; (k < (lap % 3u)) && (failed == 0); k++) {
            if (takt_ring_pop(&r, &b) != 0u) {
                if (b != expected) {
                    failed = 10;
                }
                expected++;
            }
        }
        if (takt_ring_len(&r) > 3u) {
            failed = 11;
        }
    }
    while ((failed == 0) && (takt_ring_pop(&r, &b) != 0u)) {
        if (b != expected) {
            failed = 12;
        }
        expected++;
    }
    if ((failed == 0) && (expected != next)) {
        failed = 13;
    }
    return failed;
}

/* Der Faden der ISR: schreibt die Folge, wartet nie und versucht ein Byte
 * erneut, wenn der Ring voll war. */
static void *isr(void *arg)
{
    uint32_t i = 0u;
    (void)arg;
    while (i < COUNT) {
        if (takt_ring_push(&g_ring, byte_of(i)) != 0u) {
            i++;
        }
    }
    return NULL;
}

/* Ein Schreiber und ein Leser auf zwei Faeden: 0, wenn jedes Byte genau
 * einmal und in seiner Reihenfolge ankommt. */
static int two_threads(void)
{
    pthread_t writer;
    uint32_t seen = 0u;
    uint8_t b = 0u;
    int failed = 0;
    (void)takt_ring_init(&g_ring, g_big, (uint32_t)sizeof g_big);
    if (pthread_create(&writer, NULL, isr, NULL) != 0) {
        failed = 20;
    } else {
        /* Auch nach einem Fehler bis zum Ende: Sonst wartete der Faden der
         * ISR auf einen Ring, den niemand mehr leert. */
        while (seen < COUNT) {
            if (takt_ring_pop(&g_ring, &b) != 0u) {
                if ((failed == 0) && (b != byte_of(seen))) {
                    failed = 21;
                }
                seen++;
            }
        }
        if (pthread_join(writer, NULL) != 0) {
            failed = 22;
        }
    }
    return failed;
}

int main(void)
{
    int failed = order_and_full();
    if (failed == 0) {
        failed = many_laps();
    }
    if (failed == 0) {
        failed = two_threads();
    }
    return failed;
}
