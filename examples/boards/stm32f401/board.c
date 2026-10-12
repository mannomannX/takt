/* board.c - die Register der Black Pill fuer einen C-Wirt (board.h).
 *
 * Gespiegelt aus dem Board-Crate in Rust (`takt-board-stm32f401`), so weit
 * ein C-Wirt es braucht: der Takt (`lib.rs`, `clocks`), die Leitung (`uart.rs`),
 * das Journal (`nvm.rs`) und der Stack (`stack.rs`, `mpu.rs`). Den Weg in den
 * DFU-Bootloader geht das Sprungbrett (`takt-trampoline-stm32f401`); der Wirt
 * merkt sich nur den Wunsch. Register nach RM0368 und dem Handbuch des
 * Cortex-M4.
 */
#include "board.h"

#include <stddef.h>
#include <stdint.h>

#include "takt_sdk.h"

/* Die ISR der Leitung, wie die Vektortabelle sie nennt (`startup.c`). */
void USART1_IRQHandler(void);

/* Die Panik des SDK-Archivs ruft `abort` der C-Laufzeit; ohne C-Bibliothek
 * stellt der Wirt sie. */
void abort(void);

/* RCC. */
#define RCC_CR 0x40023800u
#define RCC_PLLCFGR 0x40023804u
#define RCC_CFGR 0x40023808u
#define RCC_AHB1ENR 0x40023830u
#define RCC_APB1ENR 0x40023840u
#define RCC_APB2ENR 0x40023844u
#define RCC_CR_HSION 0x00000001u
#define RCC_CR_HSIRDY 0x00000002u
#define RCC_CR_HSEON 0x00010000u
#define RCC_CR_HSERDY 0x00020000u
#define RCC_CR_PLLON 0x01000000u
#define RCC_CR_PLLRDY 0x02000000u
#define RCC_CFGR_SW 0x00000003u
#define RCC_CFGR_SW_PLL 0x00000002u
#define RCC_CFGR_SWS 0x0000000Cu
#define RCC_CFGR_SWS_PLL 0x00000008u
#define RCC_CFGR_PRESCALERS 0x0000FCF0u /* HPRE, PPRE1, PPRE2 */
#define RCC_CFGR_PPRE1_DIV2 0x00001000u
#define RCC_AHB1ENR_GPIOA 0x00000001u
#define RCC_AHB1ENR_GPIOC 0x00000004u
#define RCC_APB1ENR_PWREN 0x10000000u
#define RCC_APB2ENR_USART1EN 0x00000010u

/* Die PLL: 25 MHz / M * N / P = 84 MHz, der Takt von USB / Q = 48 MHz; die
 * Werte an ihrer Stelle in RCC_PLLCFGR. */
#define PLL_FIELDS 0x0F437FFFu /* PLLQ, PLLSRC, PLLP, PLLN, PLLM */
#define PLL_SRC_HSE 0x00400000u
#define PLL_M_25 0x00000019u   /* M = 25 in 5:0 */
#define PLL_N_336 0x00005400u  /* N = 336 in 14:6 */
#define PLL_P_4 0x00010000u    /* P = 4 (Kodierung 01) in 17:16 */
#define PLL_Q_7 0x07000000u    /* Q = 7 in 27:24 */

/* PWR und die Backup-Register in der RTC. */
#define PWR_CR 0x40007000u
#define PWR_CR_DBP 0x00000100u
#define PWR_CR_VOS 0x0000C000u
#define PWR_CR_VOS_84MHZ 0x00008000u
#define RTC_BKP4R 0x40002860u

/* Flash: Wartezyklen, Journal (RM0368 3.5, 3.8). */
#define FLASH_ACR 0x40023C00u
#define FLASH_KEYR 0x40023C04u
#define FLASH_SR 0x40023C0Cu
#define FLASH_CR 0x40023C10u
#define FLASH_ACR_LATENCY 0x0000000Fu
#define FLASH_ACR_LATENCY_2WS 0x00000002u
#define FLASH_ACR_PRFTEN 0x00000100u
#define FLASH_ACR_CACHES 0x00000600u /* ICEN, DCEN */
#define FLASH_KEY1 0x45670123u
#define FLASH_KEY2 0xCDEF89ABu
#define FLASH_SR_EOP 0x00000001u
#define FLASH_SR_ERRORS 0x000000F2u /* OPERR, WRPERR, PGAERR, PGPERR, PGSERR */
#define FLASH_SR_BSY 0x00010000u
#define FLASH_CR_PG 0x00000001u
#define FLASH_CR_SER 0x00000002u
#define FLASH_CR_SNB 0x00000078u
#define FLASH_CR_PSIZE 0x00000300u
#define FLASH_CR_PSIZE_X32 0x00000200u
#define FLASH_CR_STRT 0x00010000u
#define FLASH_CR_LOCK 0x80000000u

