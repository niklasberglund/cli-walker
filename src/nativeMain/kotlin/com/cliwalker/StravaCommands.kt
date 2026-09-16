package com.cliwalker

import com.cliwalker.model.SessionRecord
import com.cliwalker.storage.SessionStore
import com.cliwalker.strava.DEFAULT_CALLBACK_PORT
import com.cliwalker.strava.LoginResult
import com.cliwalker.strava.StravaService
import com.cliwalker.strava.UploadResult
import com.cliwalker.strava.activityName
import com.cliwalker.strava.formatThreshold
import com.cliwalker.strava.parseThreshold
import com.cliwalker.strava.writeTcxFiles
import com.github.ajalt.clikt.core.CliktCommand
import com.github.ajalt.clikt.parameters.arguments.argument
import com.github.ajalt.clikt.parameters.arguments.optional
import com.github.ajalt.clikt.parameters.options.default
import com.github.ajalt.clikt.parameters.options.flag
import com.github.ajalt.clikt.parameters.options.option
import com.github.ajalt.clikt.parameters.types.choice
import com.github.ajalt.clikt.parameters.types.int
import com.github.ajalt.mordant.rendering.TextColors.*
import com.github.ajalt.mordant.rendering.TextStyles.*
import com.github.ajalt.mordant.terminal.Terminal
import com.github.ajalt.mordant.terminal.prompt
import kotlinx.coroutines.runBlocking
import kotlinx.datetime.TimeZone
import kotlinx.datetime.toLocalDateTime

class StravaCommand : CliktCommand(name = "strava") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Connect to Strava and upload walking sessions"

    override fun run() = Unit
}

class StravaLogin : CliktCommand(name = "login") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Authorize cli-walker against your Strava API application"

    private val clientIdOption by option("--client-id", envvar = "STRAVA_CLIENT_ID")
    private val clientSecretOption by option("--client-secret", envvar = "STRAVA_CLIENT_SECRET")
    private val port by option("--port", help = "Local port for the OAuth redirect")
        .int()
        .default(DEFAULT_CALLBACK_PORT)

    private val noBrowser by option("--no-browser", help = "Print the authorize URL instead of opening a browser")
        .flag()

    override fun run() = runBlocking {
        val terminal = Terminal()
        val service = StravaService()
        val existing = service.config()

        val clientId = clientIdOption ?: existing?.clientId ?: terminal.prompt("Strava Client ID")
        val clientSecret = clientSecretOption ?: existing?.clientSecret
            ?: terminal.prompt("Strava Client Secret", hideInput = true)

        if (clientId.isNullOrBlank() || clientSecret.isNullOrBlank()) {
            terminal.println(red("Client ID and Client Secret are required."))
            terminal.println(
                "Register an application at ${bold("https://www.strava.com/settings/api")} " +
                    "and set the callback domain to ${bold("localhost")}.",
            )
            return@runBlocking
        }

        terminal.println()
        if (!noBrowser) terminal.println("Opening your browser to authorize with Strava…")
        terminal.println(dim("Waiting for the redirect on http://localhost:$port/callback"))

        val result = service.login(clientId, clientSecret, port, openBrowser = !noBrowser) { url ->
            terminal.println(if (noBrowser) "Open this URL to authorize:" else dim("If the browser does not open, visit:"))
            terminal.println(dim(url))
        }

        terminal.println()
        when (result) {
            is LoginResult.Success -> {
                val who = result.athleteName?.let { " as ${bold(it)}" } ?: ""
                terminal.println(green("Connected to Strava$who."))
                terminal.println("Finished sessions will upload automatically. Run ${bold("cli-walker strava sync")} to send past ones.")
            }
            is LoginResult.Failure -> {
                terminal.println(red("Login failed: ${result.reason}"))
                if ("in use" in result.reason) {
                    terminal.println("Try a different port with ${bold("--port")}.")
                }
            }
        }
    }
}

