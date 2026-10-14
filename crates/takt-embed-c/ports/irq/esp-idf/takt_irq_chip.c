/* takt_irq_chip.c - der Adapter der Interruptform fuer ESP-IDF auf dem C6
 * (takt_irq_chip.h, takt_irq_esp_idf.h; M11 Schritt 21a).
 *
 * **Der Schritt rechnet in Interrupts, die ESP-IDF waehrend eines
 * Flash-Zugriffs weiterlaufen laesst** (plan/m11.md 1.7 und 2.5): Auf dem
 * einkernigen C6 haelt ein Schreib- oder Loeschzugriff den Planer an und
 * sperrt jeden Interrupt, der nicht als IRAM-sicher angemeldet ist. Beide
 * Interrupts des Schritts sind es, und alles, was sie rufen, liegt im RAM
 * (das Fragment der Komponente, `esp-idf/takt/takt.lf`).
 *
 * - **Uhr und Alarm** sind ein `gptimer` mit dem Takt
 *   `takt_irq_config.timer_hz`; er muss 10^9 teilen, dann ist ein Schritt
 *   des Zaehlers eine ganze Zahl Nanosekunden. Die ISR des Treibers und
 *   seine Steuerung liegen im IRAM (`CONFIG_GPTIMER_ISR_CACHE_SAFE`,
 *   `CONFIG_GPTIMER_CTRL_FUNC_IN_IRAM`).
 * - **Anstehen ohne Frist** ist der Software-Interrupt `FROM_CPU_INTR3`.
 *   Auf dem einkernigen C6 belegt ESP-IDF davon nur `FROM_CPU_INTR0` fuer
 *   das Umschalten der Aufgaben (`components/esp_system/crosscore_int.c`);
 *   2 und 3 gehoeren der IPC-ISR, die es nur mit zwei Kernen gibt
 *   (`CONFIG_ESP_IPC_ISR_ENABLE`), 1 dem zweiten Kern.
 * - **Beide ISRs haben die Stufe `takt_irq_config.step_priority`** und
 *   unterbrechen einander darum nicht: `takt_irq_on_step` laeuft nie zweimal
 *   zugleich. Die Stufe liegt unter der Schwelle der kritischen Abschnitte
 *   von FreeRTOS, denn der Schritt weckt die Jobs ueber FreeRTOS.
 * - **Die Jobs** rechnen in einer Aufgabe, die die ISR des Schritts weckt
 *   (`vTaskNotifyGiveFromISR`); ihren Stack und ihre Prioritaet stellt der
 *   Wirt (takt_irq_esp_idf.h). Waehrend eines Flash-Zugriffs steht sie wie
 *   jede Aufgabe; der Schritt laeuft weiter.
 * - **Ein Alarm in der Vergangenheit:** ESP-IDF sagt dazu nur, ein Alarm
 *   komme, wenn der Zaehler den Wert erreicht (`timer_ll_set_alarm_value`
 *   in `components/esp_hal_timg/esp32c6/include/hal/timer_ll.h`, v6.1-beta1);
 *   ob eine schon vergangene Frist sofort feuert, steht dort nicht. Der
 *   Adapter liest darum nach dem Stellen die Uhr und laesst die ISR selbst
 *   anstehen, wenn die Frist erreicht ist. Kommt der Alarm trotzdem, findet
 *   die ISR die naechste Frist noch nicht erreicht und tut nichts. Den Fall
 *   faehrt der Board-Test mit Ueberlauf
 *   (`the_esp_idf_port_turns_an_overrun_into_a_fault`): Dort rechnet ein Tick
 *   laenger als seine Periode, und jede Frist danach liegt beim Stellen
 *   schon zurueck.
 * - Der C6 hat keine FPU; es gibt keinen Kontext der Gleitkommaregister zu
 *   sichern (12.11).
 */
#include "takt_irq_chip.h"

#include <stddef.h>

#include "driver/gptimer.h"
#include "esp_attr.h"
#include "esp_intr_alloc.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "soc/interrupts.h"
#include "soc/intpri_reg.h"
#include "soc/soc.h"
#include "takt_irq_esp_idf.h"

#define TAKT_NS_PER_S 1000000000ULL