/* Das Journal: Sektor 2 und 3 ab 0x0800_8000. */
#define BOARD_NVM_BASE 0x08008000u
#define BOARD_NVM_SECTOR 2u

/* GPIO. */
#define GPIOA_MODER 0x40020000u
#define GPIOA_OSPEEDR 0x40020008u
#define GPIOA_PUPDR 0x4002000Cu
#define GPIOA_AFRH 0x40020024u
#define GPIOC_MODER 0x40020800u
#define GPIOC_BSRR 0x40020818u
#define PA9_PA10_MODE 0x003C0000u
#define PA9_PA10_ALTERNATE 0x00280000u
#define PA9_PA10_AF 0x00000FF0u
#define PA9_PA10_AF7 0x00000770u
#define PA9_SPEED 0x000C0000u
#define PA9_SPEED_HIGH 0x00080000u
#define PA10_PULL 0x00300000u
#define PA10_PULL_UP 0x00100000u
#define PC13_MODE 0x0C000000u
#define PC13_OUTPUT 0x04000000u
#define PC13_HIGH 0x00002000u
#define PC13_LOW 0x20000000u

/* USART1. */
#define USART1_SR 0x40011000u
#define USART1_DR 0x40011004u
#define USART1_BRR 0x40011008u
#define USART1_CR1 0x4001100Cu
#define USART_SR_ORE 0x00000008u
#define USART_SR_RXNE 0x00000020u
#define USART_SR_TXE 0x00000080u
#define USART_CR1_RE 0x00000004u
#define USART_CR1_TE 0x00000008u
#define USART_CR1_RXNEIE 0x00000020u
#define USART_CR1_TXEIE 0x00000080u
#define USART_CR1_UE 0x00002000u

/* Die Rate der Leitung, fuer jedes Programm dieselbe (FB-275): Der Teiler
 * rundet; der USART trifft 923077 Baud, wie der FT232RL am anderen Ende. */
#define BOARD_BAUD 921600u
#define BOARD_BRR ((BOARD_CORE_HZ + (BOARD_BAUD / 2u)) / BOARD_BAUD)

/* NVIC, SCB, MPU des Cortex-M4. */
#define NVIC_ISER1 0xE000E104u
#define NVIC_IPR 0xE000E400u
#define USART1_IRQN 37u
#define USART1_ISER_BIT 0x00000020u /* 37 - 32 */
#define BOARD_LINE_PRIORITY 0x10u
#define SCB_AIRCR 0xE000ED0Cu
#define SCB_AIRCR_VECTKEY 0x05FA0000u
#define SCB_AIRCR_PRIGROUP 0x00000700u
#define SCB_AIRCR_SYSRESETREQ 0x00000004u
#define SCB_SHCSR 0xE000ED24u
#define SCB_SHCSR_MEMFAULTENA 0x00010000u
#define MPU_CTRL 0xE000ED94u
#define MPU_RNR 0xE000ED98u
#define MPU_RBAR 0xE000ED9Cu
#define MPU_RASR 0xE000EDA0u
#define MPU_CTRL_ON 0x00000005u      /* ENABLE, PRIVDEFENA */
#define MPU_RASR_GUARD 0x10000009u   /* XN, kein Zugriff, 32 Byte, ENABLE */

/* DWT und Debug-Monitor des Cortex-M4: der Komparator 0 als Watchpoint. */
#define SCB_SHPR3 0xE000ED20u        /* Byte 0: Prioritaet von DebugMonitor */
#define DEMCR 0xE000EDFCu
#define DEMCR_TRCENA 0x01000000u
#define DEMCR_MON_EN 0x00010000u
#define DWT_CTRL 0xE0001000u
#define DWT_CTRL_NUMCOMP_SHIFT 28u
#define DWT_COMP0 0xE0001020u
#define DWT_MASK0 0xE0001024u
#define DWT_FUNCTION0 0xE0001028u
#define DWT_MASK_GUARD 5u            /* die unteren 5 Bit: 32 Byte */
#define DWT_FUNCTION_ACCESS 7u       /* Watchpoint auf Lesen oder Schreiben */

