# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CLI-Walker is a macOS command-line tool for logging walking sessions with a Kingsmith WalkingPad A1 Pro treadmill. It connects via Bluetooth Low Energy (BLE), reads real-time speed/distance/steps data, and displays a live TUI dashboard.

## Build Commands

```bash
./gradlew compileKotlinMacosArm64    # Compile only (fast feedback)
./gradlew macosArm64Binaries          # Build native executable
./gradlew macosArm64Test              # Run tests
./gradlew clean                       # Clean build artifacts
```

The built binary is at `build/bin/macosArm64/debugExecutable/cli-walker.kexe`.

For x86 Macs, replace `macosArm64` with `macosX64` in all commands.

## Running

```bash
./run.sh                  # Build and run the walking dashboard
./run.sh strava login     # Authorize against Strava
./run.sh strava sync      # Upload past sessions
./run.sh --demo           # Simulated pad + sample history in a temp HOME (hidden flag)
```

`docs/demo.gif` is recorded from demo mode with VHS: `vhs demo/demo.tape` after building.

The app is an interactive TUI (raw keyboard input, live dashboard), so it must be run as a direct child of the terminal — `./run.sh` builds via Gradle and then `exec`s the binary. A Gradle `Exec` task pipes stdio and breaks the TUI, so there is deliberately no Gradle `run` task.

## Tech Stack

