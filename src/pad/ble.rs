//! The real WalkingPad, over Bluetooth Low Energy with btleplug.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use btleplug::api::{
    Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{FutureExt, Stream, StreamExt};
use jiff::Timestamp;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use super::{ConnectionState, PadChannels, PadCommand, PadHandle};
use crate::model::WalkingSession;
use crate::protocol;
use crate::store::DataDir;

const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// The pad ignores commands that arrive too close together.
const MIN_COMMAND_INTERVAL: Duration = Duration::from_millis(700);
/// A stop is written without response, so give the OS time to actually transmit it before
/// the connection is torn down.
const STOP_FLUSH_DELAY: Duration = Duration::from_secs(1);
/// How long quitting waits for a status that shows whether a just-started belt is moving.
/// Together with the throttling and the stop it must fit inside the app's shutdown timeout.
const STATUS_WAIT: Duration = Duration::from_millis(1500);

/// Starts the BLE task: scan for the pad, connect, start the belt, then poll its status.
/// With `debug`, raw packets are logged to `ble.log` in the data directory rather than to
/// stderr, which would draw over the dashboard.
pub fn spawn(data_dir: &DataDir, debug: bool, start_speed: f64) -> PadHandle {
    let log = DebugLog::open(data_dir, debug);
    let (handle, channels) = super::channels();
    let task = tokio::spawn(run(channels, log, start_speed));
    handle.attach(task)
}

async fn run(mut pad: PadChannels, mut log: DebugLog, start_speed: f64) {
    // Connecting can take up to the scan timeout. Quitting meanwhile must not wait for it.
    let connected = tokio::select! {
        result = connect(&pad.connection, &mut log) => result,
        done = wait_for_shutdown(&mut pad.commands) => {
            if let Some(done) = done {
                let _ = done.send(());
            }
            return;
        }
    };
    let (adapter, peripheral, write) = match connected {
        Ok(parts) => parts,
        Err(e) => {
            log.line(&format!("CONNECT FAILED: {e:#}"));
            pad.connection
                .send_replace(ConnectionState::Error(format!("{e:#}")));
            let _ = wait_for_shutdown(&mut pad.commands)
                .await
                .map(|done| done.send(()));
            return;
        }
    };
    let mut notifications = match peripheral.notifications().await {
        Ok(stream) => stream,
        Err(e) => {
            pad.connection.send_replace(ConnectionState::Error(format!(
                "cannot read from the pad: {e}"
            )));
            let _ = wait_for_shutdown(&mut pad.commands)
                .await
                .map(|done| done.send(()));
            return;
        }
    };
    pad.connection.send_replace(ConnectionState::Connected);

    let mut link = Link {
        peripheral,
        write,
        last_command: None,
        start_unconfirmed: false,
        queried_since_start: false,
        log,
    };
    // Starting takes a couple of seconds of throttled writes. A quit in the meantime must be
    // handled now, not queued behind them; other commands wait until the belt has started.
    let mut held = Vec::new();
    let quit = {
        let start = link.start_belt(start_speed);
        tokio::pin!(start);
        loop {
            tokio::select! {
                () = &mut start => break None,
                command = pad.commands.recv() => match command {
                    Some(PadCommand::Shutdown(done)) => break Some(Some(done)),
                    Some(command) => held.push(command),
                    None => break Some(None),
                },
            }
        }
    };
    match quit {
        Some(Some(done)) => link.shut_down(&pad, &mut notifications, done).await,
        Some(None) => {
            let _ = link.peripheral.disconnect().await;
        }
        None => link.serve(pad, held, notifications).await,
    }
    let _ = adapter.stop_scan().await;
}

/// Discards commands until a shutdown arrives, for the phases where the pad cannot be driven.
async fn wait_for_shutdown(
    commands: &mut mpsc::UnboundedReceiver<PadCommand>,
) -> Option<oneshot::Sender<()>> {
    while let Some(command) = commands.recv().await {
        if let PadCommand::Shutdown(done) = command {
            return Some(done);
        }
    }
    None
}