_Static_assert(BOARD_LINE_PRIORITY < BOARD_STEP_PRIORITY, "die Leitung liegt ueber dem Schritt");

/* So oft fragt eine Schleife hoechstens ab: einige Millisekunden beim Start
 * mit 16 MHz; ein Quarz braucht unter einer. */
#define BOARD_SPIN 100000u

/* So oft fragt das Journal `BSY` hoechstens ab: ueber zwei Sekunden, das
 * Vierfache der laengsten Sektorloeschung im Datenblatt. */
#define BOARD_FLASH_SPIN (2u * BOARD_CORE_HZ)

/* Der Wunsch nach dem Bootloader im Backup-Register 4: die Bytes `DFU!`,
 * wie `takt_board_stm32f401::bootloader` ihn schreibt und das Sprungbrett
 * ihn liest. */
#define BOARD_DFU_WISH 0x21554644u

/* Die Flusssteuerung des Adapters (FB-306). */
#define BOARD_XON 0x11u
#define BOARD_XOFF 0x13u

/* Das FIFO vor dem Senderegister: gut fuenf Millisekunden der Leitung. */
#define BOARD_TX_BYTES 512u

/* Das Muster der Stack-Bemalung, wie im Bring-up in Rust; der Abstand, den
 * die Bemalung unter dem Stackzeiger laesst, weil sie selbst Stack braucht. */
#define BOARD_PAINT 0xC5C5C5C5u
#define BOARD_PAINT_MARGIN 256u

/* Aus dem Linker-Skript `stm32f401.ld`: der Waechter und das untere Ende
 * des Stacks. */
extern uint32_t board_stack_guard;
extern uint32_t board_stack_bottom;

/* Das FIFO der Leitung: `tail` schreibt, wer sendet (Schritt oder Wirt, nie
 * beide zugleich), `head` nur die ISR; beide zaehlen frei und atomar. */
static uint8_t board_tx[BOARD_TX_BYTES];
static uint32_t board_tx_head;
static uint32_t board_tx_tail;

/* Atomar: Der Adapter sandte XOFF und noch kein XON. */
static int board_paused;

/* Atomar: Der Host verlangte das Board zurueck. */
static int board_handback;

/* Atomar: Die Leitung ist eingerichtet. */
static int board_line_ready;

/* Wie viele Bytes von `TAKT` der Erkenner gesehen hat; nur die ISR der
 * Leitung, oder der Halt bei gesperrten Interrupts. */
static uint32_t board_matched;

/* Der Ausgang des letzten Vorgangs des Journals (`TAKT_NVM_*`). */
static int32_t board_nvm_state = TAKT_NVM_IDLE;

/* Ein Register an seiner Adresse aus dem Handbuch. */
static volatile uint32_t *board_reg(uintptr_t address) { return (volatile uint32_t *)address; }

static volatile uint8_t *board_reg8(uintptr_t address) { return (volatile uint8_t *)address; }

/* Die Adresse eines Objekts als Zahl: fuer den Stack und die Region der MPU. */
static uintptr_t board_address(const volatile uint32_t *object) { return (uintptr_t)object; }

/* Sperrt die Interrupts und nennt, wie PRIMASK vorher stand. */
static uint32_t board_lock(void) {
    uint32_t primask;
    __asm__ volatile("mrs %0, primask\n\tcpsid i" : "=r"(primask) : : "memory");
    return primask;
}

static void board_unlock(uint32_t primask) { __asm__ volatile("msr primask, %0" : : "r"(primask) : "memory"); }

static void board_barrier(void) { __asm__ volatile("dsb\n\tisb" : : : "memory"); }

/* Wartet beschraenkt, bis die Bits `mask` im Register `address` den Wert
 * `want` haben; 1, wenn sie ihn bekamen. */
