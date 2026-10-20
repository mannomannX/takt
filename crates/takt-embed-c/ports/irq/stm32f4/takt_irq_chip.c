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
 * Der Schutz (12.3, FB-755) ist die Region 1 der MPU ueber dem
 * Programmbereich der Arena, wie `takt_board_stm32f401::mpu` in Rust:
 * ausserhalb des Kerns nur lesbar, nie ausfuehrbar; ab 256 Byte deckt sie
 * ganze Achtel, die Achtel dahinter bleiben frei. Region 0 bleibt dem Wirt
 * (der Waechter unter seinem Stack). Ein Zugriff, den sie abweist, fuehrt in
 * `MemManage_Handler`: Er merkt sich die Adresse, uebergeht den Befehl und
 * kehrt zurueck; der Kern holt die Verletzung ab und stellt sie im naechsten
 * Tick als `Runtime(Hardware)` zu. Was sich nicht uebergehen laesst, ein
 * Fehler beim Stapeln, Entstapeln oder Befehlsabruf, setzt den Chip zurueck.
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
void MemManage_Handler(void) __attribute__((naked));

/* Der Rumpf von `MemManage_Handler`, mit dem gestapelten Rahmen. */
void takt_irq_chip_mem_manage(uint32_t *frame);

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

/* Die MPU des Cortex-M4 (ARMv7-M B3.5) und ihre Ausnahme. */
#define MPU_CTRL 0xE000ED94u
#define MPU_RNR 0xE000ED98u
#define MPU_RBAR 0xE000ED9Cu
#define MPU_RASR 0xE000EDA0u
#define MPU_CTRL_ON 0x00000005u  /* ENABLE, PRIVDEFENA */
#define MPU_RASR_RAM 0x10060001u /* XN, S, C: normales RAM, nie ausfuehrbar; ENABLE */
#define MPU_RASR_AP 0x07000000u  /* das Feld der Zugriffsrechte */
#define MPU_AP_READ 0x05000000u  /* privilegiert nur lesen */
#define MPU_AP_WRITE 0x01000000u /* privilegiert lesen und schreiben */
#define SCB_SHCSR 0xE000ED24u
#define SCB_SHCSR_MEMFAULTENA 0x00010000u
#define SCB_CFSR 0xE000ED28u
#define SCB_MMFAR 0xE000ED34u
#define SCB_AIRCR 0xE000ED0Cu
#define SCB_AIRCR_RESET 0x05FA0004u /* VECTKEY, SYSRESETREQ */
#define SCB_MMFSR 0xFFu             /* die Bits von MemManage in CFSR */
#define SCB_MMFSR_DATA 0x82u        /* DACCVIOL mit gueltiger Adresse in MMFAR */

/* Die Region der Arena; Region 0 gehoert dem Wirt. */
#define TAKT_ARENA_REGION 1u

/* Die kleinste Region und die, ab der sie Achtel hat. */
#define TAKT_REGION_MIN 32u
#define TAKT_REGION_EIGHTHS 256u

/* Ein Befehl in Thumb-2 ist vier Byte lang, wenn die oberen fuenf Bits
 * seines ersten Halbworts 0b11101 oder groesser sind, sonst zwei. */
#define TAKT_THUMB_WIDE 0x1Du

/* Wo eine Verletzung lag, die nicht im Programmbereich war. */
#define TAKT_VIOLATION_ELSEWHERE 3u

/* Die Zeitachse zaehlt mit 1 MHz: ein Schritt je Mikrosekunde, und die
 * Rechnung in Nanosekunden ist eine Multiplikation. */
#define TAKT_COUNTS_HZ 1000000u
#define TAKT_NS_PER_COUNT 1000u

/* Der Prescaler von TIM2 hat 16 Bit. */
#define TAKT_PRESCALER_MAX 65536u

/* Der Takt des Zeitgebers teilt sich nicht auf 1 MHz. */
#define TAKT_STM32F4_CLOCK (-2)

