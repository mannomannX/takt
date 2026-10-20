/* load_wifi.c - WLAN neben Takt (load.h; `TAKT_LOAD=wifi`, M11 Schritt 21b).
 *
 * **Ohne Access Point:** Der C6 ist eine Station ohne Verbindung. Eine
 * Aufgabe sendet ESP-NOW als Broadcast, eine zweite scannt alle Kanaele
 * aktiv, mit Pausen dazwischen. So laufen die ISR und die Aufgabe des WLAN,
 * die Kalibrierung der PHY und die Kanalwechsel, ohne dass der Nutzer ein
 * Netz stellt; dazu schreibt NVS jede Sekunde (load_nvs.h).
 *
 * **Die Sendezeit ist begrenzt**, damit das WLAN in der Umgebung kaum etwas
 * merkt: Die Last fuer den Kern kommt aus ISR, Aufgaben und Kalibrierung,
 * nicht aus der Dauer in der Luft. Ein Rahmen traegt 250 Byte und geht mit
 * 54 Mbit/s (802.11g, OFDM) statt der Grundrate von 1 Mbit/s hinaus: mit
 * dem Kopf von ESP-NOW rund 293 Byte, 11 OFDM-Symbole von 4 us und 20 us
 * Praeambel, zusammen etwa 64 us in der Luft. Alle 5 ms ein Rahmen belegt
 * den Kanal zu etwa 1,3 %. Der aktive Scan sendet je Kanal eine Probe
 * (1 Mbit/s, etwa 0,75 ms), elf Kanaele alle 10 s, rund 0,1 %. Zusammen
 * unter 1,5 % des Kanals.
 */
#include <stdint.h>
#include <string.h>

#include "esp_event.h"
#include "esp_now.h"
#include "esp_wifi.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "load.h"
#include "load_nvs.h"
#include "program.h"

/* Die Nutzlast eines Rahmens in Byte. */
#define WIFI_FRAME_BYTES 250U

/* Ein Rahmen alle so viele Millisekunden. */
#define WIFI_PERIOD_MS 5U

/* Die Pause zwischen zwei Durchlaeufen des Scans in Millisekunden. */
#define WIFI_SCAN_PAUSE_MS 10000U

/* Der Stack einer Aufgabe der Last in Byte. */
#define WIFI_STACK_BYTES 3072U

/* Die Adresse fuer einen Broadcast. */
static const uint8_t wifi_broadcast[ESP_NOW_ETH_ALEN] = {0xFFU, 0xFFU, 0xFFU, 0xFFU, 0xFFU, 0xFFU};

/* Sendet alle `WIFI_PERIOD_MS` einen Rahmen; ein Rahmen, den ESP-NOW nicht
 * annimmt (etwa waehrend des Scans auf einem anderen Kanal), faellt weg. */
static void wifi_send(void *arg) {
    static uint8_t frame[WIFI_FRAME_BYTES];
    (void)arg;
    TickType_t wake = xTaskGetTickCount();
    uint32_t n = 0U;
    for (;;) {
        frame[n % WIFI_FRAME_BYTES] = (uint8_t)n;
        n++;
        (void)esp_now_send(wifi_broadcast, frame, sizeof frame);
        vTaskDelayUntil(&wake, pdMS_TO_TICKS(WIFI_PERIOD_MS));
    }
}

/* Scannt alle Kanaele aktiv, dann eine Pause. */
static void wifi_scan(void *arg) {
    (void)arg;
    for (;;) {
        (void)esp_wifi_scan_start(NULL, true);
        (void)esp_wifi_clear_ap_list();
        vTaskDelay(pdMS_TO_TICKS(WIFI_SCAN_PAUSE_MS));
    }
}

int load_start(void) {
    int rc = -1;
    const wifi_init_config_t config = WIFI_INIT_CONFIG_DEFAULT();
    esp_now_peer_info_t peer;
    (void)memset(&peer, 0, sizeof peer);
    (void)memcpy(peer.peer_addr, wifi_broadcast, sizeof peer.peer_addr);
    peer.ifidx = WIFI_IF_STA;
    esp_now_rate_config_t rate;
    (void)memset(&rate, 0, sizeof rate);
    rate.phymode = WIFI_PHY_MODE_11G;
    rate.rate = WIFI_PHY_RATE_54M;
    if ((load_nvs_start() == 0) && (esp_event_loop_create_default() == ESP_OK) && (esp_wifi_init(&config) == ESP_OK) &&
        (esp_wifi_set_storage(WIFI_STORAGE_RAM) == ESP_OK) && (esp_wifi_set_mode(WIFI_MODE_STA) == ESP_OK) &&
        (esp_wifi_start() == ESP_OK) && (esp_now_init() == ESP_OK) && (esp_now_add_peer(&peer) == ESP_OK) &&
        (esp_now_set_peer_rate_config(wifi_broadcast, &rate) == ESP_OK) &&
        (xTaskCreate(wifi_send, "takt-esp-now", WIFI_STACK_BYTES, NULL, LOAD_PRIORITY, NULL) == pdPASS) &&
        (xTaskCreate(wifi_scan, "takt-scan", WIFI_STACK_BYTES, NULL, LOAD_PRIORITY, NULL) == pdPASS)) {
        rc = 0;
    }
    return rc;
}

void load_banner(void) {
    takt_board_trace("takt last wlan\r\n");
    takt_board_trace("  Last: WLAN ohne Access Point, ESP-NOW-Broadcast 250 Byte alle 5 ms mit 54 Mbit/s "
                     "(etwa 64 us in der Luft, 1,3 % des Kanals), aktiver Scan aller Kanaele alle 10 s (etwa 0,1 %); "
                     "NVS schreibt jede Sekunde\r\n");
}
