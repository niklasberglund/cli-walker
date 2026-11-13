# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

CLI-Walker is a command-line tool for logging walking sessions with a Kingsmith WalkingPad A1 Pro treadmill. It connects via Bluetooth Low Energy (BLE), reads real-time speed/distance/steps data, displays a live TUI dashboard, and uploads sessions to Strava. It is written in Rust and runs on macOS and Linux; Windows builds and passes CI but is untested with the pad.

It was ported from Kotlin/Native (macOS only) in the Linear project "CLI-Walker Rust migration". Files the Kotlin version wrote under `~/.cli-walker/` are read as-is: field names and timestamp formats match.

## Build Commands

```bash
cargo build
cargo test                                  # unit, snapshot and pseudo-terminal tests
cargo clippy --all-targets -- -D warnings
cargo fmt
cargo llvm-cov --summary-only               # coverage (needs cargo-llvm-cov)
```

On Linux, btleplug needs `libdbus-1-dev` and `pkg-config` to build. CI
(`.github/workflows/rust.yml`) runs fmt, clippy and tests on Linux, macOS and Windows, plus
coverage on Linux. The Linux test job also validates generated TCX against Garmin's schema
(set `TCX_XSD` to the `.xsd` path, with `xmllint` installed, to do the same locally).

## Tests

- **Snapshot tests** render each dashboard tab into ratatui's `TestBackend` and compare with
  insta snapshots in `src/tui/snapshots/`. They record text only; colours have their own
  assertions. Accept intended changes with `cargo insta review` (or
  `INSTA_UPDATE=always cargo test`), then read the diff.
- **`tests/tui_pty.rs`** runs the real binary in a pseudo-terminal in `--demo` mode, presses
  keys, resizes, and checks the screen and the terminal state on exit. Unix only.
- **Never let a test reach the real pad.** The app starts the belt as soon as it connects, so
  a test that found the pad would run the treadmill unattended. Every pseudo-terminal session
  sets `CLI_WALKER_NO_HARDWARE=1`, which makes the app refuse the BLE path, and passes
  `--demo`. Keep both when adding tests.
- Strava flows are tested against a wiremock server (`src/strava/tests.rs`).

## Running

```bash
./run.sh                  # Build and run the walking dashboard
./run.sh strava login     # Authorize against Strava
./run.sh strava sync      # Upload past sessions
./run.sh --demo           # Simulated pad + sample history in a temp data dir (hidden flag)
```

`docs/demo.gif` is recorded from demo mode with VHS: `cargo build && vhs demo/demo.tape`.
On Ubuntu 24.04 and later, VHS's downloaded Chromium (`~/.cache/rod/browser/chromium-*/chrome`)
needs an AppArmor profile that allows `userns`, or it cannot create its sandbox and VHS fails
with "browser exited unexpectedly". Give it a profile rather than disabling the sandbox.

The dashboard is an interactive TUI (raw keyboard input, alternate screen), so `./run.sh`
builds and then `exec`s the binary rather than running it as a child of Cargo.

## Tech Stack

