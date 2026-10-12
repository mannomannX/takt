/* string.h - was der Kern von FreeRTOS und der Port mit Faeden aus der
 * C-Bibliothek brauchen, fuer einen Wirt ohne sie (M11 Schritt 20). Die
 * Funktionen stellt das SDK-Archiv (`compiler_builtins` des Rust-Kerns) mit
 * den Signaturen des Standards. */
#ifndef BOARD_STRING_H
#define BOARD_STRING_H

#include <stddef.h>

void *memcpy(void *to, const void *from, size_t bytes);
void *memset(void *to, int value, size_t bytes);

#endif /* BOARD_STRING_H */
