package dev.homebot.android.connection

import dev.homebot.protocol.CreateBotRequest
import dev.homebot.protocol.SendMessageResponse
import dev.homebot.protocol.RecordedAction
import dev.homebot.protocol.RoutineDefinition
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.net.URLEncoder
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger

class HomeBotClientTest {
    private lateinit var server: MockWebServer
    private lateinit var scope: CoroutineScope
    private lateinit var sessions: FakeSessionStore

    @Before
    fun setUp() {
        server = MockWebServer()
        scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
        sessions = FakeSessionStore()
    }

    @After
    fun tearDown() {
        scope.cancel()
        server.close()
    }

    @Test
    fun pairingExchangesOneTimeCredentialAndRedactsPersistentSession() = runBlocking {
        server.enqueue(
            MockResponse().setResponseCode(200).setBody(PAIRING_RESPONSE)
                .addHeader("Content-Type", "application/json")
                .addHeader("Cache-Control", "no-store"),
        )
        server.start()
        val client = client()
        val endpoint = server.url("/").toString().trimEnd('/')
        val deepLink = "homebot://pair?endpoint=${URLEncoder.encode(endpoint, Charsets.UTF_8)}&token=hbpair_fixture&proof=hbproof_fixture"

        val result = client.pair(deepLink, "Pixel 9").getOrThrow()

        assertEquals("Pixel 9", result.device.name)
        val recorded = server.takeRequest()
        assertEquals("/api/v1/pairing/exchange", recorded.path)
        assertNull(recorded.getHeader("Authorization"))
        assertTrue(recorded.body.readUtf8().contains("hbpair_fixture"))
        val stored = sessions.load() ?: error("missing session")
        assertEquals(endpoint, stored.endpoint)
        assertEquals("hbds_fixture_session", stored.deviceSession)
        assertFalse(stored.toString().contains("hbds_fixture_session"))
    }

