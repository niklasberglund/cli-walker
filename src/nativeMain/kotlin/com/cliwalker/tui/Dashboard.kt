package com.cliwalker.tui

import com.cliwalker.ble.WalkingPadConnection.ConnectionState
import com.cliwalker.model.SessionRecord
import com.cliwalker.model.WalkingSession
import com.github.ajalt.mordant.animation.animation
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.table.Borders
import com.github.ajalt.mordant.table.table
import com.github.ajalt.mordant.terminal.Terminal
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.StateFlow
import kotlinx.datetime.Clock
import kotlinx.datetime.Instant
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime

enum class Tab(val label: String) {
    Session("Session"),
    Statistics("Statistics");

    fun next(): Tab = entries[(ordinal + 1) % entries.size]
    fun previous(): Tab = entries[(ordinal - 1 + entries.size) % entries.size]
}

class Dashboard(
    private val terminal: Terminal,
    private val sessionFlow: StateFlow<WalkingSession>,
    private val connectionStateFlow: StateFlow<ConnectionState>,
    private val tabFlow: StateFlow<Tab>,
    private val history: List<SessionRecord>,
) {
    suspend fun run(scope: CoroutineScope) {
        val animation = terminal.animation<Triple<WalkingSession, ConnectionState, Tab>> { (session, state, tab) ->
            renderDashboard(session, state, tab)
        }
        try {
            while (scope.isActive) {
                animation.update(Triple(sessionFlow.value, connectionStateFlow.value, tabFlow.value))
                delay(500)
            }
        } finally {
            animation.clear()
        }
    }

    private fun renderDashboard(session: WalkingSession, connState: ConnectionState, tab: Tab) = when (tab) {
        Tab.Session -> renderSessionTab(session, connState)
        Tab.Statistics -> renderStatisticsTab()
    }

    private fun renderTabBar(active: Tab): String {
        val parts = Tab.entries.map { t ->
            val label = " ${t.label} "
            if (t == active) inverse(label) else gray(label)
        }
        return parts.joinToString(" ")
    }

    private fun renderSessionTab(session: WalkingSession, connState: ConnectionState) = table {
        header {
            row {
                cell(renderTabBar(Tab.Session) + "\n" + bold("CLI-Walker — Kingsmith WalkingPad Monitor")) { columnSpan = 2 }
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
                cell(gray("↑/↓ ±0.5  ⌃↑/⌃↓ ±0.1  ␣ pause/resume  ⇥/←→ tabs  q quit")) { columnSpan = 2 }
            }
        }
    }

    private fun renderStatisticsTab() = table {
        val zone = TimeZone.currentSystemDefault()
        val cols = 5

        header {
            row {
                cell(renderTabBar(Tab.Statistics) + "\n" + bold("Statistics")) { columnSpan = cols }
            }
        }

        body {
            cellBorders = Borders.TOP_BOTTOM

            if (history.isEmpty()) {
                row { cell(gray("No sessions recorded yet — go for a walk!")) { columnSpan = cols } }
            } else {
                val totalDist = history.sumOf { it.distanceKm }
                val totalSteps = history.sumOf { it.steps }
                val totalCal = history.sumOf { it.calories }
                val totalSecs = history.sumOf { it.durationSeconds }

                row { cell(bold(underline("Totals"))) { columnSpan = cols } }
                row {
                    cell(gray("Sessions"))
                    cell(cyan(history.size.toString())) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Distance"))
                    cell(cyan("${roundTo2(totalDist)} km")) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Steps"))
                    cell(cyan(totalSteps.toString())) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Time"))
                    cell(cyan(formatDuration(totalSecs))) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Calories"))
                    cell(cyan("~$totalCal kcal")) { columnSpan = cols - 1 }
                }

                val now = Clock.System.now().toLocalDateTime(zone)
                val thisMonth = history.filter {
                    val ldt = it.startedAt.toLocalDateTime(zone)
                    ldt.year == now.year && ldt.monthNumber == now.monthNumber
                }
                row { cell(bold(underline("This Month (${MONTHS[now.monthNumber - 1]} ${now.year})"))) { columnSpan = cols } }
                if (thisMonth.isEmpty()) {
                    row { cell(gray("No sessions this month yet")) { columnSpan = cols } }
                } else {
                    val mDist = thisMonth.sumOf { it.distanceKm }
                    val mSteps = thisMonth.sumOf { it.steps }
                    val mCal = thisMonth.sumOf { it.calories }
                    val mSecs = thisMonth.sumOf { it.durationSeconds }
                    row {
                        cell(gray("Sessions"))
                        cell(cyan(thisMonth.size.toString())) { columnSpan = cols - 1 }
                    }
                    row {
                        cell(gray("Distance"))
                        cell(cyan("${roundTo2(mDist)} km")) { columnSpan = cols - 1 }
                    }
                    row {
                        cell(gray("Steps"))
                        cell(cyan(mSteps.toString())) { columnSpan = cols - 1 }
                    }
                    row {
                        cell(gray("Time"))
                        cell(cyan(formatDuration(mSecs))) { columnSpan = cols - 1 }
                    }
                    row {
                        cell(gray("Calories"))
                        cell(cyan("~$mCal kcal")) { columnSpan = cols - 1 }
                    }
                }

                val longestDist = history.maxBy { it.distanceKm }
                val longestTime = history.maxBy { it.durationSeconds }
                val fastestAvg = history.maxBy { it.avgSpeedKmh }
                val topSpeed = history.maxBy { it.maxSpeedKmh }
                val mostSteps = history.maxBy { it.steps }

                row { cell(bold(underline("Personal Bests"))) { columnSpan = cols } }
                row {
                    cell(gray("Longest distance"))
                    cell(cyan("${roundTo2(longestDist.distanceKm)} km") + gray("  ${formatDate(longestDist.startedAt, zone)}")) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Longest duration"))
                    cell(cyan(formatDuration(longestTime.durationSeconds)) + gray("  ${formatDate(longestTime.startedAt, zone)}")) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Best avg speed"))
                    cell(cyan("${roundTo1(fastestAvg.avgSpeedKmh)} km/h") + gray("  ${formatDate(fastestAvg.startedAt, zone)}")) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Top speed"))
                    cell(cyan("${roundTo1(topSpeed.maxSpeedKmh)} km/h") + gray("  ${formatDate(topSpeed.startedAt, zone)}")) { columnSpan = cols - 1 }
                }
                row {
                    cell(gray("Most steps"))
                    cell(cyan(mostSteps.steps.toString()) + gray("  ${formatDate(mostSteps.startedAt, zone)}")) { columnSpan = cols - 1 }
                }

                row { cell(bold(underline("Recent Sessions"))) { columnSpan = cols } }
                row {
                    cell(gray("Date"))
                    cell(gray("Duration"))
                    cell(gray("Distance"))
                    cell(gray("Steps"))
                    cell(gray("Avg km/h"))
                }
                history.takeLast(10).reversed().forEach { r ->
                    row {
                        cell(formatDate(r.startedAt, zone))
                        cell(formatDuration(r.durationSeconds))
                        cell("${roundTo2(r.distanceKm)} km")
                        cell(r.steps.toString())
                        cell(roundTo1(r.avgSpeedKmh))
                    }
                }
            }
        }

        footer {
            row {
                cell(gray("⇥/←→ tabs  q quit")) { columnSpan = cols }
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

private fun roundTo1(d: Double): String {
    val i = (d * 10).toInt()
    return "${i / 10}.${i % 10}"
}

private fun roundTo2(d: Double): String {
    val i = (d * 100).toInt()
    val frac = i % 100
    val fracStr = if (frac < 10) "0$frac" else "$frac"
    return "${i / 100}.$fracStr"
}

private fun formatDuration(seconds: Int): String {
    val h = seconds / 3600
    val m = (seconds % 3600) / 60
    val s = seconds % 60
    fun pad(v: Int) = if (v < 10) "0$v" else "$v"
    return if (h > 0) "$h:${pad(m)}:${pad(s)}" else "${pad(m)}:${pad(s)}"
}

private val MONTHS = listOf("Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec")

private fun formatDate(instant: Instant, zone: TimeZone): String {
    val ldt = instant.toLocalDateTime(zone)
    val day = ldt.dayOfMonth.toString().padStart(2, '0')
    val hour = ldt.hour.toString().padStart(2, '0')
    val min = ldt.minute.toString().padStart(2, '0')
    return "${MONTHS[ldt.monthNumber - 1]} $day $hour:$min"
}