class StravaStatus : CliktCommand(name = "status") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Show the Strava connection and how many sessions are waiting to upload"

    override fun run() {
        val terminal = Terminal()
        val service = StravaService()
        val config = service.config()

        if (config == null || !config.isAuthorized) {
            terminal.println(yellow("Not connected to Strava."))
            terminal.println("Run ${bold("cli-walker strava login")} to connect.")
            return
        }

        val who = config.athleteName ?: "unknown athlete"
        terminal.println(green("Connected to Strava as ${bold(who)}."))
        terminal.println(
            if (config.hideFromFeed) {
                "Uploads are ${bold("muted")} — hidden from followers' feeds, still on your profile."
            } else {
                "Uploads are ${bold("visible")} in followers' feeds. Mute them with ${bold("cli-walker strava mute on")}."
            },
        )

        terminal.println(
            if (config.minUploadDistanceKm > 0) {
                "Only sessions of at least ${bold(formatThreshold(config.minUploadDistanceKm))} are uploaded."
            } else {
                "Every session is uploaded, however short. Set a minimum with ${bold("cli-walker strava threshold 500m")}."
            },
        )

        val history = SessionStore().load()
        val pending = service.pending(history)
        if (pending.isEmpty()) {
            terminal.println("All sessions have been uploaded.")
        } else {
            terminal.println("${bold(pending.size.toString())} session(s) waiting to upload — run ${bold("cli-walker strava sync")}.")
        }
        val tooShort = service.pending(history, ignoreThreshold = true).size - pending.size
        if (tooShort > 0) {
            terminal.println(dim("$tooShort session(s) skipped for being below the threshold."))
        }
    }
}

class StravaLogout : CliktCommand(name = "logout") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Forget the stored Strava credentials and tokens"

    override fun run() {
        StravaService().logout()
        Terminal().println("Disconnected from Strava. Stored credentials removed.")
    }
}

class StravaMute : CliktCommand(name = "mute") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Keep uploaded activities out of your followers' feeds"

    override fun helpEpilog(context: com.github.ajalt.clikt.core.Context) =
        "Muting is not the same as private. Strava's API cannot set activity visibility; " +
            "for that, set Privacy Controls > Activities to \"Only You\" at " +
            "https://www.strava.com/settings/privacy"

    private val state by argument(help = "on or off").choice("on", "off").optional()

    override fun run() {
        val terminal = Terminal()
        val service = StravaService()
        val config = service.config()

        if (config == null) {
            terminal.println(yellow("Not connected to Strava. Run ${bold("cli-walker strava login")} first."))
            return
        }

        if (state == null) {
            terminal.println("Uploads are currently ${bold(if (config.hideFromFeed) "muted" else "visible")}.")
            return
        }

        val hide = state == "on"
        service.setHideFromFeed(hide)
        if (hide) {
            terminal.println(green("Uploads will be muted — they stay off your followers' feeds."))
            terminal.println(
                dim("This is not the same as private: muted activities are still visible on your profile."),
            )
            terminal.println(
                dim("For real privacy set Privacy Controls > Activities to \"Only You\" at https://www.strava.com/settings/privacy"),
            )
        } else {
            terminal.println(green("Uploads will appear in followers' feeds again."))
        }
    }
}

class StravaThreshold : CliktCommand(name = "threshold") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Set the minimum session distance that gets uploaded to Strava"

    override fun helpEpilog(context: com.github.ajalt.clikt.core.Context) =
        "Examples: `strava threshold 500m`, `strava threshold 1.5km`, `strava threshold 2` (km), " +
            "`strava threshold off`"

    private val value by argument(help = "Distance such as 500m, 1.5km, or off").optional()

    override fun run() {
        val terminal = Terminal()
        val service = StravaService()
        val config = service.config()

        if (config == null) {
            terminal.println(yellow("Not connected to Strava. Run ${bold("cli-walker strava login")} first."))
            return
        }

        val raw = value
        if (raw == null) {
            terminal.println(
                if (config.minUploadDistanceKm > 0) {
                    "Minimum upload distance is ${bold(formatThreshold(config.minUploadDistanceKm))}."
                } else {
                    "No minimum upload distance — every session is uploaded."
                },
            )
            return
        }

        val km = parseThreshold(raw)
        if (km == null) {
            terminal.println(red("Could not read \"$raw\" as a distance. Try 500m, 1.5km, or off."))
            return
        }

        service.setMinUploadDistanceKm(km)
        if (km <= 0.0) {
            terminal.println(green("Minimum upload distance removed — every session will be uploaded."))
        } else {
            terminal.println(green("Sessions shorter than ${bold(formatThreshold(km))} will not be uploaded."))
        }
    }
}

