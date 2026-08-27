package dev.homebot.android.connection

import dev.homebot.protocol.ErrorEnvelope
import dev.homebot.protocol.Snapshot

enum class ClientAlertKind { BOT_FINISHED, NEEDS_INPUT, APPROVAL_REQUIRED, ROUTINE_RESULT, ERROR }

data class ClientAlert(
    val eventId: String,
    val kind: ClientAlertKind,
    val title: String,
    val detail: String,
    val chatId: String? = null,
    val botId: String? = null,
    val activityId: String? = null,
    val routineId: String? = null,
    val runId: String? = null,
)

internal fun ClientAlert.deepLink(): String = when {
    chatId != null -> buildString {
        append("homebot://chat/").append(chatId)
        val parameters = listOfNotNull(botId?.let { "bot=$it" }, activityId?.let { "activity=$it" })
        if (parameters.isNotEmpty()) append('?').append(parameters.joinToString("&"))
    }
    routineId != null -> buildString {
        append("homebot://routine/").append(routineId)
        runId?.let { append("?run=").append(it) }
    }
    else -> "homebot://settings"
}

data class SessionCredentials(
    val endpoint: String,
    val deviceId: String,
    val deviceSession: String,
) {
    override fun toString(): String =
        "SessionCredentials(endpoint=$endpoint, deviceId=$deviceId, deviceSession=[REDACTED])"
}

interface SessionStore {
    suspend fun load(): SessionCredentials?
    suspend fun save(credentials: SessionCredentials)
    suspend fun clear()
}

sealed interface ConnectionState {
    data object Unpaired : ConnectionState
    data object Pairing : ConnectionState
    data class Connecting(val endpoint: String) : ConnectionState
    data class Hydrating(val endpoint: String, val resumeAfter: Long?) : ConnectionState
    data class Live(val endpoint: String, val cursor: Long, val snapshot: Snapshot) : ConnectionState
    data class Reconnecting(val endpoint: String, val cursor: Long?, val attempt: Int) : ConnectionState
    data class VersionIncompatible(val serverMinimum: Int?, val serverMaximum: Int?) : ConnectionState
    data object Revoked : ConnectionState
    data class Offline(val failure: ClientFailure, val cursor: Long?) : ConnectionState
}

sealed interface ClientFailure {
    data class Structured(val status: Int, val error: ErrorEnvelope) : ClientFailure
    data class InvalidEndpoint(val message: String) : ClientFailure
    data class Protocol(val message: String) : ClientFailure
    data class Network(val message: String) : ClientFailure
}

sealed interface DisconnectReason {
    data object Stopped : DisconnectReason
    data object Revoked : DisconnectReason
    data object VersionIncompatible : DisconnectReason
    data class Retry(val failure: ClientFailure) : DisconnectReason
}
