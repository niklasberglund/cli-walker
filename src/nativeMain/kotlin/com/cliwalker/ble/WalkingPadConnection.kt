package com.cliwalker.ble

import com.cliwalker.model.WalkingSession
import com.juul.kable.*
import com.juul.kable.WriteType
import com.juul.kable.logs.Logging
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import platform.posix.fflush
import platform.posix.stderr
import platform.posix.fprintf
import kotlin.time.Duration.Companion.milliseconds
import kotlin.time.Duration.Companion.seconds
import kotlin.time.TimeSource
import kotlin.uuid.ExperimentalUuidApi
import kotlin.uuid.Uuid

@OptIn(ExperimentalUuidApi::class, kotlinx.cinterop.ExperimentalForeignApi::class)
class WalkingPadConnection(
    private val scope: CoroutineScope,
    private val debug: Boolean = false,
) {

    private val _session = MutableStateFlow(WalkingSession())
    val session: StateFlow<WalkingSession> = _session.asStateFlow()

    private val _connectionState = MutableStateFlow(ConnectionState.Disconnected)
    val connectionState: StateFlow<ConnectionState> = _connectionState.asStateFlow()

    private var peripheral: Peripheral? = null
    private var pollJob: Job? = null
    private var listenJob: Job? = null
    private var pollPaused = false
    private var lastCommandTime = TimeSource.Monotonic.markNow() - 1.seconds
    private val minCommandInterval = 700.milliseconds

    private val notifyCharacteristic = characteristicOf(
        service = Uuid.parse(WalkingPadProtocol.SERVICE_UUID),
        characteristic = Uuid.parse(WalkingPadProtocol.NOTIFY_CHARACTERISTIC_UUID),
    )

    private val writeCharacteristic = characteristicOf(
        service = Uuid.parse(WalkingPadProtocol.SERVICE_UUID),
        characteristic = Uuid.parse(WalkingPadProtocol.WRITE_CHARACTERISTIC_UUID),
    )

    enum class ConnectionState {
        Disconnected,
        Scanning,
        Connecting,
        Connected,
        Error,
    }

    suspend fun connect() {
        _connectionState.value = ConnectionState.Scanning

        val serviceUuid = Uuid.parse(WalkingPadProtocol.SERVICE_UUID)
        val advertisement = withTimeoutOrNull(30.seconds) {
            Scanner {
                filters {
                    match { services = listOf(serviceUuid) }
                }
                logging { level = Logging.Level.Warnings }
            }.advertisements.first()
        }

        if (advertisement == null) {
            _connectionState.value = ConnectionState.Error
            throw IllegalStateException("No WalkingPad found after 30s. Is it powered on and not connected to another app?")
        }

        _connectionState.value = ConnectionState.Connecting

        val p = Peripheral(advertisement) {
            logging { level = Logging.Level.Warnings }
        }
        peripheral = p

        try {
            p.connect()
            _connectionState.value = ConnectionState.Connected
            startListening(p)
            startPolling(p)
        } catch (e: Exception) {
            _connectionState.value = ConnectionState.Error
            throw e
        }
    }

    private fun startListening(peripheral: Peripheral) {
        listenJob = scope.launch {
            peripheral.observe(notifyCharacteristic).collect { data ->
                if (debug) logReceived(data)

                val parsed = WalkingPadProtocol.parseResponse(data)
                if (parsed is ParsedResponse.Status) {
                    _session.value = parsed.session.copy(
                        startedAt = _session.value.startedAt ?: parsed.session.startedAt
                    )
                } else if (debug) {
                    debugLog("PARSE: unrecognized response (header=0x${(data[0].toInt() and 0xFF).toString(16)})")
                }
            }
        }
    }

    private fun startPolling(peripheral: Peripheral) {
        pollJob = scope.launch {
            while (isActive) {
                if (!pollPaused) {
                    try {
                        sendCommand(WalkingPadProtocol.buildQueryStatusCommand(), throttle = false)
                    } catch (_: Exception) { }
                }
                delay(1000)
            }
        }
    }

    private suspend inline fun withPollPaused(block: () -> Unit) {
        pollPaused = true
        try {
            block()
        } finally {
            pollPaused = false
        }
    }

    suspend fun startBelt(speedKmh: Double = 2.5) = withPollPaused {
        if (debug) debugLog("CMD: startBelt speed=$speedKmh")
        sendCommand(WalkingPadProtocol.buildSetModeManualCommand())
        delay(300)
        sendCommand(WalkingPadProtocol.buildStartCommand())
        delay(700)
        sendCommand(WalkingPadProtocol.buildSetSpeedCommand(speedKmh))
    }

    suspend fun stopBelt() {
        if (debug) debugLog("CMD: stopBelt")
        sendCommand(WalkingPadProtocol.buildStopCommand())
    }

    suspend fun setSpeed(speedKmh: Double) {
        val clamped = speedKmh.coerceIn(0.5, 6.0)
        if (debug) debugLog("CMD: setSpeed $clamped")
        sendCommand(WalkingPadProtocol.buildSetSpeedCommand(clamped))
    }

    private suspend fun sendCommand(data: ByteArray, throttle: Boolean = true) {
        val p = peripheral ?: return
        // The WalkingPad drops its BLE connection (clearing GATT services) when the
        // belt stops or the pad goes to standby. Writing then fails with
        // NoSuchElementException for the write characteristic, so bail if not connected.
        if (p.state.value !is State.Connected) return
        if (throttle) {
            val elapsed = lastCommandTime.elapsedNow()
            if (elapsed < minCommandInterval) {
                delay(minCommandInterval - elapsed)
            }
        }
        if (debug) debugLog("TX: ${data.toHex()}")
        try {
            p.write(writeCharacteristic, data, WriteType.WithoutResponse)
            lastCommandTime = TimeSource.Monotonic.markNow()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // A failed command write (e.g. peripheral disconnected mid-write) must
            // never crash the app; just log it in debug mode.
            if (debug) debugLog("TX FAILED: ${e.message}")
        }
    }

    private fun logReceived(data: ByteArray) {
        val hex = data.toHex()
        val parsed = WalkingPadProtocol.parseResponse(data)
        val detail = if (parsed is ParsedResponse.Status) {
            val s = parsed.session
            "state=${s.state} speed=${s.speed} dist=${s.distanceKm} steps=${s.steps} time=${s.elapsedSeconds}s mode=${s.mode}"
        } else {
            "unknown"
        }
        debugLog("RX: $hex | $detail")
    }

    private fun debugLog(msg: String) {
        val colored = when {
            msg.startsWith("TX:") || msg.startsWith("CMD:") -> "\u001B[32m$msg\u001B[0m" // green
            msg.startsWith("RX:") -> "\u001B[34m$msg\u001B[0m" // blue
            else -> msg
        }
        fprintf(stderr, "[DEBUG] %s\n", colored)
        fflush(stderr)
    }

    suspend fun disconnect() {
        pollJob?.cancel()
        listenJob?.cancel()
        peripheral?.disconnect()
        peripheral = null
        _connectionState.value = ConnectionState.Disconnected
    }
}

private fun ByteArray.toHex(): String = joinToString(" ") { (it.toInt() and 0xFF).toString(16).padStart(2, '0') }
