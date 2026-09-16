package com.cliwalker.strava

import kotlinx.cinterop.ByteVar
import kotlinx.cinterop.ExperimentalForeignApi
import kotlinx.cinterop.IntVar
import kotlinx.cinterop.alloc
import kotlinx.cinterop.allocArray
import kotlinx.cinterop.convert
import kotlinx.cinterop.memScoped
import kotlinx.cinterop.set
import kotlinx.cinterop.ptr
import kotlinx.cinterop.reinterpret
import kotlinx.cinterop.sizeOf
import kotlinx.cinterop.toKString
import kotlinx.cinterop.value
import platform.posix.AF_INET
import platform.posix.POLLIN
import platform.posix.SOCK_STREAM
import platform.posix.SOL_SOCKET
import platform.posix.SO_REUSEADDR
import platform.posix.accept
import platform.posix.bind
import platform.posix.close
import platform.posix.listen
import platform.posix.memset
import platform.posix.poll
import platform.posix.pollfd
import platform.posix.recv
import platform.posix.setsockopt
import platform.posix.sockaddr
import platform.posix.sockaddr_in
import platform.posix.socket
import platform.posix.write

/**
 * The result of the single request the OAuth redirect delivers to our loopback listener.
 */
/**
 * 127.0.0.1 in network byte order, as stored in `sin_addr` on a little-endian host.
 * The posix interop package exposes neither `inet_addr` nor `htonl`.
 */
private const val LOOPBACK_ADDR: UInt = 0x0100007Fu

sealed interface RedirectResult {
    data class Code(val code: String, val scopes: List<String>) : RedirectResult
    data class Denied(val error: String) : RedirectResult
    data object TimedOut : RedirectResult
    data class Failed(val reason: String) : RedirectResult
}

/**
 * A one-shot HTTP listener on 127.0.0.1 that captures the OAuth redirect.
 *
 * Strava has no device-code flow, so the authorization code comes back as a browser
 * redirect. We bind a socket, hand Strava `http://localhost:<port>/callback` as the
 * redirect URI, serve exactly one request, and shut down.
 */
@OptIn(ExperimentalForeignApi::class)
class LoopbackServer(private val port: Int) {

    fun awaitRedirect(timeoutSeconds: Int): RedirectResult = memScoped {
        val server = socket(AF_INET, SOCK_STREAM, 0)
        if (server < 0) return RedirectResult.Failed("could not open socket")

        try {
            val reuse = alloc<IntVar>()
            reuse.value = 1
            setsockopt(server, SOL_SOCKET, SO_REUSEADDR, reuse.ptr, sizeOf<IntVar>().convert())

            val addr = alloc<sockaddr_in>()
            memset(addr.ptr, 0, sizeOf<sockaddr_in>().convert())
            addr.sin_family = AF_INET.convert()
            addr.sin_port = hostToNetworkPort(port)
            addr.sin_addr.s_addr = LOOPBACK_ADDR

            if (bind(server, addr.ptr.reinterpret<sockaddr>(), sizeOf<sockaddr_in>().convert()) != 0) {
                return RedirectResult.Failed("port $port is already in use")
            }
            if (listen(server, 1) != 0) {
                return RedirectResult.Failed("could not listen on port $port")
            }

            val pfd = alloc<pollfd>()
            pfd.fd = server
            pfd.events = POLLIN.toShort()
            val ready = poll(pfd.ptr, 1.convert(), timeoutSeconds * 1000)
            if (ready == 0) return RedirectResult.TimedOut
            if (ready < 0) return RedirectResult.Failed("poll failed")

            val client = accept(server, null, null)
            if (client < 0) return RedirectResult.Failed("accept failed")

            try {
                val bufSize = 8192
                val buf = allocArray<ByteVar>(bufSize)
                val received = recv(client, buf, (bufSize - 1).convert(), 0)
                if (received <= 0) return RedirectResult.Failed("empty request")
                buf[received.toInt()] = 0
                val request = buf.toKString()

                val result = parseRequestLine(request)
                respond(client, result)
                return result
            } finally {
                close(client)
            }
        } finally {
            close(server)
        }
    }

    /** Pulls the query parameters out of `GET /callback?code=...&scope=... HTTP/1.1`. */
    private fun parseRequestLine(request: String): RedirectResult {
        val line = request.lineSequence().firstOrNull() ?: return RedirectResult.Failed("no request line")
        val target = line.split(' ').getOrNull(1) ?: return RedirectResult.Failed("malformed request line")
        val query = target.substringAfter('?', "")
        if (query.isEmpty()) return RedirectResult.Failed("redirect carried no parameters")

        val params = query.split('&').mapNotNull { pair ->
            val key = pair.substringBefore('=')
            val value = pair.substringAfter('=', "")
            if (key.isEmpty()) null else key to urlDecode(value)
        }.toMap()

        params["error"]?.let { return RedirectResult.Denied(it) }
        val code = params["code"] ?: return RedirectResult.Failed("redirect carried no code")
        val scopes = params["scope"]?.split(',')?.filter { it.isNotEmpty() } ?: emptyList()
        return RedirectResult.Code(code, scopes)
    }

    private fun respond(client: Int, result: RedirectResult) {
        val message = when (result) {
            is RedirectResult.Code -> "Authorized. You can close this tab and return to the terminal."
            is RedirectResult.Denied -> "Authorization was declined. You can close this tab."
            else -> "Something went wrong. You can close this tab and try again."
        }
        val body = """
            <!doctype html><html><head><meta charset="utf-8"><title>cli-walker</title></head>
            <body style="font-family:-apple-system,sans-serif;padding:3rem;text-align:center">
            <h2>cli-walker &times; Strava</h2><p>$message</p></body></html>
        """.trimIndent()
        val response = buildString {
            append("HTTP/1.1 200 OK\r\n")
            append("Content-Type: text/html; charset=utf-8\r\n")
            append("Content-Length: ${body.encodeToByteArray().size}\r\n")
            append("Connection: close\r\n\r\n")
            append(body)
        }
        val bytes = response.encodeToByteArray()
        memScoped {
            val out = allocArray<ByteVar>(bytes.size)
            bytes.forEachIndexed { i, b -> out[i] = b }
            write(client, out, bytes.size.convert())
        }
    }

    /** htons() is not exposed through platform.posix, and a port is just a byte swap. */
    private fun hostToNetworkPort(port: Int): UShort =
        (((port and 0xFF) shl 8) or ((port shr 8) and 0xFF)).toUShort()

    private fun urlDecode(value: String): String {
        val bytes = mutableListOf<Byte>()
        var i = 0
        while (i < value.length) {
            when {
                value[i] == '%' && i + 2 < value.length -> {
                    val hex = value.substring(i + 1, i + 3).toIntOrNull(16)
                    if (hex == null) {
                        bytes.add(value[i].code.toByte())
                        i++
                    } else {
                        bytes.add(hex.toByte())
                        i += 3
                    }
                }
                value[i] == '+' -> {
                    bytes.add(' '.code.toByte())
                    i++
                }
                else -> {
                    value[i].toString().encodeToByteArray().forEach { bytes.add(it) }
                    i++
                }
            }
        }
        return bytes.toByteArray().decodeToString()
    }
}
