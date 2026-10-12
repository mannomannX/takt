/* takt_irq_chip.c - der Adapter der Interruptform fuer STM32F4
 * (takt_irq_chip.h, 12.11).
 *
 * Die Zeitachse ist TIM2, 32 Bit breit und freilaufend mit 1 MHz; seine
 * Ueberlaeufe zaehlt die ISR als Epoche, und Epoche und Zaehler ergeben einen
 * Stand ueber 64 Bit. Der Alarm ist der Vergleich CC1: Liegt die Frist in
 * dieser Epoche, steht er auf ihr, sonst fragt der naechste Ueberlauf erneut.
 * Der Job-Interrupt ist PendSV, die Ausnahme, die jeder Cortex-M hat und die
 * hier die niedrigste Prioritaet bekommt. Es ist die Rechnung der
 * Interruptform in Rust (`takt_board_stm32f401::alarm`,
 * `takt_board_support::alarm`).
 *
 * Der Adapter belegt TIM2 und PendSV und stellt ihre ISRs unter den Namen
 * von CMSIS (`TIM2_IRQHandler`, `PendSV_Handler`). Die FPU-Register sichert
 * die Hardware: Mit `ASPEN` und `LSPEN` in FPCCR legt jede Ausnahme sie ab,
 * sobald sie die FPU benutzt.
 *
 * Die Register nach RM0368 (STM32F401) und dem Handbuch des Cortex-M4; ihre
 * Adressen sind auf jedem STM32F4 dieselben.
 */
#include <stddef.h>
#include <stdint.h>

#include "takt_irq.h"
#include "takt_irq_chip.h"

/* Die ISRs dieses Adapters, wie die Vektortabelle des Wirts sie nennt. */
void TIM2_IRQHandler(void);
void PendSV_Handler(void);

/* Der Takt von TIM2 (RCC_APB1ENR.TIM2EN). */
#define RCC_APB1ENR 0x40023840u
#define RCC_APB1ENR_TIM2EN 0x00000001u

/* TIM2 (RM0368 13.4). */
#define TIM2_CR1 0x40000000u
#define TIM2_DIER 0x4000000Cu
#define TIM2_SR 0x40000010u
#define TIM2_EGR 0x40000014u
#define TIM2_CNT 0x40000024u
#define TIM2_PSC 0x40000028u
#define TIM2_ARR 0x4000002Cu
#define TIM2_CCR1 0x40000034u
#define TIM_CR1_CEN 0x00000001u
#define TIM_DIER_UIE 0x00000001u
#define TIM_DIER_CC1IE 0x00000002u
#define TIM_SR_UIF 0x00000001u
#define TIM_SR_CC1IF 0x00000002u
#define TIM_EGR_UG 0x00000001u

/* NVIC, SCB und FPU des Cortex-M4. */
#define NVIC_ISER0 0xE000E100u
#define NVIC_ISPR0 0xE000E200u
#define NVIC_IPR 0xE000E400u
#define SCB_ICSR 0xE000ED04u
#define SCB_ICSR_PENDSVSET 0x10000000u
#define SCB_SHPR3 0xE000ED20u
#define SCB_SHPR3_PENDSV 0x00FF0000u /* die niedrigste Prioritaet fuer PendSV */
#define FPU_FPCCR 0xE000EF34u
#define FPU_FPCCR_LAZY 0xC0000000u /* ASPEN (31) und LSPEN (30) */

/* Die Leitung von TIM2 im NVIC. */
#define TIM2_IRQN 28u

/* Die Zeitachse zaehlt mit 1 MHz: ein Schritt je Mikrosekunde, und die
 * Rechnung in Nanosekunden ist eine Multiplikation. */
#define TAKT_COUNTS_HZ 1000000u
#define TAKT_NS_PER_COUNT 1000u

/* Der Prescaler von TIM2 hat 16 Bit. */
#define TAKT_PRESCALER_MAX 65536u

/* Der Takt des Zeitgebers teilt sich nicht auf 1 MHz. */
#define TAKT_STM32F4_CLOCK (-2)

/* Der Port, den beide ISRs rufen; gesetzt vor ihrer Freigabe. */
static struct takt_irq *takt_port;

/* Atomar: die Ueberlaeufe von TIM2; nur seine ISR zaehlt sie. */
static uint32_t takt_epoch;

