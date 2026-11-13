//! WalkingPad BLE protocol encoding and decoding.
//!
//! Command packets are `[0xF7, 0xA2, type, param, checksum, 0xFD]`, where the checksum is the
//! sum of the bytes between the header and the checksum, mod 256.
//!
//! Protocol reverse-engineered by ph4r05/ph4-walkingpad and DorianRudolph/QWalkingPad.

use uuid::{Uuid, uuid};

use crate::model::{BeltMode, BeltState, WalkingSession};

pub const SERVICE_UUID: Uuid = uuid!("0000fe00-0000-1000-8000-00805f9b34fb");
pub const NOTIFY_CHARACTERISTIC_UUID: Uuid = uuid!("0000fe01-0000-1000-8000-00805f9b34fb");
pub const WRITE_CHARACTERISTIC_UUID: Uuid = uuid!("0000fe02-0000-1000-8000-00805f9b34fb");

const HEADER: u8 = 0xF7;
const FOOTER: u8 = 0xFD;
const CMD_GROUP: u8 = 0xA2;
const RESP_STATUS: u8 = 0xF8;

pub fn query_status() -> [u8; 6] {
    build(0x00, 0x00)
}

pub fn start() -> [u8; 6] {
    build(0x04, 0x01)
}

pub fn stop() -> [u8; 6] {
    build(0x04, 0x00)
}

pub fn set_mode_manual() -> [u8; 6] {
    build(0x02, BeltMode::Manual as u8)
}

/// Speed is sent in tenths of km/h and capped at the pad's 6.0 km/h.
pub fn set_speed(speed_kmh: f64) -> [u8; 6] {
    let tenths = (speed_kmh * 10.0) as i64;
    build(0x01, tenths.clamp(0, 60) as u8)
}

fn build(cmd_type: u8, param: u8) -> [u8; 6] {
    let mut cmd = [HEADER, CMD_GROUP, cmd_type, param, 0x00, FOOTER];
    cmd[4] = checksum(&cmd);
    cmd
}

fn checksum(cmd: &[u8]) -> u8 {
    let sum: u32 = cmd[1..cmd.len() - 2].iter().map(|&b| u32::from(b)).sum();
    (sum % 256) as u8
}

/// Decodes a status notification. Anything else (parameter responses, short or foreign
/// packets) yields `None`.
pub fn parse_status(data: &[u8]) -> Option<WalkingSession> {
    if data.len() < 16 || data[0] != RESP_STATUS || data[1] != CMD_GROUP {
        return None;
    }
    let raw_state = data[2];
    Some(WalkingSession {
        state: BeltState::from_byte(raw_state),
        raw_state,
        speed: f64::from(data[3]) / 10.0,
        mode: BeltMode::from_byte(data[4]),
        elapsed_seconds: read_u24(data, 5),
        distance_km: f64::from(read_u24(data, 8)) / 100.0,
        steps: read_u24(data, 11),
    })
}

fn read_u24(data: &[u8], offset: usize) -> u32 {
    u32::from(data[offset]) << 16 | u32::from(data[offset + 1]) << 8 | u32::from(data[offset + 2])
}

pub fn to_hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_the_known_packets() {
        // Reference packets from ph4-walkingpad.
        assert_eq!(query_status(), [0xF7, 0xA2, 0x00, 0x00, 0xA2, 0xFD]);
        assert_eq!(start(), [0xF7, 0xA2, 0x04, 0x01, 0xA7, 0xFD]);
        assert_eq!(stop(), [0xF7, 0xA2, 0x04, 0x00, 0xA6, 0xFD]);
        assert_eq!(set_mode_manual(), [0xF7, 0xA2, 0x02, 0x01, 0xA5, 0xFD]);
        assert_eq!(set_speed(2.5), [0xF7, 0xA2, 0x01, 25, 0xBC, 0xFD]);
    }

    #[test]
    fn speed_is_clamped_to_the_pad_range() {
        assert_eq!(set_speed(9.0)[3], 60);
        assert_eq!(set_speed(-1.0)[3], 0);
        // Truncated, like the Kotlin version: 3.79 km/h is sent as 3.7.
        assert_eq!(set_speed(3.79)[3], 37);
    }

    #[test]
    fn checksum_wraps_at_256() {
        let cmd = build(0xFF, 0xFF);
        assert_eq!(cmd[4], ((0xA2 + 0xFF + 0xFF) % 256) as u8);
    }

    #[test]
    fn parses_a_status_packet() {
        let packet = [
            0xF8, 0xA2, // header
            0x01, // running
            35,   // 3.5 km/h
            0x01, // manual
            0x00, 0x06, 0x7D, // 1661 s
            0x00, 0x00, 0x8A, // 138 -> 1.38 km
            0x00, 0x08, 0x4F, // 2127 steps
            0x00, 0x00, // trailing bytes the parser ignores
        ];
        let s = parse_status(&packet).unwrap();
        assert_eq!(s.state, BeltState::Running);
        assert_eq!(s.raw_state, 1);
        assert_eq!(s.speed, 3.5);
        assert_eq!(s.mode, BeltMode::Manual);
        assert_eq!(s.elapsed_seconds, 1661);
        assert_eq!(s.distance_km, 1.38);
        assert_eq!(s.steps, 2127);
    }

    #[test]
    fn keeps_unknown_belt_states() {
        let mut packet = [0u8; 16];
        packet[..3].copy_from_slice(&[0xF8, 0xA2, 0x07]);
        let s = parse_status(&packet).unwrap();
        assert_eq!(s.state, BeltState::Unknown);
        assert_eq!(s.raw_state, 7);
    }

    #[test]
    fn ignores_other_packets() {
        assert!(parse_status(&[0xF8, 0xA2]).is_none());
        assert!(parse_status(&[0xF8, 0xA7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
        assert!(parse_status(&[0xA7; 16]).is_none());
    }
}
