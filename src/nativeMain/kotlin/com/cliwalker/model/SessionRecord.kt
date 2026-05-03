@file:UseSerializers(InstantIso8601Serializer::class)

package com.cliwalker.model

import kotlinx.datetime.Instant
import kotlinx.datetime.serializers.InstantIso8601Serializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers

@Serializable
data class SessionRecord(
    val startedAt: Instant,
    val endedAt: Instant,
    val durationSeconds: Int,
    val distanceKm: Double,
    val steps: Int,
    val calories: Int,
    val avgSpeedKmh: Double,
    val maxSpeedKmh: Double,
)