class StravaSync : CliktCommand(name = "sync") {
    override fun help(context: com.github.ajalt.clikt.core.Context) =
        "Upload every recorded session that has not reached Strava yet"

    private val limit by option("--limit", help = "Only upload the N oldest pending sessions").int()

    private val dryRun by option("--dry-run", help = "Write the TCX files that would be uploaded, and stop")
        .flag()

    private val all by option("--all", help = "Include sessions below the minimum upload distance")
        .flag()

    override fun run() = runBlocking {
        val terminal = Terminal()
        val service = StravaService()

        if (!service.isConnected()) {
            terminal.println(yellow("Not connected to Strava. Run ${bold("cli-walker strava login")} first."))
            return@runBlocking
        }

        val pending = service.pending(SessionStore().load(), ignoreThreshold = all).let { list ->
            limit?.let { list.take(it) } ?: list
        }
        if (pending.isEmpty()) {
            terminal.println(green("Nothing to upload — Strava is up to date."))
            val threshold = service.minUploadDistanceKm()
            if (!all && threshold > 0) {
                val skipped = service.pending(SessionStore().load(), ignoreThreshold = true).size
                if (skipped > 0) {
                    terminal.println(
                        dim("$skipped session(s) are below the ${formatThreshold(threshold)} threshold — upload them with ${bold("--all")}."),
                    )
                }
            }
            return@runBlocking
        }

        if (dryRun) {
            val dir = writeTcxFiles(pending)
            terminal.println("Wrote ${pending.size} TCX file(s) to ${bold(dir)} — nothing was uploaded.")
            return@runBlocking
        }

        terminal.println("Uploading ${pending.size} session(s) to Strava…")
        var uploaded = 0
        for (record in pending) {
            val label = sessionLabel(record)
            when (val result = service.upload(record, ignoreThreshold = all)) {
                is UploadResult.Uploaded -> {
                    uploaded++
                    val where = result.activityId
                        ?.let { "strava.com/activities/$it" }
                        ?: "Strava (no activity id returned)"
                    terminal.println(green("  ✓ $label → $where"))
                    result.warning?.let { terminal.println(yellow("    ! $it")) }
                }
                UploadResult.AlreadyUploaded -> terminal.println(dim("  · $label (already uploaded)"))
                is UploadResult.TooShort -> terminal.println(
                    dim("  · $label (below the ${formatThreshold(result.thresholdKm)} threshold)"),
                )
                UploadResult.NotConnected -> {
                    terminal.println(red("  ✗ Lost the Strava connection. Run login again."))
                    break
                }
                is UploadResult.Failed -> {
                    terminal.println(red("  ✗ $label — ${result.reason}"))
                    // Rate limits and auth failures will hit every remaining session too.
                    if ("rate limit" in result.reason || "login" in result.reason) break
                }
            }
        }
        terminal.println()
        terminal.println("Uploaded $uploaded of ${pending.size}.")
    }
}

internal fun sessionLabel(record: SessionRecord): String {
    val local = record.startedAt.toLocalDateTime(TimeZone.currentSystemDefault())
    val month = local.monthNumber.toString().padStart(2, '0')
    val day = local.dayOfMonth.toString().padStart(2, '0')
    val hour = local.hour.toString().padStart(2, '0')
    val minute = local.minute.toString().padStart(2, '0')
    val km = ((record.distanceKm * 100).toInt() / 100.0)
    return "${local.year}-$month-$day $hour:$minute · $km km · ${activityName(record)}"
}
