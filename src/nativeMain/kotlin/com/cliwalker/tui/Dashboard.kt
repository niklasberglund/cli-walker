package com.cliwalker.tui

import com.cliwalker.ble.ConnectionState
import com.cliwalker.model.SessionRecord
import com.cliwalker.model.WalkingSession
import com.cliwalker.model.formatDuration
import com.cliwalker.strava.formatThreshold
import com.github.ajalt.mordant.animation.animation
import com.github.ajalt.mordant.rendering.BorderType
import com.github.ajalt.mordant.rendering.Line
import com.github.ajalt.mordant.rendering.Lines
import com.github.ajalt.mordant.rendering.Span
import com.github.ajalt.mordant.rendering.TextColors
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyle
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.rendering.Widget
import com.github.ajalt.mordant.table.Borders
import com.github.ajalt.mordant.table.ColumnWidth
import com.github.ajalt.mordant.table.table
import com.github.ajalt.mordant.terminal.Terminal
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.StateFlow
import kotlinx.datetime.Clock
import kotlinx.datetime.Instant
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime

// Sentinel style used only for table borders, so rendered border spans can be picked out and
// recolored with a gradient without disturbing any styled cell content (which is never left
// without an explicit color).
private val BORDER_STYLE_SENTINEL: TextStyle = TextStyle()

// Endpoints sampled from the logo's purple-to-pink gradient (see Logo.kt).
private val GRADIENT_START = Triple(101, 78, 163)
private val GRADIENT_END = Triple(234, 175, 200)

private fun hex2(v: Int): String = v.coerceIn(0, 255).toString(16).padStart(2, '0')

private fun gradientStyle(t: Double): TextStyle {
    val clamped = t.coerceIn(0.0, 1.0)
    val r = (GRADIENT_START.first + (GRADIENT_END.first - GRADIENT_START.first) * clamped).toInt()
    val g = (GRADIENT_START.second + (GRADIENT_END.second - GRADIENT_START.second) * clamped).toInt()
    val b = (GRADIENT_START.third + (GRADIENT_END.third - GRADIENT_START.third) * clamped).toInt()
    return TextColors.rgb("#${hex2(r)}${hex2(g)}${hex2(b)}")
}

// Mordant's table layout gives columnSpan cells (used by our header/footer rows) one extra
// column of budget that isn't reflected in the border rows' width, so header/footer content
// renders one column wider than the box border. Trim that excess from the trailing padding
// (found by walking back from the line's end, skipping over the border pipe itself) so every
// line matches the border row's true width.
private fun trimToWidth(line: Line, excess: Int): Line {
    if (excess <= 0) return line
    val spans = line.spans.toMutableList()
    var remaining = excess
    var idx = spans.lastIndex
    while (idx >= 0 && remaining > 0) {
        val span = spans[idx]
        if (span.text.isBlank()) {
            val take = minOf(remaining, span.text.length)
            val newLen = span.text.length - take
            if (newLen <= 0) spans.removeAt(idx) else spans[idx] = Span.space(newLen, span.style)
            remaining -= take
        }
        idx--
    }
    return Line(spans)
}

// Wraps a table widget and recolors its (unstyled) border characters with a left-to-right
// gradient, leaving already-styled cell content untouched.
private class GradientBorderWidget(private val inner: Widget) : Widget {
    override fun measure(t: Terminal, width: Int) = inner.measure(t, width)

    override fun render(t: Terminal, width: Int): Lines {
        val rawLines = inner.render(t, width).lines
        val targetWidth = (rawLines.firstOrNull()?.spans?.sumOf { it.text.length } ?: 1).coerceAtLeast(1)
        val normalized = rawLines.map { line ->
            val lineWidth = line.spans.sumOf { it.text.length }
            trimToWidth(line, lineWidth - targetWidth)
        }
        return Lines(normalized.map { line ->
            var x = 0
            val spans = mutableListOf<Span>()
            for (span in line.spans) {
                if (span.style == BORDER_STYLE_SENTINEL) {
                    for (ch in span.text) {
                        spans.add(Span.word(ch.toString(), gradientStyle(x.toDouble() / targetWidth)))
                        x++
                    }
                } else {
                    spans.add(span)
                    x += span.text.length
                }
            }
            Line(spans)
        })
    }
}

private fun Widget.withGradientBorder(): Widget = GradientBorderWidget(this)

private const val CONNECTION_STATE_WIDTH = 13 // len("Connecting...")
private const val BELT_STATE_WIDTH = 13 // len("Unknown(0xff)")
private const val THRESHOLD_WIDTH = 8 // len("12.34 km")
private const val PENDING_WIDTH = 24 // len("Nothing waiting to upload") - 1

