/* handback_reboot.c - ein Board, das der Host ueber seine Debug-Schnittstelle
 * schreibt (ESP32-C6: JTAG des USB-Serial-JTAG), gibt er sich mit einem
 * Neustart zurueck. */
#include "handback.h"

#include <zephyr/sys/reboot.h>

FUNC_NORETURN void handback_give(void) {
    sys_reboot(SYS_REBOOT_COLD);
}