/* Der Zielstand des Alarms auf der Zeitachse und ob er steht. Beide gehoeren
 * der ISR von TIM2; wer sie sonst schreibt, sperrt sie (`takt_lock`). */
static uint64_t takt_target;
static uint32_t takt_armed;

/* Ein Register an seiner Adresse aus dem Handbuch. */
static volatile uint32_t *takt_reg(uintptr_t address) { return (volatile uint32_t *)address; }

static volatile uint8_t *takt_reg8(uintptr_t address) { return (volatile uint8_t *)address; }

/* Sperrt die Interrupts und nennt, wie PRIMASK vorher stand. */
static uint32_t takt_lock(void) {
    uint32_t primask;
    __asm__ volatile("mrs %0, primask\n\tcpsid i" : "=r"(primask) : : "memory");
    return primask;
}

/* Stellt PRIMASK wieder her, wie `takt_lock` es fand. */
static void takt_unlock(uint32_t primask) { __asm__ volatile("msr primask, %0" : : "r"(primask) : "memory"); }

/* Der Stand der Zeitachse: Epoche und Zaehler, gelesen, bis die Epoche
 * stehen bleibt. Ein Ueberlauf, dessen Interrupt noch ansteht, zaehlt mit,
 * wenn der Zaehler nach ihm gelesen wurde, erkennbar an einem kleinen Wert;
 * ohne das liefe die Zeit um 2^32 Schritte zurueck. */
static uint64_t takt_counts(void) {
    uint64_t counts = 0u;
    uint32_t settled = 0u;
    while (settled == 0u) {
        const uint32_t epoch = __atomic_load_n(&takt_epoch, __ATOMIC_SEQ_CST);
        const uint32_t low = *takt_reg(TIM2_CNT);
        const uint32_t wrapped = *takt_reg(TIM2_SR) & TIM_SR_UIF;
        if (__atomic_load_n(&takt_epoch, __ATOMIC_SEQ_CST) == epoch) {
            uint64_t high = (uint64_t)epoch;
            if ((wrapped != 0u) && (low < 0x80000000u)) {
                high += 1u;
            }
            counts = (high << 32u) | (uint64_t)low;
            settled = 1u;
        }
    }
    return counts;
}

/* Der erste Stand, der `at_ns` nicht unterschreitet: Ein Alarm darauf
 * feuert nie vor seiner Frist. */
static uint64_t takt_counts_at(int64_t at_ns) {
    uint64_t counts = 0u;
    if (at_ns > 0) {
        const uint64_t ns = (uint64_t)at_ns;
        counts = ns / TAKT_NS_PER_COUNT;
        if ((ns % TAKT_NS_PER_COUNT) != 0u) {
            counts += 1u;
        }
    }
    return counts;
}

/* Stellt den Vergleich auf das Ziel: in dieser Epoche ueber CCR1, hinter dem
 * naechsten Ueberlauf gar nicht, dessen Interrupt fragt erneut. Ein Ziel, das
 * schon erreicht ist oder beim Stellen verstrich, laesst die ISR sofort
 * anstehen. */
static void takt_aim(void) {
    const uint64_t now = takt_counts();
    if (takt_target <= now) {
        takt_irq_chip_pend_step();
    } else if ((takt_target >> 32u) == (now >> 32u)) {
        *takt_reg(TIM2_CCR1) = (uint32_t)(takt_target & 0xFFFFFFFFu);
        *takt_reg(TIM2_SR) = ~TIM_SR_CC1IF;
        *takt_reg(TIM2_DIER) |= TIM_DIER_CC1IE;
        if (takt_counts() >= takt_target) {
            takt_irq_chip_pend_step();
        }
    } else {
        *takt_reg(TIM2_DIER) &= ~TIM_DIER_CC1IE;
    }
}

