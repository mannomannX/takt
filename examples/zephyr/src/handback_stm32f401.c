/* handback_stm32f401.c - Board 1 an den DFU-Bootloader des ROM (AN2606,
 * FB-275, plan/f401.md 2): wie `takt_board_stm32f401::bootloader::enter`.
 *
 * Die Black Pill hat keinen KEY-Taster; den Bootloader erreicht die Hand nur
 * ueber BOOT0 und NRST. Darum springt die Anwendung selbst hinein, aus einem
 * Faden: Sie raeumt auf, was sie und der HID-Bootloader von WeAct
 * eingerichtet haben — Interrupts, SysTick, MPU, Takt auf HSI, PLL und HSE
 * aus, jede Peripherie ueber ihr Reset-Bit —, bildet den Systemspeicher nach
 * Adresse 0 ab und springt. Einen Watchdog startet dieses Beispiel nicht;
 * darum braucht der Sprung keinen Reset davor wie im Bring-up. */
#include "handback.h"

#include <stdint.h>

#include <soc.h>

/* Der Systemspeicher des F401 mit dem Bootloader von ST. */
#define SYSTEM_MEMORY 0x1FFF0000UL

/* `RCC_PLLCFGR` nach dem Reset (RM0368 6.3.2). */
#define PLLCFGR_RESET 0x24003010UL

/* So oft fragt ein Warten, bevor es aufgibt: bei 16 MHz einige
 * Millisekunden; ein Takt, der nicht kommt, soll nicht haengen lassen. */
#define WAIT_ROUNDS 100000U

/* Wartet beschraenkt, bis `(*reg & mask) == want`; 1, wenn es so kam. */
static int wait_for(volatile const uint32_t *reg, uint32_t mask, uint32_t want) {
    uint32_t rounds = 0U;
    while ((rounds < WAIT_ROUNDS) && ((*reg & mask) != want)) {
        rounds++;
    }
    return ((*reg & mask) == want) ? 1 : 0;
}

/* Takte wie nach dem Einschalten (RM0368, Resetwerte). Scheitert der Wechsel
 * auf HSI, bleibt die PLL der Systemtakt, und der Bootloader richtet seinen
 * Takt auf dem ein, was er vorfindet. */
static void reset_clocks(void) {
    RCC->CR |= RCC_CR_HSION;
    if (wait_for(&RCC->CR, RCC_CR_HSIRDY, RCC_CR_HSIRDY) != 0) {
        RCC->CFGR &= ~RCC_CFGR_SW;
        if (wait_for(&RCC->CFGR, RCC_CFGR_SWS, RCC_CFGR_SWS_HSI) != 0) {
            RCC->CR &= ~RCC_CR_PLLON;
            (void)wait_for(&RCC->CR, RCC_CR_PLLRDY, 0U);
        }
    }
    RCC->CFGR = 0U;
    RCC->CR &= ~(RCC_CR_HSEON | RCC_CR_CSSON | RCC_CR_PLLI2SON);
    RCC->CR &= ~RCC_CR_HSEBYP;
    RCC->PLLCFGR = PLLCFGR_RESET;
    RCC->CIR = RCC_CIR_LSIRDYC | RCC_CIR_LSERDYC | RCC_CIR_HSIRDYC | RCC_CIR_HSERDYC | RCC_CIR_PLLRDYC |
               RCC_CIR_PLLI2SRDYC | RCC_CIR_CSSC;
    /* Erst nach dem Wechsel auf 16 MHz: null Wartezyklen tragen bis 30 MHz. */
    FLASH->ACR = 0U;
}

