package com.cliwalker.demo

import com.cliwalker.ble.ConnectionState
import com.cliwalker.ble.PadConnection
import com.cliwalker.model.BeltState
import com.cliwalker.model.SessionRecord
import com.cliwalker.model.WalkingSession
import com.cliwalker.model.caloriesFor
import com.cliwalker.storage.SessionStore
import com.cliwalker.strava.StravaConfig
import com.cliwalker.strava.StravaConfigStore
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.cstr
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.toKString
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.datetime.Clock
import kotlinx.datetime.Instant
import platform.posix.mkdtemp
import platform.posix.setenv
import kotlin.random.Random
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.hours
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Duration.Companion.seconds

/** Average stride in metres, used to turn simulated distance into steps. */
private const val STRIDE_M = 0.65

/**
 * A simulated WalkingPad for screenshots and recordings. It joins a walk already in
 * progress so the dashboard shows realistic totals from the first frame.
 */
class DemoPadConnection(private val scope: CoroutineScope) : PadConnection {
    private val _session = MutableStateFlow(WalkingSession())
    override val session: StateFlow<WalkingSession> = _session.asStateFlow()

    private val _connectionState = MutableStateFlow(ConnectionState.Disconnected)
    override val connectionState: StateFlow<ConnectionState> = _connectionState.asStateFlow()

    private var tickJob: Job? = null
    private var speed = 0.0
    private var distanceKm = 1.38
    private var elapsedSeconds = 27 * 60 + 41
    private var stopped = false

    override suspend fun connect() {
        _connectionState.value = ConnectionState.Scanning
        delay(800)
        _connectionState.value = ConnectionState.Connecting
        delay(500)
        _connectionState.value = ConnectionState.Connected
        tickJob = scope.launch {
            while (isActive) {
                delay(1000)
                if (speed > 0) {
                    elapsedSeconds++
                    distanceKm += speed / 3600
                }
                publish()
            }
        }
    }

    override suspend fun startBelt(speedKmh: Double) {
        // Like the real pad, a new belt run counts up from zero.
        if (stopped) {
            distanceKm = 0.0
            elapsedSeconds = 0
            stopped = false
        }
        speed = speedKmh
        publish()
    }

    override suspend fun stopBelt() {
        speed = 0.0
        stopped = true
        publish()
    }

    override suspend fun setSpeed(speedKmh: Double) {
        if (speed > 0) speed = speedKmh.coerceIn(0.5, 6.0)
        publish()
    }

    override suspend fun disconnect() {
        tickJob?.cancel()
        _connectionState.value = ConnectionState.Disconnected
    }

    private fun publish() {
        _session.value = WalkingSession(
            state = if (speed > 0) BeltState.Running else BeltState.Idle,
            speed = speed,
            distanceKm = distanceKm,
            steps = (distanceKm * 1000 / STRIDE_M).toInt(),
            elapsedSeconds = elapsedSeconds,
            startedAt = _session.value.startedAt ?: Clock.System.now(),
        )
    }
}

/**
 * Points HOME at a fresh temporary directory seeded with sample history and a fake Strava
 * login, so demo mode exercises the real storage code without touching the user's data.
 * Must run before any store is constructed, since they read HOME on creation.
 */
@OptIn(ExperimentalForeignApi::class)
fun prepareDemoHome() {
    val home = memScoped {
        mkdtemp("/tmp/cli-walker-demo.XXXXXX".cstr.getPointer(this))?.toKString()
    } ?: error("Cannot create demo directory")
    setenv("HOME", home, 1)

    val store = SessionStore()
    for (record in sampleHistory(Clock.System.now())) store.append(record)

    StravaConfigStore().save(
        StravaConfig(
            clientId = "demo",
            clientSecret = "demo",
            accessToken = "demo",
            refreshToken = "demo",
            athleteName = "Demo Walker",
            minUploadDistanceKm = 1.0,
        ),
    )
}

/**
 * About six weeks of plausible walks, one most days. The last is from earlier today, so
 * "This Month" is never empty.
 */
private fun sampleHistory(now: Instant): List<SessionRecord> {
    val random = Random(42)
    return (42 downTo 0).mapNotNull { daysAgo ->
        if (daysAgo > 0 && random.nextInt(10) < 3) return@mapNotNull null
        val start = now - daysAgo.days - 3.hours - random.nextInt(0, 120).minutes
        val avgSpeed = 3.0 + random.nextInt(0, 16) / 10.0
        val duration = random.nextInt(20, 75).minutes + random.nextInt(0, 60).seconds
        val seconds = duration.inWholeSeconds.toInt()
        val distance = avgSpeed * seconds / 3600
        SessionRecord(
            startedAt = start,
            endedAt = start + duration,
            durationSeconds = seconds,
            distanceKm = distance,
            steps = (distance * 1000 / STRIDE_M).toInt(),
            calories = caloriesFor(seconds),
            avgSpeedKmh = avgSpeed,
            maxSpeedKmh = avgSpeed + random.nextInt(2, 8) / 10.0,
        )
    }
}
