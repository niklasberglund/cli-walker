package com.cliwalker.model

import kotlinx.datetime.Instant
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.time.Duration.Companion.seconds

private val epoch = Instant.fromEpochSeconds(1_700_000_000)

private fun running(seconds: Int, km: Double, steps: Int, speed: Double = 3.0) = WalkingSession(
    state = BeltState.Running,
    speed = speed,
    distanceKm = km,
    steps = steps,
    elapsedSeconds = seconds,
)

private val idle = WalkingSession(state = BeltState.Idle)

class SessionTrackerTest {

    /** Walks the belt for [seconds] seconds starting at [from], one sample per second. */
    private fun SessionTracker.walk(from: Int, seconds: Int, speed: Double = 3.0): SessionRecord? {
        var closed: SessionRecord? = null
        for (i in 1..seconds) {
            val sample = running(i, i * 0.001, i, speed)
            onSample(sample, epoch + (from + i).seconds)?.let { closed = it }
        }
        return closed
    }

    private fun SessionTracker.idleFor(from: Int, seconds: Int): SessionRecord? {
        var closed: SessionRecord? = null
        for (i in 1..seconds) {
            onSample(idle, epoch + (from + i).seconds)?.let { closed = it }
        }
        return closed
    }

    @Test
    fun closes_a_session_once_the_belt_has_been_stopped_past_the_window() {
        val tracker = SessionTracker(coalesceWindow = 60.seconds)
        assertNull(tracker.walk(from = 0, seconds = 100))
        assertNull(tracker.idleFor(from = 100, seconds = 59))

        val record = tracker.idleFor(from = 159, seconds = 5)
        assertEquals(100, record?.durationSeconds)
        assertEquals(0.1, record?.distanceKm)
        assertEquals(100, record?.steps)
    }

    @Test
    fun folds_a_restart_inside_the_window_into_one_session() {
        val tracker = SessionTracker(coalesceWindow = 60.seconds)
        tracker.walk(from = 0, seconds = 100)
        tracker.idleFor(from = 100, seconds = 30)
        // Second run is shorter than the first: the old peak-only logic dropped it.
        assertNull(tracker.walk(from = 130, seconds = 40))

        val record = tracker.finish(epoch + 200.seconds)
        assertEquals(140, record?.durationSeconds)
        assertEquals(140, record?.steps)
        assertEquals(0.14, record?.distanceKm)
    }

    @Test
    fun splits_a_restart_outside_the_window_into_separate_sessions() {
        val tracker = SessionTracker(coalesceWindow = 60.seconds)
        tracker.walk(from = 0, seconds = 100)
        val first = tracker.idleFor(from = 100, seconds = 70)
        assertEquals(100, first?.durationSeconds)

        tracker.walk(from = 200, seconds = 40)
        val second = tracker.finish(epoch + 250.seconds)
        assertEquals(40, second?.durationSeconds)
        assertEquals(40, second?.steps)
    }

    @Test
    fun tracks_max_speed_across_folded_runs_and_resets_it_after() {
        val tracker = SessionTracker(coalesceWindow = 60.seconds)
        tracker.walk(from = 0, seconds = 10, speed = 5.5)
        tracker.idleFor(from = 10, seconds = 10)
        tracker.walk(from = 20, seconds = 10, speed = 2.0)
        assertEquals(5.5, tracker.finish(epoch + 40.seconds)?.maxSpeedKmh)

        tracker.walk(from = 100, seconds = 10, speed = 2.0)
        assertEquals(2.0, tracker.finish(epoch + 120.seconds)?.maxSpeedKmh)
    }

    @Test
    fun ignores_a_belt_that_never_moved() {
        val tracker = SessionTracker(coalesceWindow = 60.seconds)
        tracker.idleFor(from = 0, seconds = 120)
        assertNull(tracker.finish(epoch + 130.seconds))
    }
}
