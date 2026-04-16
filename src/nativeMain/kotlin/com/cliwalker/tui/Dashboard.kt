package com.cliwalker.tui

import com.cliwalker.ble.WalkingPadConnection.ConnectionState
import com.cliwalker.model.WalkingSession
import com.github.ajalt.mordant.animation.animation
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.table.Borders
import com.github.ajalt.mordant.table.table
import com.github.ajalt.mordant.terminal.Terminal
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.StateFlow

class Dashboard(
    private val terminal: Terminal,
    private val sessionFlow: StateFlow<WalkingSession>,
    private val connectionStateFlow: StateFlow<ConnectionState>,
) {
    suspend fun run(scope: CoroutineScope) {
        val animation = terminal.animation<Pair<WalkingSession, ConnectionState>> { (session, state) ->
            renderDashboard(session, state)
        }
        try {
            while (scope.isActive) {
                animation.update(sessionFlow.value to connectionStateFlow.value)
                delay(500)
            }
        } finally {
            animation.clear()
        }
    }

    private fun renderDashboard(session: WalkingSession, connState: ConnectionState) = table {
        header {
            row {
                cell(bold("CLI-Walker — Kingsmith WalkingPad Monitor")) { columnSpan = 2 }
            }
        }

        body {
            cellBorders = Borders.TOP_BOTTOM

            row {
                cell(gray("Status"))
                cell(formatConnectionState(connState))
            }
            row {
                cell(gray("Belt"))
                cell(formatBeltState(session))
            }
            row {
                cell(gray("Speed"))
                cell(formatSpeed(session))
            }
            row {
                cell(gray("Distance"))
                cell(cyan(session.formattedDistance))
            }
            row {
                cell(gray("Steps"))
                cell(cyan(session.steps.toString()))
            }
            row {
                cell(gray("Time"))
                cell(cyan(session.formattedTime))
            }
            row {
                cell(gray("Calories"))
                cell(cyan("~${session.caloriesEstimate} kcal"))
            }
        }

        footer {
            row {
                cell(gray("↑/↓ ±0.5  ⌃↑/⌃↓ ±0.1  ␣ pause/resume  q quit")) { columnSpan = 2 }
            }
        }
    }

    private fun formatConnectionState(state: ConnectionState): String = when (state) {
        ConnectionState.Disconnected -> red("Disconnected")
        ConnectionState.Scanning -> yellow("Scanning...")
        ConnectionState.Connecting -> yellow("Connecting...")
        ConnectionState.Connected -> green("Connected")
        ConnectionState.Error -> brightRed("Error")
    }.toString()

    private fun formatBeltState(session: WalkingSession): String = when (session.state) {
        com.cliwalker.model.BeltState.Running -> green("Running")
        com.cliwalker.model.BeltState.Starting, com.cliwalker.model.BeltState.Accelerating -> yellow("Starting...")
        com.cliwalker.model.BeltState.Standby -> yellow("Standby")
        com.cliwalker.model.BeltState.Idle -> gray("Idle")
    }.toString()

    private fun formatSpeed(session: WalkingSession): String {
        val bar = buildSpeedBar(session.speed, maxSpeed = 6.0, width = 20)
        return "${cyan(session.formattedSpeed)} $bar"
    }

    private fun buildSpeedBar(speed: Double, maxSpeed: Double, width: Int): String {
        val filled = ((speed / maxSpeed) * width).toInt().coerceIn(0, width)
        val empty = width - filled
        val filledStr = when {
            speed < 2.0 -> green("█".repeat(filled))
            speed < 4.0 -> yellow("█".repeat(filled))
            else -> red("█".repeat(filled))
        }
        return "$filledStr${gray("░".repeat(empty))}"
    }
}
