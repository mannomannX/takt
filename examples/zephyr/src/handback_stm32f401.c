/* handback_stm32f401.c - Board 1 an den DFU-Bootloader des ROM (AN2606,
 * FB-275, plan/f401.md 2) auf dem gemeinsamen Weg aller Anwendungen
 * (`takt_board_stm32f401::bootloader::request`, `examples/boards/stm32f401`):
 * den Wunsch ins Backup-Register 4, dann ein Neustart.
 *
 * Nach dem Reset laeuft der HID-Bootloader von WeAct, dann das Sprungbrett
 * in Sektor 1 (`takt-trampoline-stm32f401`); das findet den Wunsch und
 * springt in den Systemspeicher. Der Reset raeumt dabei auf, was die
 * Anwendung eingerichtet hat, auch die Regionen der MPU, die Zephyr setzte:
 * Ein Sprung ohne Reset liess sie bis ins naechste Abbild stehen (FB-752). */
#include "handback.h"

#include <soc.h>
#include <zephyr/sys/reboot.h>

/* Der Wunsch nach dem Bootloader, `DFU!` als Wort wie
 * `takt_board_stm32f401::bootloader::REQUESTED`. */
#define HANDBACK_DFU_WISH 0x21554644U

FUNC_NORETURN void handback_give(void) {
    /* Das Backup-Register liegt hinter dem Schreibschutz der Backup-Domaene
     * (RM0368 5.1.2): der Takt von PWR, dann `DBP`. */
    RCC->APB1ENR |= RCC_APB1ENR_PWREN;
    (void)RCC->APB1ENR;
    PWR->CR |= PWR_CR_DBP;
    RTC->BKP4R = HANDBACK_DFU_WISH;
    PWR->CR &= ~PWR_CR_DBP;
    sys_reboot(SYS_REBOOT_COLD);
}
