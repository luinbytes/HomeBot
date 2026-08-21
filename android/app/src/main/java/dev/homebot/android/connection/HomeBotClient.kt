package dev.homebot.android.connection

import dev.homebot.protocol.BotSummary
import dev.homebot.protocol.BotMutationRequest
import dev.homebot.protocol.BotResponse
import dev.homebot.protocol.ChatSummary
import dev.homebot.protocol.ChatTimelineResponse
import dev.homebot.protocol.CheckpointDiffResponse
import dev.homebot.protocol.CheckpointRestoreSummary
import dev.homebot.protocol.ClientMessage
import dev.homebot.protocol.CreateBotRequest
import dev.homebot.protocol.CreateDirectChatRequest
import dev.homebot.protocol.CreateDirectChatResponse
import dev.homebot.protocol.DeleteBotRequest
import dev.homebot.protocol.CreateGroupChatRequest
import dev.homebot.protocol.CreateGroupChatResponse
import dev.homebot.protocol.ErrorEnvelope
import dev.homebot.protocol.ExchangePairingRequest
import dev.homebot.protocol.GroupChatSummary
import dev.homebot.protocol.GroupTimelineResponse
import dev.homebot.protocol.HandoffGroupRequest
import dev.homebot.protocol.MessageMutationRequest
import dev.homebot.protocol.MessageSummary
import dev.homebot.protocol.PROTOCOL_VERSION
import dev.homebot.protocol.PairingExchangeResponse
import dev.homebot.protocol.ProtocolRange
import dev.homebot.protocol.RepositoryWorkspaceSummary
import dev.homebot.protocol.ReactionMutationRequest
import dev.homebot.protocol.RestoreCheckpointRequest
import dev.homebot.protocol.SendGroupMessageRequest
import dev.homebot.protocol.SendMessageRequest
import dev.homebot.protocol.SendMessageResponse
import dev.homebot.protocol.Snapshot
import dev.homebot.protocol.UpdateBotRequest
import dev.homebot.protocol.VcsStatus
import dev.homebot.protocol.WorkingTreeDiffResponse
import dev.homebot.protocol.ApprovalDecisionRequest
import dev.homebot.protocol.Attachment
import dev.homebot.protocol.CreateAttachmentRequest
import dev.homebot.protocol.CreateAttachmentResponse
import dev.homebot.protocol.FinalizeAttachmentRequest
import dev.homebot.protocol.DeviceSessionSummary
import dev.homebot.protocol.PluginAssignmentRequest
import dev.homebot.protocol.PluginMutationRequest
import dev.homebot.protocol.PluginSummary
import dev.homebot.protocol.RevokeDeviceSessionRequest
import dev.homebot.protocol.RoutineRunSummary
import dev.homebot.protocol.RoutineSummary
import dev.homebot.protocol.RoutineTriggerSummary
import dev.homebot.protocol.RunRoutineRequest
import dev.homebot.protocol.SecretSummary
import dev.homebot.protocol.SkillAssignmentRequest
import dev.homebot.protocol.SkillSummary
import dev.homebot.protocol.CreateRoutineTriggerRequest
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Dispatchers
import kotlinx.serialization.encodeToString
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.long
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
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
import java.util.concurrent.atomic.AtomicReference
import java.security.MessageDigest
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
    private val mutableAlerts = MutableSharedFlow<ClientAlert>(extraBufferCapacity = 32)
    private var runner: Job? = null
    private var socket: WebSocket? = null
    private var cursor: Long? = null
    private var projection = Snapshot()

    val state: StateFlow<ConnectionState> = mutableState.asStateFlow()
    val alerts: SharedFlow<ClientAlert> = mutableAlerts.asSharedFlow()

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

    fun nudgeReconnect() {
        val previous = runner
        previous?.cancel()
        runner = null
        socket?.cancel()
        socket = null
        scope.launch {
            previous?.join()
            start()
        }
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

    suspend fun createBot(request: CreateBotRequest): Result<BotSummary> = authenticated {
        post("api/v1/bots", request, CreateBotRequest.serializer(), BotResponse.serializer()).bot
    }

    suspend fun updateBot(botId: String, request: UpdateBotRequest): Result<BotSummary> = authenticated {
        put("api/v1/bots/$botId", request, UpdateBotRequest.serializer(), BotResponse.serializer()).bot
    }

    suspend fun setBotArchived(botId: String, archived: Boolean): Result<BotSummary> = authenticated {
        val request = mutation()
        post(
            "api/v1/bots/$botId/${if (archived) "archive" else "restore"}",
            request,
            BotMutationRequest.serializer(),
            BotResponse.serializer(),
        ).bot
    }

    suspend fun setBotPinned(botId: String, pinned: Boolean): Result<BotSummary> = authenticated {
        post("api/v1/bots/$botId/${if (pinned) "pin" else "unpin"}", mutation(), BotMutationRequest.serializer(), BotResponse.serializer()).bot
    }

    suspend fun setBotHidden(botId: String, hidden: Boolean): Result<BotSummary> = authenticated {
        post("api/v1/bots/$botId/${if (hidden) "hide" else "unhide"}", mutation(), BotMutationRequest.serializer(), BotResponse.serializer()).bot
    }

    suspend fun duplicateBot(botId: String): Result<BotSummary> = authenticated {
        post("api/v1/bots/$botId/duplicate", mutation(), BotMutationRequest.serializer(), BotResponse.serializer()).bot
    }

    suspend fun deleteBot(botId: String, confirmName: String): Result<Unit> = authenticated {
        deleteDiscarding(
            "api/v1/bots/$botId",
            DeleteBotRequest(ids(), ids(), confirmName),
            DeleteBotRequest.serializer(),
        )
    }

    suspend fun createDirectChat(botId: String): Result<ChatSummary> = authenticated {
        val request = CreateDirectChatRequest(ids(), ids(), botId)
        post(
            "api/v1/chats/direct",
            request,
            CreateDirectChatRequest.serializer(),
            CreateDirectChatResponse.serializer(),
        ).chat
    }

    suspend fun directTimeline(chatId: String): Result<ChatTimelineResponse> = authenticated {
        get("api/v1/chats/$chatId/timeline", ChatTimelineResponse.serializer())
    }

    suspend fun setReaction(messageId: String, emoji: String, active: Boolean): Result<MessageSummary> = authenticated {
        val request = ReactionMutationRequest(ids(), ids(), emoji)
        if (active) {
            post("api/v1/messages/$messageId/reactions", request, ReactionMutationRequest.serializer(), MessageSummary.serializer())
        } else {
            delete("api/v1/messages/$messageId/reactions", request, ReactionMutationRequest.serializer(), MessageSummary.serializer())
        }
    }

    suspend fun groupTimeline(chatId: String): Result<GroupTimelineResponse> = authenticated {
        get("api/v1/groups/$chatId/timeline", GroupTimelineResponse.serializer())
    }

    suspend fun createGroup(request: CreateGroupChatRequest): Result<CreateGroupChatResponse> = authenticated {
        post("api/v1/groups", request, CreateGroupChatRequest.serializer(), CreateGroupChatResponse.serializer())
    }

    suspend fun sendMessage(
        chatId: String,
        content: String,
        steering: Boolean = false,
        attachmentIds: List<String> = emptyList(),
    ): Result<SendMessageResponse> = authenticated {
        val request = SendMessageRequest(ids(), ids(), content, attachmentIds, null, emptyList())
        post(
            "api/v1/chats/$chatId/${if (steering) "steer" else "messages"}",
            request,
            SendMessageRequest.serializer(),
            SendMessageResponse.serializer(),
        )
    }

    suspend fun uploadAttachment(filename: String, mediaType: String, bytes: ByteArray): Result<Attachment> = authenticated {
        require(bytes.size <= MAX_ATTACHMENT_BYTES) { "Attachments may not exceed 25 MiB" }
        val sha256 = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        val create = CreateAttachmentRequest(ids(), ids(), filename, mediaType, bytes.size.toLong(), sha256)
        val offer = post(
            "api/v1/attachments",
            create,
            CreateAttachmentRequest.serializer(),
            CreateAttachmentResponse.serializer(),
        )
        upload(offer.upload_url, mediaType, bytes)
        val finalize = FinalizeAttachmentRequest(ids(), ids(), sha256)
        post(
            "api/v1/attachments/${offer.attachment_id}/finalize",
            finalize,
            FinalizeAttachmentRequest.serializer(),
            Attachment.serializer(),
        )
    }

    suspend fun sendGroupMessage(
        chatId: String,
        content: String,
        mentionedBotIds: List<String>,
    ): Result<MessageSummary> = authenticated {
        val request = SendGroupMessageRequest(ids(), ids(), content, mentionedBotIds, emptyList())
        post(
            "api/v1/groups/$chatId/messages",
            request,
            SendGroupMessageRequest.serializer(),
            MessageSummary.serializer(),
        )
    }

    suspend fun stop(chatId: String, group: Boolean): Result<Unit> = authenticated {
        val path = if (group) "api/v1/groups/$chatId/stop" else "api/v1/chats/$chatId/stop"
        postDiscarding(path, mutation(), BotMutationRequest.serializer())
    }

    suspend fun retry(chatId: String, messageId: String): Result<Unit> = authenticated {
        val request = MessageMutationRequest(ids(), ids())
        postDiscarding(
            "api/v1/chats/$chatId/messages/$messageId/retry",
            request,
            MessageMutationRequest.serializer(),
        )
    }

    suspend fun decideApproval(approvalId: String, allow: Boolean): Result<Unit> = authenticated {
        val request = ApprovalDecisionRequest(ids(), ids(), allow)
        postDiscarding(
            "api/v1/approvals/$approvalId/decision",
            request,
            ApprovalDecisionRequest.serializer(),
        )
    }

    suspend fun handoff(chatId: String, fromBotId: String, toBotId: String): Result<Unit> = authenticated {
        val request = HandoffGroupRequest(ids(), ids(), fromBotId, toBotId, null, "Android ownership handoff")
        postDiscarding("api/v1/groups/$chatId/handoff", request, HandoffGroupRequest.serializer())
    }

    suspend fun markChatRead(chatId: String): Result<Unit> = authenticated {
        postDiscarding("api/v1/chats/$chatId/read", mutation(), BotMutationRequest.serializer())
    }

    suspend fun vcsStatus(chatId: String): Result<VcsStatus> = authenticated {
        get("api/v1/chats/$chatId/vcs/status", VcsStatus.serializer())
    }

    suspend fun workingTreeDiff(chatId: String, staged: Boolean = false): Result<WorkingTreeDiffResponse> = authenticated {
        get("api/v1/chats/$chatId/vcs/diff?staged=$staged", WorkingTreeDiffResponse.serializer())
    }

    suspend fun checkpointDiff(chatId: String, from: String, to: String): Result<CheckpointDiffResponse> = authenticated {
        get(
            "api/v1/chats/$chatId/checkpoints/diff?from_checkpoint_id=$from&to_checkpoint_id=$to",
            CheckpointDiffResponse.serializer(),
        )
    }

    suspend fun restoreCheckpoint(checkpointId: String): Result<CheckpointRestoreSummary> = authenticated {
        val request = RestoreCheckpointRequest(ids(), ids())
        post(
            "api/v1/checkpoints/$checkpointId/restore",
            request,
            RestoreCheckpointRequest.serializer(),
            CheckpointRestoreSummary.serializer(),
        )
    }

    suspend fun skills(): Result<List<SkillSummary>> = authenticated {
        get("api/v1/skills", ListSerializer(SkillSummary.serializer()))
    }

    suspend fun setSkillAssigned(skillId: String, botId: String, enabled: Boolean): Result<SkillSummary> = authenticated {
        val request = SkillAssignmentRequest(ids(), ids(), botId, enabled)
        put(
            "api/v1/skills/$skillId/assignment",
            request,
            SkillAssignmentRequest.serializer(),
            SkillSummary.serializer(),
        )
    }

    suspend fun plugins(): Result<List<PluginSummary>> = authenticated {
        get("api/v1/plugins", ListSerializer(PluginSummary.serializer()))
    }

    suspend fun mutatePlugin(pluginId: String, action: String): Result<PluginSummary> = authenticated {
        require(action in setOf("connect", "enable", "disable", "health")) { "Unsupported plugin action" }
        val request = PluginMutationRequest(ids(), ids())
        post(
            "api/v1/plugins/$pluginId/$action",
            request,
            PluginMutationRequest.serializer(),
            PluginSummary.serializer(),
        )
    }

    suspend fun setPluginAssigned(pluginId: String, botId: String, enabled: Boolean): Result<PluginSummary> = authenticated {
        val request = PluginAssignmentRequest(ids(), ids(), botId, enabled)
        put(
            "api/v1/plugins/$pluginId/assignment",
            request,
            PluginAssignmentRequest.serializer(),
            PluginSummary.serializer(),
        )
    }

    suspend fun routines(): Result<List<RoutineSummary>> = authenticated {
        get("api/v1/routines", ListSerializer(RoutineSummary.serializer()))
    }

    suspend fun routineRuns(routineId: String): Result<List<RoutineRunSummary>> = authenticated {
        get("api/v1/routines/$routineId/runs", ListSerializer(RoutineRunSummary.serializer()))
    }

    suspend fun routineTriggers(routineId: String): Result<List<RoutineTriggerSummary>> = authenticated {
        get("api/v1/routines/$routineId/triggers", ListSerializer(RoutineTriggerSummary.serializer()))
    }

    suspend fun runRoutine(routineId: String, inputs: JsonElement = buildJsonObject {}): Result<RoutineRunSummary> = authenticated {
        val request = RunRoutineRequest(ids(), ids(), inputs)
        post(
            "api/v1/routines/$routineId/run",
            request,
            RunRoutineRequest.serializer(),
            RoutineRunSummary.serializer(),
        )
    }

    suspend fun mutateRoutine(routineId: String, enabled: Boolean): Result<RoutineSummary> = authenticated {
        val request = PluginMutationRequest(ids(), ids())
        post(
            "api/v1/routines/$routineId/${if (enabled) "enable" else "disable"}",
            request,
            PluginMutationRequest.serializer(),
            RoutineSummary.serializer(),
        )
    }

    suspend fun scheduleRoutineOnce(routineId: String, atUnixMs: Long): Result<RoutineTriggerSummary> = authenticated {
        val definition = buildJsonObject {
            put("source", buildJsonObject {
                put("kind", "schedule")
                put("schedule", buildJsonObject {
                    put("kind", "one_shot")
                    put("at_unix_ms", atUnixMs)
                })
            })
            put("missed_run_policy", "run_once")
            put("overlap_policy", buildJsonObject { put("kind", "queue") })
            put("retry_policy", buildJsonObject {
                put("maximum_attempts", 1)
                put("initial_backoff_seconds", 5)
                put("maximum_backoff_seconds", 300)
            })
            put("catch_up_limit", 1)
        }
        val request = CreateRoutineTriggerRequest(ids(), ids(), definition, true)
        post(
            "api/v1/routines/$routineId/triggers",
            request,
            CreateRoutineTriggerRequest.serializer(),
            RoutineTriggerSummary.serializer(),
        )
    }

    suspend fun secrets(): Result<List<SecretSummary>> = authenticated {
        get("api/v1/secrets", ListSerializer(SecretSummary.serializer()))
    }

    suspend fun currentDevice(): Result<DeviceSessionSummary> = authenticated {
        get("api/v1/device", DeviceSessionSummary.serializer())
    }

    suspend fun revokeCurrentDevice(): Result<DeviceSessionSummary> = authenticated {
        val request = RevokeDeviceSessionRequest(ids(), ids())
        val device = post(
            "api/v1/device/revoke",
            request,
            RevokeDeviceSessionRequest.serializer(),
            DeviceSessionSummary.serializer(),
        )
        sessions.clear()
        mutableState.value = ConnectionState.Revoked
        device
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
        private val events = Channel<Pair<WebSocket, String>>(EVENT_BUFFER_CAPACITY)
        private val terminalFailure = AtomicReference<ClientFailure?>(null)
        private val processor = scope.launch {
            for ((webSocket, text) in events) {
                runCatching { handleEvent(webSocket, endpoint, text) }
                    .onFailure { failure ->
                        reject(
                            webSocket,
                            1002,
                            "Invalid HomeBot event",
                            ClientFailure.Protocol(failure.message ?: "Invalid HomeBot event"),
                        )
                    }
            }
        }

        private fun reject(
            webSocket: WebSocket,
            code: Int,
            reason: String,
            failure: ClientFailure.Protocol,
        ) {
            terminalFailure.compareAndSet(null, failure)
            events.cancel()
            if (!webSocket.close(code, reason)) {
                webSocket.cancel()
                return
            }
            scope.launch {
                delay(REJECTED_SOCKET_GRACE_MS)
                if (!disconnected.isCompleted) webSocket.cancel()
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
            if (text.toByteArray(Charsets.UTF_8).size > MAX_EVENT_BYTES) {
                reject(
                    webSocket,
                    1009,
                    "HomeBot event exceeded the size limit",
                    ClientFailure.Protocol("HomeBot event exceeded the size limit"),
                )
                return
            }
            if (events.trySend(webSocket to text).isFailure) {
                reject(
                    webSocket,
                    1013,
                    "HomeBot event processor is backpressured",
                    ClientFailure.Protocol("HomeBot event processor is backpressured"),
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
                    DisconnectReason.Retry(
                        terminalFailure.get()
                            ?: ClientFailure.Network("HomeBot event stream closed"),
                    ),
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
                disconnected.complete(
                    DisconnectReason.Retry(terminalFailure.get() ?: throwable.toClientFailure()),
                )
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
            "bot_deleted" -> projection.copy(
                bots = projection.bots.filterNot { it.id == event.requiredString("bot_id") },
                chats = projection.chats.filterNot { it.bot_id == event.requiredString("bot_id") },
            )
            "chat_changed" -> projection.copy(
                chats = projection.chats.upsert(
                    json.decodeFromJsonElement<ChatSummary>(event.getValue("chat")),
                ) { it.id },
            )
            "group_chat_changed" -> projection.copy(
                group_chats = projection.group_chats.upsert(
                    json.decodeFromJsonElement<GroupChatSummary>(event.getValue("group")),
                ) { it.id },
            )
            else -> projection
        }
        NotificationEventMapper.map(kind, event, json)?.let(mutableAlerts::tryEmit)
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

    private suspend fun <T> authenticated(block: suspend AuthenticatedApi.() -> T): Result<T> = runCatching {
        val session = sessions.load() ?: throw ClientException(
            ClientFailure.Structured(401, ErrorEnvelope("unauthenticated", "Pair this device first", false)),
        )
        val endpoint = EndpointPolicy.normalize(session.endpoint).getOrThrow()
        AuthenticatedApi(endpoint, session).block()
    }.onFailure { failure ->
        if (failure is ClientException && failure.failure is ClientFailure.Structured && failure.failure.status == 401) {
            sessions.clear()
            mutableState.value = ConnectionState.Revoked
        }
    }

    private inner class AuthenticatedApi(
        private val endpoint: HttpUrl,
        private val session: SessionCredentials,
    ) {
        suspend fun <RequestType, ResponseType> post(
            path: String,
            payload: RequestType,
            requestSerializer: kotlinx.serialization.KSerializer<RequestType>,
            responseSerializer: kotlinx.serialization.KSerializer<ResponseType>,
        ): ResponseType = request("POST", path, json.encodeToString(requestSerializer, payload), responseSerializer)

        suspend fun <RequestType, ResponseType> put(
            path: String,
            payload: RequestType,
            requestSerializer: kotlinx.serialization.KSerializer<RequestType>,
            responseSerializer: kotlinx.serialization.KSerializer<ResponseType>,
        ): ResponseType = request("PUT", path, json.encodeToString(requestSerializer, payload), responseSerializer)

        suspend fun <RequestType, ResponseType> delete(
            path: String,
            payload: RequestType,
            requestSerializer: kotlinx.serialization.KSerializer<RequestType>,
            responseSerializer: kotlinx.serialization.KSerializer<ResponseType>,
        ): ResponseType = request("DELETE", path, json.encodeToString(requestSerializer, payload), responseSerializer)

        suspend fun <ResponseType> get(
            path: String,
            responseSerializer: kotlinx.serialization.KSerializer<ResponseType>,
        ): ResponseType = request("GET", path, null, responseSerializer)

        suspend fun <RequestType> postDiscarding(
            path: String,
            payload: RequestType,
            requestSerializer: kotlinx.serialization.KSerializer<RequestType>,
        ) {
            val request = requestBuilder(path).post(json.encodeToString(requestSerializer, payload).jsonBody()).build()
            executeDiscarding(request)
        }

        suspend fun <RequestType> deleteDiscarding(
            path: String,
            payload: RequestType,
            requestSerializer: kotlinx.serialization.KSerializer<RequestType>,
        ) {
            val request = requestBuilder(path)
                .delete(json.encodeToString(requestSerializer, payload).jsonBody())
                .build()
            executeDiscarding(request)
        }

        suspend fun upload(path: String, mediaType: String, bytes: ByteArray) {
            val relative = path.removePrefix("/")
            val request = requestBuilder(relative)
                .put(bytes.toRequestBody(mediaType.toMediaType()))
                .build()
            executeDiscarding(request)
        }

        private suspend fun <ResponseType> request(
            method: String,
            path: String,
            payload: String?,
            serializer: kotlinx.serialization.KSerializer<ResponseType>,
        ): ResponseType {
            val builder = requestBuilder(path)
            when (method) {
                "GET" -> builder.get()
                "POST" -> builder.post(requireNotNull(payload).jsonBody())
                "PUT" -> builder.put(requireNotNull(payload).jsonBody())
                "DELETE" -> builder.delete(requireNotNull(payload).jsonBody())
            }
            return executeJson(builder.build(), serializer)
        }

        private fun requestBuilder(path: String): Request.Builder = Request.Builder()
            .url(endpoint.api(path))
            .header("Authorization", "Bearer ${session.deviceSession}")
            .header("X-HomeBot-Protocol", PROTOCOL_VERSION.toString())
            .header("Cache-Control", "no-store")
    }

    private suspend fun executeDiscarding(request: Request) = withContext(Dispatchers.IO) {
        http.newCall(request).execute().use { response ->
            if (!response.isSuccessful) throw ClientException(response.structuredFailure())
        }
    }

    private fun ids(): String = UUID.randomUUID().toString()
    private fun mutation(): BotMutationRequest = BotMutationRequest(ids(), ids())

    private fun Response.structuredFailure(): ClientFailure.Structured {
        val payload = runCatching { json.decodeFromString<ErrorEnvelope>(body.string()) }.getOrElse {
            ErrorEnvelope("http_error", "HomeBot request failed", code >= 500)
        }
        return ClientFailure.Structured(code, payload)
    }

    private fun HttpUrl.api(path: String): HttpUrl {
        val components = path.split('?', limit = 2)
        val builder = newBuilder().addPathSegments(components[0])
        components.getOrNull(1)?.split('&')?.forEach { parameter ->
            val pair = parameter.split('=', limit = 2)
            builder.addQueryParameter(pair[0], pair.getOrNull(1))
        }
        return builder.build()
    }
    private fun String.jsonBody() = toRequestBody(JSON_MEDIA_TYPE)

    private class ClientException(val failure: ClientFailure) : Exception(
        when (failure) {
            is ClientFailure.Structured -> failure.error.message
            is ClientFailure.InvalidEndpoint -> failure.message
            is ClientFailure.Protocol -> failure.message
            is ClientFailure.Network -> failure.message
        },
    )
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
        const val EVENT_BUFFER_CAPACITY = 128
        const val MAX_ATTACHMENT_BYTES = 25 * 1024 * 1024
        const val MAX_EVENT_BYTES = 256 * 1024
        const val REJECTED_SOCKET_GRACE_MS = 250L
        val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
    }
}

private fun JsonObject.requiredString(key: String): String =
    getValue(key).jsonPrimitive.content

private fun JsonObject.requiredInt(key: String): Int = getValue(key).jsonPrimitive.int
private fun JsonObject.requiredLong(key: String): Long = getValue(key).jsonPrimitive.long

private fun <T> List<T>.upsert(value: T, id: (T) -> String): List<T> =
    if (any { id(it) == id(value) }) map { if (id(it) == id(value)) value else it } else this + value
