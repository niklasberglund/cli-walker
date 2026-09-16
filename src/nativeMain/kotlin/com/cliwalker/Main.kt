package com.cliwalker

import com.cliwalker.ble.WalkingPadConnection
import com.cliwalker.model.SessionRecord
import com.cliwalker.model.SessionTracker
import com.cliwalker.model.formatDuration
import com.cliwalker.storage.SessionStore
import com.cliwalker.strava.StravaService
import com.cliwalker.strava.UploadResult
import com.cliwalker.strava.formatThreshold
import com.cliwalker.tui.Dashboard
import com.cliwalker.tui.KeyEvent
import com.cliwalker.tui.KeyboardInput
import com.cliwalker.tui.StravaUiState
import com.cliwalker.tui.Tab
import com.cliwalker.tui.printLogo
import kotlinx.coroutines.flow.MutableStateFlow
import com.github.ajalt.clikt.core.CliktCommand
import com.github.ajalt.clikt.core.main
import com.github.ajalt.clikt.core.subcommands
import com.github.ajalt.clikt.parameters.options.default
import com.github.ajalt.clikt.parameters.options.flag
import com.github.ajalt.clikt.parameters.options.option
import com.github.ajalt.clikt.parameters.types.double
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.terminal.Terminal
import kotlinx.coroutines.*
import kotlinx.datetime.Clock

