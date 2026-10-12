/* handback.c - der Wunsch des Hosts nach dem Board (handback.h). */
#include "handback.h"

#include <zephyr/kernel.h>

static K_SEM_DEFINE(handback_wished, 0, 1);

void handback_wish(void) {
    k_sem_give(&handback_wished);
}

void handback_serve(void) {
    if (k_sem_take(&handback_wished, K_NO_WAIT) == 0) {
        handback_give();
    }
}

void handback_await(void) {
    (void)k_sem_take(&handback_wished, K_FOREVER);
    handback_give();
}
