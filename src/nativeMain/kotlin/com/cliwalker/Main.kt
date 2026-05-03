package com.cliwalker

import com.cliwalker.ble.WalkingPadConnection
import com.cliwalker.model.SessionRecord
import com.cliwalker.storage.SessionStore
import com.cliwalker.tui.Dashboard
import com.cliwalker.tui.KeyEvent
import com.cliwalker.tui.KeyboardInput
import com.cliwalker.tui.Tab
import kotlinx.coroutines.flow.MutableStateFlow
import com.github.ajalt.clikt.core.CliktCommand
import com.github.ajalt.clikt.core.main
import com.github.ajalt.clikt.parameters.options.default
import com.github.ajalt.clikt.parameters.options.flag
import com.github.ajalt.clikt.parameters.options.option
import com.github.ajalt.clikt.parameters.types.double
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.terminal.Terminal
import kotlinx.coroutines.*
import kotlinx.datetime.Clock
import kotlinx.datetime.Instant

class CliWalker : CliktCommand(
    name = "cli-walker",
) {
    private val startSpeed by option("--speed", help = "Starting speed in km/h")
        .double()
        .default(2.5)

    private val debug by option("--debug", help = "Enable verbose BLE packet logging")
        .flag()

    private val speedStep = 0.5
    private val fineSpeedStep = 0.1

    override fun run() = runBlocking {
        val terminal = Terminal()
        val connection = WalkingPadConnection(this, debug = debug)
        val activeTab = MutableStateFlow(Tab.Session)
        val sessionStore = SessionStore()
        val history = sessionStore.load()
        val dashboard = Dashboard(terminal, connection.session, connection.connectionState, activeTab, history)
        val quit = CompletableDeferred<Unit>()
        var connectionError: String? = null
        var currentSpeed = startSpeed
        var sessionStartedAt: Instant? = null
        var maxSpeed = 0.0

        val dashboardJob = launch { dashboard.run(this) }

        val trackerJob = launch {
            connection.session.collect { s ->
                if (s.isActive && sessionStartedAt == null) {
                    sessionStartedAt = Clock.System.now()
                }
                if (s.speed > maxSpeed) maxSpeed = s.speed
            }
        }

        val keyboard = KeyboardInput()
        val inputJob = launch {
            keyboard.events().collect { event ->
                when (event) {
                    KeyEvent.ArrowUp -> {
                        currentSpeed = (currentSpeed + speedStep).coerceAtMost(6.0)
                        connection.setSpeed(currentSpeed)
                    }
                    KeyEvent.ArrowDown -> {
                        currentSpeed = (currentSpeed - speedStep).coerceAtLeast(0.5)
                        connection.setSpeed(currentSpeed)
                    }
                    KeyEvent.CtrlArrowUp -> {
                        currentSpeed = (currentSpeed + fineSpeedStep).coerceAtMost(6.0)
                        connection.setSpeed(currentSpeed)
                    }
                    KeyEvent.CtrlArrowDown -> {
                        currentSpeed = (currentSpeed - fineSpeedStep).coerceAtLeast(0.5)
                        connection.setSpeed(currentSpeed)
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
                        try { connection.stopBelt() } catch (_: Exception) {}
                        quit.complete(Unit)
                    }
                }
            }
        }

        launch {
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
            connection.disconnect()
            print("\u001B[?25h") // Restore cursor
            connectionError?.let { println(red("Connection error: $it")) }
            persistSession(sessionStore, connection, sessionStartedAt, maxSpeed)
            printSessionSummary(terminal, connection)
        }
    }

    private fun persistSession(
        store: SessionStore,
        connection: WalkingPadConnection,
        startedAt: Instant?,
        maxSpeed: Double,
    ) {
        val s = connection.session.value
        if (startedAt == null || s.elapsedSeconds <= 0 || s.distanceKm <= 0.0) return
        val avgSpeed = s.distanceKm / (s.elapsedSeconds / 3600.0)
        val record = SessionRecord(
            startedAt = startedAt,
            endedAt = Clock.System.now(),
            durationSeconds = s.elapsedSeconds,
            distanceKm = s.distanceKm,
            steps = s.steps,
            calories = s.caloriesEstimate,
            avgSpeedKmh = avgSpeed,
            maxSpeedKmh = maxSpeed,
        )
        try {
            store.append(record)
        } catch (_: Throwable) {
            // best-effort persistence; don't fail on write error
        }
    }

    private fun printSessionSummary(terminal: Terminal, connection: WalkingPadConnection) {
        val session = connection.session.value
        if (session.elapsedSeconds > 0) {
            println()
            println(green(bold("Session Summary")))
            println("  Distance: ${session.formattedDistance}")
            println("  Steps:    ${session.steps}")
            println("  Time:     ${session.formattedTime}")
            println("  Calories: ~${session.caloriesEstimate} kcal")
        }
    }
}

fun main(args: Array<String>) = CliWalker().main(args)