enum class Tab(val label: String) {
    Session("Session"),
    Statistics("Statistics"),
    Strava("Strava");

    fun next(): Tab = entries[(ordinal + 1) % entries.size]
    fun previous(): Tab = entries[(ordinal - 1 + entries.size) % entries.size]
}

/** Everything the Strava tab shows, kept in one flow so the tab re-renders on any change. */
data class StravaUiState(
    val connected: Boolean = false,
    val athleteName: String? = null,
    val muted: Boolean = false,
    val thresholdKm: Double = 0.0,
    val pendingCount: Int = 0,
    val belowThresholdCount: Int = 0,
)

private data class DashboardState(
    val session: WalkingSession,
    val connState: ConnectionState,
    val tab: Tab,
    val strava: StravaUiState,
)

class Dashboard(
    private val terminal: Terminal,
    private val sessionFlow: StateFlow<WalkingSession>,
    private val connectionStateFlow: StateFlow<ConnectionState>,
    private val tabFlow: StateFlow<Tab>,
    private val history: List<SessionRecord>,
    private val stravaFlow: StateFlow<StravaUiState>,
) {
    suspend fun run(scope: CoroutineScope) {
        val animation = terminal.animation<DashboardState> { state ->
            renderDashboard(state)
        }
        // Mordant's Size has no value equality, so compare its dimensions.
        var size = terminal.updateSize().let { it.width to it.height }
        try {
            while (scope.isActive) {
                // A resize reflows what is already on screen, so the animation's cursor
                // arithmetic no longer lines up with its last frame; redraw from scratch.
                val newSize = terminal.updateSize().let { it.width to it.height }
                if (newSize != size) {
                    size = newSize
                    animation.stop()
                    printHeader()
                }
                animation.update(
                    DashboardState(sessionFlow.value, connectionStateFlow.value, tabFlow.value, stravaFlow.value),
                )
                delay(500)
            }
        } finally {
            animation.clear()
        }
    }

    private fun renderDashboard(state: DashboardState) = when (state.tab) {
        Tab.Session -> renderSessionTab(state.session, state.connState)
        Tab.Statistics -> renderStatisticsTab()
        Tab.Strava -> renderStravaTab(state.strava)
    }

    // Natural width of the widest of the Statistics and Strava tabs, which every tab is padded
    // out to so switching tabs never changes the box width. The history is fixed for the
    // lifetime of the dashboard and the Strava rows are padded to a constant width, so this
    // is computed once.
    private val tabWidth by lazy {
        maxOf(
            buildStatisticsTable(valueColumnWidth = null).measure(terminal).max,
            buildStravaTable(stravaFlow.value, valueColumnWidth = null).measure(terminal).max,
        )
    }

    // Same trick as [sessionChromeWidth], for the Statistics table.
    private val statisticsChromeWidth by lazy {
        buildStatisticsTable(valueColumnWidth = 1).measure(terminal).max - 1
    }

    // Same trick as [sessionChromeWidth], for the Strava table.
    private val stravaChromeWidth by lazy {
        buildStravaTable(stravaFlow.value, valueColumnWidth = 1).measure(terminal).max - 1
    }

    // Width of the session table without its value column (outer borders + label column).
    // Measured empirically by rendering the table with the value column shrunk to 1, because
    // the spanning header/footer cells inflate the label column in non-obvious ways. Constant
    // for the lifetime of the dashboard since labels, header, and footer never change.
    private val sessionChromeWidth by lazy {
        buildSessionTable(sessionFlow.value, connectionStateFlow.value, valueColumnWidth = 1)
            .measure(terminal).max - 1
    }

    private fun renderTabBar(active: Tab): String {
        val parts = Tab.entries.map { t ->
            val label = " ${t.label} "
            if (t == active) (black on brightCyan)(label) else gray(label)
        }
        return parts.joinToString(" ")
    }

    // Pad the session table's value column so every tab renders at the same width.
    private fun renderSessionTab(session: WalkingSession, connState: ConnectionState): Widget {
        val natural = buildSessionTable(session, connState, valueColumnWidth = null)
        if (tabWidth <= natural.measure(terminal).max) return natural
        return buildSessionTable(session, connState, valueColumnWidth = tabWidth - sessionChromeWidth)
    }

    private fun buildSessionTable(session: WalkingSession, connState: ConnectionState, valueColumnWidth: Int?): Widget = table {
        borderStyle = BORDER_STYLE_SENTINEL
        borderType = BorderType.ROUNDED

        if (valueColumnWidth != null) {
            column(1) { width = ColumnWidth.Fixed(valueColumnWidth) }
        }

        header {
            row {
                cell(renderTabBar(Tab.Session) + "\n" + bold(brightCyan("CLI-Walker - Kingsmith WalkingPad Monitor"))) { columnSpan = 2 }
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
                cell(green(session.formattedDistance))
            }
            row {
                cell(gray("Steps"))
                cell(magenta(session.steps.toString()))
            }
            row {
                cell(gray("Time"))
                cell(formatTime(session))
            }
            row {
                cell(gray("Calories"))
                cell(yellow("~${session.caloriesEstimate} kcal"))
            }
        }

        footer {
            row {
                cell(gray("Up/Down +/-0.5  Ctrl+Up/Down +/-0.1  Space pause/resume  Tab/Arrows tabs  q quit")) { columnSpan = 2 }
            }
        }
    }.withGradientBorder()

    // Pad the Strava table's value column so every tab renders at the same width.
    private fun renderStravaTab(strava: StravaUiState): Widget {
        val natural = buildStravaTable(strava, valueColumnWidth = null)
        if (tabWidth <= natural.measure(terminal).max) return natural
        return buildStravaTable(strava, valueColumnWidth = tabWidth - stravaChromeWidth)
    }

    private fun buildStravaTable(strava: StravaUiState, valueColumnWidth: Int?): Widget = table {
        borderStyle = BORDER_STYLE_SENTINEL
        borderType = BorderType.ROUNDED

        if (valueColumnWidth != null) {
            column(1) { width = ColumnWidth.Fixed(valueColumnWidth) }
        }

        header {
            row {
                cell(renderTabBar(Tab.Strava) + "\n" + bold(brightCyan("Strava"))) { columnSpan = 2 }
            }
        }

        body {
            cellBorders = Borders.TOP_BOTTOM

            row {
                cell(gray("Account"))
                cell(
                    if (strava.connected) {
                        green("Connected") + gray("  ${strava.athleteName ?: "unknown athlete"}")
                    } else {
                        yellow("Not connected") + gray("  run `cli-walker strava login`")
                    },
                )
            }
            row {
                cell(gray("Feed"))
                cell(
                    if (strava.muted) yellow("Muted") + gray("  hidden from followers' feeds")
                    else green("Visible") + gray("  shown in followers' feeds"),
                )
            }
            row {
                cell(gray("Min distance"))
                // Padded so the table width does not shift while the threshold is adjusted.
                cell(cyan(formatThreshold(strava.thresholdKm).padEnd(THRESHOLD_WIDTH)) + gray("  shorter sessions are not uploaded"))
            }
            row {
                cell(gray("Pending"))
                cell(
                    if (strava.pendingCount == 0) gray("Nothing waiting to upload".padEnd(PENDING_WIDTH))
                    else magenta("${strava.pendingCount} session(s) waiting".padEnd(PENDING_WIDTH)),
                )
            }
            if (strava.belowThresholdCount > 0) {
                row {
                    cell(gray("Below min"))
                    cell(gray("${strava.belowThresholdCount} session(s) skipped by the threshold"))
                }
            }
        }

        footer {
            row {
                cell(gray("Up/Down +/-100 m  Ctrl+Up/Down +/-10 m  m mute/unmute  Tab/Arrows tabs  q quit")) { columnSpan = 2 }
            }
        }
    }.withGradientBorder()

    private fun renderStatisticsTab(): Widget {
        val natural = buildStatisticsTable(valueColumnWidth = null)
        if (tabWidth <= natural.measure(terminal).max) return natural
        return buildStatisticsTable(valueColumnWidth = tabWidth - statisticsChromeWidth)
    }

    private fun buildStatisticsTable(valueColumnWidth: Int?): Widget = table {
        val zone = TimeZone.currentSystemDefault()

        borderStyle = BORDER_STYLE_SENTINEL
        borderType = BorderType.ROUNDED

        if (valueColumnWidth != null) {
            column(1) { width = ColumnWidth.Fixed(valueColumnWidth) }
        }

        header {
            row {
                cell(renderTabBar(Tab.Statistics) + "\n" + bold(brightCyan("Statistics"))) { columnSpan = 2 }
            }
        }

        body {
            cellBorders = Borders.TOP_BOTTOM

            if (history.isEmpty()) {
                row { cell(gray("No sessions recorded yet - go for a walk!")) { columnSpan = 2 } }
            } else {
                fun totalsLine(sessions: Int, dist: Double, steps: Int, secs: Int, cal: Int) =
                    cyan("$sessions sessions") + "  " +
                        green("${roundTo2(dist)} km") + "  " +
                        magenta("$steps steps") + "  " +
                        blue(formatDuration(secs)) + "  " +
                        yellow("~$cal kcal")

                row {
                    cell(gray("Totals"))
                    cell(totalsLine(history.size, history.sumOf { it.distanceKm }, history.sumOf { it.steps }, history.sumOf { it.durationSeconds }, history.sumOf { it.calories }))
                }

                val now = Clock.System.now().toLocalDateTime(zone)
                val thisMonth = history.filter {
                    val ldt = it.startedAt.toLocalDateTime(zone)
                    ldt.year == now.year && ldt.monthNumber == now.monthNumber
                }
                row {
                    cell(gray("This Month"))
                    if (thisMonth.isEmpty()) {
                        cell(gray("No sessions yet"))
                    } else {
                        cell(totalsLine(thisMonth.size, thisMonth.sumOf { it.distanceKm }, thisMonth.sumOf { it.steps }, thisMonth.sumOf { it.durationSeconds }, thisMonth.sumOf { it.calories }))
                    }
                }

                val longestDist = history.maxBy { it.distanceKm }
                val longestTime = history.maxBy { it.durationSeconds }
                val fastestAvg = history.maxBy { it.avgSpeedKmh }
                val topSpeed = history.maxBy { it.maxSpeedKmh }
                val mostSteps = history.maxBy { it.steps }

                row {
                    cell(gray("Best distance"))
                    cell(yellow("★ ") + green("${roundTo2(longestDist.distanceKm)} km") + gray("  ${formatDate(longestDist.startedAt, zone)}"))
                }
                row {
                    cell(gray("Best duration"))
                    cell(yellow("★ ") + blue(formatDuration(longestTime.durationSeconds)) + gray("  ${formatDate(longestTime.startedAt, zone)}"))
                }
                row {
                    cell(gray("Top speed"))
                    cell(yellow("★ ") + cyan("${roundTo1(topSpeed.maxSpeedKmh)} km/h") + gray("  avg ${roundTo1(fastestAvg.avgSpeedKmh)} km/h"))
                }
                row {
                    cell(gray("Most steps"))
                    cell(yellow("★ ") + magenta(mostSteps.steps.toString()) + gray("  ${formatDate(mostSteps.startedAt, zone)}"))
                }

                row { cell(bold(brightCyan("Recent Sessions"))) { columnSpan = 2 } }
                history.takeLast(5).reversed().forEach { r ->
                    row {
                        cell(gray(formatDate(r.startedAt, zone)))
                        cell(blue(formatDuration(r.durationSeconds)) + "  " +
                            green("${roundTo2(r.distanceKm)} km") + "  " +
                            magenta("${r.steps} steps") + "  " +
                            cyan("${roundTo1(r.avgSpeedKmh)} km/h avg"))
                    }
                }
            }
        }

        footer {
            row {
                cell(gray("Tab/Arrows tabs  q quit")) { columnSpan = 2 }
            }
        }
    }.withGradientBorder()

    // Padded to the widest possible label so the table's width never changes as the
    // connection/belt state changes, which would otherwise leave stale border fragments
    // on redraw in some terminals.
    private fun formatConnectionState(state: ConnectionState): String = when (state) {
        ConnectionState.Disconnected -> red("Disconnected".padEnd(CONNECTION_STATE_WIDTH))
        ConnectionState.Scanning -> yellow("Scanning...".padEnd(CONNECTION_STATE_WIDTH))
        ConnectionState.Connecting -> yellow("Connecting...".padEnd(CONNECTION_STATE_WIDTH))
        ConnectionState.Connected -> green("Connected".padEnd(CONNECTION_STATE_WIDTH))
        ConnectionState.Error -> brightRed("Error".padEnd(CONNECTION_STATE_WIDTH))
    }.toString()

    private fun formatBeltState(session: WalkingSession): String = when (session.state) {
        com.cliwalker.model.BeltState.Running -> green("Running".padEnd(BELT_STATE_WIDTH))
        com.cliwalker.model.BeltState.Starting, com.cliwalker.model.BeltState.Accelerating -> yellow("Starting...".padEnd(BELT_STATE_WIDTH))
        com.cliwalker.model.BeltState.Standby -> yellow("Standby".padEnd(BELT_STATE_WIDTH))
        com.cliwalker.model.BeltState.Idle -> gray("Idle".padEnd(BELT_STATE_WIDTH))
        com.cliwalker.model.BeltState.Unknown ->
            magenta("Unknown(0x${session.rawState.toString(16).padStart(2, '0')})".padEnd(BELT_STATE_WIDTH))
    }.toString()

    // Kept on the Time row rather than in a row of its own so the dashboard never changes height.
    private fun formatTime(session: WalkingSession): String {
        val time = blue(session.formattedTime)
        val remaining = session.secondsUntilAutoStop ?: return time
        return time + "  " + (bold + brightRed)("Pad auto-rests in ${formatDuration(remaining)}")
    }

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
