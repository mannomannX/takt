/* load_quiet.c - der Wirt ohne fremde Last neben der Rechenlast von
 * `app_main` (load.h; `TAKT_LOAD=quiet`). */
#include "load.h"

int load_start(void) { return 0; }

void load_banner(void) {}

void load_report(void) {}
