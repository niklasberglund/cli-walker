package com.cliwalker.model

import kotlinx.datetime.Instant
import kotlin.time.Duration
import kotlin.time.Duration.Companion.minutes

/**
 * Turns the pad's status samples into discrete session records.
 *
 * The pad zeroes its counters (time/distance/steps) whenever the belt stops, so each
 * belt run is observed as a fresh climb from zero. Runs separated by less than
 * [coalesceWindow] are folded into one record — stepping off the belt for a moment
 * (or letting it auto-stop) should not split a walk into fragments that each fall
 * below the Strava upload threshold.
 */
class SessionTracker(private val coalesceWindow: Duration = 1.minutes) {

    private var startedAt: Instant? = null
    private var stoppedAt: Instant? = null
    private var maxSpeed = 0.0
    private var baseDistanceKm = 0.0
    private var baseSteps = 0
    private var baseSeconds = 0

    /**
     * Highest-elapsed sample of the belt run in progress. Counters grow monotonically
     * while running, so this sample holds that run's final totals once it ends.
     */
    private var peak: WalkingSession? = null

    /** Feeds a status sample, returning a record if it closed a session. */
    fun onSample(sample: WalkingSession, now: Instant): SessionRecord? {
        if (sample.isActive) {
            if (startedAt == null) startedAt = now
            // Restarted inside the coalesce window: bank the run that just ended, since
            // the counters we are about to see start over from zero.
            if (stoppedAt != null) {
                bankRun()
                stoppedAt = null
            }
            if (sample.speed > maxSpeed) maxSpeed = sample.speed
            if (sample.elapsedSeconds >= (peak?.elapsedSeconds ?: 0)) peak = sample
        } else if (startedAt != null && stoppedAt == null) {
            stoppedAt = now
        }
        return tick(now)
    }

    /**
     * Closes the session once the belt has been stopped for longer than the coalesce
     * window. Needed as a separate entry point because the status flow goes quiet while
     * the pad sits idle, reporting the same values over and over.
     */
    fun tick(now: Instant): SessionRecord? {
        val stopped = stoppedAt ?: return null
        if (now - stopped < coalesceWindow) return null
        return close(stopped)
    }

    /** Closes whatever is in flight, for use when the app exits. */
    fun finish(now: Instant): SessionRecord? = close(stoppedAt ?: now)

    private fun close(endedAt: Instant): SessionRecord? {
        bankRun()
        val start = startedAt
        val record = if (start != null && baseSeconds > 0 && baseDistanceKm > 0.0) {
            SessionRecord(
                startedAt = start,
                endedAt = endedAt,
                durationSeconds = baseSeconds,
                distanceKm = baseDistanceKm,
                steps = baseSteps,
                calories = caloriesFor(baseSeconds),
                avgSpeedKmh = baseDistanceKm / (baseSeconds / 3600.0),
                maxSpeedKmh = maxSpeed,
            )
        } else {
            null
        }
        startedAt = null
        stoppedAt = null
        maxSpeed = 0.0
        baseDistanceKm = 0.0
        baseSteps = 0
        baseSeconds = 0
        peak = null
        return record
    }

    private fun bankRun() {
        val p = peak ?: return
        baseDistanceKm += p.distanceKm
        baseSteps += p.steps
        baseSeconds += p.elapsedSeconds
        peak = null
    }
}
