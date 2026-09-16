package com.cliwalker.strava

import com.cliwalker.model.SessionRecord
import io.ktor.client.HttpClient
import io.ktor.client.engine.darwin.Darwin
import io.ktor.client.request.forms.formData
import io.ktor.client.request.forms.submitForm
import io.ktor.client.request.forms.submitFormWithBinaryData
import io.ktor.client.request.get
import io.ktor.client.request.header
import io.ktor.client.request.put
import io.ktor.client.request.setBody
import io.ktor.client.statement.HttpResponse
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.Headers
import io.ktor.http.HttpHeaders
import io.ktor.http.HttpStatusCode
import io.ktor.http.contentType
import io.ktor.http.parameters
import kotlinx.coroutines.delay
import kotlinx.datetime.Clock
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

const val STRAVA_OAUTH_BASE = "https://www.strava.com/oauth"
const val STRAVA_API_BASE = "https://www.strava.com/api/v3"

/**
 * `activity:write` creates the upload; `activity:read` is what lets us read the resulting
 * activity back. Private ("Only You") activities need `activity:read_all` instead, which
 * is the state to expect while developing against Strava's privacy default.
 */
const val STRAVA_SCOPE = "activity:write,activity:read_all"

/** How long to wait for Strava to turn an upload into an activity. */
private const val PROCESSING_TIMEOUT_SECONDS = 90
private const val PROCESSING_POLL_SECONDS = 2

class StravaException(message: String) : Exception(message)

/**
 * The outcome of an upload. [warning] describes anything that went wrong *after* Strava
 * had already accepted the activity — the upload still counts as done.
 */
data class ActivityUpload(val activityId: Long?, val warning: String? = null)

@Serializable
private data class TokenResponse(
    @SerialName("access_token") val accessToken: String,
    @SerialName("refresh_token") val refreshToken: String,
    @SerialName("expires_at") val expiresAt: Long,
    val athlete: Athlete? = null,
) {
    @Serializable
    data class Athlete(val firstname: String? = null, val lastname: String? = null)
}

@Serializable
private data class UploadResponse(
    val id: Long,
    val error: String? = null,
    val status: String? = null,
    @SerialName("activity_id") val activityId: Long? = null,
)

@Serializable
private data class ErrorResponse(val message: String? = null, val errors: List<Detail> = emptyList()) {
    @Serializable
    data class Detail(val resource: String? = null, val field: String? = null, val code: String? = null)
}

class StravaApi(private val store: StravaConfigStore) {
    private val json = Json { ignoreUnknownKeys = true }
    private val client = HttpClient(Darwin)

    fun close() = client.close()

    /**
     * Exchanges an authorization code for tokens and persists them.
     */
    suspend fun exchangeCode(config: StravaConfig, code: String): StravaConfig {
        val response = client.submitForm(
            url = "$STRAVA_OAUTH_BASE/token",
            formParameters = parameters {
                append("client_id", config.clientId)
                append("client_secret", config.clientSecret)
                append("code", code)
                append("grant_type", "authorization_code")
            },
        )
        val tokens = json.decodeFromString<TokenResponse>(requireSuccess(response, "token exchange"))
        val name = tokens.athlete?.let { listOfNotNull(it.firstname, it.lastname).joinToString(" ") }
        val updated = config.copy(
            accessToken = tokens.accessToken,
            refreshToken = tokens.refreshToken,
            expiresAt = tokens.expiresAt,
            athleteName = name?.takeIf { it.isNotBlank() },
        )
        store.save(updated)
        return updated
    }

    /**
     * Returns a valid access token, refreshing it first if it is expired or nearly so.
     *
     * Strava rotates refresh tokens, so whatever comes back is persisted — including
     * the refresh token, which is not always the one we sent.
     */
    suspend fun validAccessToken(config: StravaConfig): Pair<String, StravaConfig> {
        val refreshToken = config.refreshToken
            ?: throw StravaException("Not connected to Strava. Run `cli-walker strava login` first.")

        val stillValid = config.accessToken != null &&
            config.expiresAt - 60 > Clock.System.now().epochSeconds
        if (stillValid) return config.accessToken!! to config

        val response = client.submitForm(
            url = "$STRAVA_OAUTH_BASE/token",
            formParameters = parameters {
                append("client_id", config.clientId)
                append("client_secret", config.clientSecret)
                append("refresh_token", refreshToken)
                append("grant_type", "refresh_token")
            },
        )
        val tokens = json.decodeFromString<TokenResponse>(requireSuccess(response, "token refresh"))
        val updated = config.copy(
            accessToken = tokens.accessToken,
            refreshToken = tokens.refreshToken,
            expiresAt = tokens.expiresAt,
        )
        store.save(updated)
        return tokens.accessToken to updated
    }

