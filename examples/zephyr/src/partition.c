/* partition.c - die Probe der Partition (partition.h). */
#include "partition.h"

#include <stdint.h>

#include <zephyr/fatal.h>
#include <zephyr/kernel.h>

K_APPMEM_PARTITION_DEFINE(takt_arena);

/* Der Stack der Probe, ein Stack fuer den Benutzermodus. */
#define PROBE_STACK_BYTES 1024U
K_THREAD_STACK_DEFINE(probe_stack, PROBE_STACK_BYTES);

static struct k_thread probe_thread;

/* Ob der Zugriff der Probe fiel; nur der Fehlerpfad setzt es. */
static atomic_t probe_refused;

/* Die Probe im Benutzermodus: ein Byte an das Ziel. */
static void probe(void *target, void *unused1, void *unused2) {
    volatile uint8_t *byte = target;
    ARG_UNUSED(unused1);
    ARG_UNUSED(unused2);
    *byte = 0x5AU;
}

/* Der Fehlerpfad des Zephyr-Kerns: Fiel der Zugriff der Probe, haelt er es
 * fest und kehrt zurueck, und der Zephyr-Kern beendet nur sie. Jeden anderen
 * Fehler haelt er an wie die Vorgabe des Zephyr-Kerns. */
void k_sys_fatal_error_handler(unsigned int reason, const struct arch_esf *esf) {
    ARG_UNUSED(esf);
    if (k_current_get() == &probe_thread) {
        (void)atomic_set(&probe_refused, 1);
    } else {
        k_fatal_halt(reason);
    }
}

int partition_probe(void *target) {
    int rc;
    (void)atomic_set(&probe_refused, 0);
    (void)k_thread_create(&probe_thread, probe_stack, K_THREAD_STACK_SIZEOF(probe_stack), probe, target, NULL, NULL,
                          k_thread_priority_get(k_current_get()), K_USER, K_NO_WAIT);
    rc = k_thread_join(&probe_thread, K_SECONDS(1));
    if (rc == 0) {
        rc = (atomic_get(&probe_refused) != 0) ? PARTITION_REFUSED : PARTITION_OPEN;
    }
    return rc;
}