/* Keine Region deckt die Arena: `align` ist keine Zweierpotenz ab 32, die
 * Arena liegt nicht an `align`, oder `bytes` passt nicht hinein. */
#define TAKT_STM32F4_REGION (-3)

/* Der Port, den beide ISRs rufen; gesetzt vor ihrer Freigabe. */
static struct takt_irq *takt_port;

/* Atomar: die Ueberlaeufe von TIM2; nur seine ISR zaehlt sie. */
static uint32_t takt_epoch;

/* Der Zielstand des Alarms auf der Zeitachse und ob er steht. Beide gehoeren
 * der ISR von TIM2; wer sie sonst schreibt, sperrt sie (`takt_lock`). */
static uint64_t takt_target;
static uint32_t takt_armed;

/* `RASR` der Region der Arena, geschlossen; 0, solange es keine gibt.
 * Gesetzt, bevor die MPU sie bewacht, danach nur gelesen. */
static uint32_t takt_arena_rasr;

/* Wo die Region beginnt und wie viele Byte sie deckt. */
static uint32_t takt_arena_start;
static uint32_t takt_arena_bytes;

/* Atomar: wo die letzte Verletzung lag (TAKT_VIOLATION_*, 0 keine) und
 * ihre Adresse; der Handler schreibt, der Kern holt ab. */
static uint32_t takt_violation;
static uint32_t takt_violation_at;

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

/* Eine Aenderung der MPU gilt fuer den naechsten Befehl. */
static void takt_barrier(void) { __asm__ volatile("dsb\n\tisb" : : : "memory"); }

/* Setzt die Rechte der Region der Arena. Wer `RNR` sonst umstellt, eine ISR
 * zwischen `takt_irq_chip_isr_enter` und `_leave`, stellt sie wieder auf
 * diese Region. */
static void takt_arena_access(uint32_t rasr) {
    *takt_reg(MPU_RNR) = TAKT_ARENA_REGION;
    *takt_reg(MPU_RASR) = rasr;
    takt_barrier();
}

/* Die Haken fuer den Kern (`struct takt_protection`). */
static void takt_arena_open(void *ctx) {
    (void)ctx;
    takt_arena_access((takt_arena_rasr & ~MPU_RASR_AP) | MPU_AP_WRITE);
}

static void takt_arena_close(void *ctx) {
    (void)ctx;
    takt_arena_access(takt_arena_rasr);
}

