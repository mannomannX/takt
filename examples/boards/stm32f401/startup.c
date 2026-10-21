/* startup.c - Start und Vektortabelle der Black Pill fuer einen C-Wirt
 * (M11 Schritt 18b), mit dem Linker-Skript `stm32f401.ld` oder, unter
 * FreeRTOS-MPU, `stm32f401-mpu.ld`.
 *
 * Das Sprungbrett in Sektor 1 startet die Anwendung ueber ihre
 * Vektortabelle: Stackzeiger und Resetvektor. `Reset_Handler` stellt VTOR
 * auf diese Tabelle (nach dem Reset zeigt es auf Sektor 0, den
 * HID-Bootloader), gibt die FPU frei, kopiert `.data`, nullt `.bss` und ruft
 * `main`.
 *
 * Jeder Handler, den ein Port oder der Wirt nicht stellt, nennt Grund und
 * Adresse der Ausnahme, haelt das Board an und laesst es fuer `TAKT`
 * erreichbar (`board_fault`). Die Namen sind die von
 * CMSIS: Der Port der Interruptform stellt `TIM2_IRQHandler` und
 * `PendSV_Handler`, die Leitung `USART1_IRQHandler`, ein RTOS `SVC_Handler`,
 * `PendSV_Handler` und `SysTick_Handler`, die Partition unter FreeRTOS-MPU
 * `MemManage_Handler`. Ein Treffer des Watchpoints unter dem Stack der
 * Interrupts (`DebugMon_Handler`) haelt an.
 */
#include <stddef.h>
#include <stdint.h>

#include "board.h"

/* Aus dem Linker-Skript: der Anfang der Vektortabelle, wo `.data` im Flash
 * liegt, wo `.data` und `.bss` im RAM liegen; das obere Ende des Stacks
 * nennt `board.h`. */
extern uint32_t board_vectors;
extern uint32_t board_data_load;
extern uint32_t board_data_start;
extern uint32_t board_data_end;
extern uint32_t board_bss_start;
extern uint32_t board_bss_end;

int main(void);

void Reset_Handler(void);
void Default_Handler(void);
void NMI_Handler(void) __attribute__((weak, alias("Default_Handler")));
void HardFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void MemManage_Handler(void) __attribute__((weak, alias("Default_Handler")));
void BusFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void UsageFault_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SVC_Handler(void) __attribute__((weak, alias("Default_Handler")));
void DebugMon_Handler(void) __attribute__((weak, alias("Default_Handler")));
void PendSV_Handler(void) __attribute__((weak, alias("Default_Handler")));
void SysTick_Handler(void) __attribute__((weak, alias("Default_Handler")));
void TIM2_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));
void USART1_IRQHandler(void) __attribute__((weak, alias("Default_Handler")));

/* VTOR und CPACR des Cortex-M4. */
#define SCB_VTOR 0xE000ED08u
#define SCB_CPACR 0xE000ED88u
#define SCB_CPACR_FPU 0x00F00000u /* CP10 und CP11: volle Rechte */

/* Die Interrupts des STM32F401 (RM0368, Abschnitt 10, die Vektortabelle). */
#define BOARD_IRQS 85u

typedef void (*board_handler)(void);

/* Die Vektortabelle des Cortex-M4: Stackzeiger, Reset, die Ausnahmen 2 bis
 * 15 (0 an den reservierten Stellen), dann die Interrupts des Chips. */
struct board_vectors {
    const void *stack;
    board_handler reset;
    board_handler exceptions[14];
    board_handler irqs[BOARD_IRQS];
};

__attribute__((section(".vectors"), used)) static const struct board_vectors board_vector_table = {
    &board_stack_top,
    Reset_Handler,
    {NMI_Handler, HardFault_Handler, MemManage_Handler, BusFault_Handler, UsageFault_Handler, NULL, NULL, NULL, NULL,
     SVC_Handler, DebugMon_Handler, NULL, PendSV_Handler, SysTick_Handler},
    {
        /* 0 bis 7: WWDG, PVD, TAMP_STAMP, RTC_WKUP, FLASH, RCC, EXTI0, EXTI1 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 8 bis 15: EXTI2, EXTI3, EXTI4, DMA1_Stream0 bis 4 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 16 bis 23: DMA1_Stream5, DMA1_Stream6, ADC, vier reserviert, EXTI9_5 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 24 bis 31: TIM1_BRK_TIM9, TIM1_UP_TIM10, TIM1_TRG_COM_TIM11, TIM1_CC, TIM2, TIM3, TIM4, I2C1_EV */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, TIM2_IRQHandler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 32 bis 39: I2C1_ER, I2C2_EV, I2C2_ER, SPI1, SPI2, USART1, USART2, reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, USART1_IRQHandler,
        Default_Handler, Default_Handler,
        /* 40 bis 47: EXTI15_10, RTC_Alarm, OTG_FS_WKUP, vier reserviert, DMA1_Stream7 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 48 bis 55: reserviert, SDIO, TIM5, SPI3, vier reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 56 bis 63: DMA2_Stream0 bis 4, zwei reserviert, reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 64 bis 71: reserviert, reserviert, reserviert, OTG_FS, DMA2_Stream5 bis 7, USART6 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 72 bis 79: I2C3_EV, I2C3_ER, sechs reserviert */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
        Default_Handler, Default_Handler,
        /* 80 bis 84: reserviert, FPU, zwei reserviert, SPI4 */
        Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
    },
};

/* Ein Wort an seiner Adresse: ein Register aus dem Handbuch oder ein Wort
 * von `.data` und `.bss`. */
static volatile uint32_t *board_word(uintptr_t address) { return (volatile uint32_t *)address; }

/* Die Adresse eines Objekts als Zahl. */
static uintptr_t board_start_address(const volatile uint32_t *object) { return (uintptr_t)object; }

/* Der Start nach dem Ausschalten der MPU: `.data`, `.bss`, `main`. */
void board_reset(void);

/* Der Resetvektor schaltet vor allem anderen die MPU aus (FB-752): Ein
 * Vorgaenger, der ohne Reset in den Bootloader sprang, kann sie mit seinen
 * Regionen eingeschaltet lassen, und schon der erste Rahmen auf dem Stack
 * oder das Nullen von `.bss` fiele unter eine fremde Region. Ohne Prolog,
 * also ohne Stack; `MPU_CTRL` steht bei 0xE000ED94. */
__attribute__((naked)) void Reset_Handler(void) {
    __asm__ volatile("movw r0, #0xED94\n\t"
                     "movt r0, #0xE000\n\t"
                     "movs r1, #0\n\t"
                     "str r1, [r0]\n\t"
                     "dsb\n\t"
                     "isb\n\t"
                     "b board_reset\n\t");
}

void board_reset(void) {
    const uintptr_t load = board_start_address(&board_data_load);
    const uintptr_t data = board_start_address(&board_data_start);
    const uintptr_t data_end = board_start_address(&board_data_end);
    const uintptr_t bss_end = board_start_address(&board_bss_end);
    *board_word(SCB_VTOR) = (uint32_t)board_start_address(&board_vectors);
    *board_word(SCB_CPACR) |= SCB_CPACR_FPU;
    __asm__ volatile("dsb\n\tisb" : : : "memory");
    for (uintptr_t at = data; at < data_end; at += sizeof(uint32_t)) {
        *board_word(at) = *board_word(load + (at - data));
    }
    for (uintptr_t at = board_start_address(&board_bss_start); at < bss_end; at += sizeof(uint32_t)) {
        *board_word(at) = 0u;
    }
    (void)main();
    board_halt();
}

void Default_Handler(void) { board_fault(); }
