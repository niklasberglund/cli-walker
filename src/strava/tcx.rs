//! Renders a session as a Garmin TCX document for upload.
//!
//! The pad only reports totals, never a per-second stream, so the track is synthesised by
//! interpolating distance linearly across the session. That is enough for Strava to accept the
//! file, attribute the activity to this app, and draw a distance/pace graph.
//!
//! `Sport` is `Other` because TCX only allows Running, Biking and Other; the activity is
//! corrected to Walk with a follow-up update once Strava has processed the upload.
//!
//! The output must validate against `TrainingCenterDatabasev2.xsd` in full (`Device_t`
//! requires `Version`, `Application_t` requires `Build`): other importers are stricter than
//! Strava.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use jiff::{SignedDuration, Timestamp};

use crate::model::SessionRecord;
use crate::store::{DataDir, create_private_dir};

/// Named in the activity description. Strava takes no device name from TCX.
pub const DEVICE_NAME: &str = "WalkingPad A1 Pro";

/// Seconds between synthesised trackpoints.
const TRACKPOINT_INTERVAL: u32 = 30;

pub fn build_tcx(record: &SessionRecord) -> String {
    let start = whole_seconds(record.started_at);
    let total_meters = record.distance_km * 1000.0;
    let duration = record.duration_seconds;

    let mut trackpoints = String::new();
    let mut elapsed = 0;
    while elapsed < duration {
        push_trackpoint(&mut trackpoints, start, elapsed, total_meters, duration);
        elapsed += TRACKPOINT_INTERVAL;
    }
    // Always close the track on the true end of the session, whatever the interval.
    push_trackpoint(&mut trackpoints, start, duration, total_meters, duration);

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<TrainingCenterDatabase
    xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2"
    xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:schemaLocation="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2 http://www.garmin.com/xmlschemas/TrainingCenterDatabasev2.xsd">
  <Activities>
    <Activity Sport="Other">
      <Id>{start}</Id>
      <Lap StartTime="{start}">
        <TotalTimeSeconds>{duration}</TotalTimeSeconds>
        <DistanceMeters>{distance}</DistanceMeters>
        <MaximumSpeed>{max_speed}</MaximumSpeed>
        <Calories>{calories}</Calories>
        <Intensity>Active</Intensity>
        <TriggerMethod>Manual</TriggerMethod>
        <Track>
{trackpoints}        </Track>
      </Lap>
      <Creator xsi:type="Device_t">
        <Name>{device}</Name>
        <UnitId>0</UnitId>
        <ProductID>0</ProductID>
        <Version>
          <VersionMajor>1</VersionMajor>
          <VersionMinor>0</VersionMinor>
          <BuildMajor>0</BuildMajor>
          <BuildMinor>0</BuildMinor>
        </Version>
      </Creator>
    </Activity>
  </Activities>
  <Author xsi:type="Application_t">
    <Name>cli-walker</Name>
    <Build>
      <Version>
        <VersionMajor>1</VersionMajor>
        <VersionMinor>0</VersionMinor>
        <BuildMajor>0</BuildMajor>
        <BuildMinor>0</BuildMinor>
      </Version>
    </Build>
    <LangID>en</LangID>
    <PartNumber>000-00000-00</PartNumber>
  </Author>
</TrainingCenterDatabase>
"#,
        distance = fixed(total_meters, 1),
        max_speed = fixed(record.max_speed_kmh / 3.6, 3),
        calories = record.calories,
        device = escape_xml(DEVICE_NAME),
    )
}

fn push_trackpoint(out: &mut String, start: Timestamp, elapsed: u32, meters: f64, duration: u32) {
    let fraction = if duration == 0 {
        1.0
    } else {
        f64::from(elapsed) / f64::from(duration)
    };
    let time = start + SignedDuration::from_secs(i64::from(elapsed));
    let _ = write!(
        out,
        "          <Trackpoint>\n            <Time>{time}</Time>\n            \
         <DistanceMeters>{}</DistanceMeters>\n          </Trackpoint>\n",
        fixed(meters * fraction, 1)
    );
}

fn whole_seconds(ts: Timestamp) -> Timestamp {
    Timestamp::from_second(ts.as_second()).expect("whole-second timestamp")
}