/* Der Fehler des Adapters, wenn ESP-IDF etwas nicht anlegt. */
#define TAKT_IRQ_CHIP_FAILED (-1)

/* Was der Wirt fuer die Jobs nennt (takt_irq_esp_idf.h). */
static StackType_t *takt_jobs_stack;
static size_t takt_jobs_bytes;
static UBaseType_t takt_jobs_priority;

/* Der Port, dessen ISRs der Adapter ruft; gesetzt, bevor eine feuert. */
static struct takt_irq *takt_chip_irq;

static gptimer_handle_t takt_chip_timer;

/* Nanosekunden je Schritt des Zaehlers. */
static uint64_t takt_chip_ns_per_count;

/* Die Aufgabe der Jobs. */
static TaskHandle_t takt_jobs_task;

/* Die ISR des Schritts hat eine Aufgabe geweckt, die vor der unterbrochenen
 * laufen soll. Nur die beiden ISRs des Schritts schreiben es, und die
 * unterbrechen einander nicht. */
static BaseType_t takt_chip_woken;

void takt_irq_esp_idf_jobs(StackType_t *stack, size_t bytes, UBaseType_t priority) {
    takt_jobs_stack = stack;
    takt_jobs_bytes = bytes;
    takt_jobs_priority = priority;
}

int takt_irq_esp_idf_job_depth(uint32_t *depth) {
    int rc = -1;
    if (takt_jobs_task != NULL) {
        const uint64_t unused = (uint64_t)uxTaskGetStackHighWaterMark2(takt_jobs_task) * sizeof(StackType_t);
        const uint64_t size = (uint64_t)takt_jobs_bytes;
        const uint64_t used = (unused < size) ? (size - unused) : 0U;
        *depth = (used > (uint64_t)UINT32_MAX) ? UINT32_MAX : (uint32_t)used;
        rc = 0;
    }
    return rc;
}

/* Der Schritt, aus beiden ISRs: Die Frist ist erreicht, wenn die Uhr bei ihr
 * steht. Gibt zurueck, ob eine geweckte Aufgabe vor der unterbrochenen
 * laufen soll. */
static IRAM_ATTR BaseType_t takt_chip_step(void) {
    takt_chip_woken = pdFALSE;
    takt_irq_on_step(takt_chip_irq, (takt_irq_chip_now() >= takt_chip_irq->at) ? 1 : 0);
    return takt_chip_woken;
}

/* Die ISR des Alarms (der Treiber ruft sie im IRAM). */
static IRAM_ATTR bool takt_chip_on_alarm(gptimer_handle_t timer, const gptimer_alarm_event_data_t *event,
                                         void *ctx) {
    (void)timer;
    (void)event;
    (void)ctx;
    return takt_chip_step() != pdFALSE;
}

/* Der Software-Interrupt: Er steht, bis die ISR ihn zuruecknimmt. */
static IRAM_ATTR void takt_chip_on_pend(void *arg) {
    (void)arg;
    WRITE_PERI_REG(INTPRI_CPU_INTR_FROM_CPU_3_REG, 0U);
    portYIELD_FROM_ISR(takt_chip_step());
}

/* Die Aufgabe der Jobs: wartet, bis der Schritt sie weckt, und rechnet. */
static void takt_chip_jobs(void *arg) {
    (void)arg;
    for (;;) {
        (void)ulTaskNotifyTake(pdTRUE, portMAX_DELAY);
        takt_irq_on_jobs(takt_chip_irq);
    }
}

/* Die Flags von `esp_intr_alloc` fuer eine IRAM-ISR der Stufe `level`, 1 bis
 * 3. */
static int takt_chip_flags(uint8_t level) {
    int flag = ESP_INTR_FLAG_LEVEL3;
    if (level == 1U) {
        flag = ESP_INTR_FLAG_LEVEL1;
    } else if (level == 2U) {
        flag = ESP_INTR_FLAG_LEVEL2;
    } else {
        /* Stufe 3. */
    }
    return ESP_INTR_FLAG_IRAM | flag;
}