static uint32_t board_wait(uintptr_t address, uint32_t mask, uint32_t want) {
    uint32_t reached = 0u;
    for (uint32_t i = 0u; (i < BOARD_SPIN) && (reached == 0u); i++) {
        if ((*board_reg(address) & mask) == want) {
            reached = 1u;
        }
    }
    return reached;
}

/* Der Systemtakt auf HSI, die PLL aus: Nach dem HID-Bootloader laeuft sie,
 * und solange sie laeuft, verpufft jede neue Konfiguration still. */
static uint32_t board_on_hsi(void) {
    uint32_t ok;
    *board_reg(RCC_CR) |= RCC_CR_HSION;
    ok = board_wait(RCC_CR, RCC_CR_HSIRDY, RCC_CR_HSIRDY);
    if (ok != 0u) {
        *board_reg(RCC_CFGR) &= ~RCC_CFGR_SW;
        ok = board_wait(RCC_CFGR, RCC_CFGR_SWS, 0u);
    }
    if (ok != 0u) {
        *board_reg(RCC_CR) &= ~RCC_CR_PLLON;
        ok = board_wait(RCC_CR, RCC_CR_PLLRDY, 0u);
    }
    return ok;
}

/* 84 MHz aus dem Quarz, in der Reihenfolge des Bring-ups: der Regler fuer
 * 84 MHz, die Wartezyklen des Flash (zurueckgelesen), HSI, HSE, die PLL,
 * die Teiler der Busse, dann der Wechsel. Die Caches des Flash bleiben aus:
 * Die Zeit eines Ticks haengt sonst am Inhalt, den der vorige hinterliess,
 * und das Journal liest das Flash selbst. 1, wenn alles kam. */
static uint32_t board_clocks(void) {
    uint32_t ok;
    *board_reg(RCC_APB1ENR) |= RCC_APB1ENR_PWREN;
    (void)*board_reg(RCC_APB1ENR);
    *board_reg(PWR_CR) = (*board_reg(PWR_CR) & ~PWR_CR_VOS) | PWR_CR_VOS_84MHZ;
    *board_reg(FLASH_ACR) = (*board_reg(FLASH_ACR) & ~(FLASH_ACR_LATENCY | FLASH_ACR_CACHES)) |
                            FLASH_ACR_LATENCY_2WS | FLASH_ACR_PRFTEN;
    ok = board_wait(FLASH_ACR, FLASH_ACR_LATENCY, FLASH_ACR_LATENCY_2WS);
    if (ok != 0u) {
        ok = board_on_hsi();
    }
    if (ok != 0u) {
        *board_reg(RCC_CR) |= RCC_CR_HSEON;
        ok = board_wait(RCC_CR, RCC_CR_HSERDY, RCC_CR_HSERDY);
    }
    if (ok != 0u) {
        *board_reg(RCC_PLLCFGR) =
            (*board_reg(RCC_PLLCFGR) & ~PLL_FIELDS) | PLL_SRC_HSE | PLL_Q_7 | PLL_P_4 | PLL_N_336 | PLL_M_25;
        *board_reg(RCC_CR) |= RCC_CR_PLLON;
        ok = board_wait(RCC_CR, RCC_CR_PLLRDY, RCC_CR_PLLRDY);
    }
    if (ok != 0u) {
        /* AHB und APB2 ohne Teiler, APB1 durch zwei: hoechstens 42 MHz dort,
         * die Zeitgeber daran bekommen den doppelten Takt. */
        *board_reg(RCC_CFGR) = (*board_reg(RCC_CFGR) & ~RCC_CFGR_PRESCALERS) | RCC_CFGR_PPRE1_DIV2;
        *board_reg(RCC_CFGR) = (*board_reg(RCC_CFGR) & ~RCC_CFGR_SW) | RCC_CFGR_SW_PLL;
        ok = board_wait(RCC_CFGR, RCC_CFGR_SWS, RCC_CFGR_SWS_PLL);
    }
    return ok;
}

/* USART1 an PA9 (TX) und PA10 (RX, mit Pull-up: ohne Gegenstelle laege der
 * Pin in der Luft), 921600 Baud; der Interrupt liest jedes Byte. */