async fn connect(
    connection: &watch::Sender<ConnectionState>,
    log: &mut DebugLog,
) -> Result<(Adapter, Peripheral, Characteristic)> {
    connection.send_replace(ConnectionState::Scanning);
    let manager = Manager::new().await.context("Bluetooth is unavailable")?;
    let adapter = manager
        .adapters()
        .await
        .context("cannot list Bluetooth adapters")?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no Bluetooth adapter found"))?;

    let peripheral = timeout(SCAN_TIMEOUT, find_pad(&adapter))
        .await
        .map_err(|_| {
            anyhow!(
                "No WalkingPad found after 30s. Is it powered on and not connected to another app?"
            )
        })??;
    let _ = adapter.stop_scan().await;
    log.line(&format!("FOUND: {}", peripheral.id()));

    connection.send_replace(ConnectionState::Connecting);
    peripheral
        .connect()
        .await
        .context("cannot connect to the pad")?;
    peripheral
        .discover_services()
        .await
        .context("cannot read the pad's services")?;
    let find = |uuid| {
        peripheral
            .characteristics()
            .into_iter()
            .find(|c| c.uuid == uuid)
            .ok_or_else(|| anyhow!("the pad has no characteristic {uuid}"))
    };
    let notify = find(protocol::NOTIFY_CHARACTERISTIC_UUID)?;
    let write = find(protocol::WRITE_CHARACTERISTIC_UUID)?;
    peripheral
        .subscribe(&notify)
        .await
        .context("cannot subscribe to status updates")?;
    Ok((adapter, peripheral, write))
}

/// Finds the pad by its service UUID. The A1 Pro advertises as `KS-ST-A1P`, not
/// "WalkingPad", so matching on the name would miss it.
async fn find_pad(adapter: &Adapter) -> Result<Peripheral> {
    let mut events = adapter.events().await?;
    adapter
        .start_scan(ScanFilter {
            services: vec![protocol::SERVICE_UUID],
        })
        .await
        .context("cannot start scanning")?;

    // BlueZ reports devices it already knows as updates rather than discoveries.
    for peripheral in adapter.peripherals().await? {
        if advertises_pad_service(&peripheral).await {
            return Ok(peripheral);
        }
    }
    while let Some(event) = events.next().await {
        let (CentralEvent::DeviceDiscovered(id)
        | CentralEvent::DeviceUpdated(id)
        | CentralEvent::ServicesAdvertisement { id, .. }) = event
        else {
            continue;
        };
        if let Ok(peripheral) = adapter.peripheral(&id).await
            && advertises_pad_service(&peripheral).await
        {
            return Ok(peripheral);
        }
    }
    Err(anyhow!("Bluetooth scanning stopped unexpectedly"))
}

async fn advertises_pad_service(peripheral: &Peripheral) -> bool {
    matches!(
        peripheral.properties().await,
        Ok(Some(props)) if props.services.contains(&protocol::SERVICE_UUID)
    )
}

struct Link {
    peripheral: Peripheral,
    write: Characteristic,
    /// When the last command went out; `None` until the first one has.
    last_command: Option<Instant>,
    /// A start went out and no status has shown its effect yet. Until one does, the last
    /// status may say Idle while the belt is in fact starting.
    start_unconfirmed: bool,
    /// A status query went out after the latest start, so the next status reflects it.
    queried_since_start: bool,
    log: DebugLog,
}

