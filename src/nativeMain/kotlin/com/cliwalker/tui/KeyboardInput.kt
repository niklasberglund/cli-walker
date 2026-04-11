package com.cliwalker.tui

import kotlinx.cinterop.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import platform.posix.*

enum class KeyEvent {
    ArrowUp,
    ArrowDown,
    CtrlArrowUp,
    CtrlArrowDown,
    Space,
    Quit,
}

@OptIn(ExperimentalForeignApi::class)
class KeyboardInput {

    private var savedTermios: CValue<termios>? = null

    fun events(): Flow<KeyEvent> = flow {
        enableRawMode()
        try {
            while (currentCoroutineContext().isActive) {
                val byte = readByte()
                if (byte == null) {
                    delay(30)
                    continue
                }

                when (byte) {
                    'q'.code, 'Q'.code -> {
                        emit(KeyEvent.Quit)
                        return@flow
                    }
                    ' '.code -> emit(KeyEvent.Space)
                    0x1B -> {
                        // CSI sequence: ESC [ params final
                        // Plain arrow up/down:   ESC [ A / ESC [ B
                        // Ctrl+arrow up/down:    ESC [ 1 ; 5 A / ESC [ 1 ; 5 B
                        val next = readByte() ?: continue
                        if (next != '['.code) continue

                        // Accumulate parameter bytes until we hit a final byte
                        // (any letter in 0x40..0x7E per the CSI spec).
                        val params = StringBuilder()
                        var finalByte = -1
                        for (i in 0 until 8) {
                            val b = readByte() ?: break
                            if (b in 0x40..0x7E) {
                                finalByte = b
                                break
                            }
                            params.append(b.toChar())
                        }
                        if (finalByte == -1) continue

                        val ctrl = params.toString() == "1;5"
                        when (finalByte) {
                            'A'.code -> emit(if (ctrl) KeyEvent.CtrlArrowUp else KeyEvent.ArrowUp)
                            'B'.code -> emit(if (ctrl) KeyEvent.CtrlArrowDown else KeyEvent.ArrowDown)
                        }
                    }
                }
            }
        } finally {
            restoreTerminalMode()
        }
    }

    private fun enableRawMode() = memScoped {
        val orig = alloc<termios>()
        tcgetattr(STDIN_FILENO, orig.ptr)
        savedTermios = orig.readValue()

        val raw = alloc<termios>()
        tcgetattr(STDIN_FILENO, raw.ptr)
        raw.c_lflag = raw.c_lflag and (ICANON or ECHO).toULong().inv()
        // Fully non-blocking: VMIN=0, VTIME=0 — read() returns immediately.
        // The coroutine yields via delay() between polls so the event loop
        // can run other coroutines (BLE poll, notify collector, dashboard).
        raw.c_cc[VMIN] = 0u
        raw.c_cc[VTIME] = 0u
        tcsetattr(STDIN_FILENO, TCSANOW, raw.ptr)
    }

    private fun restoreTerminalMode() {
        savedTermios?.let { orig ->
            memScoped {
                val restored = alloc<termios>()
                orig.useContents {
                    restored.c_iflag = c_iflag
                    restored.c_oflag = c_oflag
                    restored.c_cflag = c_cflag
                    restored.c_lflag = c_lflag
                    for (i in 0 until NCCS) {
                        restored.c_cc[i] = c_cc[i]
                    }
                }
                tcsetattr(STDIN_FILENO, TCSANOW, restored.ptr)
            }
        }
    }

    private fun readByte(): Int? {
        val buf = ByteArray(1)
        val n = buf.usePinned { pinned ->
            read(STDIN_FILENO, pinned.addressOf(0), 1u)
        }
        return if (n == 1L) (buf[0].toInt() and 0xFF) else null
    }
}
