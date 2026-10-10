#!/usr/bin/env bash
# Baut und prueft, was auf die MCU geht (12.3, 12.8).
#
# **Warum ein eigenes Skript.** Board- und Bring-up-Crate liegen ausserhalb
# des Workspace — Registerzugriff braucht `unsafe`, und `forbid` (13.4)
# laesst sich nicht lokal aufheben. Ein Crate ausserhalb des Workspace wird
# von `cargo test --workspace` nicht erfasst und darum leicht vergessen;
# genau das verhindert dieses Skript.
#
# Vier Pruefungen, jede mit eigenem Zweck:
#
#   1. Die `no_std`-Kerne bauen fuer *beide* M5-Ziele — ohne Board-Crate.
#      Geht das nicht mehr, ist Board-Wissen in eine Schicht gesickert,
#      die keines haben darf (plan/m5.md 2.2).
#   2. Das Board-Crate baut fuer sein Ziel und ist clippy-sauber.
#   3. Das Bring-up-Programm baut — **von aussen**, mit `--manifest-path`.
#      Das ist Absicht: So faellt auf, wenn die Linker-Argumente wieder in
#      `.cargo/config.toml` wandern, die nur im Crate-Verzeichnis gilt.
#      Dann entstuende eine Binaerdatei ohne Vektortabelle: Sie baut, sie
#      linkt, und sie tut am Board nichts.
#   4. Die rechnende Haelfte laeuft auf dem Wirt mit ihren Tests. Sie ist
#      der Grund, warum die Ausnahme klein ist.
#
# Keine Pruefung faellt still weg: Fehlt ein Werkzeug oder eine
# Binaerdatei, endet das Skript mit Fehler, statt am Schluss „alles
# geprueft“ zu melden (RT-037).
set -euo pipefail

cd "$(dirname "$0")/.."

targets=(thumbv7em-none-eabihf riscv32imac-unknown-none-elf)
cores=(takt-rt-core takt-rt-baremetal)

for t in "${targets[@]}"; do
    if ! rustup target list --installed | grep -qx "$t"; then
        echo "Ziel $t fehlt — mit 'rustup target add $t' nachruesten" >&2
        exit 1
    fi
done

echo "== 1. Die Kerne, fuer beide Ziele, ohne Board"
for t in "${targets[@]}"; do
    for c in "${cores[@]}"; do
        echo "-- $c fuer $t"
        cargo build -p "$c" --target "$t" "$@"
    done
done

echo
echo "== 2. Das Board-Crate (eigener Workspace, thumbv7em)"
#
# Ohne `--all-targets`: Tests fuer ein `no_std`-Ziel brauchen einen
# Testlaeufer und einen `panic_handler`, die es dort nicht gibt. Was
# testbar ist, liegt ohnehin in `takt-board-support` — und genau das ist
# der Grund fuer die Trennung.
(
    cd crates/takt-board-stm32f401
    cargo build --target thumbv7em-none-eabihf "$@"
    cargo clippy --target thumbv7em-none-eabihf "$@" -- -D warnings
)

echo
echo "== 3. Die Bring-up-Programme, von aussen gebaut"
#
# Drei Binaries, drei Stufen der Fehlersuche: `blink` schaltet einen Pin
# ohne PLL, `minimal` prueft die Tickquelle, `takt` fuehrt ein echtes
# Takt-Programm aus. Jedes laesst weg, was das naechste braucht — so
# halbiert ein Fehlerbild den Suchraum, statt ihn zu durchmustern. Mit
# `bench` baut auch das Messprogramm von `takt bench` (13.8).
cargo build --release --target thumbv7em-none-eabihf --features bench \
    --manifest-path crates/takt-bringup-stm32f401/Cargo.toml "$@"

# Das Zielverzeichnis kann umgelenkt sein (`CARGO_TARGET_DIR`, oder in
# `.cargo/config.toml` des Nutzers); `cargo metadata` weiss, wohin.
target_dir="$(cargo metadata --format-version 1 --no-deps 2>/dev/null |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"

