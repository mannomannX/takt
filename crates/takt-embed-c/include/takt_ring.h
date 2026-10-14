/* takt_ring.h - ein Ring fuer einen Schreiber und einen Leser (12.6,
 * plan/m11.md 2.7): Der Treiber eines Eingabestroms schreibt in seiner ISR,
 * sein `poll` liest im Schritt, ohne Sperre.
 *
 * Kopf und Ende schreibt je nur ihre Seite, und beide laufen modulo
 * `2 * size`: So unterscheiden sie einen vollen Ring von einem leeren, fuer
 * jede Groesse. Sie stehen hinter den `__atomic`-Builtins von GCC und
 * clang mit `__ATOMIC_SEQ_CST`: Der Schreiber legt das Byte ab, bevor er
 * den Kopf weiterstellt, und der Leser nimmt es, bevor er das Ende
 * weiterstellt. Lesen und Speichern genuegen, ein Compare-and-Swap braucht
 * der Ring nicht; er laeuft auch auf Kernen ohne (Cortex-M0).
 *
 * Gebrauch: Der Wirt legt Speicher und Ring statisch an, ruft vor dem
 * ersten Interrupt `takt_ring_init`, die ISR ruft `takt_ring_push`, das
 * `poll` des Treibers `takt_ring_pop`. Ein Ring hat genau einen Schreiber
 * und genau einen Leser.
 */
#ifndef TAKT_RING_H
#define TAKT_RING_H

#include <stddef.h>
#include <stdint.h>

#if !defined(__GNUC__) && !defined(__clang__)
#error "takt_ring.h braucht die __atomic-Builtins von GCC oder clang"
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* Ein Ring ueber `size` Bytes des Wirts; die Felder liest und schreibt nur
 * dieser Kopf. */
struct takt_ring {
    uint8_t *slots;
    uint32_t size;
    /* Der naechste Platz des Schreibers, modulo `2 * size`. */
    uint32_t head;
    /* Der naechste Platz des Lesers, modulo `2 * size`. */
    uint32_t tail;
    /* Wie viele Bytes der Schreiber verwarf, weil der Ring voll war. */
    uint32_t lost;
};

/* Die groesste Zahl von Plaetzen: `2 * size` passt in 32 Bit. */
#define TAKT_RING_MAX 0x7FFFFFFFu

/* Bindet den Ring an `size` Bytes ab `slots`; 1 bei Erfolg, 0 ohne
 * Speicher oder mit `size` ausserhalb 1 bis `TAKT_RING_MAX`. */
static inline uint8_t takt_ring_init(struct takt_ring *r, uint8_t *slots, uint32_t size)
{
    uint8_t ok = 0u;
    if ((r != NULL) && (slots != NULL) && (size > 0u) && (size <= TAKT_RING_MAX)) {
        r->slots = slots;
        r->size = size;
        __atomic_store_n(&r->head, 0u, __ATOMIC_SEQ_CST);
        __atomic_store_n(&r->tail, 0u, __ATOMIC_SEQ_CST);
        __atomic_store_n(&r->lost, 0u, __ATOMIC_SEQ_CST);
        ok = 1u;
    }
    return ok;
}

/* Wie viele Bytes zwischen Ende und Kopf liegen. */
static inline uint32_t takt_ring_between(const struct takt_ring *r, uint32_t head, uint32_t tail)
{
    uint32_t n;
    if (head >= tail) {
        n = head - tail;
    } else {
        n = ((2u * r->size) - tail) + head;
    }
    return n;
}

/* Der Stand nach `at`. */
static inline uint32_t takt_ring_next(const struct takt_ring *r, uint32_t at)
{
    uint32_t next = at + 1u;
    if (next == (2u * r->size)) {
        next = 0u;
    }
    return next;
}

/* Schreiber: legt `byte` ab; 1 bei Erfolg, 0 wenn der Ring voll ist. Das
 * Byte geht dann verloren und zaehlt in `lost`. */
static inline uint8_t takt_ring_push(struct takt_ring *r, uint8_t byte)
{
    uint8_t ok = 0u;
    uint32_t head = __atomic_load_n(&r->head, __ATOMIC_SEQ_CST);
    uint32_t tail = __atomic_load_n(&r->tail, __ATOMIC_SEQ_CST);
    if (takt_ring_between(r, head, tail) < r->size) {
        r->slots[head % r->size] = byte;
        __atomic_store_n(&r->head, takt_ring_next(r, head), __ATOMIC_SEQ_CST);
        ok = 1u;
    } else {
        uint32_t lost = __atomic_load_n(&r->lost, __ATOMIC_SEQ_CST);
        if (lost < UINT32_MAX) {
            __atomic_store_n(&r->lost, lost + 1u, __ATOMIC_SEQ_CST);
        }
    }
    return ok;
}

/* Leser: das aelteste Byte nach `*byte`; 1, wenn eines wartete, sonst 0. */
static inline uint8_t takt_ring_pop(struct takt_ring *r, uint8_t *byte)
{
    uint8_t ok = 0u;
    uint32_t tail = __atomic_load_n(&r->tail, __ATOMIC_SEQ_CST);
    if (tail != __atomic_load_n(&r->head, __ATOMIC_SEQ_CST)) {
        *byte = r->slots[tail % r->size];
        __atomic_store_n(&r->tail, takt_ring_next(r, tail), __ATOMIC_SEQ_CST);
        ok = 1u;
    }
    return ok;
}

/* Wie viele Bytes warten. */
static inline uint32_t takt_ring_len(const struct takt_ring *r)
{
    uint32_t head = __atomic_load_n(&r->head, __ATOMIC_SEQ_CST);
    uint32_t tail = __atomic_load_n(&r->tail, __ATOMIC_SEQ_CST);
    return takt_ring_between(r, head, tail);
}

/* Wie viele Bytes der Schreiber seit `takt_ring_init` verwarf. */
static inline uint32_t takt_ring_lost(const struct takt_ring *r)
{
    return __atomic_load_n(&r->lost, __ATOMIC_SEQ_CST);
}

#ifdef __cplusplus
}
#endif

#endif /* TAKT_RING_H */
