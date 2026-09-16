package com.cliwalker.model

import kotlinx.datetime.Instant

enum class BeltState(val value: Int) {
    Idle(0),
    Running(1),
    Starting(4),
    Standby(5),
    Accelerating(9);

    companion object {
        fun fromByte(b: Int): BeltState = entries.find { it.value == b } ?: Idle
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
}

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