# Ein Werkzeug aus `llvm-tools`; fehlt es, ist das ein Fehler, keine
# uebersprungene Pruefung.
llvm_tool() {
    local tool="$sysroot/lib/rustlib/$host/bin/$1"
    if [ -x "$tool" ] || [ -x "$tool.exe" ]; then
        echo "$tool"
    else
        echo "FEHLER: $1 fehlt — mit 'rustup component add llvm-tools' nachruesten" >&2
        exit 1
    fi
}
objcopy="$(llvm_tool llvm-objcopy)"
nm="$(llvm_tool llvm-nm)"

# Das Binary mit dem Takt-Programm eines Bring-ups; die anderen tragen
# keines und saegen die Frage nicht, um die es hier geht.
binary_of() {
    local bin="${target_dir:-target}/$1/release/takt"
    if [ ! -f "$bin" ]; then
        echo "FEHLER: $bin fehlt nach dem Bau" >&2
        exit 1
    fi
    echo "$bin"
}

# **Traegt das Binary wirklich das Programm aus `takt.toml`?**
#
# Ein Bau ohne gesetztes `TAKT_PROGRAM` fiel frueher still auf einen
# Default zurueck, und das Ergebnis lief korrekt und blieb dabei dunkel —
# das Programm hing an einem Kommando, das auf dem Board niemand sendet
# (FB-141). Von aussen sah es aus wie ein Defekt. Die Maschine steht als
# Symbol im Binary, also ist die Frage in einer Zeile zu beantworten — fuer
# jedes Bring-up.
program_in_binary() {
    local bringup="$1" bin="$2" konfiguriert maschine
    konfiguriert="$(grep -o 'program *= *"[^"]*"' "$bringup/takt.toml" | cut -d'"' -f2)"
    if [ -z "$konfiguriert" ]; then
        echo "FEHLER: $bringup/takt.toml nennt kein Programm" >&2
        exit 1
    fi
    maschine="$(grep -o '^machine [A-Za-z_][A-Za-z0-9_]*' "$bringup/$konfiguriert" | head -1 | cut -d' ' -f2)"
    # `grep` liest bis zum Ende: `grep -q` hoerte beim ersten Treffer auf,
    # `nm` schriebe in die geschlossene Leitung, und unter `pipefail` galte
    # die Pruefung als gescheitert (FB-443).
    if [ -n "$maschine" ] && "$nm" "$bin" 2>/dev/null | grep "${maschine}_step" >/dev/null; then
        echo "  Programm im Binary: $maschine (aus $konfiguriert)"
    else
        echo "  FEHLER: ${maschine}_step fehlt in $bin — gebaut wurde ein anderes Programm." >&2
        exit 1
    fi
}

# **Haelt das Abbild, was die Lieferform verspricht?** (12.11, M11 Schritt
# 11): `takt check-image` gegen das Manifest, das der Bau des Bring-ups
# schrieb — ABI und Logik-Hash, die Arena, nichts Beschreibbares daneben,
# unter `xip_flash` der Tick-Pfad im RAM. Das Werkzeug ist das, mit dem das
# Bring-up gebaut hat (`release`, FB-193).
image_checked() {
    local triple="$1" crate="$2" bin="$3" library="${4:-app}" manifest
    manifest="$(ls -t "${target_dir:-target}/$triple/release/build/$crate"-*/out/takt/$library/$library.manifest 2>/dev/null | head -1)"
    if [ -z "$manifest" ]; then
        echo "FEHLER: kein $library.manifest im Bau von $crate" >&2
        exit 1
    fi
    "${target_dir:-target}/release/takt" check-image "$bin" --manifest "$manifest"
}

# Die Probe: Liegt die Vektortabelle dort, wo der Bootloader sie erwartet?
# `takt-flash-weact --dry-run` prueft Stackzeiger, Resetvektor und Groesse,
# ohne ein Board anzufassen.
bin="$(binary_of thumbv7em-none-eabihf)"
tmp="$(mktemp -t takt-bringup-XXXXXX)"
"$objcopy" -O binary "$bin" "$tmp"
cargo run -q -p takt-flash-weact --bin takt-flash-weact -- "$tmp" --dry-run
rm -f "$tmp"
program_in_binary crates/takt-bringup-stm32f401 "$bin"
image_checked thumbv7em-none-eabihf takt-bringup-stm32f401 "$bin"

echo
echo "== 4. Board 2: ESP32-C6 (eigener Workspace, riscv32imac; plan/esp32c6.md)"
(
    cd crates/takt-board-esp32c6
    cargo build --target riscv32imac-unknown-none-elf "$@"
    cargo clippy --target riscv32imac-unknown-none-elf "$@" -- -D warnings
)
# Die Einstiege der Natives fuer beide Ziele und als Bibliothek des Wirts,
# mit jedem Job; ihre Puffervertraege mit Tests auf dem Wirt (FB-397) —
# ohne `host`, dessen Panic-Handler der statischen Bibliothek gehoert.
(
    cd crates/takt-native-abi
    for t in "${targets[@]}"; do
        cargo clippy --target "$t" "$@" -- -D warnings
    done
    cargo clippy --features host,ecdsa,rsa,aes-gcm "$@" -- -D warnings
    cargo clippy --all-targets --features ecdsa,rsa,aes-gcm "$@" -- -D warnings
    cargo test --features ecdsa,rsa,aes-gcm "$@"
)
# Von aussen gilt `.cargo/config.toml` des Bring-ups nicht; die Schalter fuer
# `esp-hal` setzt darum der Aufruf, wie der Board-Harness (12.3, FB-449).
ESP_HAL_CONFIG_USE_RWTEXT_LD_HOOK=true ESP_HAL_CONFIG_PLACE_SWITCH_TABLES_IN_RAM=false \
    cargo build --release --target riscv32imac-unknown-none-elf --features bench \
    --manifest-path crates/takt-bringup-esp32c6/Cargo.toml "$@"
program_in_binary crates/takt-bringup-esp32c6 "$(binary_of riscv32imac-unknown-none-elf)"
image_checked riscv32imac-unknown-none-elf takt-bringup-esp32c6 "$(binary_of riscv32imac-unknown-none-elf)"
# Das Messprogramm misst aus dem RAM wie die Programme, deren Kosten es misst.
image_checked riscv32imac-unknown-none-elf takt-bringup-esp32c6 \
    "${target_dir:-target}/riscv32imac-unknown-none-elf/release/bench" takt_bench
# Die Schleife des Wirts (13.8, `takt driver-test --crate`): eigener
# Workspace wie die Bring-ups; das Programm bringt, wer sie bindet.
cargo clippy --all-targets --manifest-path crates/takt-bringup-host/Cargo.toml "$@" -- -D warnings
# Takt als Baustein in Rust (12.11): die Traits fuer beide Ziele, Bauhelfer
# und Testhilfe auf dem Wirt mit ihren Tests.
(
    cd crates/takt-embed
    for t in "${targets[@]}"; do
        cargo clippy --target "$t" "$@" -- -D warnings
    done
    cargo clippy --all-targets --features build,testing "$@" -- -D warnings
    cargo test --features build,testing "$@"
)

echo
echo "== 5. Die rechnende Haelfte auf dem Wirt"
cargo test -p takt-board-support -p takt-flash-weact "$@"
# Takt als Baustein in einem Projekt wie dem eines Nutzers (12.11): Programme,
# Messprogramm und erzeugte Module ohne Befund von Clippy, die Tests gegen den
# Interpreter und `takt bench --import` (FB-452).
TAKT="${target_dir:-target}/release/takt" \
    cargo clippy --all-targets --manifest-path examples/rust-host/Cargo.toml "$@" -- -D warnings
TAKT="${target_dir:-target}/release/takt" cargo test --manifest-path examples/rust-host/Cargo.toml "$@"

echo
echo "Gebaut und geprueft: Kerne (${cores[*]}) fuer ${targets[*]}, beide Board-Crates, beide Bring-ups mit"
echo "Abbild-, Programm- und Bindungspruefung (check-image), Natives mit Tests, takt-embed, takt-bringup-host,"
echo "die rechnende Haelfte, examples/rust-host."
