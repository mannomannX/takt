/* Was das Bring-up des ESP32-C6 selbst in den RAM legt (12.3, `xip_flash`).
 *
 * Den Tick-Pfad des Programms nennt das Fragment, das `takt build --emit
 * embed` schreibt (`app_ram.x`): Bibliothek, Kleber, Huelle, Schleife,
 * Treiberrand, Mathematik und Natives. Das Bauskript haengt diese Zeilen
 * dahinter und legt beides als `rwtext_hook.x` ab, das `esp-hal` in
 * `.rwtext` einbindet. Hier steht nur, was das Board dazulegt:
 * `libtaktboard.a` mit Millicode und der C-Referenz von `takt bench`, und
 * die rechnende Haelfte der Board-Unterstuetzung, die ohne `esp-hal` kein
 * `#[ram]` kennt (FIFO und Erkenner der Leitung), und `core::fmt`, mit dem
 * der Trace eine Fliesskommazahl als kuerzeste Ziffernfolge schreibt (4.2:
 * keine eigene Dezimalkonversion). Was das Board-Crate und das Bring-up im
 * Tick rufen, traegt dort `#[esp_hal::ram]`. `takt check-image` prueft das
 * gebundene Abbild; die Ziele der indirekten Aufrufe in `core::fmt` nennt
 * es offen.
 */
*libtaktboard.a:(.text .text.* .rodata .rodata.* .srodata .srodata.*)
*(.text.*18takt_board_support[0-9A-Z_]* .rodata.*18takt_board_support[0-9A-Z_]* .srodata.*18takt_board_support[0-9A-Z_]*)
*(.text.*4core3fmt[0-9A-Z_]* .rodata.*4core3fmt[0-9A-Z_]* .srodata.*4core3fmt[0-9A-Z_]*)

/* Das Pruefgeraet des Treiberrands, ein Treiber dieses Pruefstands (12.6). */
*(.text.*17takt_driver_probe[0-9A-Z_]* .rodata.*17takt_driver_probe[0-9A-Z_]* .srodata.*17takt_driver_probe[0-9A-Z_]*)

/* Was die Geraete des Boards im Tick aus `esp-hal` rufen: die LED ueber den
 * RMT-Baustein, die Pins, Takt und Freigabe der Peripherie und die Sperre
 * darum.
 */
*(.text.*7esp_hal3rmt[0-9A-Z_]* .rodata.*7esp_hal3rmt[0-9A-Z_]* .srodata.*7esp_hal3rmt[0-9A-Z_]*)
*(.text.*7esp_hal4gpio[0-9A-Z_]* .rodata.*7esp_hal4gpio[0-9A-Z_]* .srodata.*7esp_hal4gpio[0-9A-Z_]*)
*(.text.*7esp_hal6system[0-9A-Z_]* .rodata.*7esp_hal6system[0-9A-Z_]* .srodata.*7esp_hal6system[0-9A-Z_]*)
*(.text.*7esp_hal3soc[0-9A-Z_]* .rodata.*7esp_hal3soc[0-9A-Z_]* .srodata.*7esp_hal3soc[0-9A-Z_]*)
*(.text.*8esp_sync[0-9A-Z_]* .rodata.*8esp_sync[0-9A-Z_]* .srodata.*8esp_sync[0-9A-Z_]*)