    @Test
    fun snapshotReconnectResumesThenAcceptsStaleCursorFallbackWithoutDuplicates() = runBlocking {
        server.start()
        sessions.save(credentials())
        val sockets = AtomicInteger()
        val hellos = CopyOnWriteArrayList<String>()
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when (request.path) {
                "/api/v1/version" -> jsonResponse(VERSION_RESPONSE)
                "/api/v1/events" -> {
                    val socketNumber = sockets.incrementAndGet()
                    MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                        override fun onMessage(webSocket: WebSocket, text: String) {
                            if (socketNumber == 1 && text.contains("\"kind\":\"pong\"")) {
                                webSocket.close(1001, "fixture restart")
                            } else if (text.contains("\"kind\":\"hello\"")) {
                                hellos += text
                                webSocket.send(hello("snapshot_required"))
                                webSocket.send(snapshot(if (socketNumber == 1) 5 else 9))
                                if (socketNumber == 1) webSocket.send(ping())
                            }
                        }

                        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                            webSocket.close(code, reason)
                        }
                    })
                }
                else -> MockResponse().setResponseCode(404)
            }
        }
        val client = client(reconnectDelayMs = { 1 })
        client.start()

        val recovered = withTimeout(10_000) {
            client.state.filterIsInstance<ConnectionState.Live>().first { it.cursor == 9L }
        }
        assertTrue(recovered.snapshot.bots.isEmpty())
        assertFalse(hellos.first().contains("\"resume_after\""))
        assertTrue(hellos.drop(1).first().contains("\"resume_after\":5"))
        assertEquals(2, sockets.get())
        client.stop()
    }

    @Test
    fun replayedResumeHydratesFromRetainedProjectionAndIgnoresDuplicateSequence() = runBlocking {
        server.start()
        sessions.save(credentials())
        val sockets = AtomicInteger()
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when (request.path) {
                "/api/v1/version" -> jsonResponse(VERSION_RESPONSE)
                "/api/v1/events" -> {
                    val socketNumber = sockets.incrementAndGet()
                    MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                        override fun onMessage(webSocket: WebSocket, text: String) {
                            if (socketNumber == 1) {
                                webSocket.send(hello("snapshot_required"))
                                webSocket.send(snapshot(4))
                                webSocket.close(1001, "resume fixture")
                            } else {
                                webSocket.send(hello("replayed"))
                                webSocket.send(botChanged(5, "Nova"))
                                webSocket.send(botChanged(5, "Duplicate must be ignored"))
                            }
                        }

                        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                            webSocket.close(code, reason)
                        }
                    })
                }
                else -> MockResponse().setResponseCode(404)
            }
        }
        val client = client(reconnectDelayMs = { 1 })
        client.start()

        val live = withTimeout(10_000) {
            client.state.filterIsInstance<ConnectionState.Live>()
                .first { it.cursor == 5L && it.snapshot.bots.isNotEmpty() }
        }
        assertEquals("Nova", live.snapshot.bots.single().name)
        assertEquals(1, live.snapshot.bots.size)
        client.stop()
    }

    @Test
    fun revokedSessionIsDeletedAndVersionSkewIsStructured() = runBlocking {
        server.enqueue(MockResponse().setResponseCode(401).setBody(ERROR_RESPONSE))
        server.start()
        sessions.save(credentials())
        val revokedClient = client()
        assertEquals(DisconnectReason.Revoked, revokedClient.connectOnce())
        assertEquals(ConnectionState.Revoked, revokedClient.state.value)
        assertNull(sessions.load())
        server.close()

        server = MockWebServer().apply {
            enqueue(MockResponse().setResponseCode(426).setBody(ERROR_RESPONSE))
            start()
        }
        sessions.save(credentials())
        val incompatible = client()
        assertEquals(DisconnectReason.VersionIncompatible, incompatible.connectOnce())
        assertTrue(incompatible.state.value is ConnectionState.VersionIncompatible)
    }

    @Test
    fun oversizedServerEventIsRejectedWithoutUnboundedBuffering() = runBlocking {
        server.start()
        sessions.save(credentials())
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse = when (request.path) {
                "/api/v1/version" -> jsonResponse(VERSION_RESPONSE)
                "/api/v1/events" -> MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                    override fun onMessage(webSocket: WebSocket, text: String) {
                        if (text.contains("\"kind\":\"hello\"")) {
                            webSocket.send("x".repeat(256 * 1024 + 1))
                        }
                    }

                    override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                        webSocket.close(code, reason)
                    }
                })
                else -> MockResponse().setResponseCode(404)
            }
        }

        repeat(5) {
            val result = withTimeout(10_000) { client().connectOnce() }
            assertTrue(result is DisconnectReason.Retry)
            assertTrue((result as DisconnectReason.Retry).failure is ClientFailure.Protocol)
        }
    }

    @Test
    fun endpointPolicyAllowsLoopbackHttpAndRequiresHttpsEverywhereElse() {
        assertTrue(EndpointPolicy.normalize("http://127.0.0.1:7123").isSuccess)
        assertTrue(EndpointPolicy.normalize("http://10.0.2.2:7123").isSuccess)
        assertTrue(EndpointPolicy.normalize("https://100.64.10.2:7123").isSuccess)
        assertTrue(EndpointPolicy.normalize("https://homebot.tailnet.ts.net:7123").isSuccess)
        assertTrue(EndpointPolicy.normalize("https://homebot.example.com").isSuccess)
        assertTrue(EndpointPolicy.normalize("http://192.168.1.4:7123").isFailure)
        assertTrue(EndpointPolicy.normalize("http://100.64.10.2:7123").isFailure)
        assertTrue(EndpointPolicy.normalize("http://homebot.example.com").isFailure)
        assertTrue(EndpointPolicy.normalize("https://user:pass@homebot.example.com").isFailure)
        assertTrue(EndpointPolicy.normalize("https://homebot.example.com/path").isFailure)
    }

    @Test
    fun productMutationsUseAuthenticatedServerApisAndPreserveQueueSemantics() = runBlocking {
        server.enqueue(jsonResponse(BOT_RESPONSE))
        server.enqueue(jsonResponse(QUEUED_RESPONSE))
        server.start()
        sessions.save(credentials())
        val client = client()

        val bot = client.createBot(
            CreateBotRequest(
                request_id = "00000000-0000-0000-0000-000000000020",
                idempotency_key = "00000000-0000-0000-0000-000000000021",
                name = "Nova",
                title = "Researcher",
                shape = "circle",
                color = "violet",
                permission_profile = "ask_before_changes",
            ),
        ).getOrThrow()
        val queued = client.sendMessage(CHAT_ID, "Follow up", steering = true).getOrThrow()

        assertEquals("Nova", bot.name)
        assertTrue(queued is SendMessageResponse.Queued)
        val create = server.takeRequest()
        assertEquals("/api/v1/bots", create.path)
        assertEquals("Bearer hbds_fixture_session", create.getHeader("Authorization"))
        assertEquals("1", create.getHeader("X-HomeBot-Protocol"))
        assertTrue(create.body.readUtf8().contains("00000000-0000-0000-0000-000000000021"))
        val steer = server.takeRequest()
        assertEquals("/api/v1/chats/$CHAT_ID/steer", steer.path)
        assertEquals("Bearer hbds_fixture_session", steer.getHeader("Authorization"))
        val payload = steer.body.readUtf8()
        assertTrue(payload.contains("Follow up"))
        assertTrue(payload.contains("idempotency_key"))
    }

    @Test
    fun attachmentUsesAuthenticatedCreateUploadFinalizeTransport() = runBlocking {
        server.enqueue(jsonResponse(ATTACHMENT_OFFER))
        server.enqueue(MockResponse().setResponseCode(204))
        server.enqueue(jsonResponse(ATTACHMENT_RESPONSE))
        server.start()
        sessions.save(credentials())

        val attachment = client().uploadAttachment("notes.txt", "text/plain", "home".toByteArray()).getOrThrow()

        assertEquals("notes.txt", attachment.filename)
        val create = server.takeRequest()
        assertEquals("/api/v1/attachments", create.path)
        assertTrue(create.body.readUtf8().contains("text/plain"))
        val upload = server.takeRequest()
        assertEquals("PUT", upload.method)
        assertEquals("/api/v1/attachments/$ATTACHMENT_ID/content", upload.path)
        assertEquals("home", upload.body.readUtf8())
        assertEquals("Bearer hbds_fixture_session", upload.getHeader("Authorization"))
        val finalize = server.takeRequest()
        assertEquals("/api/v1/attachments/$ATTACHMENT_ID/finalize", finalize.path)
        assertTrue(finalize.body.readUtf8().contains("sha256"))
    }

    @Test
    fun servicesProjectionReadsOnlyServerOwnedStatusesAndOpaqueSecretReferences() = runBlocking {
        server.enqueue(jsonResponse(PLUGINS_RESPONSE))
        server.enqueue(jsonResponse(SECRETS_RESPONSE))
        server.enqueue(jsonResponse(CURRENT_DEVICE_RESPONSE))
        server.start()
        sessions.save(credentials())

        val plugins = client().plugins().getOrThrow()
        val secrets = client().secrets().getOrThrow()
        val device = client().currentDevice().getOrThrow()

        assertEquals("Repository MCP", plugins.single().name)
        assertEquals("ready", secrets.single().status)
        assertFalse(secrets.single().toString().contains("secret-value"))
        assertEquals("Pixel 9", device.name)
        repeat(3) {
            assertEquals("Bearer hbds_fixture_session", server.takeRequest().getHeader("Authorization"))
        }
    }

    @Test
    fun authoritativeRoutineLifecycleUsesEveryServerMutationPath() = runBlocking {
        repeat(3) { server.enqueue(jsonResponse(ROUTINE_RESPONSE)) }
        server.enqueue(jsonResponse(ROUTINE_RUN_RESPONSE))
        server.enqueue(jsonResponse(RECORDING_EMPTY_RESPONSE))
        server.enqueue(jsonResponse(RECORDING_ACTION_RESPONSE))
        server.enqueue(jsonResponse(ROUTINE_RESPONSE))
        server.enqueue(MockResponse().setResponseCode(204))
        server.enqueue(MockResponse().setResponseCode(204))
        server.start()
        sessions.save(credentials())
        val client = client()
        val definition = RoutineDefinition(
            inputs = emptyList(),
            steps = listOf(botPromptStep("Check the repository")),
            expected_outputs = emptyList(),
        )

        client.createRoutine(BOT_ID, "Review", "Repository review", definition, draft = false).getOrThrow()
        client.updateRoutine(ROUTINE_ID, "Review", "Updated", definition, draft = false).getOrThrow()
        client.duplicateRoutine(ROUTINE_ID, "Review copy").getOrThrow()
        client.dryRunRoutine(ROUTINE_ID).getOrThrow()
        val recording = client.startRoutineRecording(BOT_ID, "Recorded review", "").getOrThrow()
        client.appendRoutineRecording(recording.id, RecordedAction("user", botPromptStep("Run tests"))).getOrThrow()
        client.finishRoutineRecording(recording.id).getOrThrow()
        client.cancelRoutineRecording(recording.id).getOrThrow()
        client.deleteRoutine(ROUTINE_ID).getOrThrow()

        val requests = List(9) { server.takeRequest() }
        assertEquals(
            listOf(
                "/api/v1/routines",
                "/api/v1/routines/$ROUTINE_ID",
                "/api/v1/routines/$ROUTINE_ID/duplicate",
                "/api/v1/routines/$ROUTINE_ID/dry-run",
                "/api/v1/routine-recordings",
                "/api/v1/routine-recordings/$RECORDING_ID/actions",
                "/api/v1/routine-recordings/$RECORDING_ID/finish",
                "/api/v1/routine-recordings/$RECORDING_ID/cancel",
                "/api/v1/routines/$ROUTINE_ID",
            ),
            requests.map { it.path },
        )
        assertEquals(listOf("POST", "PUT", "POST", "POST", "POST", "POST", "POST", "POST", "DELETE"), requests.map { it.method })
        assertTrue(requests[0].body.readUtf8().contains("bot_prompt"))
        assertTrue(requests[5].body.readUtf8().contains("Run tests"))
        requests.forEach { assertEquals("Bearer hbds_fixture_session", it.getHeader("Authorization")) }
    }

    @Test
    fun completeBotLifecycleUsesReachableAuthenticatedMutations() = runBlocking {
        repeat(3) { server.enqueue(jsonResponse(BOT_RESPONSE)) }
        server.enqueue(MockResponse().setResponseCode(204))
        server.start()
        sessions.save(credentials())
        val client = client()

        client.setBotPinned(BOT_ID, true).getOrThrow()
        client.setBotHidden(BOT_ID, true).getOrThrow()
        client.duplicateBot(BOT_ID).getOrThrow()
        client.deleteBot(BOT_ID, "Nova").getOrThrow()

        val requests = List(4) { server.takeRequest() }
        assertEquals(
            listOf(
                "/api/v1/bots/$BOT_ID/pin",
                "/api/v1/bots/$BOT_ID/hide",
                "/api/v1/bots/$BOT_ID/duplicate",
                "/api/v1/bots/$BOT_ID",
            ),
            requests.map { it.path },
        )
        assertEquals("DELETE", requests.last().method)
        assertTrue(requests.last().body.readUtf8().contains("Nova"))
        requests.forEach { assertEquals("Bearer hbds_fixture_session", it.getHeader("Authorization")) }
    }

    private fun botPromptStep(prompt: String) = buildJsonObject {
        put("kind", "bot_prompt")
        put("bot_id", BOT_ID)
        put("prompt_template", prompt)
        put("requires_approval", false)
    }

    private fun client(reconnectDelayMs: (Int) -> Long = { 1 }) = HomeBotClient(
        http = OkHttpClient.Builder().build(),
        sessions = sessions,
        scope = scope,
        reconnectDelayMs = reconnectDelayMs,
    )

    private fun credentials() = SessionCredentials(
        endpoint = server.url("/").toString().trimEnd('/'),
        deviceId = DEVICE_ID,
        deviceSession = "hbds_fixture_session",
    )

    private fun jsonResponse(body: String) = MockResponse().setResponseCode(200)
        .addHeader("Content-Type", "application/json")
        .setBody(body)

    private class FakeSessionStore : SessionStore {
        @Volatile private var credentials: SessionCredentials? = null
        override suspend fun load(): SessionCredentials? = credentials
        override suspend fun save(credentials: SessionCredentials) { this.credentials = credentials }
        override suspend fun clear() { credentials = null }
    }

    private companion object {
        const val DEVICE_ID = "018f47b8-c9aa-7c6f-b9e1-111111111111"
        const val CHAT_ID = "00000000-0000-0000-0000-000000000030"
        const val ATTACHMENT_ID = "00000000-0000-0000-0000-000000000050"
        const val BOT_ID = "00000000-0000-0000-0000-000000000010"
        const val ROUTINE_ID = "00000000-0000-0000-0000-000000000080"
        const val ROUTINE_VERSION_ID = "00000000-0000-0000-0000-000000000081"
        const val RECORDING_ID = "00000000-0000-0000-0000-000000000082"
        const val VERSION_RESPONSE = """{"server_version":"1.0.0","protocol":{"minimum":1,"maximum":1}}"""
        const val ERROR_RESPONSE = """{"code":"unauthenticated","message":"Device session is invalid","retryable":false,"request_id":null}"""
        const val PAIRING_RESPONSE = """{"device":{"id":"$DEVICE_ID","name":"Pixel 9","endpoint_kind":"loopback","created_at_unix_ms":1,"last_seen_at_unix_ms":null,"revoked_at_unix_ms":null},"device_session":"hbds_fixture_session"}"""
        const val BOT_RESPONSE = """{"bot":{"id":"00000000-0000-0000-0000-000000000010","name":"Nova","title":"Researcher","description":"","shape":"circle","color":"violet","archived":false,"unread_count":0,"attention":"none","provider":"not_configured","advanced":{"provider_profile_id":null,"permission_profile":"ask_before_changes"}}}"""
        const val QUEUED_RESPONSE = """{"kind":"queued","prompt":{"id":"00000000-0000-0000-0000-000000000040","chat_id":"$CHAT_ID","content":"Follow up","attachment_ids":[],"skill_ids":[],"kind":"steering","position":0,"created_at_ms":1}}"""
        const val ATTACHMENT_OFFER = """{"attachment_id":"$ATTACHMENT_ID","upload_url":"/api/v1/attachments/$ATTACHMENT_ID/content","expires_at_unix_ms":9999999999999}"""
        const val ATTACHMENT_RESPONSE = """{"id":"$ATTACHMENT_ID","filename":"notes.txt","media_type":"text/plain","size_bytes":4,"sha256":"4740ae6347b0172c98f8364c3e4b3e45a69e2afc6f6f6f24913a24f2c8472a8"}"""
        const val PLUGINS_RESPONSE = """[{"id":"00000000-0000-0000-0000-000000000060","name":"Repository MCP","description":"Repository tools","kind":"local_mcp","enabled":true,"connection_state":"connected","auth_state":"ready","tools":[],"bot_ids":[],"updated_at_unix_ms":1}]"""
        const val SECRETS_RESPONSE = """[{"id":"00000000-0000-0000-0000-000000000070","label":"OpenAI work","status":"ready","created_at_unix_ms":1,"updated_at_unix_ms":1}]"""
        const val CURRENT_DEVICE_RESPONSE = """{"id":"$DEVICE_ID","name":"Pixel 9","endpoint_kind":"loopback","created_at_unix_ms":1,"last_seen_at_unix_ms":2,"revoked_at_unix_ms":null}"""
        const val ROUTINE_RESPONSE = """{"id":"$ROUTINE_ID","bot_id":"$BOT_ID","name":"Review","description":"Repository review","enabled":true,"draft":false,"active_version_id":"$ROUTINE_VERSION_ID","version":1,"definition":{"inputs":[],"steps":[{"kind":"bot_prompt","bot_id":"$BOT_ID","prompt_template":"Check the repository","requires_approval":false}],"expected_outputs":[]},"created_at_unix_ms":1,"updated_at_unix_ms":1}"""
        const val ROUTINE_RUN_RESPONSE = """{"id":"00000000-0000-0000-0000-000000000083","routine_id":"$ROUTINE_ID","routine_version_id":"$ROUTINE_VERSION_ID","bot_id":"$BOT_ID","status":"succeeded","trigger":{},"input_metadata":{},"dry_run":true,"results":[],"attempt_count":1,"started_at_unix_ms":1,"finished_at_unix_ms":2}"""
        const val RECORDING_EMPTY_RESPONSE = """{"id":"$RECORDING_ID","bot_id":"$BOT_ID","name":"Recorded review","description":"","actions":[],"created_at_unix_ms":1,"updated_at_unix_ms":1}"""
        const val RECORDING_ACTION_RESPONSE = """{"id":"$RECORDING_ID","bot_id":"$BOT_ID","name":"Recorded review","description":"","actions":[{"actor":"user","step":{"kind":"bot_prompt","bot_id":"$BOT_ID","prompt_template":"Run tests","requires_approval":false}}],"created_at_unix_ms":1,"updated_at_unix_ms":2}"""

        fun hello(resume: String) = """{"protocol_version":1,"sequence":0,"event_id":"00000000-0000-0000-0000-000000000001","kind":"hello","server_version":"1.0.0","supported_protocols":{"minimum":1,"maximum":1},"resume":"$resume","heartbeat_interval_ms":30000,"heartbeat_timeout_ms":60000}"""
        fun snapshot(sequence: Int) = """{"protocol_version":1,"sequence":$sequence,"event_id":"00000000-0000-0000-0000-000000000002","kind":"snapshot","boundary_sequence":$sequence,"snapshot":{"bots":[],"chats":[]}}"""
        fun ping() = """{"protocol_version":1,"sequence":5,"event_id":"00000000-0000-0000-0000-000000000004","kind":"ping","nonce":"00000000-0000-0000-0000-000000000005"}"""
        fun botChanged(sequence: Int, name: String) = """{"protocol_version":1,"sequence":$sequence,"event_id":"00000000-0000-0000-0000-000000000003","kind":"bot_changed","bot":{"id":"00000000-0000-0000-0000-000000000010","name":"$name","title":"Helper","description":"","shape":"circle","color":"violet","archived":false,"unread_count":0,"attention":"none","provider":"not_configured","advanced":{"provider_profile_id":null,"permission_profile":"ask_before_changes"}}}"""
    }
}
