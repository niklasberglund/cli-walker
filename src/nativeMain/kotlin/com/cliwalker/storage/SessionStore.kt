package com.cliwalker.storage

import com.cliwalker.model.SessionRecord
import kotlinx.cinterop.ByteVar
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.allocArray
import kotlinx.cinterop.convert
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.toKString
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import platform.posix.S_IRWXU
import platform.posix.fclose
import platform.posix.fflush
import platform.posix.fgets
import platform.posix.fopen
import platform.posix.fputs
import platform.posix.getenv
import platform.posix.mkdir

@OptIn(ExperimentalForeignApi::class)
class SessionStore {
    private val home: String = getenv("HOME")?.toKString() ?: "."
    private val dir: String = "$home/.cli-walker"
    private val path: String = "$dir/sessions.jsonl"
    private val json = Json { ignoreUnknownKeys = true }

    fun load(): List<SessionRecord> {
        val file = fopen(path, "r") ?: return emptyList()
        val records = mutableListOf<SessionRecord>()
        try {
            memScoped {
                val bufSize = 8192
                val buf = allocArray<ByteVar>(bufSize)
                while (fgets(buf, bufSize, file) != null) {
                    val line = buf.toKString().trim()
                    if (line.isNotEmpty()) {
                        try {
                            records.add(json.decodeFromString(line))
                        } catch (_: Throwable) {
                            // skip malformed lines
                        }
                    }
                }
            }
        } finally {
            fclose(file)
        }
        return records
    }

    fun append(record: SessionRecord) {
        ensureDir()
        val file = fopen(path, "a") ?: return
        try {
            fputs(json.encodeToString(record) + "\n", file)
            fflush(file)
        } finally {
            fclose(file)
        }
    }

    private fun ensureDir() {
        mkdir(dir, S_IRWXU.convert())
    }
}