static int32_t takt_arena_violation(void *ctx, uint32_t *address) {
    (void)ctx;
    const uint32_t region = __atomic_exchange_n(&takt_violation, 0u, __ATOMIC_SEQ_CST);
    *address = __atomic_load_n(&takt_violation_at, __ATOMIC_SEQ_CST);
    return (int32_t)region;
}

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
        const uint32_t again = __atomic_load_n(&takt_epoch, __ATOMIC_SEQ_CST);
        if (again == epoch) {
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

int64_t takt_irq_chip_now(void) {
    const uint64_t ns = takt_counts() * TAKT_NS_PER_COUNT;
    return (int64_t)ns;
}

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
        /* Nur diese ISR zaehlt die Epoche. */
        const uint32_t epoch = __atomic_load_n(&takt_epoch, __ATOMIC_SEQ_CST);
        *takt_reg(TIM2_SR) = ~TIM_SR_UIF;
        __atomic_store_n(&takt_epoch, epoch + 1u, __ATOMIC_SEQ_CST);
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

int takt_irq_chip_protect(void *arena, uint32_t align, uint32_t bytes, struct takt_protection *protection) {
    const uint32_t start = (uint32_t)(uintptr_t)arena;
    int rc = TAKT_STM32F4_REGION;
    if ((arena == NULL) || (protection == NULL)) {
        rc = TAKT_IRQ_INVALID;
    } else if ((align >= TAKT_REGION_MIN) && ((align & (align - 1u)) == 0u) && ((start & (align - 1u)) == 0u) &&
               (bytes > 0u) && (bytes <= align)) {
        /* Ab 256 Byte deckt die Region ganze Achtel, die dahinter schaltet
         * das Feld SRD ab (ARMv7-M B3.5.8); darunter deckt sie ganz. */
        uint32_t eighths = 8u;
        if (align >= TAKT_REGION_EIGHTHS) {
            eighths = (bytes + (align / 8u) - 1u) / (align / 8u);
        }
        const uint32_t disabled = (eighths >= 8u) ? 0u : ((0xFFu << eighths) & 0xFFu);
        /* SIZE ist log2(align) - 1. */
        uint32_t size = 0u;
        for (uint32_t span = align; span > 2u; span >>= 1u) {
            size++;
        }
        takt_arena_rasr = MPU_RASR_RAM | MPU_AP_READ | (disabled << 8u) | (size << 1u);
        takt_arena_start = start;
        takt_arena_bytes = (align / 8u) * eighths;
        *takt_reg(MPU_RNR) = TAKT_ARENA_REGION;
        *takt_reg(MPU_RBAR) = start;
        /* Offen, bis der Kern sie schliesst (`takt_core_protect`). */
        *takt_reg(MPU_RASR) = (takt_arena_rasr & ~MPU_RASR_AP) | MPU_AP_WRITE;
        *takt_reg(SCB_SHCSR) |= SCB_SHCSR_MEMFAULTENA;
        *takt_reg(MPU_CTRL) = MPU_CTRL_ON;
        takt_barrier();
        protection->ctx = NULL;
        protection->open = takt_arena_open;
        protection->close = takt_arena_close;
        protection->take_violation = takt_arena_violation;
        rc = 0;
    } else {
        /* Keine Region deckt die Arena. */
    }
    return rc;
}

uint32_t takt_irq_chip_isr_enter(void) {
    uint32_t before = 0u;
    if (takt_arena_rasr != 0u) {
        *takt_reg(MPU_RNR) = TAKT_ARENA_REGION;
        before = *takt_reg(MPU_RASR);
        *takt_reg(MPU_RASR) = takt_arena_rasr;
        takt_barrier();
    }
    return before;
}

void takt_irq_chip_isr_leave(uint32_t before) {
    if (takt_arena_rasr != 0u) {
        takt_arena_access(before);
    }
}

/* Holt den gestapelten Rahmen vom Stack, der beim Eintritt galt, und springt
 * in den Rumpf; dessen Rueckkehr ist die aus der Ausnahme, denn `lr` traegt
 * noch EXC_RETURN. */
void MemManage_Handler(void) {
    __asm__ volatile("tst lr, #4\n\tite eq\n\tmrseq r0, msp\n\tmrsne r0, psp\n\tb takt_irq_chip_mem_manage\n");
}

/* Merkt sich, wo der Zugriff lag, und uebergeht den Befehl: Index 6 des
 * Rahmens ist der PC des Befehls, der die Verletzung ausloeste. */
void takt_irq_chip_mem_manage(uint32_t *frame) {
    const uint32_t mmfsr = *takt_reg(SCB_CFSR) & SCB_MMFSR;
    if (mmfsr != SCB_MMFSR_DATA) {
        /* Ohne verlaesslichen Rahmen geht es nicht weiter. */
        *takt_reg(SCB_AIRCR) = SCB_AIRCR_RESET;
        for (;;) {
            takt_barrier();
        }
    }
    const uint32_t address = *takt_reg(SCB_MMFAR);
    const uint32_t region =
        ((address - takt_arena_start) < takt_arena_bytes) ? (uint32_t)TAKT_VIOLATION_STATE : TAKT_VIOLATION_ELSEWHERE;
    __atomic_store_n(&takt_violation_at, address, __ATOMIC_SEQ_CST);
    __atomic_store_n(&takt_violation, region, __ATOMIC_SEQ_CST);
    const uint32_t pc = frame[6];
    const uint16_t first = *(const volatile uint16_t *)(uintptr_t)pc;
    frame[6] = pc + ((((uint32_t)first >> 11u) >= TAKT_THUMB_WIDE) ? 4u : 2u);
    *takt_reg(SCB_CFSR) = mmfsr;
}