- **Language**: Kotlin/Native (macOS target, not JVM)
- **BLE**: [Kable](https://github.com/JuulLabs/kable) — Kotlin coroutines-powered BLE
- **TUI**: [Mordant](https://github.com/ajalt/mordant) — terminal styling, tables, animations
- **CLI**: [Clikt](https://github.com/ajalt/clikt) — command-line argument parsing
- **Build**: Gradle with Kotlin Multiplatform plugin

Since this is Kotlin/Native, not JVM: `String.format()` is unavailable, JVM-specific APIs don't exist, and some libraries behave differently than on JVM.

## Architecture

The app has these layers:

1. **BLE Protocol** (`ble/WalkingPadProtocol.kt`) — Stateless encoder/decoder for the WalkingPad BLE protocol. Builds command packets (query status, start, stop, set speed) and parses status responses. All packets follow the format `[0xF7, 0xA2, cmd, param, 0xFF, checksum, 0xFD]` with checksum = `sum(bytes[1..n-2]) % 256`.

2. **BLE Connection** (`ble/WalkingPadConnection.kt`) — Manages the Kable peripheral lifecycle: scanning for a device named "WalkingPad", connecting, subscribing to notifications on `fe01`, and polling status via writes to `fe02`. Exposes `StateFlow<WalkingSession>` and `StateFlow<ConnectionState>`.

3. **Data Model** (`model/WalkingSession.kt`) — Immutable data class holding session state (speed, distance, steps, time, belt state/mode). Includes formatting helpers and a rough calorie estimate.

4. **Session Tracking** (`model/SessionTracker.kt`) — Turns the pad's status samples into
   discrete `SessionRecord`s. The pad zeroes its counters whenever the belt stops, so each
   belt run is seen as a fresh climb from zero; the tracker banks the highest-elapsed sample
   of each run and sums them. Runs separated by less than the coalesce window (1 minute) fold
   into a single record, so stepping off the belt — or letting it auto-stop — does not split
   a walk into fragments that each fall below the Strava upload threshold. A longer gap closes
   the record and starts a new one, so one app run can produce several sessions.

5. **TUI Dashboard** (`tui/Dashboard.kt`) — Mordant `textAnimation` that renders a table with live metrics. Collects from the connection's StateFlows and re-renders on each update. Includes a color-coded speed bar. Three tabs (Session, Statistics, Strava); the Strava tab shows auth status, feed muting, and the upload threshold, and edits the latter two in place via `StravaUiState`. Every tab is padded to the same width so switching never resizes the box.

6. **Strava Upload** (`strava/`) — OAuth 2.0 authorization-code flow plus activity
   creation. `LoopbackServer.kt` is a one-shot HTTP listener on 127.0.0.1 that catches the
   OAuth redirect (Strava has no device-code flow); `StravaApi.kt` wraps the token and
   activity endpoints over Ktor's Darwin engine; `StravaService.kt` ties them together;
   `StravaConfig.kt` and `UploadLog.kt` persist credentials and uploaded-session ids under
   `~/.cli-walker/`. Commands live in `StravaCommands.kt`.

7. **Entry Point** (`Main.kt`) — Clikt command that wires everything together. Launches the dashboard coroutine, connects BLE, feeds status samples to the `SessionTracker`, and persists each session as it closes. On exit it prints a summary of every session recorded during the run and uploads them.

## WalkingPad BLE Protocol Reference

- Service UUID: `0000fe00-0000-1000-8000-00805f9b34fb`
- Notify characteristic (read status): `0000fe01-0000-1000-8000-00805f9b34fb`
- Write characteristic (send commands): `0000fe02-0000-1000-8000-00805f9b34fb`
- Status response: header `[0xF8, 0xA2]`, then belt state, speed (÷10), mode, 3-byte time, 3-byte distance (÷100 for km), 3-byte steps
- Protocol docs: [ph4-walkingpad](https://github.com/ph4r05/ph4-walkingpad), [QWalkingPad](https://github.com/DorianRudolph/QWalkingPad)

## Strava Integration

Each user registers their own API application at https://www.strava.com/settings/api with
the *Authorization Callback Domain* set to `localhost`. `strava login` stores the client id
and secret in `~/.cli-walker/strava.json` (mode 0600) along with the tokens.

- Scope requested: `activity:write,activity:read_all`. The write scope alone is not enough:
  `PUT /activities/{id}` answers **404 Record Not Found** when the token cannot read the
  activity, and `activity:read` cannot see private ("Only You") activities.
- Access tokens last ~6 hours; `StravaApi.validAccessToken` refreshes on demand. Strava
  rotates refresh tokens, so whatever the refresh returns is persisted, not just the access
  token.
- Sessions are uploaded as TCX files (`POST /uploads`), not created as manual activities.
  The upload is what gives the activity an `upload_id` and app attribution. It is
  asynchronous: poll `GET /uploads/{id}` until it yields an `activity_id`.
- TCX has no Walk sport (only Running/Biking/Other), so every upload needs a follow-up
  `PUT /activities/{id}` setting `sport_type=Walk`. Muting folds into that same call.
- The pad reports only totals, so `TcxWriter.kt` synthesises trackpoints every 30s with
  linearly interpolated distance. Strava turns these into distance/velocity streams.
- Strava does not take `device_name` from a TCX `<Creator>` block — verified with a
  schema-valid file, so this is not a malformed-XML problem. Neither the upload nor the
  update endpoint accepts a device field, so `device_name` cannot be set at all from TCX;
  only a FIT upload would carry it. The pad name lives in the description instead.
- The generated TCX validates against `TrainingCenterDatabasev2.xsd`. Keep it that way:
  `Device_t` requires `Version` and `Application_t` requires `Build`, and other importers
  (HealthFit and friends) are stricter than Strava.
- **Anything that fails after Strava has accepted an activity is a warning, not an error.**
  The activity exists; reporting failure would re-upload it as a duplicate. This applies to
  the empty-body create bug, to processing timeouts, and to the `sport_type` update.
- `~/.cli-walker/strava-uploads.json` maps session start times to Strava activity ids so
  sessions are never uploaded twice, storing `0` for "uploaded, id unknown". It is kept
  separate from the append-only `sessions.jsonl` so uploading never rewrites the session log.
- Finished sessions upload automatically when connected — one upload per session closed by
  the `SessionTracker`, so several start/stop cycles in one app run can yield several
  activities. `--no-strava` skips it, and
  `strava sync --dry-run` writes the TCX files to `~/.cli-walker/tcx/` without uploading.
- Activity visibility follows the athlete's account default (Privacy Controls); the API
  cannot set it. Verified: an API upload lands as `visibility: only_me` when that is the
  account default.
- A minimum upload distance (`minUploadDistanceKm` in `strava.json`) filters out short
  sessions. It is settable both from the CLI (`strava threshold 500m|1.5km|off`) and from the
  dashboard's Strava tab (up/down = ±100 m, Ctrl+up/down = ±10 m). `StravaService.pending`
  and `upload` both honour it, so the pending count matches what a sync would send;
  `strava sync --all` ignores it. A session skipped this way is reported on exit.
- `strava mute on` sets `hide_from_home` on each upload via a follow-up `PUT`, since the
  create endpoint accepts no such field. This only keeps activities out of followers'
  feeds — the API exposes no activity visibility field at all, so true "Only You" privacy
  is an account setting at https://www.strava.com/settings/privacy.
