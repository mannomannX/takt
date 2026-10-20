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
 *
 * **Die Last belegt sich selbst:** Sie zaehlt die Rahmen, die ESP-NOW als
 * gesendet meldet, die abgeschlossenen Scans und ihre Dauer.
 * `load_start` kehrt erst zurueck, wenn der erste Scan fertig und der erste
 * Rahmen gesendet ist; von da an zaehlt `load_report`.
 */
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

#include "esp_event.h"
#include "esp_now.h"
#include "esp_timer.h"
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

/* So lange wartet `load_start` auf den ersten Scan und den ersten Rahmen, in
 * Schritten von 10 ms. */
#define WIFI_READY_TRIES 500U

/* Die Adresse fuer einen Broadcast. */
static const uint8_t wifi_broadcast[ESP_NOW_ETH_ALEN] = {0xFFU, 0xFFU, 0xFFU, 0xFFU, 0xFFU, 0xFFU};

/* Atomar: die Rahmen, die ESP-NOW als gesendet meldet, die abgeschlossenen
 * Scans und ihre Dauer in Millisekunden. */
static uint32_t wifi_sent;
static uint32_t wifi_scans;
static uint32_t wifi_scan_ms;

/* Die Zaehler, als die Last stand, und wann das war (us); nur `app_main`
 * schreibt und liest sie. */
static uint32_t wifi_base_sent;
static uint32_t wifi_base_scans;
static uint32_t wifi_base_scan_ms;
static uint32_t wifi_base_nvs;
static int64_t wifi_since;

/* Aus der Aufgabe des WLAN: ein Rahmen ist hinaus. */
static void wifi_on_sent(const esp_now_send_info_t *info, esp_now_send_status_t status) {
    (void)info;
    if (status == ESP_NOW_SEND_SUCCESS) {
        (void)__atomic_add_fetch(&wifi_sent, 1U, __ATOMIC_RELAXED);
    }
}

/* Aus der Ereignisschleife: ein Scan ist fertig, 0 heisst erfolgreich. */
static void wifi_on_scan(void *arg, esp_event_base_t base, int32_t id, void *data) {
    wifi_event_sta_scan_done_t done;
    (void)arg;
    (void)base;
    (void)id;
    (void)memcpy(&done, data, sizeof done);
    if (done.status == 0U) {
        (void)__atomic_add_fetch(&wifi_scans, 1U, __ATOMIC_RELAXED);
    }
}

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

/* Scannt alle Kanaele aktiv und zaehlt die Dauer, dann eine Pause. */
static void wifi_scan(void *arg) {
    (void)arg;
    for (;;) {
        const int64_t begin = esp_timer_get_time();
        (void)esp_wifi_scan_start(NULL, true);
        const int64_t took_ms = (esp_timer_get_time() - begin) / 1000;
        (void)__atomic_add_fetch(&wifi_scan_ms, (uint32_t)took_ms, __ATOMIC_RELAXED);
        (void)esp_wifi_clear_ap_list();
        vTaskDelay(pdMS_TO_TICKS(WIFI_SCAN_PAUSE_MS));
    }
}

/* Der erste Scan ist fertig, und der erste Rahmen ist hinaus. */
static bool wifi_ready(void) {
    return (__atomic_load_n(&wifi_scans, __ATOMIC_RELAXED) > 0U) && (__atomic_load_n(&wifi_sent, __ATOMIC_RELAXED) > 0U);
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
    if ((load_nvs_start() == 0) && (esp_event_loop_create_default() == ESP_OK) &&
        (esp_event_handler_register(WIFI_EVENT, (int32_t)WIFI_EVENT_SCAN_DONE, wifi_on_scan, NULL) == ESP_OK) &&
        (esp_wifi_init(&config) == ESP_OK) && (esp_wifi_set_storage(WIFI_STORAGE_RAM) == ESP_OK) &&
        (esp_wifi_set_mode(WIFI_MODE_STA) == ESP_OK) && (esp_wifi_start() == ESP_OK) && (esp_now_init() == ESP_OK) &&
        (esp_now_register_send_cb(wifi_on_sent) == ESP_OK) && (esp_now_add_peer(&peer) == ESP_OK) &&
        (esp_now_set_peer_rate_config(wifi_broadcast, &rate) == ESP_OK) &&
        (xTaskCreate(wifi_send, "takt-esp-now", WIFI_STACK_BYTES, NULL, LOAD_PRIORITY, NULL) == pdPASS) &&
        (xTaskCreate(wifi_scan, "takt-scan", WIFI_STACK_BYTES, NULL, LOAD_PRIORITY, NULL) == pdPASS)) {
        uint32_t tries = 0U;
        while ((tries < WIFI_READY_TRIES) && !wifi_ready()) {
            vTaskDelay(pdMS_TO_TICKS(10U));
            tries++;
        }
        if (wifi_ready()) {
            wifi_base_sent = __atomic_load_n(&wifi_sent, __ATOMIC_RELAXED);
            wifi_base_scans = __atomic_load_n(&wifi_scans, __ATOMIC_RELAXED);
            wifi_base_scan_ms = __atomic_load_n(&wifi_scan_ms, __ATOMIC_RELAXED);
            wifi_base_nvs = load_nvs_commits();
            wifi_since = esp_timer_get_time();
            rc = 0;
        }
    }
    return rc;
}

void load_banner(void) {
    takt_board_trace("takt last wlan\r\n");
    takt_board_trace("  Last: WLAN ohne Access Point, ESP-NOW-Broadcast 250 Byte alle 5 ms mit 54 Mbit/s "
                     "(etwa 64 us in der Luft, 1,3 % des Kanals), aktiver Scan aller Kanaele alle 10 s (etwa 0,1 %); "
                     "NVS schreibt jede Sekunde\r\n");
}

void load_report(void) {
    const int64_t ms = (esp_timer_get_time() - wifi_since) / 1000;
    const uint32_t sent = __atomic_load_n(&wifi_sent, __ATOMIC_RELAXED) - wifi_base_sent;
    const uint32_t scans = __atomic_load_n(&wifi_scans, __ATOMIC_RELAXED) - wifi_base_scans;
    const uint32_t scan_ms = __atomic_load_n(&wifi_scan_ms, __ATOMIC_RELAXED) - wifi_base_scan_ms;
    const uint32_t commits = load_nvs_commits() - wifi_base_nvs;
    takt_board_trace("takt last dauer ");
    takt_board_trace_u64((uint64_t)ms);
    takt_board_trace("espnow ");
    takt_board_trace_u64(sent);
    takt_board_trace("scans ");
    takt_board_trace_u64(scans);
    takt_board_trace("scanzeit ");
    takt_board_trace_u64(scan_ms);
    takt_board_trace("nvs ");
    takt_board_trace_u64(commits);
    takt_board_trace("\r\n");
}
