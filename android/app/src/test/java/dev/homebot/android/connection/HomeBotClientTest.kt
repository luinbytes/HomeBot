package dev.homebot.android.connection

import dev.homebot.protocol.CreateBotRequest
import dev.homebot.protocol.SendMessageResponse
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
        val deepLink = "homebot://pair?endpoint=${URLEncoder.encode(endpoint, Charsets.UTF_8)}&token=hbpair_fixture"

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
        const val VERSION_RESPONSE = """{"server_version":"0.1.0","protocol":{"minimum":1,"maximum":1}}"""
        const val ERROR_RESPONSE = """{"code":"unauthenticated","message":"Device session is invalid","retryable":false,"request_id":null}"""
        const val PAIRING_RESPONSE = """{"device":{"id":"$DEVICE_ID","name":"Pixel 9","endpoint_kind":"loopback","created_at_unix_ms":1,"last_seen_at_unix_ms":null,"revoked_at_unix_ms":null},"device_session":"hbds_fixture_session"}"""
        const val BOT_RESPONSE = """{"bot":{"id":"00000000-0000-0000-0000-000000000010","name":"Nova","title":"Researcher","description":"","shape":"circle","color":"violet","archived":false,"unread_count":0,"attention":"none","provider":"not_configured","advanced":{"provider_profile_id":null,"permission_profile":"ask_before_changes"}}}"""
        const val QUEUED_RESPONSE = """{"kind":"queued","prompt":{"id":"00000000-0000-0000-0000-000000000040","chat_id":"$CHAT_ID","content":"Follow up","attachment_ids":[],"skill_ids":[],"kind":"steering","position":0,"created_at_ms":1}}"""

        fun hello(resume: String) = """{"protocol_version":1,"sequence":0,"event_id":"00000000-0000-0000-0000-000000000001","kind":"hello","server_version":"0.1.0","supported_protocols":{"minimum":1,"maximum":1},"resume":"$resume","heartbeat_interval_ms":30000,"heartbeat_timeout_ms":60000}"""
        fun snapshot(sequence: Int) = """{"protocol_version":1,"sequence":$sequence,"event_id":"00000000-0000-0000-0000-000000000002","kind":"snapshot","boundary_sequence":$sequence,"snapshot":{"bots":[],"chats":[]}}"""
        fun ping() = """{"protocol_version":1,"sequence":5,"event_id":"00000000-0000-0000-0000-000000000004","kind":"ping","nonce":"00000000-0000-0000-0000-000000000005"}"""
        fun botChanged(sequence: Int, name: String) = """{"protocol_version":1,"sequence":$sequence,"event_id":"00000000-0000-0000-0000-000000000003","kind":"bot_changed","bot":{"id":"00000000-0000-0000-0000-000000000010","name":"$name","title":"Helper","description":"","shape":"circle","color":"violet","archived":false,"unread_count":0,"attention":"none","provider":"not_configured","advanced":{"provider_profile_id":null,"permission_profile":"ask_before_changes"}}}"""
    }
}
