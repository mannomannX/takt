/* takt_irq_esp_idf.h - was der Adapter der Interruptform unter ESP-IDF vom
 * Wirt braucht (12.11, M11 Schritt 21a), neben `takt_irq_chip.h`.
 *
 * Unter ESP-IDF rechnen die Jobs in einer Aufgabe, nicht in einem
 * Interrupt: Ihren Stack stellt der Wirt, so gross, wie die Lieferform es
 * verlangt (`P_JOB_STACK_BYTES` und Reserve), und ihre Prioritaet waehlt er
 * ueber allem, was er selbst rechnet. Beides nennt er vor `takt_irq_start`.
 * Nach dem Lauf misst der Adapter die Tiefe dieses Stacks.
 */
#ifndef TAKT_IRQ_ESP_IDF_H
#define TAKT_IRQ_ESP_IDF_H

#include <stddef.h>
#include <stdint.h>

#include "freertos/FreeRTOS.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Der Stack `stack` mit `bytes` Byte und die Prioritaet `priority` der
 * Aufgabe, in der die Jobs rechnen; vor `takt_irq_start`. */
void takt_irq_esp_idf_jobs(StackType_t *stack, size_t bytes, UBaseType_t priority);

/* Die Tiefe des Job-Stacks nach dem Lauf in Byte, aus der Hochwassermarke
 * von FreeRTOS; 0, oder -1 ohne Aufgabe. */
int takt_irq_esp_idf_job_depth(uint32_t *depth);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_IRQ_ESP_IDF_H */
