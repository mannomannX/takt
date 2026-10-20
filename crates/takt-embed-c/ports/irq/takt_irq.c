/* takt_irq.c - der Port der Interruptform (takt_irq.h, 12.11). */
#include "takt_irq.h"

#include <stddef.h>

#include "takt_irq_chip.h"

/* Das Ende des Laufs in Nanosekunden nach Tick 0. */
static int64_t takt_irq_end(const struct takt_irq_config *c) {
    const int64_t most = INT64_MAX / c->tick_ns;
    int64_t end = INT64_MAX;
    if ((c->ticks != 0u) && (c->ticks <= (uint64_t)most)) {
        end = (int64_t)c->ticks * c->tick_ns;
    }
    return end;
}

/* Stellt den Alarm auf `at`: in Echtzeit den Zeitgeber, in logischer Zeit
 * das Tor, das die Hauptschleife oeffnet. */
static void takt_irq_arm(struct takt_irq *r, int64_t at) {
    r->at = at;
    if (r->config.logical != 0u) {
        __atomic_store_n(&r->gate, 1, __ATOMIC_SEQ_CST);
    } else {
        takt_irq_chip_arm(at);
    }
}

/* Der Schrittkontext: rechnet jede Grenze bis `now`, laesst bei einem
 * wartenden Job den Job-Interrupt anstehen und stellt den Alarm auf die
 * naechste Frist. Vor der Frist gerufen, verteilt er nur den naechsten Job
 * (12.11). Am Ende des Laufs steht kein Alarm mehr. */
static void takt_irq_serve(struct takt_irq *r, int64_t now) {
    const takt_next next = takt_core_service(r->core, now);
    if (next.jobs != 0u) {
        takt_irq_chip_pend_jobs();
    }
    if ((next.ended != 0) || (next.deadline_ns >= r->end)) {
        takt_irq_chip_disarm();
        __atomic_store_n(&r->running, 0, __ATOMIC_SEQ_CST);
    } else {
        takt_irq_arm(r, next.deadline_ns);
    }
}

int takt_irq_start(struct takt_irq *irq, takt_core *core, const struct takt_irq_config *config) {
    int rc = TAKT_IRQ_INVALID;
    if ((irq != NULL) && (core != NULL) && (config != NULL) && (config->tick_ns > 0)) {
        irq->core = core;
        irq->config = *config;
        irq->end = takt_irq_end(config);
        irq->at = 0;
        irq->now = 0;
        __atomic_store_n(&irq->gate, 0, __ATOMIC_SEQ_CST);
        __atomic_store_n(&irq->open, 0, __ATOMIC_SEQ_CST);
        __atomic_store_n(&irq->done, 0, __ATOMIC_SEQ_CST);
        __atomic_store_n(&irq->running, 1, __ATOMIC_SEQ_CST);
        rc = takt_irq_chip_start(irq);
        if (rc == 0) {
            takt_irq_arm(irq, 0);
        } else {
            __atomic_store_n(&irq->running, 0, __ATOMIC_SEQ_CST);
        }
    }
    return rc;
}

int takt_irq_running(const struct takt_irq *irq) { return __atomic_load_n(&irq->running, __ATOMIC_SEQ_CST); }

void takt_irq_release(struct takt_irq *irq) {
    if (__atomic_exchange_n(&irq->gate, 0, __ATOMIC_SEQ_CST) != 0) {
        __atomic_store_n(&irq->open, 1, __ATOMIC_SEQ_CST);
        takt_irq_chip_pend_step();
    }
}

/* An der Frist rechnet der Schrittkontext den Schritt, nach einem Auftrag
 * des Job-Interrupts verteilt er den naechsten; sonst, etwa nach einem
 * Ueberlauf des Zeitgebers, ist nichts zu tun. In logischer Zeit steht die
 * Uhr mit dem Alarm auf seiner Frist. */
void takt_irq_on_step(struct takt_irq *irq, int due) {
    const int done = __atomic_exchange_n(&irq->done, 0, __ATOMIC_SEQ_CST);
    if (__atomic_load_n(&irq->running, __ATOMIC_SEQ_CST) != 0) {
        if (irq->config.logical != 0u) {
            if (__atomic_exchange_n(&irq->open, 0, __ATOMIC_SEQ_CST) != 0) {
                irq->now = irq->at;
                takt_irq_serve(irq, irq->now);
            } else if (done != 0) {
                takt_irq_serve(irq, irq->now);
            } else {
                /* Der Alarm wartet noch hinter dem Tor. */
            }
        } else if ((due != 0) || (done != 0)) {
            takt_irq_serve(irq, takt_irq_chip_now());
        } else {
            /* Die Frist ist noch nicht erreicht. */
        }
    }
}

/* Der Job-Interrupt (4.5): rechnet den Auftrag, den der Schritt gab, und
 * laesst den Schrittkontext den naechsten verteilen. */
void takt_irq_on_jobs(struct takt_irq *irq) {
    takt_core_job_work(irq->core);
    __atomic_store_n(&irq->done, 1, __ATOMIC_SEQ_CST);
    takt_irq_chip_pend_step();
}