static void board_usart(void) {
    *board_reg(RCC_APB2ENR) |= RCC_APB2ENR_USART1EN;
    (void)*board_reg(RCC_APB2ENR);
    *board_reg(GPIOA_MODER) = (*board_reg(GPIOA_MODER) & ~PA9_PA10_MODE) | PA9_PA10_ALTERNATE;
    *board_reg(GPIOA_AFRH) = (*board_reg(GPIOA_AFRH) & ~PA9_PA10_AF) | PA9_PA10_AF7;
    *board_reg(GPIOA_PUPDR) = (*board_reg(GPIOA_PUPDR) & ~PA10_PULL) | PA10_PULL_UP;
    *board_reg(GPIOA_OSPEEDR) = (*board_reg(GPIOA_OSPEEDR) & ~PA9_SPEED) | PA9_SPEED_HIGH;
    *board_reg(USART1_BRR) = BOARD_BRR;
    *board_reg(USART1_CR1) = USART_CR1_UE | USART_CR1_TE | USART_CR1_RE | USART_CR1_RXNEIE;
    *board_reg8(NVIC_IPR + USART1_IRQN) = (uint8_t)BOARD_LINE_PRIORITY;
    *board_reg(NVIC_ISER1) = USART1_ISER_BIT;
}

int board_init(void) {
    int rc = -1;
    *board_reg(RCC_AHB1ENR) |= RCC_AHB1ENR_GPIOA | RCC_AHB1ENR_GPIOC;
    (void)*board_reg(RCC_AHB1ENR);
    *board_reg(GPIOC_BSRR) = PC13_HIGH;
    *board_reg(GPIOC_MODER) = (*board_reg(GPIOC_MODER) & ~PC13_MODE) | PC13_OUTPUT;
    if (board_clocks() != 0u) {
        board_usart();
        __atomic_store_n(&board_line_ready, 1, __ATOMIC_SEQ_CST);
        rc = 0;
    } else {
        /* Ohne Takt keine Leitung: Die LED leuchtet. */
        board_led(1);
    }
    return rc;
}

void board_led(int on) { *board_reg(GPIOC_BSRR) = (on != 0) ? PC13_LOW : PC13_HIGH; }

/* Gibt das Board an den Host zurueck: der Wunsch ins Backup-Register, dann
 * der Reset. Nach ihm laeuft der HID-Bootloader, dann das Sprungbrett, und
 * das findet den Wunsch. Kehrt nicht zurueck. */
static void board_hand_back(void) {
    *board_reg(RCC_APB1ENR) |= RCC_APB1ENR_PWREN;
    (void)*board_reg(RCC_APB1ENR);
    *board_reg(PWR_CR) |= PWR_CR_DBP;
    *board_reg(RTC_BKP4R) = BOARD_DFU_WISH;
    *board_reg(PWR_CR) &= ~PWR_CR_DBP;
    board_barrier();
    *board_reg(SCB_AIRCR) = SCB_AIRCR_VECTKEY | (*board_reg(SCB_AIRCR) & SCB_AIRCR_PRIGROUP) | SCB_AIRCR_SYSRESETREQ;
    board_barrier();
    for (;;) {
        /* Der Reset kommt. */
    }
}

/* Der Erkenner von `TAKT`: 1 mit dem letzten Byte, danach beginnt die Suche
 * neu (`takt_board_support::console::Magic`). */
static uint32_t board_feed(uint8_t byte) {
    static const uint8_t magic[4] = {0x54u, 0x41u, 0x4Bu, 0x54u};
    uint32_t found = 0u;
    if (byte == magic[board_matched]) {
        board_matched++;
    } else {
        board_matched = (byte == magic[0]) ? 1u : 0u;
    }
    if (board_matched == 4u) {
        board_matched = 0u;
        found = 1u;
    }
    return found;
}

uint8_t board_line_put(void *ctx, uint8_t byte) {
    const uint32_t tail = __atomic_load_n(&board_tx_tail, __ATOMIC_SEQ_CST);
    const uint32_t head = __atomic_load_n(&board_tx_head, __ATOMIC_SEQ_CST);
    uint8_t taken = 0u;
    (void)ctx;
    if ((tail - head) < BOARD_TX_BYTES) {
        board_tx[tail % BOARD_TX_BYTES] = byte;
        __atomic_store_n(&board_tx_tail, tail + 1u, __ATOMIC_SEQ_CST);
        taken = 1u;
    }
    return taken;
}