impl Link {
    async fn serve(
        &mut self,
        mut pad: PadChannels,
        held: Vec<PadCommand>,
        mut notifications: impl Stream<Item = btleplug::api::ValueNotification> + Unpin,
    ) {
        for command in held {
            self.drive(command).await;
        }
        let mut poll = interval(POLL_INTERVAL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = poll.tick() => {
                    if !self.peripheral.is_connected().await.unwrap_or(false) {
                        // The pad drops the link when it goes to standby.
                        pad.connection.send_if_modified(|state| {
                            let changed = *state != ConnectionState::Disconnected;
                            *state = ConnectionState::Disconnected;
                            changed
                        });
                        continue;
                    }
                    self.query_status(false).await;
                }
                notification = notifications.next() => {
                    let Some(notification) = notification else { continue };
                    self.notified(&pad, &notification);
                }
                command = pad.commands.recv() => match command {
                    Some(PadCommand::Shutdown(done)) => {
                        self.shut_down(&pad, &mut notifications, done).await;
                        return;
                    }
                    Some(command) => self.drive(command).await,
                    None => {
                        let _ = self.peripheral.disconnect().await;
                        return;
                    }
                },
            }
        }
    }

    /// Belt commands. Shutdown is handled by the caller, which owns the notification stream.
    async fn drive(&mut self, command: PadCommand) {
        match command {
            PadCommand::StartBelt(speed) => self.start_belt(speed).await,
            PadCommand::StopBelt => {
                self.log.line("CMD: stopBelt");
                self.send(&protocol::stop(), true).await;
            }
            PadCommand::SetSpeed(speed) => {
                self.log.line(&format!("CMD: setSpeed {speed}"));
                self.send(&protocol::set_speed(speed), true).await;
            }
            PadCommand::Shutdown(done) => {
                let _ = done.send(());
            }
        }
    }

    /// Stops the belt if it is moving, then disconnects.
    async fn shut_down(
        &mut self,
        pad: &PadChannels,
        notifications: &mut (impl Stream<Item = btleplug::api::ValueNotification> + Unpin),
        done: oneshot::Sender<()>,
    ) {
        // The pad starts the belt when sent a stop while idle, so only stop a belt that is
        // actually moving.
        if self.belt_moving(pad, notifications).await {
            self.log.line("CMD: stopBelt (quit)");
            self.send(&protocol::stop(), true).await;
            sleep(STOP_FLUSH_DELAY).await;
        }
        let _ = self.peripheral.disconnect().await;
        pad.connection.send_replace(ConnectionState::Disconnected);
        let _ = done.send(());
    }

    /// Whether the belt is moving. Right after a start the last status predates it, so this
    /// asks the pad for a fresh one rather than trusting it.
    async fn belt_moving(
        &mut self,
        pad: &PadChannels,
        notifications: &mut (impl Stream<Item = btleplug::api::ValueNotification> + Unpin),
    ) -> bool {
        // Take in whatever already arrived, so the answer below is the pad's current state.
        while let Some(Some(notification)) = notifications.next().now_or_never() {
            self.notified(pad, &notification);
        }
        if self.start_unconfirmed {
            self.query_status(true).await;
            let _ = timeout(STATUS_WAIT, async {
                while self.start_unconfirmed {
                    let Some(notification) = notifications.next().await else {
                        break;
                    };
                    self.notified(pad, &notification);
                }
            })
            .await;
            if self.start_unconfirmed {
                // Stopping a belt that never started would start it, so leave it alone.
                self.log
                    .line("QUIT: no status since the belt was started; not stopping");
                return false;
            }
        }
        pad.session.borrow().is_active()
    }

    async fn query_status(&mut self, throttle: bool) {
        if self.start_unconfirmed {
            self.queried_since_start = true;
        }
        self.send(&protocol::query_status(), throttle).await;
    }

    fn notified(&mut self, pad: &PadChannels, notification: &btleplug::api::ValueNotification) {
        if notification.uuid == protocol::NOTIFY_CHARACTERISTIC_UUID {
            self.received(pad, &notification.value);
        }
    }

    fn received(&mut self, pad: &PadChannels, data: &[u8]) {
        match protocol::parse_status(data) {
            Some(session) => {
                self.log_status(data, &session);
                pad.session.send_replace(session);
                if self.queried_since_start {
                    self.start_unconfirmed = false;
                    self.queried_since_start = false;
                }
            }
            None => self.log.line(&format!(
                "RX: {} | unrecognized response",
                protocol::to_hex(data)
            )),
        }
    }

    /// Manual mode, then start, then the speed. The pad needs the pauses between them.
    async fn start_belt(&mut self, speed: f64) {
        self.log.line(&format!("CMD: startBelt speed={speed}"));
        self.start_unconfirmed = true;
        self.queried_since_start = false;
        self.send(&protocol::set_mode_manual(), true).await;
        sleep(Duration::from_millis(300)).await;
        self.send(&protocol::start(), true).await;
        sleep(Duration::from_millis(700)).await;
        self.send(&protocol::set_speed(speed), true).await;
    }

    /// Writes a command. A failed write (the pad dropped the link mid-write, say) must never
    /// crash the app, so failures are only logged.
    async fn send(&mut self, data: &[u8], throttle: bool) {
        if !self.peripheral.is_connected().await.unwrap_or(false) {
            return;
        }
        if throttle {
            let wait = self.last_command.map_or(Duration::ZERO, |last| {
                MIN_COMMAND_INTERVAL.saturating_sub(last.elapsed())
            });
            if !wait.is_zero() {
                sleep(wait).await;
            }
        }
        self.log.line(&format!("TX: {}", protocol::to_hex(data)));
        match self
            .peripheral
            .write(&self.write, data, WriteType::WithoutResponse)
            .await
        {
            Ok(()) => self.last_command = Some(Instant::now()),
            Err(e) => self.log.line(&format!("TX FAILED: {e}")),
        }
    }

    fn log_status(&mut self, data: &[u8], s: &WalkingSession) {
        if self.log.enabled() {
            self.log.line(&format!(
                "RX: {} | state={:?}({}) speed={} dist={} steps={} time={}s mode={:?}",
                protocol::to_hex(data),
                s.state,
                s.raw_state,
                s.speed,
                s.distance_km,
                s.steps,
                s.elapsed_seconds,
                s.mode
            ));
        }
    }
}

/// `--debug` packet log. Disabled unless asked for.
struct DebugLog(Option<File>);

impl DebugLog {
    fn open(data_dir: &DataDir, enabled: bool) -> Self {
        if !enabled || data_dir.ensure().is_err() {
            return Self(None);
        }
        Self(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(data_dir.file("ble.log"))
                .ok(),
        )
    }

    fn enabled(&self) -> bool {
        self.0.is_some()
    }

    fn line(&mut self, msg: &str) {
        if let Some(file) = &mut self.0 {
            let _ = writeln!(file, "{} {msg}", Timestamp::now());
        }
    }
}
