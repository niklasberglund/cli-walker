package com.cliwalker.strava

import com.cliwalker.model.SessionRecord
import io.ktor.http.encodeURLParameter
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime
import platform.posix.system

/** Default port for the loopback OAuth listener. */
const val DEFAULT_CALLBACK_PORT = 8721

sealed interface LoginResult {
    data class Success(val athleteName: String?) : LoginResult
    data class Failure(val reason: String) : LoginResult
}

sealed interface UploadResult {
    /**
     * [activityId] is null when Strava accepted the activity but had not finished
     * processing it. [warning] flags anything that failed after it was already accepted.
     */
    data class Uploaded(val activityId: Long?, val warning: String? = null) : UploadResult
    data object AlreadyUploaded : UploadResult
    data object NotConnected : UploadResult
    /** The session is shorter than the configured minimum upload distance. */
    data class TooShort(val distanceKm: Double, val thresholdKm: Double) : UploadResult
    data class Failed(val reason: String) : UploadResult
}

/**
 * Ties together config storage, the OAuth loopback dance, and activity creation.
 */
class StravaService(
    private val store: StravaConfigStore = StravaConfigStore(),
    private val uploadLog: UploadLog = UploadLog(),
) {
    fun config(): StravaConfig? = store.load()

    fun isConnected(): Boolean = store.load()?.isAuthorized == true

    fun logout() = store.clear()

    /**
     * Turns feed muting on or off for future uploads. Returns false if there is nothing
     * configured yet to attach the setting to.
     */
    fun setHideFromFeed(hide: Boolean): Boolean {
        val config = store.load() ?: return false
        store.save(config.copy(hideFromFeed = hide))
        return true
    }

    /** Minimum session distance (km) that will be uploaded; 0 means "upload everything". */
    fun minUploadDistanceKm(): Double = store.load()?.minUploadDistanceKm ?: 0.0

    /**
     * Sets the minimum session distance (km) for uploads. Returns false when there is no
     * config yet to attach the setting to (i.e. before `strava login`).
     */
    fun setMinUploadDistanceKm(km: Double): Boolean {
        val config = store.load() ?: return false
        store.save(config.copy(minUploadDistanceKm = km.coerceAtLeast(0.0)))
        return true
    }

    fun activityIdFor(record: SessionRecord): Long? = uploadLog.activityIdFor(record)

    /**
     * Runs the full authorization code flow. [onPrompt] is called with the authorize URL
     * so the caller can display it before the browser opens.
     */
    @OptIn(ExperimentalForeignApi::class)
    suspend fun login(
        clientId: String,
        clientSecret: String,
        port: Int = DEFAULT_CALLBACK_PORT,
        timeoutSeconds: Int = 180,
        openBrowser: Boolean = true,
        onPrompt: (String) -> Unit = {},
    ): LoginResult {
        // Re-authorizing (for a new scope, say) must not silently reset preferences, and
        // must not drop working tokens before the new grant has actually succeeded.
        val existing = store.load()
        val config = StravaConfig(
            clientId = clientId,
            clientSecret = clientSecret,
            hideFromFeed = existing?.hideFromFeed ?: false,
            minUploadDistanceKm = existing?.minUploadDistanceKm ?: 0.0,
        )
        if (existing == null) store.save(config)

        val redirectUri = "http://localhost:$port/callback"
        val authorizeUrl = buildString {
            append("$STRAVA_OAUTH_BASE/authorize")
            append("?client_id=${clientId.encodeURLParameter()}")
            append("&redirect_uri=${redirectUri.encodeURLParameter()}")
            append("&response_type=code")
            append("&approval_prompt=auto")
            append("&scope=${STRAVA_SCOPE.encodeURLParameter()}")
        }

        val server = LoopbackServer(port)
        onPrompt(authorizeUrl)
        if (openBrowser) system("open '$authorizeUrl' >/dev/null 2>&1")

        return when (val result = server.awaitRedirect(timeoutSeconds)) {
            is RedirectResult.Code -> {
                // STRAVA_SCOPE is a comma-joined request; Strava grants them individually.
                val missing = STRAVA_SCOPE.split(",").filterNot { it in result.scopes }
                if (missing.isNotEmpty()) {
                    return LoginResult.Failure(
                        "Strava did not grant ${missing.joinToString(", ")} — " +
                            "re-run login and leave every permission checkbox ticked.",
                    )
                }
                val api = StravaApi(store)
                try {
                    val updated = api.exchangeCode(config, result.code)
                    LoginResult.Success(updated.athleteName)
                } catch (e: Exception) {
                    LoginResult.Failure(e.message ?: "token exchange failed")
                } finally {
                    api.close()
                }
            }
            is RedirectResult.Denied -> LoginResult.Failure("authorization was declined (${result.error})")
            is RedirectResult.Failed -> LoginResult.Failure(result.reason)
            RedirectResult.TimedOut -> LoginResult.Failure("timed out waiting for the browser redirect")
        }
    }

    /**
     * Uploads one session as a manual Strava activity, skipping sessions already sent.
     */
    suspend fun upload(record: SessionRecord, ignoreThreshold: Boolean = false): UploadResult {
        if (uploadLog.isUploaded(record)) return UploadResult.AlreadyUploaded
        val config = store.load()?.takeIf { it.isAuthorized } ?: return UploadResult.NotConnected
        if (!ignoreThreshold && record.distanceKm < config.minUploadDistanceKm) {
            return UploadResult.TooShort(record.distanceKm, config.minUploadDistanceKm)
        }

        val api = StravaApi(store)
        return try {
            val (upload, _) = api.uploadActivity(config, record, activityName(record))
            uploadLog.record(record, upload.activityId)
            UploadResult.Uploaded(upload.activityId, upload.warning)
        } catch (e: Exception) {
            UploadResult.Failed(e.message ?: "upload failed")
        } finally {
            api.close()
        }
    }

    /**
     * Sessions that have never been uploaded, oldest first. Sessions below the configured
     * minimum distance are left out unless [ignoreThreshold] is set, so the pending count
     * matches what a sync would actually send.
     */
    fun pending(records: List<SessionRecord>, ignoreThreshold: Boolean = false): List<SessionRecord> {
        val threshold = if (ignoreThreshold) 0.0 else minUploadDistanceKm()
        return records
            .filterNot { uploadLog.isUploaded(it) }
            .filter { it.distanceKm >= threshold }
            .sortedBy { it.startedAt }
    }
}