void board_line_flush(void *ctx) {
    const uint32_t tail = __atomic_load_n(&board_tx_tail, __ATOMIC_SEQ_CST);
    const uint32_t head = __atomic_load_n(&board_tx_head, __ATOMIC_SEQ_CST);
    (void)ctx;
    if (__atomic_load_n(&board_handback, __ATOMIC_SEQ_CST) != 0) {
        board_hand_back();
    }
    if ((tail != head) && (__atomic_load_n(&board_paused, __ATOMIC_SEQ_CST) == 0)) {
        const uint32_t primask = board_lock();
        *board_reg(USART1_CR1) |= USART_CR1_TXEIE;
        board_unlock(primask);
    }
}

/* Die Leitung: erst empfangen, damit ein XOFF vor dem naechsten Byte wirkt,
 * hoechstens vier Bytes je Aufruf; SR vor DR zu lesen loescht auch einen
 * Ueberlauf. Dann je TXE ein Byte aus dem FIFO; ist es leer oder die
 * Leitung angehalten, schweigt der Sende-Interrupt bis zum naechsten
 * `board_line_flush` oder XON. Den Wunsch des Hosts fuehrt nicht die ISR
 * aus, sondern das naechste `board_line_flush`: nie mitten in einem Schritt. */
void USART1_IRQHandler(void) {
    uint32_t reading = 1u;
    for (uint32_t i = 0u; (i < 4u) && (reading != 0u); i++) {
        const uint32_t sr = *board_reg(USART1_SR);
        if ((sr & (USART_SR_RXNE | USART_SR_ORE)) == 0u) {
            reading = 0u;
        } else {
            const uint8_t byte = (uint8_t)(*board_reg(USART1_DR) & 0xFFu);
            if (byte == BOARD_XOFF) {
                __atomic_store_n(&board_paused, 1, __ATOMIC_SEQ_CST);
            } else if (byte == BOARD_XON) {
                __atomic_store_n(&board_paused, 0, __ATOMIC_SEQ_CST);
                *board_reg(USART1_CR1) |= USART_CR1_TXEIE;
            } else if (board_feed(byte) != 0u) {
                __atomic_store_n(&board_handback, 1, __ATOMIC_SEQ_CST);
            } else {
                /* Ein Byte ohne Bedeutung fuer diesen Wirt: Er liest keine Tunes. */
            }
        }
    }
    if (((*board_reg(USART1_CR1) & USART_CR1_TXEIE) != 0u) && ((*board_reg(USART1_SR) & USART_SR_TXE) != 0u)) {
        const uint32_t head = __atomic_load_n(&board_tx_head, __ATOMIC_SEQ_CST);
        const uint32_t tail = __atomic_load_n(&board_tx_tail, __ATOMIC_SEQ_CST);
        if ((__atomic_load_n(&board_paused, __ATOMIC_SEQ_CST) == 0) && (head != tail)) {
            *board_reg(USART1_DR) = (uint32_t)board_tx[head % BOARD_TX_BYTES];
            __atomic_store_n(&board_tx_head, head + 1u, __ATOMIC_SEQ_CST);
        } else {
            *board_reg(USART1_CR1) &= ~USART_CR1_TXEIE;
        }
    }
}

/* Wartet, bis das Flash frei ist; 1, wenn es binnen der Schranke frei wurde. */
static uint32_t board_flash_idle(void) {
    uint32_t idle = 0u;
    for (uint32_t i = 0u; (i < BOARD_FLASH_SPIN) && (idle == 0u); i++) {
        if ((*board_reg(FLASH_SR) & FLASH_SR_BSY) == 0u) {
            idle = 1u;
        }
    }
    return idle;
}

/* Der laufende Vorgang endete ohne Fehlerbit. */
static uint32_t board_flash_settled(void) {
    uint32_t ok = board_flash_idle();
    if ((*board_reg(FLASH_SR) & FLASH_SR_ERRORS) != 0u) {
        ok = 0u;
    }
    return ok;
}

/* Entsperrt FLASH_CR und loescht alte Fehler; ein falscher Schluessel
 * sperrte das Register bis zum Reset, darum nur, wenn es gesperrt ist. */
