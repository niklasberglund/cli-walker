#!/usr/bin/env bash
# Builds CLI-Walker and runs it in the current terminal.
# The dashboard is an interactive TUI (raw keyboard input, alternate screen), so the
# binary is exec'd directly rather than run as a child of a build tool.
set -euo pipefail
cd "$(dirname "$0")"

cargo build --quiet
exec ./target/debug/cli-walker "$@"