class CliWalker : CliktCommand(
    name = "cli-walker",
) {
    private val startSpeed by option("--speed", help = "Starting speed in km/h")
        .double()
        .default(2.5)

    private val debug by option("--debug", help = "Enable verbose BLE packet logging")
        .flag()

    private val noStrava by option("--no-strava", help = "Do not upload this session to Strava")
        .flag()

    // Registering subcommands would normally make one mandatory; the bare `cli-walker`
    // invocation must keep launching the dashboard.
    override val invokeWithoutSubcommand = true

    private val speedStep = 0.5
    private val fineSpeedStep = 0.1

    // Threshold adjustment steps, in km (100 m coarse, 10 m fine).
    private val thresholdStep = 0.1
    private val fineThresholdStep = 0.01

    override fun run() {
        // The root command doubles as the walking dashboard, so only run it when the
        // user did not ask for a subcommand like `strava login`.
        if (currentContext.invokedSubcommand != null) return
        runBlocking { walk() }
    }

    private suspend fun walk() = coroutineScope {
        print("[2J[H") // Clear screen and move cursor to top
        println()
        printLogo()
        val terminal = Terminal()
        val connection = WalkingPadConnection(this, debug = debug)
        val activeTab = MutableStateFlow(Tab.Session)
        val sessionStore = SessionStore()
        val history = sessionStore.load()
        val strava = StravaService()
        val stravaState = MutableStateFlow(readStravaState(strava, history))
        val dashboard = Dashboard(
            terminal,
            connection.session,
            connection.connectionState,
            activeTab,
            history,
            stravaState,
        )
        val quit = CompletableDeferred<Unit>()
        var connectionError: String? = null
        var currentSpeed = startSpeed
        val tracker = SessionTracker()
        val recorded = mutableListOf<SessionRecord>()

        // Persisted as each session closes rather than only on exit, so a crash or an
        // unplugged terminal cannot lose a walk that already finished.
        fun record(session: SessionRecord?) {
            if (session == null) return
            recorded.add(session)
            try {
                sessionStore.append(session)
            } catch (_: Throwable) {
                // best-effort persistence; don't fail on write error
            }
        }

        val dashboardJob = launch { dashboard.run(this) }

        val trackerJob = launch {
            connection.session.collect { s -> record(tracker.onSample(s, Clock.System.now())) }
        }

        // The status flow goes quiet when the pad idles at unchanging values, so the
        // coalesce window has to be checked on a timer rather than on samples alone.
        val tickJob = launch {
            while (isActive) {
                delay(1000)
                record(tracker.tick(Clock.System.now()))
            }
        }

        val keyboard = KeyboardInput()
        val inputJob = launch {
            keyboard.events().collect { event ->
                when (event) {
                    // On the Strava tab the up/down keys edit the upload threshold instead
                    // of the belt speed, so settings can be changed without touching the pad.
                    KeyEvent.ArrowUp -> {
                        if (activeTab.value == Tab.Strava) {
                            adjustThreshold(strava, stravaState, history, thresholdStep)
                        } else {
                            currentSpeed = (currentSpeed + speedStep).coerceAtMost(6.0)
                            connection.setSpeed(currentSpeed)
                        }
                    }
                    KeyEvent.ArrowDown -> {
                        if (activeTab.value == Tab.Strava) {
                            adjustThreshold(strava, stravaState, history, -thresholdStep)
                        } else {
                            currentSpeed = (currentSpeed - speedStep).coerceAtLeast(0.5)
                            connection.setSpeed(currentSpeed)
                        }
                    }
                    KeyEvent.CtrlArrowUp -> {
                        if (activeTab.value == Tab.Strava) {
                            adjustThreshold(strava, stravaState, history, fineThresholdStep)
                        } else {
                            currentSpeed = (currentSpeed + fineSpeedStep).coerceAtMost(6.0)
                            connection.setSpeed(currentSpeed)
                        }
                    }
                    KeyEvent.CtrlArrowDown -> {
                        if (activeTab.value == Tab.Strava) {
                            adjustThreshold(strava, stravaState, history, -fineThresholdStep)
                        } else {
                            currentSpeed = (currentSpeed - fineSpeedStep).coerceAtLeast(0.5)
                            connection.setSpeed(currentSpeed)
                        }
                    }
                    KeyEvent.ToggleMute -> {
                        if (activeTab.value == Tab.Strava) {
                            val muted = !stravaState.value.muted
                            if (strava.setHideFromFeed(muted)) {
                                stravaState.value = stravaState.value.copy(muted = muted)
                            }
                        }
                    }
                    KeyEvent.Space -> {
                        if (connection.session.value.isActive) {
                            connection.stopBelt()
                        } else {
                            connection.startBelt(currentSpeed)
                        }
                    }
                    KeyEvent.Tab, KeyEvent.ArrowRight -> {
                        activeTab.value = activeTab.value.next()
                    }
                    KeyEvent.ArrowLeft -> {
                        activeTab.value = activeTab.value.previous()
                    }
                    KeyEvent.Quit -> {
                        try {
                            connection.stopBelt()
                            // stopBelt writes with WriteType.WithoutResponse, so give
                            // CoreBluetooth time to actually transmit the stop packet
                            // before the connection is torn down on exit.
                            delay(1000)
                        } catch (_: Exception) {}
                        quit.complete(Unit)
                    }
                }
            }
        }

        val connectJob = launch {
            try {
                connection.connect()
                connection.startBelt(startSpeed)
            } catch (e: Exception) {
                if (e !is CancellationException) {
                    connectionError = e.message
                }
            }
        }

        try {
            quit.await()
        } finally {
            inputJob.cancel()
            dashboardJob.cancel()
            trackerJob.cancel()
            tickJob.cancel()
            connectJob.cancel()
            connection.disconnect()
            print("\u001B[?25h") // Restore cursor
            println()
            connectionError?.let { println(red("Connection error: $it")) }
            record(tracker.finish(Clock.System.now()))
            printSessionSummary(recorded)
            if (!noStrava) {
                for (session in recorded) uploadToStrava(strava, session)
            }
        }
    }

    private suspend fun uploadToStrava(service: StravaService, record: SessionRecord) {
        if (!service.isConnected()) return
        val threshold = service.minUploadDistanceKm()
        if (record.distanceKm < threshold) {
            println()
            println(
                yellow("Not uploaded to Strava: ") +
                    "${roundKm(record.distanceKm)} km is below the ${formatThreshold(threshold)} threshold." ,
            )
            println(dim("  Upload it anyway with `cli-walker strava sync --all`."))
            return
        }
        println()
        println(dim("Uploading to Strava…"))
        when (val result = service.upload(record)) {
            is UploadResult.Uploaded -> {
                val id = result.activityId
                if (id != null) {
                    println(green("Uploaded to Strava: https://www.strava.com/activities/$id"))
                } else {
                    println(green("Uploaded to Strava.") + dim(" (still processing)"))
                }
                result.warning?.let { println(yellow("  ! $it")) }
            }
            is UploadResult.Failed ->
                println(yellow("Strava upload failed: ${result.reason}") + dim(" (retry with `cli-walker strava sync`)"))
            is UploadResult.TooShort ->
                println(
                    yellow("Not uploaded to Strava: ") +
                        "${roundKm(result.distanceKm)} km is below the ${formatThreshold(result.thresholdKm)} threshold.",
                )
            UploadResult.AlreadyUploaded, UploadResult.NotConnected -> Unit
        }
    }

    private fun printSessionSummary(sessions: List<SessionRecord>) {
        if (sessions.isEmpty()) return
        val many = sessions.size > 1
        println()
        println(green(bold(if (many) "Session Summary (${sessions.size} sessions)" else "Session Summary")))
        val indent = if (many) "    " else "  "
        sessions.forEachIndexed { index, s ->
            if (many) println("  " + bold("Session ${index + 1}"))
            println("${indent}Distance: ${roundKm(s.distanceKm)} km")
            println("${indent}Steps:    ${s.steps}")
            println("${indent}Time:     ${formatDuration(s.durationSeconds)}")
            println("${indent}Calories: ~${s.calories} kcal")
        }
        if (many) {
            val distance = roundKm(sessions.sumOf { it.distanceKm })
            val time = formatDuration(sessions.sumOf { it.durationSeconds })
            println("  " + dim("Total: $distance km, $time, ${sessions.sumOf { it.steps }} steps"))
        }
    }
}