int takt_irq_chip_start(struct takt_irq *irq) {
    static StaticTask_t tcb;
    int rc = TAKT_IRQ_CHIP_FAILED;
    const uint32_t hz = irq->config.timer_hz;
    const uint8_t level = irq->config.step_priority;
    /* Die Stufen 1 bis 3 rechnen in C (`ESP_INTR_FLAG_LOWMED`). Ohne Jobs
     * gibt es keinen Job-Stack und keine Aufgabe. */
    const int jobs = (takt_jobs_bytes != 0U) ? 1 : 0;
    if ((hz != 0U) && ((TAKT_NS_PER_S % hz) == 0U) && ((jobs == 0) || (takt_jobs_stack != NULL)) && (level >= 1U) &&
        (level <= 3U)) {
        takt_chip_irq = irq;
        takt_chip_ns_per_count = TAKT_NS_PER_S / hz;
        const gptimer_config_t timer = {
            .clk_src = GPTIMER_CLK_SRC_DEFAULT,
            .direction = GPTIMER_COUNT_UP,
            .resolution_hz = hz,
            .intr_priority = (int)level,
        };
        const gptimer_event_callbacks_t callbacks = {.on_alarm = takt_chip_on_alarm};
        if (jobs != 0) {
            /* Die Tiefe zaehlt FreeRTOS in `StackType_t`, unter ESP-IDF ein Byte. */
            takt_jobs_task = xTaskCreateStatic(takt_chip_jobs, "takt-jobs", (uint32_t)(takt_jobs_bytes / sizeof(StackType_t)),
                                               NULL, takt_jobs_priority, takt_jobs_stack, &tcb);
        }
        if (((jobs == 0) || (takt_jobs_task != NULL)) && (gptimer_new_timer(&timer, &takt_chip_timer) == ESP_OK) &&
            (gptimer_register_event_callbacks(takt_chip_timer, &callbacks, NULL) == ESP_OK) &&
            (esp_intr_alloc(ETS_FROM_CPU_INTR3_SOURCE, takt_chip_flags(level), takt_chip_on_pend, NULL, NULL) == ESP_OK) &&
            (gptimer_enable(takt_chip_timer) == ESP_OK) && (gptimer_start(takt_chip_timer) == ESP_OK)) {
            rc = 0;
        }
    }
    return rc;
}

IRAM_ATTR int64_t takt_irq_chip_now(void) {
    uint64_t count = 0U;
    (void)gptimer_get_raw_count(takt_chip_timer, &count);
    const uint64_t ns = count * takt_chip_ns_per_count;
    return (ns > (uint64_t)INT64_MAX) ? INT64_MAX : (int64_t)ns;
}

/* Die Frist in Schritten des Zaehlers, aufgerundet: Der Alarm kommt nie zu
 * frueh. */
IRAM_ATTR void takt_irq_chip_arm(int64_t at_ns) {
    /* Mit `CONFIG_GPTIMER_CTRL_FUNC_IN_IRAM` muss die Einstellung im internen
     * RAM liegen, nicht unter den Konstanten im Flash. */
    static gptimer_alarm_config_t alarm;
    const uint64_t at = (at_ns > 0) ? (uint64_t)at_ns : 0U;
    alarm.alarm_count = (at + takt_chip_ns_per_count - 1U) / takt_chip_ns_per_count;
    (void)gptimer_set_alarm_action(takt_chip_timer, &alarm);
    if (takt_irq_chip_now() >= at_ns) {
        takt_irq_chip_pend_step();
    }
}

IRAM_ATTR void takt_irq_chip_disarm(void) { (void)gptimer_set_alarm_action(takt_chip_timer, NULL); }

IRAM_ATTR void takt_irq_chip_pend_step(void) {
    WRITE_PERI_REG(INTPRI_CPU_INTR_FROM_CPU_3_REG, INTPRI_CPU_INTR_FROM_CPU_3);
}

/* Aus der ISR des Schritts (`takt_irq_serve`); ein Programm ohne Jobs hat
 * keine Aufgabe und laesst keinen Job anstehen. */
IRAM_ATTR void takt_irq_chip_pend_jobs(void) {
    if (takt_jobs_task != NULL) {
        BaseType_t woken = pdFALSE;
        vTaskNotifyGiveFromISR(takt_jobs_task, &woken);
        if (woken != pdFALSE) {
            takt_chip_woken = pdTRUE;
        }
    }
}