static uint32_t board_flash_open(void) {
    const uint32_t ok = board_flash_idle();
    if (ok != 0u) {
        if ((*board_reg(FLASH_CR) & FLASH_CR_LOCK) != 0u) {
            *board_reg(FLASH_KEYR) = FLASH_KEY1;
            *board_reg(FLASH_KEYR) = FLASH_KEY2;
        }
        *board_reg(FLASH_SR) = FLASH_SR_ERRORS | FLASH_SR_EOP;
    }
    return ok;
}

/* Loescht den Sektor `sector` (RM0368 3.5.3). */
static uint32_t board_flash_erase(uint32_t sector) {
    uint32_t ok = board_flash_open();
    if (ok != 0u) {
        *board_reg(FLASH_CR) = (*board_reg(FLASH_CR) & ~(FLASH_CR_SNB | FLASH_CR_PSIZE)) | FLASH_CR_PSIZE_X32 |
                               FLASH_CR_SER | (sector << 3u);
        *board_reg(FLASH_CR) |= FLASH_CR_STRT;
        ok = board_flash_settled();
        *board_reg(FLASH_CR) &= ~FLASH_CR_SER;
    }
    *board_reg(FLASH_CR) |= FLASH_CR_LOCK;
    return ok;
}

/* Programmiert `len` Byte wortweise ab `address`, mit 0xFF aufgefuellt: Im
 * geloeschten Flash stehen dort ohnehin diese Bytes (RM0368 3.5.4). */
static uint32_t board_flash_program(uintptr_t address, const uint8_t *bytes, uint32_t len) {
    uint32_t ok = board_flash_open();
    if (ok != 0u) {
        *board_reg(FLASH_CR) = (*board_reg(FLASH_CR) & ~FLASH_CR_PSIZE) | FLASH_CR_PSIZE_X32 | FLASH_CR_PG;
        for (uint32_t at = 0u; (at < len) && (ok != 0u); at += 4u) {
            uint32_t word = 0xFFFFFFFFu;
            for (uint32_t i = 0u; (i < 4u) && (i < (len - at)); i++) {
                const uint32_t shift = 8u * i;
                const uint32_t mask = (uint32_t)0xFFu << shift;
                word = (word & ~mask) | ((uint32_t)bytes[at + i] << shift);
            }
            *board_reg(address + at) = word;
            ok = board_flash_settled();
        }
        *board_reg(FLASH_CR) &= ~FLASH_CR_PG;
    }
    *board_reg(FLASH_CR) |= FLASH_CR_LOCK;
    return ok;
}

/* Wo `offset` im Slot `slot` liegt; die Aufrufer pruefen beide vorher. */
static uintptr_t board_nvm_at(uint8_t slot, uint32_t offset) {
    return (uintptr_t)BOARD_NVM_BASE + ((uintptr_t)slot * (uintptr_t)BOARD_NVM_SLOT) + (uintptr_t)offset;
}

/* Passen `len` Byte ab `offset` in einen Slot? */
static uint32_t board_nvm_fits(uint32_t offset, uint32_t len) {
    return ((offset <= BOARD_NVM_SLOT) && (len <= (BOARD_NVM_SLOT - offset))) ? 1u : 0u;
}

uint8_t board_nvm_erase(void *ctx, uint8_t slot) {
    uint8_t started = 0u;
    (void)ctx;
    if (slot < 2u) {
        board_nvm_state = (board_flash_erase(BOARD_NVM_SECTOR + (uint32_t)slot) != 0u) ? TAKT_NVM_DONE : TAKT_NVM_FAILED;
        started = 1u;
    }
    return started;
}

uint8_t board_nvm_write(void *ctx, uint8_t slot, uint32_t offset, const uint8_t *bytes, uint32_t len) {
    uint8_t started = 0u;
    (void)ctx;
    if ((slot < 2u) && ((offset % 4u) == 0u) && (board_nvm_fits(offset, len) != 0u) && (bytes != NULL)) {
        board_nvm_state =
            (board_flash_program(board_nvm_at(slot, offset), bytes, len) != 0u) ? TAKT_NVM_DONE : TAKT_NVM_FAILED;
        started = 1u;
    }
    return started;
}

int32_t board_nvm_poll(void *ctx) {
    const int32_t state = board_nvm_state;
    (void)ctx;
    board_nvm_state = TAKT_NVM_IDLE;
    return state;
}