/** Names the activity the way Strava names its own: by time of day. */
fun activityName(record: SessionRecord): String {
    val hour = record.startedAt.toLocalDateTime(TimeZone.currentSystemDefault()).hour
    val partOfDay = when (hour) {
        in 5..11 -> "Morning"
        in 12..13 -> "Lunch"
        in 14..17 -> "Afternoon"
        in 18..21 -> "Evening"
        else -> "Night"
    }
    return "$partOfDay Treadmill Walk"
}

/**
 * Renders a distance threshold the way it is most readable: metres below 1 km, km above.
 * 0 means no threshold at all.
 */
fun formatThreshold(km: Double): String = when {
    km <= 0.0 -> "off"
    km < 1.0 -> "${(km * 1000).toInt()} m"
    else -> {
        val hundreds = (km * 100).toInt()
        val frac = hundreds % 100
        if (frac == 0) "${hundreds / 100} km"
        else "${hundreds / 100}.${frac.toString().padStart(2, '0').trimEnd('0')} km"
    }
}

/**
 * Parses a user-typed threshold into km. Accepts "500m", "1.2km", a bare number (km),
 * and "off"/"none"/"0". Returns null if it cannot be understood.
 */
fun parseThreshold(input: String): Double? {
    val text = input.trim().lowercase().replace(",", ".")
    if (text.isEmpty()) return null
    if (text == "off" || text == "none" || text == "0") return 0.0
    val (number, factor) = when {
        text.endsWith("km") -> text.dropLast(2) to 1.0
        text.endsWith("m") -> text.dropLast(1) to 0.001
        else -> text to 1.0
    }
    val value = number.trim().toDoubleOrNull() ?: return null
    if (value < 0) return null
    return value * factor
}
