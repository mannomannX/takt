/* partition.c - die Probe der Partition (partition.h). */
#include "partition.h"

#include <stdint.h>

#include <zephyr/fatal.h>
#include <zephyr/kernel.h>

K_APPMEM_PARTITION_DEFINE(takt_arena);

/* Der Stack der Probe, ein Stack fuer den Benutzermodus. */
#define PROBE_STACK_BYTES 1024U
static K_THREAD_STACK_DEFINE(probe_stack, PROBE_STACK_BYTES);

static struct k_thread probe_thread;

/* Ob der Zugriff der Probe fiel; nur der Fehlerpfad setzt es. */
static atomic_t probe_refused;

/* Der Kern, dem eine Schutzverletzung im Lauf gilt; NULL bis
 * `partition_guard`. */
static takt_core *guarded;

/* Die Probe im Benutzermodus: ein Byte an das Ziel. */
static void probe(void *target, void *unused1, void *unused2) {
    volatile uint8_t *byte = target;
    ARG_UNUSED(unused1);
    ARG_UNUSED(unused2);
    *byte = 0x5AU;
}

/* Der Fehlerpfad des Zephyr-Kerns: Fiel der Zugriff der Probe, haelt er es
 * fest. Faellt im Lauf ein Faden im Benutzermodus, wies die Schutzeinheit
 * seinen Zugriff ab, und der Kern bekommt den Fault (`partition_guard`).
 * Beides kehrt zurueck, und der Zephyr-Kern beendet nur den Faden. Jeden
 * anderen Fehler haelt er an wie die Vorgabe des Zephyr-Kerns. Ob ein Faden
 * im Benutzermodus laeuft, steht in seinen Optionen (`K_USER`); eine
 * Abfrage dafuer hat der Zephyr-Kern nicht. */
void k_sys_fatal_error_handler(unsigned int reason, const struct arch_esf *esf) {
    const struct k_thread *thread = k_current_get();
    const int user = ((thread->base.user_options & K_USER) != 0U) ? 1 : 0;
    ARG_UNUSED(esf);
    if (thread == &probe_thread) {
        (void)atomic_set(&probe_refused, 1);
    }
    if ((user != 0) && (guarded != NULL)) {
        takt_core_hardware_fault(guarded);
    } else if (thread != &probe_thread) {
        k_fatal_halt(reason);
    } else {
        /* Die Probe vor dem Lauf: festgehalten. */
    }
}

void partition_guard(takt_core *core) {
    guarded = core;
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