uint8_t board_nvm_read(void *ctx, uint8_t slot, uint32_t offset, uint8_t *into, uint32_t len) {
    uint8_t ok = 0u;
    (void)ctx;
    if ((slot < 2u) && (board_nvm_fits(offset, len) != 0u) && (into != NULL)) {
        const uintptr_t from = board_nvm_at(slot, offset);
        for (uint32_t i = 0u; i < len; i++) {
            into[i] = *board_reg8(from + i);
        }
        ok = 1u;
    }
    return ok;
}

int board_nvm_wipe(void) {
    int rc = -1;
    if ((board_flash_erase(BOARD_NVM_SECTOR) != 0u) && (board_flash_erase(BOARD_NVM_SECTOR + 1u) != 0u)) {
        rc = 0;
    }
    return rc;
}

void board_protect_stack(void) {
    *board_reg(MPU_RNR) = 0u;
    *board_reg(MPU_RBAR) = (uint32_t)board_address(&board_stack_guard);
    *board_reg(MPU_RASR) = MPU_RASR_GUARD;
    *board_reg(SCB_SHCSR) |= SCB_SHCSR_MEMFAULTENA;
    *board_reg(MPU_CTRL) = MPU_CTRL_ON;
    board_barrier();
}

/* DebugMonitor bekommt die hoechste Prioritaet; ohne Debugger, der anhaelt,
 * fuehrt ein Treffer dorthin (`DebugMon_Handler`, der Halt in `startup.c`). */
void board_watch_stack(void) {
    *board_reg(DEMCR) |= DEMCR_TRCENA;
    board_barrier();
    if ((*board_reg(DWT_CTRL) >> DWT_CTRL_NUMCOMP_SHIFT) == 0u) {
        board_halt();
    }
    *board_reg8(SCB_SHPR3) = 0u;
    *board_reg(DWT_COMP0) = (uint32_t)board_address(&board_stack_guard);
    *board_reg(DWT_MASK0) = DWT_MASK_GUARD;
    *board_reg(DWT_FUNCTION0) = DWT_FUNCTION_ACCESS;
    *board_reg(DEMCR) |= DEMCR_MON_EN;
    board_barrier();
}

void board_paint_stack(void) {
    uint32_t here = 0u;
    const uintptr_t limit = (board_address(&here) - BOARD_PAINT_MARGIN) & ~(uintptr_t)3u;
    for (uintptr_t at = board_address(&board_stack_bottom); at < limit; at += 4u) {
        *board_reg(at) = BOARD_PAINT;
    }
}

uint32_t board_stack_depth(void) {
    const uintptr_t top = board_address(&board_stack_top);
    uintptr_t at = board_address(&board_stack_bottom);
    while ((at < top) && (*board_reg(at) == BOARD_PAINT)) {
        at += 4u;
    }
    return (uint32_t)(top - at);
}

void board_park(void) {
    for (;;) {
        __asm__ volatile("wfi" : : : "memory");
        board_line_flush(NULL);
    }
}

/* Ein Byte an die Leitung, ohne Interrupt: hoechstens so lange, wie ein
 * Byte bei 921600 Baud braucht, vielfach. */
static void board_put_polled(uint8_t byte) {
    if (board_wait(USART1_SR, USART_SR_TXE, USART_SR_TXE) != 0u) {
        *board_reg(USART1_DR) = (uint32_t)byte;
    }
}

void board_halt(void) {
    static const char text[] = "\r\ntakt: angehalten\r\n";
    (void)board_lock();
    if (__atomic_load_n(&board_line_ready, __ATOMIC_SEQ_CST) != 0) {
        for (uint32_t i = 0u; text[i] != '\0'; i++) {
            board_put_polled((uint8_t)text[i]);
        }
        board_matched = 0u;
        for (;;) {
            if ((*board_reg(USART1_SR) & (USART_SR_RXNE | USART_SR_ORE)) != 0u) {
                if (board_feed((uint8_t)(*board_reg(USART1_DR) & 0xFFu)) != 0u) {
                    board_hand_back();
                }
            }
        }
    }
    for (;;) {
        /* Ohne Leitung bleibt nur die Hand am Board (BOOT0, NRST). */
    }
}

void abort(void) { board_halt(); }
