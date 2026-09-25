#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
nvim="$(command -v nvim)"
export BLINK_PLUGIN="${BLINK_PLUGIN:-${HOME}/.local/share/nvim/lazy/blink.cmp}"
test -d "$BLINK_PLUGIN" || { echo "Install Blink or set BLINK_PLUGIN to a checkout" >&2; exit 1; }
MBX_TARGET_VIEWS=0 mbx build --locked
export PKL_LSP_BINARY="$root/target/debug/pkl-lsp-rs"
export PKL_FIXTURE="$root/tests/fixtures/hk-public.pkl"

# Run with only absolute executable paths and without loading the user's Neovim config.
PATH=/nonexistent "$nvim" --headless -u NONE -i NONE -l "$root/tests/neovim_blink.lua"
