package com.cliwalker.strava

import com.cliwalker.model.SessionRecord
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.convert
import kotlinx.cinterop.toKString
import kotlinx.datetime.Instant
import platform.posix.S_IRWXU
import platform.posix.fclose
import platform.posix.fflush
import platform.posix.fopen
import platform.posix.fputs
import platform.posix.getenv
import platform.posix.mkdir

/** Reported to Strava as the recording device, so activities are attributed to the pad. */
const val DEVICE_NAME = "WalkingPad A1 Pro"

/** Seconds between synthesised trackpoints. */
private const val TRACKPOINT_INTERVAL = 30

/**
 * Renders a session as a Garmin TCX document.
 *
 * The pad only reports totals, never a per-second stream, so the track is synthesised by
 * interpolating distance linearly across the session. That is enough for Strava to accept
 * the file, attribute the activity to this app, and draw a distance/pace graph.
 *
 * `Sport` is `Other` because TCX only allows Running, Biking and Other — the activity is
 * corrected to Walk with a follow-up update once Strava has processed the upload.
 *
 * The `Creator` and `Author` blocks must validate against the Garmin schema in full —
 * `Device_t` requires `Version`, `Application_t` requires `Build` — or Strava silently
 * drops them and the activity arrives with no device name and no app attribution.
 */
fun buildTcx(record: SessionRecord): String {
    val start = Instant.fromEpochSeconds(record.startedAt.epochSeconds)
    val startStamp = start.toString()
    val totalMeters = record.distanceKm * 1000

    val trackpoints = buildString {
        var elapsed = 0
        while (elapsed < record.durationSeconds) {
            appendTrackpoint(start, elapsed, totalMeters, record.durationSeconds)
            elapsed += TRACKPOINT_INTERVAL
        }
        // Always close the track on the true end of the session, whatever the interval.
        appendTrackpoint(start, record.durationSeconds, totalMeters, record.durationSeconds)
    }

    return """
        <?xml version="1.0" encoding="UTF-8"?>
        <TrainingCenterDatabase
            xmlns="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2"
            xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
            xsi:schemaLocation="http://www.garmin.com/xmlschemas/TrainingCenterDatabase/v2 http://www.garmin.com/xmlschemas/TrainingCenterDatabasev2.xsd">
          <Activities>
            <Activity Sport="Other">
              <Id>$startStamp</Id>
              <Lap StartTime="$startStamp">
                <TotalTimeSeconds>${record.durationSeconds}</TotalTimeSeconds>
                <DistanceMeters>${totalMeters.format(1)}</DistanceMeters>
                <MaximumSpeed>${(record.maxSpeedKmh / 3.6).format(3)}</MaximumSpeed>
                <Calories>${record.calories}</Calories>
                <Intensity>Active</Intensity>
                <TriggerMethod>Manual</TriggerMethod>
                <Track>
        $trackpoints
                </Track>
              </Lap>
              <Creator xsi:type="Device_t">
                <Name>${escapeXml(DEVICE_NAME)}</Name>
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
    """.trimIndent()
}

private fun StringBuilder.appendTrackpoint(
    start: Instant,
    elapsed: Int,
    totalMeters: Double,
    durationSeconds: Int,
) {
    val fraction = if (durationSeconds <= 0) 1.0 else elapsed.toDouble() / durationSeconds
    val time = Instant.fromEpochSeconds(start.epochSeconds + elapsed)
    append("          <Trackpoint>\n")
    append("            <Time>$time</Time>\n")
    append("            <DistanceMeters>${(totalMeters * fraction).format(1)}</DistanceMeters>\n")
    append("          </Trackpoint>\n")
}

/** Kotlin/Native has no String.format(), so decimals are rendered by hand. */
private fun Double.format(decimals: Int): String {
    var scale = 1L
    repeat(decimals) { scale *= 10 }
    val scaled = (this * scale + 0.5).toLong()
    val whole = scaled / scale
    val fraction = (scaled % scale).toString().padStart(decimals, '0')
    return if (decimals == 0) "$whole" else "$whole.$fraction"
}

private fun escapeXml(value: String): String = value
    .replace("&", "&amp;")
    .replace("<", "&lt;")
    .replace(">", "&gt;")
    .replace("\"", "&quot;")
    .replace("'", "&apos;")

/**
 * Writes the TCX for each session to ~/.cli-walker/tcx/ and returns the directory.
 * Used by `strava sync --dry-run` to inspect exactly what would be uploaded.
 */
@OptIn(ExperimentalForeignApi::class)
fun writeTcxFiles(records: List<SessionRecord>): String {
    val home = getenv("HOME")?.toKString() ?: "."
    val dir = "$home/.cli-walker/tcx"
    mkdir("$home/.cli-walker", S_IRWXU.convert())
    mkdir(dir, S_IRWXU.convert())

    for (record in records) {
        val path = "$dir/cli-walker-${record.startedAt.epochSeconds}.tcx"
        val file = fopen(path, "w") ?: continue
        try {
            fputs(buildTcx(record), file)
            fflush(file)
        } finally {
            fclose(file)
        }
    }
    return dir
}
