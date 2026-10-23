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

- macOS on Apple Silicon or Intel
- JDK 17 or newer (for Gradle; the app itself is a native binary)
- A WalkingPad A1 Pro. Other WalkingPad models that use the same BLE protocol may work but
  are untested.

On first launch, macOS asks for Bluetooth access for your terminal app. Allow it.

## Build and run

```bash
./run.sh
```

This builds the native binary and runs it in the current terminal. The dashboard is an
interactive TUI, so run it directly rather than through a Gradle task.

The first build downloads the Kotlin/Native toolchain and takes a while. Later builds are
quicker.

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
./gradlew compileKotlinMacosArm64   # compile only
./gradlew macosArm64Test            # run tests
```

On Intel Macs, replace `macosArm64` with `macosX64`.

`cli-walker --demo` runs the dashboard against a simulated pad and sample history in a
throwaway home directory, so it needs no hardware and leaves your data alone. The README
GIF is recorded from it with [VHS](https://github.com/charmbracelet/vhs):
`vhs demo/demo.tape`. See [CLAUDE.md](CLAUDE.md) for an
architecture overview and protocol notes.

## Acknowledgements

The WalkingPad BLE protocol was worked out by
[ph4-walkingpad](https://github.com/ph4r05/ph4-walkingpad) and
[QWalkingPad](https://github.com/DorianRudolph/QWalkingPad).

Built with [Kable](https://github.com/JuulLabs/kable),
[Mordant](https://github.com/ajalt/mordant), [Clikt](https://github.com/ajalt/clikt) and
[Ktor](https://ktor.io).

## License

[MIT](LICENSE)