int takt_irq_chip_start(struct takt_irq *irq) {
    int rc = TAKT_STM32F4_CLOCK;
    if (irq == NULL) {
        rc = TAKT_IRQ_INVALID;
    } else if ((irq->config.timer_hz >= TAKT_COUNTS_HZ) && ((irq->config.timer_hz % TAKT_COUNTS_HZ) == 0u) &&
               ((irq->config.timer_hz / TAKT_COUNTS_HZ) <= TAKT_PRESCALER_MAX)) {
        takt_port = irq;
        takt_armed = 0u;
        __atomic_store_n(&takt_epoch, 0u, __ATOMIC_SEQ_CST);
        *takt_reg(FPU_FPCCR) |= FPU_FPCCR_LAZY;
        *takt_reg(RCC_APB1ENR) |= RCC_APB1ENR_TIM2EN;
        /* Zwischen dem Freigeben eines Takts und dem ersten Zugriff liegen
         * zwei Takte des Busses (Errata von ST, „Delay after an RCC
         * peripheral clock enabling“): ein Lesen. */
        (void)*takt_reg(RCC_APB1ENR);
        *takt_reg(TIM2_CR1) = 0u;
        *takt_reg(TIM2_DIER) = 0u;
        *takt_reg(TIM2_PSC) = (irq->config.timer_hz / TAKT_COUNTS_HZ) - 1u;
        *takt_reg(TIM2_ARR) = 0xFFFFFFFFu;
        /* Prescaler und Zaehler uebernehmen; das dabei gesetzte UIF geht
         * wieder, sonst zaehlte der Aufbau als erster Ueberlauf. */
        *takt_reg(TIM2_EGR) = TIM_EGR_UG;
        *takt_reg(TIM2_SR) = 0u;
        *takt_reg(TIM2_DIER) = TIM_DIER_UIE;
        *takt_reg8(NVIC_IPR + TIM2_IRQN) = irq->config.step_priority;
        *takt_reg(SCB_SHPR3) |= SCB_SHPR3_PENDSV;
        *takt_reg(NVIC_ISER0) = (uint32_t)1u << TIM2_IRQN;
        *takt_reg(TIM2_CR1) = TIM_CR1_CEN;
        rc = 0;
    } else {
        /* Der Takt teilt sich nicht auf 1 MHz. */
    }
    return rc;
}

int64_t takt_irq_chip_now(void) { return (int64_t)(takt_counts() * TAKT_NS_PER_COUNT); }

void takt_irq_chip_arm(int64_t at_ns) {
    const uint32_t primask = takt_lock();
    takt_target = takt_counts_at(at_ns);
    takt_armed = 1u;
    takt_aim();
    takt_unlock(primask);
}

void takt_irq_chip_disarm(void) {
    const uint32_t primask = takt_lock();
    takt_armed = 0u;
    *takt_reg(TIM2_DIER) &= ~TIM_DIER_CC1IE;
    takt_unlock(primask);
}

void takt_irq_chip_pend_step(void) { *takt_reg(NVIC_ISPR0) = (uint32_t)1u << TIM2_IRQN; }

void takt_irq_chip_pend_jobs(void) { *takt_reg(SCB_ICSR) = SCB_ICSR_PENDSVSET; }

/* Der Schrittkontext: zaehlt einen Ueberlauf, loescht den Vergleich und
 * fragt, ob die Frist erreicht ist; ist sie es, steht kein Alarm mehr, und
 * der Port stellt den naechsten. Entschieden wird am Stand der Zeitachse,
 * nicht an den Flags: Ein Vergleich, der zwischen Lesen und Loeschen kam,
 * geht so nicht verloren. */
void TIM2_IRQHandler(void) {
    const uint32_t sr = *takt_reg(TIM2_SR);
    int due = 0;
    if ((sr & TIM_SR_UIF) != 0u) {
        *takt_reg(TIM2_SR) = ~TIM_SR_UIF;
        (void)__atomic_add_fetch(&takt_epoch, 1u, __ATOMIC_SEQ_CST);
    }
    if ((sr & TIM_SR_CC1IF) != 0u) {
        *takt_reg(TIM2_SR) = ~TIM_SR_CC1IF;
    }
    if (takt_armed != 0u) {
        if (takt_counts() >= takt_target) {
            takt_armed = 0u;
            *takt_reg(TIM2_DIER) &= ~TIM_DIER_CC1IE;
            due = 1;
        } else {
            /* Nach einem Ueberlauf liegt das Ziel vielleicht in dieser Epoche. */
            takt_aim();
        }
    }
    if (takt_port != NULL) {
        takt_irq_on_step(takt_port, due);
    }
}

/* Der Job-Interrupt. */
void PendSV_Handler(void) {
    if (takt_port != NULL) {
        takt_irq_on_jobs(takt_port);
    }
}