/// Rounds half up to a fixed number of decimals, as the Kotlin version did.
fn fixed(value: f64, decimals: u32) -> String {
    let scale = 10_i64.pow(decimals);
    let scaled = (value * scale as f64 + 0.5) as i64;
    if decimals == 0 {
        return scaled.to_string();
    }
    format!(
        "{}.{:0width$}",
        scaled / scale,
        scaled % scale,
        width = decimals as usize
    )
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// File name used for both the dry-run file and the upload.
pub fn file_stem(record: &SessionRecord) -> String {
    format!("cli-walker-{}", record.started_at.as_second())
}

/// Writes the TCX for each session to `tcx/` in the data directory, for
/// `strava sync --dry-run` to show exactly what would be uploaded.
pub fn write_tcx_files(dir: &DataDir, records: &[SessionRecord]) -> Result<PathBuf> {
    let out = dir.file("tcx");
    create_private_dir(&out)?;
    for record in records {
        fs::write(
            out.join(format!("{}.tcx", file_stem(record))),
            build_tcx(record),
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quick_xml::Reader;
    use quick_xml::events::Event;

    fn record() -> SessionRecord {
        SessionRecord {
            started_at: "2026-10-21T19:43:05.512Z".parse().unwrap(),
            ended_at: "2026-10-21T19:44:15Z".parse().unwrap(),
            duration_seconds: 70,
            distance_km: 0.0875,
            steps: 134,
            calories: 4,
            avg_speed_kmh: 4.5,
            max_speed_kmh: 4.8,
        }
    }

    /// Text of every element with the given name, in document order.
    fn texts(xml: &str, name: &str) -> Vec<String> {
        let mut reader = Reader::from_str(xml);
        let mut found = Vec::new();
        let mut inside = false;
        loop {
            match reader.read_event().expect("well-formed XML") {
                Event::Start(e) if AsRef::<str>::as_ref(&e.name()) == name => inside = true,
                Event::Text(t) if inside => {
                    found.push(t.into_inner().into_owned());
                    inside = false;
                }
                Event::End(_) => inside = false,
                Event::Eof => break,
                _ => {}
            }
        }
        found
    }

    #[test]
    fn synthesises_a_track_every_30_seconds_ending_on_the_session_end() {
        let xml = build_tcx(&record());
        assert_eq!(
            texts(&xml, "Time"),
            [
                "2026-10-21T19:43:05Z",
                "2026-10-21T19:43:35Z",
                "2026-10-21T19:44:05Z",
                "2026-10-21T19:44:15Z"
            ]
        );
        // Lap total first, then the interpolated trackpoint distances.
        assert_eq!(
            texts(&xml, "DistanceMeters"),
            ["87.5", "0.0", "37.5", "75.0", "87.5"]
        );
    }

    #[test]
    fn carries_the_lap_totals() {
        let xml = build_tcx(&record());
        assert_eq!(texts(&xml, "Id"), ["2026-10-21T19:43:05Z"]);
        assert_eq!(texts(&xml, "TotalTimeSeconds"), ["70"]);
        assert_eq!(texts(&xml, "MaximumSpeed"), ["1.333"]);
        assert_eq!(texts(&xml, "Calories"), ["4"]);
        assert!(xml.contains(r#"<Activity Sport="Other">"#));
        assert!(xml.contains(r#"<Lap StartTime="2026-10-21T19:43:05Z">"#));
    }

    #[test]
    fn zero_length_session_still_has_one_trackpoint() {
        let mut r = record();
        r.duration_seconds = 0;
        assert_eq!(texts(&build_tcx(&r), "Time"), ["2026-10-21T19:43:05Z"]);
    }

    #[test]
    fn rounds_half_up() {
        assert_eq!(fixed(37.45, 1), "37.5");
        assert_eq!(fixed(1.333_333_3, 3), "1.333");
        assert_eq!(fixed(2.0, 0), "2");
    }

    #[test]
    fn writes_dry_run_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::new(tmp.path());
        let out = write_tcx_files(&dir, &[record()]).unwrap();
        let written = fs::read_to_string(out.join("cli-walker-1792611785.tcx")).unwrap();
        assert_eq!(written, build_tcx(&record()));
    }

    /// Validates against Garmin's schema when one is provided. CI downloads it and installs
    /// xmllint; locally: `TCX_XSD=/path/to/TrainingCenterDatabasev2.xsd cargo test`.
    #[test]
    fn validates_against_the_garmin_schema() {
        let Ok(xsd) = std::env::var("TCX_XSD") else {
            eprintln!("TCX_XSD not set; skipping schema validation");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("session.tcx");
        fs::write(&file, build_tcx(&record())).unwrap();
        let output = std::process::Command::new("xmllint")
            .args(["--noout", "--schema", &xsd])
            .arg(&file)
            .output()
            .expect("run xmllint");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
