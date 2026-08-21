package dev.homebot.android.connection

import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

object EndpointPolicy {
    fun normalize(raw: String): Result<HttpUrl> = runCatching {
        val url = raw.trim().toHttpUrlOrNull()
            ?: error("HomeBot endpoint must be an absolute HTTP or HTTPS URL")
        require(url.username.isEmpty() && url.password.isEmpty()) {
            "HomeBot endpoint cannot contain credentials"
        }
        require(url.encodedPath == "/" && url.query == null && url.fragment == null) {
            "HomeBot endpoint cannot contain a path, query, or fragment"
        }
        require(url.isHttps || isLoopbackHost(url.host)) {
            "Non-loopback HomeBot endpoints require HTTPS"
        }
        url.newBuilder().encodedPath("/").build()
    }

    private fun isLoopbackHost(host: String): Boolean {
        val normalized = host.lowercase().removePrefix("[").removeSuffix("]")
        return normalized == "localhost" || normalized == "127.0.0.1" ||
            normalized == "::1" || normalized == "10.0.2.2"
    }

}
