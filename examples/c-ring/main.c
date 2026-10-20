/* Der Ring fuer Stream-Treiber aus dem C-SDK (`takt_ring.h`, 12.6,
 * plan/m11.md 2.7), als Pruefprogramm auf dem Wirt.
 *
 * Erst die Grenzen auf einem Faden: Reihenfolge, voller Ring mit seiner
 * Luecke, viele Umlaeufe mit einer Groesse, die keine Zweierpotenz ist.
 * Dann wie im Treiber: Ein Faden schreibt wie eine ISR, der Hauptfaden
 * liest wie `poll`; jedes Byte kommt genau einmal und in seiner Reihenfolge
 * an. Der Rueckgabewert ist 0, wenn alles stimmt, sonst die Nummer der
 * ersten Pruefung, die scheiterte.
 *
 *     clang -std=c11 -O2 -pthread -I crates/takt-embed-c/include examples/c-ring/main.c
 */
#define _POSIX_C_SOURCE 200809L

#include <pthread.h>
#include <stdint.h>

#include "takt_ring.h"

/* So viele Bytes schreibt der Faden der ISR. */
#define COUNT 200000u

/* Was die Pruefung auf einem Faden mitschreibt: je Byte sein Wert, eine
 * Luecke als `GAP`. */
#define GAP 0xFFFFu
#define LOG 4096u

static uint8_t g_small[3];
static struct takt_ring g_ring;

/* Das `i`-te Byte der Folge. */
static uint8_t byte_of(uint32_t i)
{
    return (uint8_t)(i % 251u);
}

/* Reihenfolge, voller Ring und Luecke: 0, wenn alles stimmt. */
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
    } else if ((takt_ring_pop(&r, &b) != TAKT_RING_BYTE) || (b != 1u)) {
        failed = 4;
    } else if (takt_ring_push(&r, 5u) != 0u) {
        /* Bis zur Luecke nimmt der Schreiber nichts an. */
        failed = 5;
    } else if ((takt_ring_pop(&r, &b) != TAKT_RING_BYTE) || (takt_ring_pop(&r, &b) != TAKT_RING_BYTE)) {
        failed = 6;
    } else if ((takt_ring_pop(&r, &b) != TAKT_RING_LOST) || (takt_ring_pop(&r, &b) != TAKT_RING_EMPTY)) {
        failed = 7;
    } else if ((takt_ring_push(&r, 6u) == 0u) || (takt_ring_pop(&r, &b) != TAKT_RING_BYTE) || (b != 6u)) {
        failed = 8;
    } else {
        failed = 0;
    }
    return failed;
}

/* Viele Umlaeufe mit drei Plaetzen: 0, wenn ankommt, was gesendet wurde,
 * mit einer Luecke genau dort, wo Bytes verloren gingen. */
static int many_laps(void)
{
    static uint16_t sent[LOG];
    static uint16_t got[LOG];
    struct takt_ring r;
    uint32_t n_sent = 0u;
    uint32_t n_got = 0u;
    uint8_t next = 0u;
    uint8_t b = 0u;
    int failed = 0;
    (void)takt_ring_init(&r, g_small, 3u);
    for (uint32_t lap = 0u; (lap < 300u) && (n_sent < (LOG - 8u)); lap++) {
        for (uint32_t k = 0u; k <= (lap % 5u); k++) {
            if (takt_ring_push(&r, next) != 0u) {
                sent[n_sent] = next;
                n_sent++;
            } else if ((n_sent == 0u) || (sent[n_sent - 1u] != GAP)) {
                sent[n_sent] = GAP;
                n_sent++;
            } else {
                /* Dieselbe Luecke. */
            }
            next++;
        }
        for (uint32_t k = 0u; (k < (lap % 4u)) && (n_got < LOG); k++) {
            uint8_t what = takt_ring_pop(&r, &b);
            if (what == TAKT_RING_BYTE) {
                got[n_got] = b;
                n_got++;
            } else if (what == TAKT_RING_LOST) {
                got[n_got] = GAP;
                n_got++;
            } else {
                /* leer */
            }
        }
    }
    uint8_t rest = takt_ring_pop(&r, &b);
    while ((rest != TAKT_RING_EMPTY) && (n_got < LOG)) {
        got[n_got] = (rest == TAKT_RING_BYTE) ? (uint16_t)b : (uint16_t)GAP;
        n_got++;
        rest = takt_ring_pop(&r, &b);
    }
    if (n_got != n_sent) {
        failed = 10;
    }
    for (uint32_t i = 0u; (i < n_got) && (failed == 0); i++) {
        if (got[i] != sent[i]) {
            failed = 11;
        }
    }
    return failed;
}

/* Der Faden der ISR: schreibt die Folge, wartet nie und versucht ein Byte
 * erneut, wenn der Ring es nicht nahm. */
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
 * einmal und in seiner Reihenfolge ankommt; Luecken stehen vor einem Byte,
 * das der Schreiber erneut schrieb. */
static int two_threads(void)
{
    static uint8_t big[16];
    pthread_t writer;
    int failed = 0;
    (void)takt_ring_init(&g_ring, big, (uint32_t)sizeof big);
    if (pthread_create(&writer, NULL, isr, NULL) != 0) {
        failed = 20;
    } else {
        uint32_t seen = 0u;
        uint8_t b = 0u;
        /* Auch nach einem Fehler bis zum Ende: Sonst wartete der Faden der
         * ISR auf einen Ring, den niemand mehr leert. */
        while (seen < COUNT) {
            if (takt_ring_pop(&g_ring, &b) == TAKT_RING_BYTE) {
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