- **Async**: tokio; pad state flows through `tokio::sync::watch` channels
- **BLE**: [btleplug](https://github.com/deviceplug/btleplug) (CoreBluetooth, BlueZ over D-Bus, WinRT)
- **TUI**: [ratatui](https://ratatui.rs) + crossterm, full screen on the alternate screen
- **CLI**: clap (derive)
- **HTTP**: reqwest with rustls
- **Time**: jiff

**Dependencies.** `.cargo/config.toml` sets a 7-day minimum publish age: Cargo skips crate
versions younger than that when resolving, though versions already in `Cargo.lock` are kept.
`cargo add` of a fresh release therefore fails; lower the requirement to the version Cargo
suggests, or override once with `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow` if the new
version is really needed. `[lints.cargo] unused_dependencies = "deny"` fails the build on an
unused direct dependency (dev-dependencies are not checked).

crossterm is built with `use-dev-tty`. Its default edge-triggered reader drops a key that
arrives together with a resize until the next key comes in; the poll-based reader does not.
`redraws_after_a_resize_and_keeps_keys_pressed_with_it` covers this.

## Architecture

1. **BLE Protocol** (`protocol.rs`) — Stateless encoder/decoder. Command packets are
   `[0xF7, 0xA2, type, param, checksum, 0xFD]` with checksum = sum of the bytes between header
   and checksum, mod 256.

2. **Pad** (`pad/`) — `PadHandle` is how the app drives a treadmill: `watch` receivers for the
   latest `WalkingSession` and `ConnectionState`, and a command queue. Each pad runs as its own
   task. `pad/ble.rs` scans for the pad **by service UUID** (the A1 Pro advertises as
   `KS-ST-A1P`, not "WalkingPad"), connects, starts the belt, subscribes to `fe01` and polls
   status via writes to `fe02`, throttling commands to one per 700 ms. On quit it stops the
   belt only if it is moving (the pad *starts* the belt when sent a stop while idle).
   `pad/demo.rs` is the simulated pad and seeds demo mode's throwaway data directory.

3. **Data Model** (`model.rs`) — `WalkingSession` (one status sample) and `SessionRecord` (a
   finished walk, one JSON line in `sessions.jsonl`), plus formatting helpers and a rough
   calorie estimate. `model::instant` formats timestamps exactly like kotlinx-datetime.

4. **Session Tracking** (`tracker.rs`) — Turns the pad's status samples into
   discrete `SessionRecord`s. The pad zeroes its counters whenever the belt stops, so each
   belt run is seen as a fresh climb from zero; the tracker banks the highest-elapsed sample
   of each run and sums them. Runs separated by less than the coalesce window (1 minute) fold
   into a single record, so stepping off the belt — or letting it auto-stop — does not split
   a walk into fragments that each fall below the Strava upload threshold. A longer gap closes
   the record and starts a new one, so one app run can produce several sessions.

5. **Storage** (`store.rs`) — `DataDir` is `~/.cli-walker` (or demo mode's temp directory) and
   is passed to every store explicitly; nothing reads `HOME` behind the scenes.

6. **TUI** (`tui/`) — `dashboard.rs` draws the logo and a card (tab bar, rows separated by
   gradient rules, key hints) as a pure function of `View`. Three tabs: Session, Statistics,
   Strava; the Strava tab edits the upload threshold and feed muting in place. Every tab is
   `CARD_WIDTH` wide so switching never resizes the box. `tui/mod.rs` runs the event loop and
   maps actions (`keys.rs`) to pad commands and settings.

7. **Strava Upload** (`strava/`) — OAuth 2.0 authorization-code flow plus TCX uploads.
   `loopback.rs` is a one-shot HTTP listener on 127.0.0.1 that catches the OAuth redirect
   (Strava has no device-code flow); `api.rs` wraps the token and upload endpoints;
   `mod.rs` (`StravaService`) ties them together; `config.rs` and `upload_log.rs` persist
   credentials and uploaded-session ids. Commands live in `commands.rs`.

8. **Entry Point** (`main.rs`, `walk.rs`) — clap parses the command line. `walk.rs` runs the
   dashboard, feeds status samples to the `SessionTracker`, and persists each session as it
   closes. On exit it prints a summary of every session recorded during the run and uploads
   them. `paint.rs` styles plain output and drops escape codes when stdout is not a terminal.

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
- Access tokens last ~6 hours; `StravaApi::valid_access_token` refreshes on demand. Strava
  rotates refresh tokens, so whatever the refresh returns is persisted, not just the access
  token.
- Sessions are uploaded as TCX files (`POST /uploads`), not created as manual activities.
  The upload is what gives the activity an `upload_id` and app attribution. It is
  asynchronous: poll `GET /uploads/{id}` until it yields an `activity_id`.
- TCX has no Walk sport (only Running/Biking/Other), so every upload needs a follow-up
  `PUT /activities/{id}` setting `sport_type=Walk`. Muting folds into that same call.
- The pad reports only totals, so `strava/tcx.rs` synthesises trackpoints every 30s with
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
  processing timeouts, to losing contact with Strava while polling the upload, to the
  `sport_type` update, and to failing to write the upload log. A rejection reported by
  Strava while processing (`error` on the upload) is still a failure, so it can be retried.
- `~/.cli-walker/strava-uploads.json` maps session start times to Strava activity ids so
  sessions are never uploaded twice, storing `0` for "uploaded, id unknown". Keys are
  compared as parsed instants, not strings. It is kept
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
  dashboard's Strava tab (up/down = ±100 m, Ctrl+up/down = ±10 m). `StravaService::pending`
  and `upload` both honour it, so the pending count matches what a sync would send;
  `strava sync --all` ignores it. A session skipped this way is reported on exit.
- `strava mute on` sets `hide_from_home` on each upload via a follow-up `PUT`, since the
  create endpoint accepts no such field. This only keeps activities out of followers'
  feeds — the API exposes no activity visibility field at all, so true "Only You" privacy
  is an account setting at https://www.strava.com/settings/privacy.
