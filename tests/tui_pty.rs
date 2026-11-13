//! Runs the real binary in a pseudo-terminal, the way a user would, against the simulated pad,
//! and checks what ends up on the screen. This catches what in-memory snapshot tests cannot:
//! raw-mode keyboard input, the alternate screen, resizing, and leaving the terminal usable on
//! exit.
//!
//! Every session sets `CLI_WALKER_NO_HARDWARE`, which makes the app refuse the real pad: it
//! starts the belt as soon as it connects, and a test must never do that.
//!
//! Unix only for now: ConPTY on Windows answers terminal queries itself and needs separate
//! handling.
#![cfg(unix)]

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const TIMEOUT: Duration = Duration::from_secs(15);
const UP: &[u8] = b"w";
const DOWN: &[u8] = b"s";
const CTRL_UP: &[u8] = b"\x1b[1;5A";
const TAB: &[u8] = b"\t";

/// The binary running in a pseudo-terminal, with a virtual screen fed by its output.
struct Session {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    screen: vt100::Parser,
    _tmp: tempfile::TempDir,
}

#[derive(Debug, PartialEq)]
enum Pump {
    Data,
    Idle,
    Closed,
}

impl Session {
    fn spawn(args: &[&str], rows: u16, cols: u16) -> Self {
        let pty = native_pty_system()
            .openpty(size(rows, cols))
            .expect("open pty");
        let tmp = tempfile::tempdir().unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_cli-walker"));
        cmd.args(args);
        cmd.env("CLI_WALKER_NO_HARDWARE", "1");
        cmd.env("TERM", "xterm-256color");
        // Demo mode's throwaway data directory goes here, not into the real temp dir.
        cmd.env("TMPDIR", tmp.path());
        let child = pty.slave.spawn_command(cmd).expect("spawn cli-walker");
        drop(pty.slave);

        let mut reader = pty.master.try_clone_reader().expect("pty reader");
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });

        let writer = pty.master.take_writer().expect("pty writer");
        Session {
            master: pty.master,
            child,
            writer,
            output,
            screen: vt100::Parser::new(rows.try_into().unwrap(), cols.try_into().unwrap(), 0),
            _tmp: tmp,
        }
    }

    fn demo() -> Self {
        Self::spawn(&["--demo"], 45, 100)
    }

    /// Feeds everything the program has written so far to the virtual screen, waiting up to
    /// `wait` for the first chunk.
    fn pump(&mut self, wait: Duration) -> Pump {
        let first = match self.output.recv_timeout(wait) {
            Ok(chunk) => chunk,
            Err(mpsc::RecvTimeoutError::Timeout) => return Pump::Idle,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Pump::Closed,
        };
        self.screen.process(&first);
        while let Ok(chunk) = self.output.try_recv() {
            self.screen.process(&chunk);
        }
        Pump::Data
    }

    fn contents(&self) -> String {
        self.screen.screen().contents()
    }

    fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + TIMEOUT;
        while !self.contents().contains(text) {
            assert!(
                Instant::now() < deadline && self.pump(Duration::from_millis(50)) != Pump::Closed,
                "timed out waiting for {text:?}; screen:\n{}",
                self.contents()
            );
        }
    }

    fn press(&mut self, keys: &[u8]) {
        self.writer.write_all(keys).unwrap();
        self.writer.flush().unwrap();
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.master.resize(size(rows, cols)).unwrap();
        self.screen
            .screen_mut()
            .set_size(rows.try_into().unwrap(), cols.try_into().unwrap());
    }

    /// Waits for the program to exit and returns whether it succeeded.
    fn wait_for_exit(&mut self) -> bool {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // Drain whatever the program wrote on its way out.
                while self.pump(Duration::from_millis(200)) == Pump::Data {}
                return status.success();
            }
            assert!(
                Instant::now() < deadline,
                "program did not exit; screen:\n{}",
                self.contents()
            );
            self.pump(Duration::from_millis(50));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[test]
