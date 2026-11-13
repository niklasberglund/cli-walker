# CLI-Walker

A terminal dashboard for the **Kingsmith WalkingPad A1 Pro**. It connects to the pad over
Bluetooth, controls the belt from the keyboard, logs every walk, and can upload walks to
Strava.

![CLI-Walker dashboard](docs/demo.gif)

- Live speed, distance, steps, time and calories with a color-coded speed bar
- Start, stop and adjust speed from the keyboard
- Session history and statistics: totals, this month, personal bests
- Automatic Strava upload as *Walk* activities, with an optional minimum distance and feed muting

> **Unofficial.** This project is not affiliated with Kingsmith, WalkingPad or Strava. It
> drives a moving treadmill belt, so stay aware of the pad while using it. Use at your own
> risk.

## Requirements

- macOS or Linux. Windows builds too, but is untested with the pad.
- [Rust](https://rustup.rs) (stable) to build it.
- On Linux: BlueZ, plus `libdbus-1-dev` and `pkg-config` to build
  (`sudo apt install libdbus-1-dev pkg-config` on Debian and Ubuntu).
- A WalkingPad A1 Pro. Other WalkingPad models that use the same BLE protocol may work but
  are untested.

On first launch, macOS asks for Bluetooth access for your terminal app. Allow it. On Linux,
no extra permissions are needed for a normal user with BlueZ running.

## Build and run

```bash
./run.sh
```

This builds the binary with Cargo and runs it in the current terminal. The pad is found by
its Bluetooth service, so its advertised name (the A1 Pro shows up as `KS-ST-A1P`) does not
matter. The belt starts at the starting speed as soon as the pad connects.

### Options

| Option        | Description                                     |
| ------------- | ----------------------------------------------- |
| `--speed N`   | Starting speed in km/h (default 2.5)            |
| `--no-strava` | Don't upload this run's sessions to Strava      |
| `--debug`     | Log raw BLE packets to `~/.cli-walker/ble.log` |

### Keys

| Key                   | Action                                         |
| --------------------- | ---------------------------------------------- |
| Up / Down             | Speed ±0.5 km/h                                |
| Ctrl + Up / Down      | Speed ±0.1 km/h                                |
| Space                 | Stop / start the belt                          |
| Tab, Left / Right     | Switch tabs (Session, Statistics, Strava)      |
| `m` (Strava tab)      | Toggle feed muting                             |
| Up / Down (Strava tab)| Minimum upload distance ±100 m (Ctrl: ±10 m)   |
| `q`, Ctrl+C           | Quit (stops the belt if it is moving)          |

## Sessions

The pad resets its counters whenever the belt stops. CLI-Walker joins belt runs separated by
less than a minute into one session, so stepping off briefly doesn't split a walk. A longer
pause starts a new session.

Sessions are appended to `~/.cli-walker/sessions.jsonl`. A summary prints on exit.

## Strava

Strava has no shared API key for apps like this, so you register your own API application:

1. Go to <https://www.strava.com/settings/api> and create an application.
2. Set **Authorization Callback Domain** to `localhost`.
3. Run `./run.sh strava login` and enter the Client ID and Client Secret. A browser window
   opens for authorization.

Credentials and tokens are stored in `~/.cli-walker/strava.json` (mode 0600). You can also
pass them as `--client-id` / `--client-secret` or `STRAVA_CLIENT_ID` /
`STRAVA_CLIENT_SECRET`.

Once you're logged in, sessions upload automatically when the dashboard exits.

| Command                              | Description                                         |
| ------------------------------------ | --------------------------------------------------- |
| `strava status`                      | Show authorization and pending uploads              |
| `strava sync`                        | Upload sessions that haven't been uploaded yet      |
| `strava sync --dry-run`              | Write TCX files to `~/.cli-walker/tcx/` instead     |
| `strava sync --all`                  | Include sessions below the minimum distance         |
| `strava threshold 500m\|1.5km\|off`  | Set the minimum upload distance                     |
| `strava mute on\|off`                | Hide uploads from followers' feeds                  |
| `strava logout`                      | Forget stored credentials                           |

Notes:

- Activity visibility follows your Strava account default. The API can't set it. For
  private walks, change the default under
  [Privacy Controls](https://www.strava.com/settings/privacy).
- The pad reports only totals, so uploads have evenly interpolated distance rather than
  real pace data.
- Each session is uploaded at most once. Upload state lives in
  `~/.cli-walker/strava-uploads.json`.

## Development

```bash
cargo build
cargo test                      # unit, snapshot and pseudo-terminal tests
cargo clippy --all-targets
cargo llvm-cov --summary-only   # coverage, with cargo-llvm-cov installed
```

`cli-walker --demo` runs the dashboard against a simulated pad and sample history in a
throwaway home directory, so it needs no hardware and leaves your data alone. The README
GIF is recorded from it with [VHS](https://github.com/charmbracelet/vhs):
`cargo build && vhs demo/demo.tape`. See [CLAUDE.md](CLAUDE.md) for an
architecture overview and protocol notes.

## Acknowledgements

The WalkingPad BLE protocol was worked out by
[ph4-walkingpad](https://github.com/ph4r05/ph4-walkingpad) and
[QWalkingPad](https://github.com/DorianRudolph/QWalkingPad).

Built with [btleplug](https://github.com/deviceplug/btleplug),
[ratatui](https://ratatui.rs), [clap](https://github.com/clap-rs/clap),
[reqwest](https://github.com/seanmonstar/reqwest) and [tokio](https://tokio.rs).

## License

[MIT](LICENSE)
