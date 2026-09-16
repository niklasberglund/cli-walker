package com.cliwalker.strava

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

/**
 * Remembers which sessions already reached Strava, keyed by session start time.
 *
 * Kept beside the append-only session log rather than inside it, so uploading never
 * has to rewrite sessions.jsonl.
 */
/**
 * Marks a session that reached Strava but whose activity id we never learned, because
 * Strava answered the create with an empty body. Logging it still matters: it is what
 * stops the session being uploaded a second time.
 */
private const val UNKNOWN_ACTIVITY_ID = 0L

@OptIn(ExperimentalForeignApi::class)
class UploadLog {
    private val home: String = getenv("HOME")?.toKString() ?: "."
    private val dir: String = "$home/.cli-walker"
    private val path: String = "$dir/strava-uploads.json"
    private val json = Json { ignoreUnknownKeys = true; prettyPrint = true }

    private var cache: MutableMap<String, Long>? = null

    private fun entries(): MutableMap<String, Long> {
        cache?.let { return it }
        val loaded = readFile()?.let {
            try {
                json.decodeFromString<Map<String, Long>>(it).toMutableMap()
            } catch (_: Throwable) {
                null
            }
        } ?: mutableMapOf()
        cache = loaded
        return loaded
    }

    /** The Strava activity id, or null if it is unknown (see [UNKNOWN_ACTIVITY_ID]). */
    fun activityIdFor(record: SessionRecord): Long? =
        entries()[key(record)]?.takeIf { it != UNKNOWN_ACTIVITY_ID }

    // Deliberately keyed on presence, not on the id: a session logged with an unknown
    // id has still been uploaded and must never be sent again.
    fun isUploaded(record: SessionRecord): Boolean = entries().containsKey(key(record))

    fun record(record: SessionRecord, activityId: Long?) {
        val current = entries()
        current[key(record)] = activityId ?: UNKNOWN_ACTIVITY_ID
        mkdir(dir, S_IRWXU.convert())
        val file = fopen(path, "w") ?: return
        try {
            fputs(json.encodeToString(current.toMap()), file)
            fflush(file)
        } finally {
            fclose(file)
        }
    }

    private fun key(record: SessionRecord) = record.startedAt.toString()

    private fun readFile(): String? {
        val file = fopen(path, "r") ?: return null
        val sb = StringBuilder()
        try {
            memScoped {
                val bufSize = 8192
                val buf = allocArray<ByteVar>(bufSize)
                while (fgets(buf, bufSize, file) != null) {
                    sb.append(buf.toKString())
                }
            }
        } finally {
            fclose(file)
        }
        return sb.toString()
    }
}
