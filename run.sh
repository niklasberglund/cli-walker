#!/usr/bin/env bash
# Builds CLI-Walker and runs it in the current terminal.
# A Gradle Exec task cannot hand over a real TTY, so interactive
# TUI features (raw keyboard input, live dashboard) only work when
# the binary is exec'd directly like this.
set -euo pipefail
cd "$(dirname "$0")"

arch="$(uname -m)"
if [ "$arch" = "arm64" ]; then
    target="macosArm64"
else
    target="macosX64"
fi

./gradlew "${target}Binaries"
exec "./build/bin/${target}/debugExecutable/cli-walker.kexe" "$@"