/** Snapshot of the Strava settings and upload backlog for the dashboard's Strava tab. */
private fun readStravaState(service: StravaService, history: List<SessionRecord>): StravaUiState {
    val config = service.config()
    val pending = service.pending(history)
    return StravaUiState(
        connected = config?.isAuthorized == true,
        athleteName = config?.athleteName,
        muted = config?.hideFromFeed == true,
        thresholdKm = config?.minUploadDistanceKm ?: 0.0,
        pendingCount = pending.size,
        belowThresholdCount = service.pending(history, ignoreThreshold = true).size - pending.size,
    )
}

/** Nudges the upload threshold, persists it, and recomputes the backlog it affects. */
private fun adjustThreshold(
    service: StravaService,
    state: MutableStateFlow<StravaUiState>,
    history: List<SessionRecord>,
    deltaKm: Double,
) {
    // Round to whole 10 m so repeated floating-point steps never drift to 0.30000000000000004.
    val meters = ((state.value.thresholdKm + deltaKm) * 1000).let { kotlin.math.round(it / 10) * 10 }
    val km = (meters / 1000).coerceIn(0.0, 42.0)
    if (!service.setMinUploadDistanceKm(km)) return
    val pending = service.pending(history)
    state.value = state.value.copy(
        thresholdKm = km,
        pendingCount = pending.size,
        belowThresholdCount = service.pending(history, ignoreThreshold = true).size - pending.size,
    )
}

private fun roundKm(km: Double): String {
    val hundredths = (km * 100).toInt()
    val frac = hundredths % 100
    return "${hundredths / 100}.${frac.toString().padStart(2, '0')}"
}

fun main(args: Array<String>) = CliWalker()
    .subcommands(
        StravaCommand().subcommands(
            StravaLogin(),
            StravaStatus(),
            StravaSync(),
            StravaMute(),
            StravaThreshold(),
            StravaLogout(),
        ),
    )
    .main(args)
