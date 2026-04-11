# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CLI-Walker is a macOS command-line tool for logging walking sessions with a Kingsmith WalkingPad A1 Pro treadmill. It connects via Bluetooth Low Energy (BLE), reads real-time speed/distance/steps data, and displays a live TUI dashboard.

## Build Commands

```bash
./gradlew compileKotlinMacosArm64    # Compile only (fast feedback)
./gradlew macosArm64Binaries          # Build native executable
./gradlew clean                       # Clean build artifacts
```

The built binary is at `build/bin/macosArm64/debugExecutable/cli-walker.kexe`.

For x86 Macs, replace `macosArm64` with `macosX64` in all commands.

## Tech Stack

- **Language**: Kotlin/Native (macOS target, not JVM)
- **BLE**: [Kable](https://github.com/JuulLabs/kable) — Kotlin coroutines-powered BLE
- **TUI**: [Mordant](https://github.com/ajalt/mordant) — terminal styling, tables, animations
- **CLI**: [Clikt](https://github.com/ajalt/clikt) — command-line argument parsing
- **Build**: Gradle with Kotlin Multiplatform plugin

Since this is Kotlin/Native, not JVM: `String.format()` is unavailable, JVM-specific APIs don't exist, and some libraries behave differently than on JVM.

## Architecture

The app has four layers:

1. **BLE Protocol** (`ble/WalkingPadProtocol.kt`) — Stateless encoder/decoder for the WalkingPad BLE protocol. Builds command packets (query status, start, stop, set speed) and parses status responses. All packets follow the format `[0xF7, 0xA2, cmd, param, 0xFF, checksum, 0xFD]` with checksum = `sum(bytes[1..n-2]) % 256`.

2. **BLE Connection** (`ble/WalkingPadConnection.kt`) — Manages the Kable peripheral lifecycle: scanning for a device named "WalkingPad", connecting, subscribing to notifications on `fe01`, and polling status via writes to `fe02`. Exposes `StateFlow<WalkingSession>` and `StateFlow<ConnectionState>`.

3. **Data Model** (`model/WalkingSession.kt`) — Immutable data class holding session state (speed, distance, steps, time, belt state/mode). Includes formatting helpers and a rough calorie estimate.

4. **TUI Dashboard** (`tui/Dashboard.kt`) — Mordant `textAnimation` that renders a table with live metrics. Collects from the connection's StateFlows and re-renders on each update. Includes a color-coded speed bar.

5. **Entry Point** (`Main.kt`) — Clikt command that wires everything together. Launches the dashboard coroutine, connects BLE, and prints a session summary on exit.

## WalkingPad BLE Protocol Reference

- Service UUID: `0000fe00-0000-1000-8000-00805f9b34fb`
- Notify characteristic (read status): `0000fe01-0000-1000-8000-00805f9b34fb`
- Write characteristic (send commands): `0000fe02-0000-1000-8000-00805f9b34fb`
- Status response: header `[0xF8, 0xA2]`, then belt state, speed (÷10), mode, 3-byte time, 3-byte distance (÷100 for km), 3-byte steps
- Protocol docs: [ph4-walkingpad](https://github.com/ph4r05/ph4-walkingpad), [QWalkingPad](https://github.com/DorianRudolph/QWalkingPad)
