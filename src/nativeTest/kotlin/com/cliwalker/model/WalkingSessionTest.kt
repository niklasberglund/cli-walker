package com.cliwalker.model

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class WalkingSessionTest {

    private fun session(state: BeltState, seconds: Int) = WalkingSession(state = state, elapsedSeconds = seconds)

    @Test
    fun noAutoStopWarningBeforeWindow() {
        assertNull(session(BeltState.Running, MAX_RUN_SECONDS - AUTO_STOP_WARNING_SECONDS - 1).secondsUntilAutoStop)
    }

    @Test
    fun autoStopWarningCountsDownInWindow() {
        assertEquals(AUTO_STOP_WARNING_SECONDS, session(BeltState.Running, 6900).secondsUntilAutoStop)
        assertEquals(1, session(BeltState.Running, MAX_RUN_SECONDS - 1).secondsUntilAutoStop)
        assertEquals(0, session(BeltState.Running, MAX_RUN_SECONDS + 3).secondsUntilAutoStop)
    }

    @Test
    fun noAutoStopWarningWhenBeltStopped() {
        assertNull(session(BeltState.Idle, 7000).secondsUntilAutoStop)
    }
}
