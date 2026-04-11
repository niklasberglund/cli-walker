package com.cliwalker.tui

import kotlinx.cinterop.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import platform.posix.*

enum class KeyEvent {
    ArrowUp,
    ArrowDown,
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
                val byte = readByte() ?: continue

                when (byte) {
                    'q'.code, 'Q'.code -> {
                        emit(KeyEvent.Quit)
                        return@flow
                    }
                    ' '.code -> emit(KeyEvent.Space)
                    0x1B -> {
                        // Escape sequence: read [ and direction
                        val next = readByte() ?: continue
                        if (next != '['.code) continue
                        val dir = readByte() ?: continue
                        when (dir) {
                            'A'.code -> emit(KeyEvent.ArrowUp)
                            'B'.code -> emit(KeyEvent.ArrowDown)
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
        // Non-blocking: VMIN=0, VTIME=1 (0.1s timeout)
        raw.c_cc[VMIN] = 0u
        raw.c_cc[VTIME] = 1u
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
