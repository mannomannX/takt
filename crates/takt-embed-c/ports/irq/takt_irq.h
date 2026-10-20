/* takt_irq.h - der Port der Interruptform (12.11, M11 Schritt 18b): ein Kern
 * des SDK (`takt_sdk.h`) in der ISR eines Zeitgebers, neben einer fremden
 * Hauptschleife und ohne RTOS, ueber die Primitive eines Chip-Adapters
 * (`takt_irq_chip.h`). Jeder Chip ohne RTOS teilt diesen Port ueber seinen
 * Adapter (`ports/irq/stm32f4`).
 *
 * Der Schritt rechnet in der ISR des Zeitgebers: Sie ruft `takt_core_service`
 * zu der Frist, die der Kern nennt, stellt den Zeitgeber auf die naechste und
 * laesst den Job-Interrupt anstehen, wenn Jobs warten. Der Job-Interrupt
 * liegt unter allen anderen und ruft `takt_core_job_work`; danach laesst er
 * die ISR des Zeitgebers anstehen, und `takt_core_service` verteilt vor der
 * Frist den naechsten wartenden Job (12.11). Darunter laeuft die
 * Hauptschleife des Wirts mit ihrer eigenen Arbeit, die jede ISR
 * unterbricht.
 *
 * Schritt und Jobs rechnen auf dem Stack der Interrupts, also teilen sie ihn
 * (`jobstack geteilt`, FB-459): Seine Schranke ist `P_TICK_STACK_BYTES +
 * P_JOB_STACK_BYTES`, und der Wirt bemalt und misst ihn (12.3).
 *
 * In logischer Zeit (13.8) wartet der Alarm hinter einem Tor, das die
 * Hauptschleife oeffnet (`takt_irq_release`): Wenn sie laeuft, ruht jede ISR
 * und jeder Job ist fertig. Die Uhr steht dann mit jedem Alarm auf seiner
 * Frist; so haelt jeder Job seine Dauer, und mit einer verlustfreien Senke
 * (`takt_sink_lossless`) geht keine Zeile verloren.
 *
 * Ablauf: `takt_irq_start`, dann in der Hauptschleife fremde Arbeit, in
 * logischer Zeit dazu `takt_irq_release`, solange `takt_irq_running` wahr
 * ist; danach `takt_core_report`.
 */
#ifndef TAKT_IRQ_H
#define TAKT_IRQ_H

#include <stdint.h>

#include "takt_sdk.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Was der Wirt dem Port gibt. */
struct takt_irq_config {
    uint64_t ticks;        /* so viele Ticks; 0: bis das Programm endet (12.7) */
    int64_t tick_ns;       /* die Tickperiode, `P_TICK_NS` */
    uint32_t timer_hz;     /* der Takt, der den Zeitgeber des Chips speist */
    uint8_t step_priority; /* der Schritt, in der Kodierung des Chips; unter ihm nur der Job-Interrupt */
    uint8_t logical;       /* 1: in logischer Zeit (13.8) */
};

/* Der Port; der Wirt stellt den Speicher, die Felder gehoeren dem Port. */
struct takt_irq {
    takt_core *core;
    struct takt_irq_config config;
    int64_t end; /* das Ende des Laufs in Nanosekunden nach Tick 0 */
    int64_t at;  /* die Frist, auf die der Alarm steht */
    int64_t now; /* in logischer Zeit die Uhr */
    int gate;    /* atomar, logische Zeit: der Alarm wartet hinter dem Tor */
    int open;    /* atomar, logische Zeit: die Hauptschleife gab ihn frei */
    int done;    /* atomar: der Job-Interrupt rechnete einen Auftrag */
    int running; /* atomar: der Lauf geht */
};

/* Ungueltige Konfiguration: ein Zeiger fehlt, oder die Tickperiode ist nicht
 * positiv. */
#define TAKT_IRQ_INVALID (-1)

/* Die Kopfzeile `Schutz` des Laufs (11.3, 12.11), ohne Zeilenende; der Wirt
 * schreibt sie vor die Marke. Ohne RTOS gehoert die Schutzeinheit Takt, doch
 * dieser Port oeffnet und schliesst keine Region um den Schritt: Der Kern
 * des SDK ruft den Wirt dafuer nicht (FB-755). Die Arena schuetzt niemand. */
#define TAKT_IRQ_PROTECTION "  Schutz: keiner"

/* Startet den Lauf des Kerns `core` (aus `takt_core_init`): Tick 0 liegt
 * jetzt. 0, TAKT_IRQ_INVALID oder der Fehler des Adapters; bei einem Fehler
 * steht kein Alarm. */
int takt_irq_start(struct takt_irq *irq, takt_core *core, const struct takt_irq_config *config);

/* Wahr, solange der Lauf geht. */
int takt_irq_running(const struct takt_irq *irq);

/* Aus der Hauptschleife, in logischer Zeit: Wartet der Alarm hinter dem Tor,
 * steht seine ISR jetzt an. */
void takt_irq_release(struct takt_irq *irq);

/* Aus der ISR des Zeitgebers, die der Adapter stellt: `due` ist wahr, wenn
 * die Frist des Alarms erreicht ist. */
void takt_irq_on_step(struct takt_irq *irq, int due);

/* Aus dem Job-Interrupt, den der Adapter stellt. */
void takt_irq_on_jobs(struct takt_irq *irq);

#ifdef __cplusplus
}
#endif

#endif /* TAKT_IRQ_H */
