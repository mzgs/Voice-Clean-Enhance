#!/bin/sh
set -eu

PROJECT_ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$PROJECT_ROOT"

cargo build --release --locked --bin clean-voice --target-dir "$PROJECT_ROOT/target"
cp "$PROJECT_ROOT/target/release/clean-voice" "$PROJECT_ROOT/clean-voice"
chmod +x "$PROJECT_ROOT/clean-voice"
printf 'Built %s/clean-voice\n' "$PROJECT_ROOT"
