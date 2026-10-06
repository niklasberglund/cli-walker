package com.cliwalker.ble

import com.cliwalker.model.WalkingSession
import kotlinx.coroutines.flow.StateFlow

enum class ConnectionState {
    Disconnected,
    Scanning,
    Connecting,
    Connected,
    Error,
}

/** A treadmill the dashboard can observe and drive: the real pad, or the demo simulation. */
interface PadConnection {
    val session: StateFlow<WalkingSession>
    val connectionState: StateFlow<ConnectionState>

    suspend fun connect()
    suspend fun startBelt(speedKmh: Double = 2.5)
    suspend fun stopBelt()
    suspend fun setSpeed(speedKmh: Double)
    suspend fun disconnect()
}
