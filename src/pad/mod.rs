//! A treadmill the dashboard can observe and drive: the real pad over BLE, or the demo
//! simulation. Each runs as its own task; this handle is how the rest of the app talks to it.

pub mod ble;
pub mod demo;

use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::model::WalkingSession;

pub const MIN_SPEED_KMH: f64 = 0.5;
pub const MAX_SPEED_KMH: f64 = 6.0;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Scanning,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug)]
pub enum PadCommand {
    StartBelt(f64),
    StopBelt,
    SetSpeed(f64),
    /// Stop the belt if it is moving, disconnect, then report back.
    Shutdown(oneshot::Sender<()>),
}

/// The app's side of a running pad task.
#[derive(Debug)]
pub struct PadHandle {
    pub session: watch::Receiver<WalkingSession>,
    pub connection: watch::Receiver<ConnectionState>,
    commands: mpsc::UnboundedSender<PadCommand>,
    task: Option<JoinHandle<()>>,
}

impl PadHandle {
    pub fn start_belt(&self, speed_kmh: f64) {
        self.send(PadCommand::StartBelt(speed_kmh));
    }

    pub fn stop_belt(&self) {
        self.send(PadCommand::StopBelt);
    }

    pub fn set_speed(&self, speed_kmh: f64) {
        self.send(PadCommand::SetSpeed(
            speed_kmh.clamp(MIN_SPEED_KMH, MAX_SPEED_KMH),
        ));
    }

    /// Asks the pad task to stop a moving belt and disconnect, waiting up to `timeout`.
    pub async fn shutdown(mut self, timeout: Duration) {
        let (done, finished) = oneshot::channel();
        self.send(PadCommand::Shutdown(done));
        let _ = tokio::time::timeout(timeout, finished).await;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    fn send(&self, command: PadCommand) {
        // A pad task that has already ended has nothing left to drive.
        let _ = self.commands.send(command);
    }
}

impl Drop for PadHandle {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// The pad task's side: publishes state and receives commands.
pub struct PadChannels {
    pub session: watch::Sender<WalkingSession>,
    pub connection: watch::Sender<ConnectionState>,
    pub commands: mpsc::UnboundedReceiver<PadCommand>,
}

/// Creates both ends of a pad. The caller spawns a task that drives `PadChannels`, and hands
/// its join handle to `PadHandle::attach`.
pub fn channels() -> (PadHandle, PadChannels) {
    let (session_tx, session_rx) = watch::channel(WalkingSession::default());
    let (connection_tx, connection_rx) = watch::channel(ConnectionState::default());
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    (
        PadHandle {
            session: session_rx,
            connection: connection_rx,
            commands: command_tx,
            task: None,
        },
        PadChannels {
            session: session_tx,
            connection: connection_tx,
            commands: command_rx,
        },
    )
}

impl PadHandle {
    pub fn attach(mut self, task: JoinHandle<()>) -> Self {
        self.task = Some(task);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn speed_is_clamped_before_it_reaches_the_pad() {
        let (handle, mut pad) = channels();
        handle.set_speed(9.0);
        handle.set_speed(0.1);
        assert!(matches!(pad.commands.recv().await, Some(PadCommand::SetSpeed(s)) if s == 6.0));
        assert!(matches!(pad.commands.recv().await, Some(PadCommand::SetSpeed(s)) if s == 0.5));
    }

    #[tokio::test]
    async fn shutdown_gives_up_on_a_pad_that_never_answers() {
        let (handle, _pad) = channels();
        let started = std::time::Instant::now();
        handle.shutdown(Duration::from_millis(50)).await;
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
