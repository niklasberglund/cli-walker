package com.cliwalker.model

import kotlinx.datetime.Instant

enum class BeltState(val value: Int) {
    Idle(0),
    Running(1),
    Starting(4),
    Standby(5),
    Accelerating(9),

    // A state byte the protocol docs do not describe; WalkingSession.rawState keeps the value.
    Unknown(-1);

    companion object {
        fun fromByte(b: Int): BeltState = entries.find { it.value == b } ?: Unknown
    }
}

enum class BeltMode(val value: Int) {
    Automatic(0),
    Manual(1),
    Sleep(2);

    companion object {
        fun fromByte(b: Int): BeltMode = entries.find { it.value == b } ?: Manual
    }
}

data class WalkingSession(
    val state: BeltState = BeltState.Idle,
    val rawState: Int = state.value,
    val mode: BeltMode = BeltMode.Manual,
    val speed: Double = 0.0,         // km/h
    val distanceKm: Double = 0.0,
    val steps: Int = 0,
    val elapsedSeconds: Int = 0,
    val startedAt: Instant? = null,
) {
    val isActive: Boolean get() = state == BeltState.Running || state == BeltState.Starting || state == BeltState.Accelerating

    val formattedTime: String get() = formatDuration(elapsedSeconds)

    val formattedSpeed: String get() = "${roundTo1(speed)} km/h"
    val formattedDistance: String get() = "${roundTo2(distanceKm)} km"

    val caloriesEstimate: Int get() = caloriesFor(elapsedSeconds)

    /** Seconds until the pad's run cap stops the belt, once the warning window has begun; else null. */
    val secondsUntilAutoStop: Int?
        get() {
            val remaining = MAX_RUN_SECONDS - elapsedSeconds
            return if (isActive && remaining <= AUTO_STOP_WARNING_SECONDS) remaining.coerceAtLeast(0) else null
        }
}

// The A1 Pro stops the belt after two hours of continuous running and then locks itself in a
// 30-minute REST countdown. None of this is visible over BLE, so the cap is tracked from elapsed time.
const val MAX_RUN_SECONDS = 2 * 60 * 60
const val AUTO_STOP_WARNING_SECONDS = 5 * 60

fun formatDuration(seconds: Int): String {
    val h = seconds / 3600
    val m = (seconds % 3600) / 60
    val s = seconds % 60
    return if (h > 0) "${h}:${m.padTwo()}:${s.padTwo()}" else "${m.padTwo()}:${s.padTwo()}"
}

fun caloriesFor(seconds: Int): Int {
    val hours = seconds / 3600.0
    return (3.5 * 70.0 * hours * 1.05 / 60.0 * 60.0).toInt()
}

private fun Int.padTwo(): String = if (this < 10) "0$this" else "$this"

private fun roundTo1(d: Double): String {
    val i = (d * 10).toInt()
    return "${i / 10}.${i % 10}"
}

private fun roundTo2(d: Double): String {
    val i = (d * 100).toInt()
    return "${i / 100}.${(i % 100).padTwo()}"
}

private fun Int.padTwo2(): String = if (this < 10) "0$this" else "$this"