fn walks_through_every_tab_and_prints_a_summary_on_quit() {
    let mut s = Session::demo();
    s.wait_for("CLI-Walker - Kingsmith WalkingPad Monitor");
    s.wait_for("Connected");
    s.wait_for("2.5 km/h");
    assert!(
        s.screen.screen().alternate_screen(),
        "dashboard should use the alternate screen"
    );
    assert!(
        s.screen.screen().hide_cursor(),
        "cursor should be hidden while drawing"
    );

    // Speed: +0.5, then +0.1 with Ctrl.
    s.press(UP);
    s.wait_for("3.0 km/h");
    s.press(CTRL_UP);
    s.wait_for("3.1 km/h");

    s.press(TAB);
    s.wait_for("Recent Sessions");
    s.wait_for("This Month");

    s.press(TAB);
    s.wait_for("Demo Walker");
    s.wait_for("1 km");
    // On this tab, w and s edit the threshold, not the speed.
    s.press(DOWN);
    s.wait_for("900 m");
    s.press(b"m");
    s.wait_for("Muted");

    s.press(TAB);
    s.wait_for("3.1 km/h");

    s.press(b"q");
    assert!(
        s.wait_for_exit(),
        "non-zero exit status; screen:\n{}",
        s.contents()
    );

    let screen = s.screen.screen();
    assert!(!screen.alternate_screen(), "alternate screen left on");
    assert!(!screen.hide_cursor(), "cursor left hidden");
    let text = s.contents();
    // The demo pad joins a walk already 27 minutes in.
    assert!(text.contains("Session Summary"), "{text}");
    assert!(
        text.contains("Distance: 1.3") || text.contains("Distance: 1.4"),
        "{text}"
    );
    assert!(text.contains("Time:     27:"), "{text}");
}

/// A key pressed together with a resize must not wait for the next key. crossterm's default
/// edge-triggered reader lost it about one run in eight; the `use-dev-tty` feature fixes that.
#[test]
fn redraws_after_a_resize_and_keeps_keys_pressed_with_it() {
    let mut s = Session::demo();
    s.wait_for("Running");
    for round in 0..4 {
        let (rows, cols) = if round % 2 == 0 { (50, 120) } else { (30, 60) };
        s.resize(rows, cols);
        s.press(TAB);
        let expected = ["Totals", "Min distance", "Calories", "Totals"][round];
        s.wait_for(expected);
    }
    // The box shrank with the terminal and still ends on its border.
    assert!(
        s.contents()
            .lines()
            .any(|l| l.starts_with('╭') && l.ends_with('╮'))
    );
    s.press(b"q");
    assert!(s.wait_for_exit());
}

#[test]
fn space_stops_and_restarts_the_belt() {
    let mut s = Session::demo();
    s.wait_for("Running");
    s.press(b" ");
    s.wait_for("Idle");
    s.press(b" ");
    s.wait_for("Running");
    // Like the real pad, a new belt run counts from zero.
    s.wait_for("00:0");
    s.press(b"q");
    assert!(s.wait_for_exit());
    // Two belt runs a moment apart fold into one session.
    assert!(
        s.contents().contains("Session Summary\n"),
        "{}",
        s.contents()
    );
}

#[test]
fn quits_on_ctrl_c() {
    let mut s = Session::demo();
    s.wait_for("Connected");
    // In raw mode Ctrl+C arrives as a key press, not as SIGINT.
    s.press(b"\x03");
    assert!(s.wait_for_exit(), "non-zero exit status");
    assert!(s.contents().contains("Session Summary"));
}

#[test]
fn unrelated_keys_do_nothing() {
    let mut s = Session::demo();
    s.wait_for("Connected");
    s.press(b"x");
    s.press(b"\x1b");
    s.pump(Duration::from_millis(300));
    assert!(
        s.child.try_wait().unwrap().is_none(),
        "exited on an unrelated key"
    );
    s.press(b"q");
    assert!(s.wait_for_exit());
}

#[test]
fn refuses_the_real_pad_under_test() {
    let mut s = Session::spawn(&[], 24, 80);
    assert!(
        !s.wait_for_exit(),
        "the real BLE path must not run under test"
    );
    assert!(
        s.contents().contains("CLI_WALKER_NO_HARDWARE"),
        "{}",
        s.contents()
    );
}
