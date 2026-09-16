package com.cliwalker.strava

import kotlinx.cinterop.ByteVar
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.allocArray
import kotlinx.cinterop.convert
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.toKString
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import platform.posix.S_IRUSR
import platform.posix.S_IRWXU
import platform.posix.S_IWUSR
import platform.posix.O_CREAT
import platform.posix.O_TRUNC
import platform.posix.O_WRONLY
import platform.posix.chmod
import platform.posix.close
import platform.posix.fclose
import platform.posix.fdopen
import platform.posix.fflush
import platform.posix.fgets
import platform.posix.fopen
import platform.posix.fputs
import platform.posix.getenv
import platform.posix.mkdir
import platform.posix.open
import platform.posix.remove

/**
 * Strava credentials and OAuth tokens, persisted to ~/.cli-walker/strava.json (mode 0600).
 *
 * The client secret is not really a secret in a distributed CLI — Strava's token
 * endpoint requires it, and there is no PKCE-style flow that avoids it. Each user
 * registers their own API application at https://www.strava.com/settings/api.
 */
@Serializable
data class StravaConfig(
    val clientId: String,
    val clientSecret: String,
    val accessToken: String? = null,
    val refreshToken: String? = null,
    /** Unix epoch seconds at which [accessToken] expires. */
    val expiresAt: Long = 0,
    val athleteName: String? = null,
    /**
     * Mute uploaded activities so they never reach followers' feeds. This is Strava's
     * `hide_from_home` flag, not true privacy — the activity is still on your profile.
     * Real "Only You" visibility is not exposed by the API; it is an account setting.
     */
    val hideFromFeed: Boolean = false,
    /**
     * Sessions shorter than this distance (in km) are never uploaded. 0 uploads everything.
     * Stored in km even though the UI lets you think in metres.
     */
    val minUploadDistanceKm: Double = 0.0,
) {
    val isAuthorized: Boolean get() = refreshToken != null
}

@OptIn(ExperimentalForeignApi::class)
class StravaConfigStore {
    private val home: String = getenv("HOME")?.toKString() ?: "."
    private val dir: String = "$home/.cli-walker"
    private val path: String = "$dir/strava.json"
    private val json = Json { ignoreUnknownKeys = true; prettyPrint = true }

    fun load(): StravaConfig? {
        val text = readFile(path) ?: return null
        return try {
            json.decodeFromString<StravaConfig>(text)
        } catch (_: Throwable) {
            null
        }
    }

    fun save(config: StravaConfig) {
        mkdir(dir, S_IRWXU.convert())

        // Create the file 0600 up front rather than chmod-ing after writing: the tokens
        // must never exist on disk under the default umask, even briefly.
        val ownerReadWrite = (S_IRUSR or S_IWUSR)
        val fd = open(path, O_WRONLY or O_CREAT or O_TRUNC, ownerReadWrite)
        if (fd < 0) error("Cannot write $path")

        val file = fdopen(fd, "w")
        if (file == null) {
            close(fd)
            error("Cannot write $path")
        }
        try {
            fputs(json.encodeToString(config), file)
            fflush(file)
        } finally {
            fclose(file)
        }

        // O_CREAT only applies the mode to a file it actually creates, so an existing
        // file keeps whatever permissions it already had.
        chmod(path, ownerReadWrite.convert())
    }

    fun clear() {
        remove(path)
    }

    private fun readFile(path: String): String? {
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