    /**
     * Uploads a session to Strava as a TCX file and returns the resulting activity id.
     *
     * Uploading rather than creating a manual activity is what earns the app attribution
     * and device name on the activity page. The cost is that it is asynchronous: Strava
     * accepts the file, then processes it into an activity we have to wait for.
     *
     * A null id means the file was accepted but processing did not finish in time. The
     * activity still arrives, so callers must treat that as success — retrying would
     * upload a duplicate.
     */
    suspend fun uploadActivity(
        config: StravaConfig,
        record: SessionRecord,
        name: String,
    ): Pair<ActivityUpload, StravaConfig> {
        val (token, updated) = validAccessToken(config)
        val externalId = "cli-walker-${record.startedAt.epochSeconds}"
        val tcx = buildTcx(record).encodeToByteArray()

        val response = client.submitFormWithBinaryData(
            url = "$STRAVA_API_BASE/uploads",
            formData = formData {
                append(
                    "file",
                    tcx,
                    Headers.build {
                        append(HttpHeaders.ContentType, "application/xml")
                        append(HttpHeaders.ContentDisposition, "filename=\"$externalId.tcx\"")
                    },
                )
                append("data_type", "tcx")
                append("name", name)
                append("description", describe(record))
                append("trainer", "1")
                append("external_id", externalId)
            },
        ) {
            header("Authorization", "Bearer $token")
        }

        val upload = json.decodeFromString<UploadResponse>(requireSuccess(response, "activity upload"))
        upload.error?.let { throw StravaException("Strava rejected the upload: $it") }

        val activityId = awaitProcessing(token, upload.id)

        // Past this point the activity exists on Strava. Anything that fails from here is
        // a warning, never an error: reporting failure would re-upload it as a duplicate.
        var warning: String? = null
        if (activityId != null) {
            // TCX has no Walk sport, so the activity lands as a generic workout until fixed.
            try {
                finishActivity(token, activityId, config.hideFromFeed)
            } catch (e: Exception) {
                warning = "uploaded, but could not set the activity type: ${e.message}"
            }
        }

        return ActivityUpload(activityId, warning) to updated
    }

    /**
     * Polls the upload until Strava turns it into an activity. Returns null if it is still
     * processing when we give up — the activity is still on its way.
     */
    private suspend fun awaitProcessing(token: String, uploadId: Long): Long? {
        var waited = 0
        while (waited < PROCESSING_TIMEOUT_SECONDS) {
            delay(PROCESSING_POLL_SECONDS * 1000L)
            waited += PROCESSING_POLL_SECONDS

            val response = client.get("$STRAVA_API_BASE/uploads/$uploadId") {
                header("Authorization", "Bearer $token")
            }
            val upload = json.decodeFromString<UploadResponse>(requireSuccess(response, "upload status"))
            upload.error?.let { throw StravaException("Strava rejected the upload: $it") }
            upload.activityId?.let { return it }
        }
        return null
    }

    /**
     * Corrects the processed activity: TCX cannot express a walk, and muting is only ever
     * available on update. Both fit in the one call.
     */
    private suspend fun finishActivity(token: String, activityId: Long, hideFromFeed: Boolean) {
        val fields = buildString {
            append("""{"sport_type":"Walk"""")
            if (hideFromFeed) append(""","hide_from_home":true""")
            append("}")
        }
        val response = client.put("$STRAVA_API_BASE/activities/$activityId") {
            header("Authorization", "Bearer $token")
            contentType(ContentType.Application.Json)
            setBody(fields)
        }
        requireSuccess(response, "setting activity type")
    }

    private suspend fun requireSuccess(response: HttpResponse, what: String): String {
        val body = response.bodyAsText()
        if (response.status.isSuccess()) return body

        val detail = try {
            val error = json.decodeFromString<ErrorResponse>(body)
            val fields = error.errors.mapNotNull { it.field?.let { f -> "$f ${it.code ?: ""}".trim() } }
            listOfNotNull(error.message, fields.takeIf { it.isNotEmpty() }?.joinToString(", "))
                .joinToString(": ")
                .ifBlank { body }
        } catch (_: Throwable) {
            body
        }
        val hint = when (response.status) {
            HttpStatusCode.Unauthorized -> " — try `cli-walker strava login` again"
            HttpStatusCode.TooManyRequests -> " — Strava rate limit reached, try again later"
            else -> ""
        }
        throw StravaException("Strava $what failed (${response.status.value}): $detail$hint")
    }
}

private fun HttpStatusCode.isSuccess() = value in 200..299

private fun describe(record: SessionRecord) =
    "$DEVICE_NAME · ${record.steps} steps · " +
        "avg ${formatSpeed(record.avgSpeedKmh)} km/h · max ${formatSpeed(record.maxSpeedKmh)} km/h"

private fun formatSpeed(value: Double): String {
    val rounded = (value * 10).toInt()
    return "${rounded / 10}.${rounded % 10}"
}
