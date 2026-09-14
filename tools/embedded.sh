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
echo "== 3. Das Bring-up-Programm, von aussen gebaut"
cargo build --release --target thumbv7em-none-eabihf \
    --manifest-path crates/takt-bringup-stm32f401/Cargo.toml "$@"

# Die Probe: Liegt die Vektortabelle dort, wo der Bootloader sie erwartet?
# `takt-flash-weact --dry-run` prueft Stackzeiger und Resetvektor, ohne
# ein Board anzufassen.
# Das Zielverzeichnis kann umgelenkt sein (`CARGO_TARGET_DIR`, oder in
# `.cargo/config.toml` des Nutzers); `cargo metadata` weiss, wohin.
target_dir="$(cargo metadata --format-version 1 --no-deps 2>/dev/null |
    sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
bin="${target_dir:-target}/thumbv7em-none-eabihf/release/takt-bringup-stm32f401"
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
objcopy="$sysroot/lib/rustlib/$host/bin/llvm-objcopy"
if [ -f "$bin" ] && { [ -x "$objcopy" ] || [ -x "$objcopy.exe" ]; }; then
    tmp="$(mktemp -t takt-bringup-XXXXXX)"
    "$objcopy" -O binary "$bin" "$tmp"
    cargo run -q -p takt-flash-weact -- "$tmp" --dry-run
    rm -f "$tmp"
else
    echo "  (Abbildpruefung uebersprungen: Binaerdatei oder llvm-objcopy fehlt)"
fi

echo
echo "== 4. Die rechnende Haelfte auf dem Wirt"
cargo test -p takt-board-support -p takt-flash-weact "$@"

echo
echo "Alles gebaut und geprueft."