/* Jede Peripherie ueber ihr Reset-Bit, dann alle Takte aus. */
static void reset_peripherals(void) {
    RCC->AHB1RSTR = RCC_AHB1RSTR_GPIOARST | RCC_AHB1RSTR_GPIOBRST | RCC_AHB1RSTR_GPIOCRST | RCC_AHB1RSTR_GPIODRST |
                    RCC_AHB1RSTR_GPIOERST | RCC_AHB1RSTR_GPIOHRST | RCC_AHB1RSTR_CRCRST | RCC_AHB1RSTR_DMA1RST |
                    RCC_AHB1RSTR_DMA2RST;
    RCC->AHB2RSTR = RCC_AHB2RSTR_OTGFSRST;
    RCC->APB1RSTR = RCC_APB1RSTR_TIM2RST | RCC_APB1RSTR_TIM3RST | RCC_APB1RSTR_TIM4RST | RCC_APB1RSTR_TIM5RST |
                    RCC_APB1RSTR_WWDGRST | RCC_APB1RSTR_SPI2RST | RCC_APB1RSTR_SPI3RST | RCC_APB1RSTR_USART2RST |
                    RCC_APB1RSTR_I2C1RST | RCC_APB1RSTR_I2C2RST | RCC_APB1RSTR_I2C3RST | RCC_APB1RSTR_PWRRST;
    RCC->APB2RSTR = RCC_APB2RSTR_TIM1RST | RCC_APB2RSTR_USART1RST | RCC_APB2RSTR_USART6RST | RCC_APB2RSTR_ADCRST |
                    RCC_APB2RSTR_SDIORST | RCC_APB2RSTR_SPI1RST | RCC_APB2RSTR_SPI4RST | RCC_APB2RSTR_SYSCFGRST |
                    RCC_APB2RSTR_TIM9RST | RCC_APB2RSTR_TIM10RST | RCC_APB2RSTR_TIM11RST;
    RCC->AHB1RSTR = 0U;
    RCC->AHB2RSTR = 0U;
    RCC->APB1RSTR = 0U;
    RCC->APB2RSTR = 0U;
    RCC->AHB1ENR = 0U;
    RCC->AHB2ENR = 0U;
    RCC->APB1ENR = 0U;
    RCC->APB2ENR = 0U;
}

FUNC_NORETURN void handback_give(void) {
    const volatile uint32_t *const table = (const volatile uint32_t *)SYSTEM_MEMORY;
    uint32_t word = 0U;
    __disable_irq();
    SysTick->CTRL = 0U;
    /* Die Regionen des Zephyr-Kerns gelten fuer den Bootloader nicht. */
    MPU->CTRL = 0U;
    __DSB();
    __ISB();
    reset_clocks();
    reset_peripherals();
    for (word = 0U; word < (sizeof NVIC->ICER / sizeof NVIC->ICER[0]); word++) {
        NVIC->ICER[word] = UINT32_MAX;
        NVIC->ICPR[word] = UINT32_MAX;
    }
    /* Der Kontextwechsel des Zephyr-Kerns steht womoeglich noch an. */
    SCB->ICSR = SCB_ICSR_PENDSVCLR_Msk | SCB_ICSR_PENDSTCLR_Msk;

    /* Den Systemspeicher nach Adresse 0 abbilden, wie ein Start mit BOOT0,
     * und seine Vektortabelle nehmen. */
    RCC->APB2ENR |= RCC_APB2ENR_SYSCFGEN;
    (void)RCC->APB2ENR;
    SYSCFG->MEMRMP = (SYSCFG->MEMRMP & ~SYSCFG_MEMRMP_MEM_MODE) | SYSCFG_MEMRMP_MEM_MODE_0;
    SCB->VTOR = SYSTEM_MEMORY;
    __set_BASEPRI(0U);
    __DSB();
    __ISB();
    /* Der Bootloader richtet seine Interrupts selbst ein und gibt sie nicht
     * selbst frei; jeder ist im NVIC gesperrt, und keiner steht an. */
    __enable_irq();
    /* CONTROL wie nach dem Reset (Hauptstack, keine FPU-Marke), dann sein
     * Stackzeiger und sein Resetvektor; in einem Block, weil danach kein
     * Wert dieses Fadens auf einem Stack mehr gilt. */
    __asm volatile("msr control, %2\n\t"
                   "isb\n\t"
                   "msr msp, %0\n\t"
                   "bx %1\n\t"
                   :
                   : "r"(table[0]), "r"(table[1]), "r"(0U)
                   : "memory");
    CODE_UNREACHABLE;
}
