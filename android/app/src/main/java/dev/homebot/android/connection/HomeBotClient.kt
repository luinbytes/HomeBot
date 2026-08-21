package dev.homebot.android.connection

import dev.homebot.protocol.BotSummary
import dev.homebot.protocol.ChatSummary
import dev.homebot.protocol.ClientMessage
import dev.homebot.protocol.ErrorEnvelope
import dev.homebot.protocol.ExchangePairingRequest
import dev.homebot.protocol.PROTOCOL_VERSION
import dev.homebot.protocol.PairingExchangeResponse
import dev.homebot.protocol.ProtocolRange
import dev.homebot.protocol.Snapshot
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Dispatchers
import kotlinx.serialization.encodeToString
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import okhttp3.HttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import java.net.URI
import java.net.URLDecoder
import java.util.UUID
import kotlin.coroutines.cancellation.CancellationException

class HomeBotClient(
    private val http: OkHttpClient,
    private val sessions: SessionStore,
    private val scope: CoroutineScope,
    private val reconnectDelayMs: (Int) -> Long = { attempt ->
        (250L shl attempt.coerceAtMost(5)).coerceAtMost(8_000L)
    },
) {
    private val json = Json {
        ignoreUnknownKeys = true
        explicitNulls = false
        classDiscriminator = "kind"
    }
    private val mutableState = MutableStateFlow<ConnectionState>(ConnectionState.Unpaired)
    private var runner: Job? = null
    private var socket: WebSocket? = null
    private var cursor: Long? = null
    private var projection = Snapshot()

    val state: StateFlow<ConnectionState> = mutableState.asStateFlow()

    fun start() {
        if (runner?.isActive == true) return
        runner = scope.launch { reconnectingLoop() }
    }

    fun stop() {
        runner?.cancel()
        runner = null
        socket?.close(1000, "HomeBot Android stopped")
        socket = null
    }

    suspend fun updateEndpoint(raw: String): Result<String> = runCatching {
        val normalized = EndpointPolicy.normalize(raw).getOrThrow().toString().trimEnd('/')
        val existing = sessions.load() ?: error("Pair this Android device before changing endpoint")
        sessions.save(existing.copy(endpoint = normalized))
        stop()
        start()
        normalized
    }

    suspend fun pair(deepLink: String, deviceName: String): Result<PairingExchangeResponse> {
        mutableState.value = ConnectionState.Pairing
        return runCatching {
            val offer = PairingLink.parse(deepLink)
            val endpoint = EndpointPolicy.normalize(offer.endpoint).getOrThrow()
            require(offer.token.startsWith("hbpair_") && offer.token.length <= 128) {
                "Pairing credential is invalid"
            }
            require(deviceName.trim().length in 1..80) { "Device name must contain 1 to 80 characters" }
            val request = ExchangePairingRequest(
                request_id = UUID.randomUUID().toString(),
                pairing_token = offer.token,
                device_name = deviceName.trim(),
            )
            val response = executeJson(
                Request.Builder()
                    .url(endpoint.api("api/v1/pairing/exchange"))
                    .post(json.encodeToString(request).jsonBody())
                    .header("Cache-Control", "no-store")
                    .build(),
                PairingExchangeResponse.serializer(),
            )
            sessions.save(
                SessionCredentials(
                    endpoint = endpoint.toString().trimEnd('/'),
                    deviceId = response.device.id,
                    deviceSession = response.device_session,
                ),
            )
            cursor = null
            projection = Snapshot()
            response
        }.onFailure { failure ->
            mutableState.value = ConnectionState.Offline(failure.toClientFailure(), cursor)
        }
    }

    internal suspend fun connectOnce(): DisconnectReason {
        val session = sessions.load() ?: run {
            mutableState.value = ConnectionState.Unpaired
            return DisconnectReason.Stopped
        }
        val endpoint = EndpointPolicy.normalize(session.endpoint).getOrElse {
            val failure = ClientFailure.InvalidEndpoint(it.message ?: "Invalid endpoint")
            mutableState.value = ConnectionState.Offline(failure, cursor)
            return DisconnectReason.Retry(failure)
        }
        mutableState.value = ConnectionState.Connecting(endpoint.toString().trimEnd('/'))
        when (val negotiation = negotiate(endpoint, session)) {
            is Negotiation.Failed -> return negotiation.reason
            is Negotiation.Supported -> Unit
        }
        mutableState.value = ConnectionState.Hydrating(endpoint.toString().trimEnd('/'), cursor)
        val disconnected = CompletableDeferred<DisconnectReason>()
        val listener = EventListener(endpoint, session, disconnected)
        val request = Request.Builder()
            .url(endpoint.api("api/v1/events"))
            .header("Authorization", "Bearer ${session.deviceSession}")
            .header("X-HomeBot-Protocol", PROTOCOL_VERSION.toString())
            .build()
        socket = http.newWebSocket(request, listener)
        return try {
            disconnected.await()
        } finally {
            socket = null
        }
    }

    private suspend fun reconnectingLoop() {
        var attempt = 0
        while (scope.isActive) {
            when (val reason = try {
                connectOnce()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (failure: Throwable) {
                DisconnectReason.Retry(failure.toClientFailure())
            }) {
                DisconnectReason.Stopped,
                DisconnectReason.Revoked,
                DisconnectReason.VersionIncompatible,
                -> return
                is DisconnectReason.Retry -> {
                    val session = sessions.load() ?: return
                    attempt += 1
                    mutableState.value = ConnectionState.Reconnecting(session.endpoint, cursor, attempt)
                    delay(reconnectDelayMs(attempt))
                    if (reason.failure is ClientFailure.Protocol) {
                        // Retain the last safe projection; the server decides replay versus snapshot.
                    }
                }
            }
        }
    }

    private suspend fun negotiate(endpoint: HttpUrl, session: SessionCredentials): Negotiation =
        withContext(Dispatchers.IO) {
            val response = runCatching {
                http.newCall(
                    Request.Builder()
                        .url(endpoint.api("api/v1/version"))
                        .header("Authorization", "Bearer ${session.deviceSession}")
                        .header("X-HomeBot-Protocol", PROTOCOL_VERSION.toString())
                        .build(),
                ).execute()
            }.getOrElse { failure ->
                val clientFailure = failure.toClientFailure()
                mutableState.value = ConnectionState.Offline(clientFailure, cursor)
                return@withContext Negotiation.Failed(DisconnectReason.Retry(clientFailure))
            }
            response.use {
                if (it.code == 401) {
                    sessions.clear()
                    mutableState.value = ConnectionState.Revoked
                    return@withContext Negotiation.Failed(DisconnectReason.Revoked)
                }
                if (it.code == 426) {
                    mutableState.value = ConnectionState.VersionIncompatible(null, null)
                    return@withContext Negotiation.Failed(DisconnectReason.VersionIncompatible)
                }
                if (!it.isSuccessful) {
                    val failure = it.structuredFailure()
                    mutableState.value = ConnectionState.Offline(failure, cursor)
                    return@withContext Negotiation.Failed(DisconnectReason.Retry(failure))
                }
                val body = it.body.string()
                val range = json.parseToJsonElement(body).jsonObject["protocol"]
                    ?.let { value -> json.decodeFromJsonElement<ProtocolRange>(value) }
                    ?: return@withContext Negotiation.Failed(
                        DisconnectReason.Retry(ClientFailure.Protocol("Version response omitted protocol range")),
                    )
                if (range.minimum > PROTOCOL_VERSION || range.maximum < PROTOCOL_VERSION) {
                    mutableState.value = ConnectionState.VersionIncompatible(range.minimum, range.maximum)
                    Negotiation.Failed(DisconnectReason.VersionIncompatible)
                } else {
                    Negotiation.Supported(range)
                }
            }
        }

    private inner class EventListener(
        private val endpoint: HttpUrl,
        private val session: SessionCredentials,
        private val disconnected: CompletableDeferred<DisconnectReason>,
    ) : WebSocketListener() {
        private val events = Channel<Pair<WebSocket, String>>(Channel.UNLIMITED)
        private val processor = scope.launch {
            for ((webSocket, text) in events) {
                runCatching { handleEvent(webSocket, endpoint, text) }
                    .onFailure { failure ->
                        webSocket.close(1002, "Invalid HomeBot event")
                        disconnected.complete(
                            DisconnectReason.Retry(
                                ClientFailure.Protocol(failure.message ?: "Invalid HomeBot event"),
                            ),
                        )
                        events.cancel()
                    }
            }
        }

        override fun onOpen(webSocket: WebSocket, response: Response) {
            val hello = ClientMessage.Hello(
                protocol_version = PROTOCOL_VERSION,
                client_version = CLIENT_VERSION,
                device_session = session.deviceId,
                resume_after = cursor,
            )
            webSocket.send(json.encodeToString<ClientMessage>(hello))
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            if (events.trySend(webSocket to text).isFailure) {
                webSocket.close(1002, "HomeBot event processor unavailable")
                disconnected.complete(
                    DisconnectReason.Retry(ClientFailure.Protocol("HomeBot event processor unavailable")),
                )
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(code, reason)
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            events.close()
            scope.launch {
                processor.join()
                disconnected.complete(
                    DisconnectReason.Retry(ClientFailure.Network("HomeBot event stream closed")),
                )
            }
        }

        override fun onFailure(webSocket: WebSocket, throwable: Throwable, response: Response?) {
            events.cancel()
            if (response?.code == 401) {
                scope.launch { sessions.clear() }
                mutableState.value = ConnectionState.Revoked
                disconnected.complete(DisconnectReason.Revoked)
            } else {
                disconnected.complete(DisconnectReason.Retry(throwable.toClientFailure()))
            }
        }
    }

    private suspend fun handleEvent(webSocket: WebSocket, endpoint: HttpUrl, text: String) {
        val event = json.parseToJsonElement(text).jsonObject
        val protocol = event.requiredInt("protocol_version")
        require(protocol == PROTOCOL_VERSION) { "Unsupported event protocol $protocol" }
        when (val kind = event.requiredString("kind")) {
            "hello" -> handleHello(endpoint, event)
            "snapshot" -> {
                val boundary = event.requiredLong("boundary_sequence")
                projection = json.decodeFromJsonElement(event.getValue("snapshot"))
                cursor = boundary
                mutableState.value = ConnectionState.Live(
                    endpoint.toString().trimEnd('/'),
                    boundary,
                    projection,
                )
            }
            "ping" -> {
                val pong = ClientMessage.Pong(event.requiredString("nonce"))
                webSocket.send(json.encodeToString<ClientMessage>(pong))
            }
            else -> applyIncremental(endpoint, kind, event)
        }
    }

    private fun handleHello(endpoint: HttpUrl, event: JsonObject) {
        val range = json.decodeFromJsonElement<ProtocolRange>(event.getValue("supported_protocols"))
        require(range.minimum <= PROTOCOL_VERSION && range.maximum >= PROTOCOL_VERSION) {
            "Server event stream is protocol incompatible"
        }
        if (event.requiredString("resume") == "replayed" && cursor != null) {
            mutableState.value = ConnectionState.Live(
                endpoint.toString().trimEnd('/'),
                cursor ?: 0,
                projection,
            )
        }
    }

    private fun applyIncremental(endpoint: HttpUrl, kind: String, event: JsonObject) {
        val sequence = event.requiredLong("sequence")
        val current = cursor ?: error("Incremental event arrived before snapshot or replay")
        if (sequence <= current) return
        require(sequence == current + 1) { "HomeBot event sequence gap: expected ${current + 1}, got $sequence" }
        projection = when (kind) {
            "bot_changed" -> projection.copy(
                bots = projection.bots.upsert(
                    json.decodeFromJsonElement<BotSummary>(event.getValue("bot")),
                ) { it.id },
            )
            "chat_changed" -> projection.copy(
                chats = projection.chats.upsert(
                    json.decodeFromJsonElement<ChatSummary>(event.getValue("chat")),
                ) { it.id },
            )
            else -> projection
        }
        cursor = sequence
        mutableState.value = ConnectionState.Live(
            endpoint.toString().trimEnd('/'),
            sequence,
            projection,
        )
    }

    private suspend fun <T> executeJson(
        request: Request,
        serializer: kotlinx.serialization.KSerializer<T>,
    ): T = withContext(Dispatchers.IO) {
        http.newCall(request).execute().use { response ->
            if (!response.isSuccessful) throw ClientException(response.structuredFailure())
            json.decodeFromString(serializer, response.body.string())
        }
    }

    private fun Response.structuredFailure(): ClientFailure.Structured {
        val payload = runCatching { json.decodeFromString<ErrorEnvelope>(body.string()) }.getOrElse {
            ErrorEnvelope("http_error", "HomeBot request failed", code >= 500)
        }
        return ClientFailure.Structured(code, payload)
    }

    private fun HttpUrl.api(path: String): HttpUrl = newBuilder().addPathSegments(path).build()
    private fun String.jsonBody() = toRequestBody(JSON_MEDIA_TYPE)

    private class ClientException(val failure: ClientFailure) : Exception()
    private fun Throwable.toClientFailure(): ClientFailure = when (this) {
        is ClientException -> failure
        is IllegalArgumentException -> ClientFailure.InvalidEndpoint(message ?: "Invalid value")
        is SerializationException,
        is NoSuchElementException,
        -> ClientFailure.Protocol(message ?: "Invalid HomeBot protocol payload")
        else -> ClientFailure.Network(message ?: "HomeBot connection failed")
    }

    private sealed interface Negotiation {
        data class Supported(val range: ProtocolRange) : Negotiation
        data class Failed(val reason: DisconnectReason) : Negotiation
    }

    private data class PairingLink(val endpoint: String, val token: String) {
        companion object {
            fun parse(raw: String): PairingLink {
                val uri = URI(raw.trim())
                require(uri.scheme == "homebot" && uri.host == "pair") { "Pairing link is invalid" }
                val values = uri.rawQuery.orEmpty().split('&').mapNotNull { item ->
                    val parts = item.split('=', limit = 2)
                    if (parts.size == 2) {
                        URLDecoder.decode(parts[0], Charsets.UTF_8.name()) to
                            URLDecoder.decode(parts[1], Charsets.UTF_8.name())
                    } else {
                        null
                    }
                }.toMap()
                return PairingLink(
                    endpoint = values["endpoint"] ?: error("Pairing link omitted endpoint"),
                    token = values["token"] ?: error("Pairing link omitted token"),
                )
            }
        }
    }

    private companion object {
        const val CLIENT_VERSION = "homebot-android/0.1.0"
        val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}

private fun JsonObject.requiredString(key: String): String =
    getValue(key).jsonPrimitive.content

private fun JsonObject.requiredInt(key: String): Int = getValue(key).jsonPrimitive.int
private fun JsonObject.requiredLong(key: String): Long = getValue(key).jsonPrimitive.long

private fun <T> List<T>.upsert(value: T, id: (T) -> String): List<T> =
    if (any { id(it) == id(value) }) map { if (id(it) == id(value)) value else it } else this + value
