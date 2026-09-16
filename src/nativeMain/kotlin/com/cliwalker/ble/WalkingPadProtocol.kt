package com.cliwalker.ble

import com.cliwalker.model.BeltMode
import com.cliwalker.model.BeltState
import com.cliwalker.model.WalkingSession

/**
 * WalkingPad BLE protocol encoding/decoding.
 *
 * Packet format: [0xF7, 0xA2, cmd_type, param, 0xFF, checksum, 0xFD]
 * Checksum: sum of bytes[1..n-2] mod 256
 *
 * Protocol reverse-engineered by ph4r05/ph4-walkingpad and DorianRudolph/QWalkingPad.
 */
object WalkingPadProtocol {

    // BLE UUIDs
    const val SERVICE_UUID = "0000fe00-0000-1000-8000-00805f9b34fb"
    const val NOTIFY_CHARACTERISTIC_UUID = "0000fe01-0000-1000-8000-00805f9b34fb"
    const val WRITE_CHARACTERISTIC_UUID = "0000fe02-0000-1000-8000-00805f9b34fb"

    // Packet framing
    private const val HEADER: Byte = 0xF7.toByte()
    private const val FOOTER: Byte = 0xFD.toByte()

    // Command types
    private const val CMD_STATUS: Byte = 0xA2.toByte()

    // Response headers
    private const val RESP_STATUS: Byte = 0xF8.toByte()
    private const val RESP_PARAMS: Byte = 0xA7.toByte()

    fun buildQueryStatusCommand(): ByteArray {
        return buildCommand(0xA2.toByte(), 0x00, 0x00)
    }

    fun buildStartCommand(): ByteArray {
        return buildCommand(0xA2.toByte(), 0x04, 0x01)
    }

    fun buildStopCommand(): ByteArray {
        return buildCommand(0xA2.toByte(), 0x04, 0x00)
    }

    fun buildSetModeManualCommand(): ByteArray {
        return buildCommand(0xA2.toByte(), 0x02, 0x01)
    }

    fun buildSetSpeedCommand(speedKmh: Double): ByteArray {
        val speedValue = (speedKmh * 10).toInt().coerceIn(0, 60)
        return buildCommand(0xA2.toByte(), 0x01, speedValue.toByte())
    }

    fun buildSetModeCommand(mode: BeltMode): ByteArray {
        return buildCommand(0xA2.toByte(), 0x02, mode.value.toByte())
    }

    private fun buildCommand(cmdGroup: Byte, cmdType: Byte, param: Byte): ByteArray {
        val cmd = byteArrayOf(HEADER, cmdGroup, cmdType, param, 0x00, FOOTER)
        cmd[4] = checksum(cmd)
        return cmd
    }

    private fun checksum(cmd: ByteArray): Byte {
        // Sum bytes between header and checksum position: indices 1..size-3
        var sum = 0
        for (i in 1 until cmd.size - 2) {
            sum += cmd[i].toInt() and 0xFF
        }
        return (sum % 256).toByte()
    }

    fun parseResponse(data: ByteArray): ParsedResponse? {
        if (data.size < 3) return null
        val header = data[0].toInt() and 0xFF

        return when (header) {
            0xF8 -> parseStatusResponse(data)
            else -> null
        }
    }

    private fun parseStatusResponse(data: ByteArray): ParsedResponse.Status? {
        if (data.size < 16) return null

        val subType = data[1].toInt() and 0xFF
        if (subType != 0xA2) return null

        val beltState = BeltState.fromByte(data[2].toInt() and 0xFF)
        val speed = (data[3].toInt() and 0xFF) / 10.0
        val mode = BeltMode.fromByte(data[4].toInt() and 0xFF)
        val time = read3ByteInt(data, 5)
        val distance = read3ByteInt(data, 8)
        val steps = read3ByteInt(data, 11)

        return ParsedResponse.Status(
            session = WalkingSession(
                state = beltState,
                mode = mode,
                speed = speed,
                distanceKm = distance / 100.0,
                steps = steps,
                elapsedSeconds = time,
            )
        )
    }

    private fun read3ByteInt(data: ByteArray, offset: Int): Int {
        return ((data[offset].toInt() and 0xFF) shl 16) or
                ((data[offset + 1].toInt() and 0xFF) shl 8) or
                (data[offset + 2].toInt() and 0xFF)
    }
}

sealed class ParsedResponse {
    data class Status(val session: WalkingSession) : ParsedResponse()
}
