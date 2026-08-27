package dev.homebot.android.connection

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NotificationEventMapperTest {
    private val json = Json { ignoreUnknownKeys = true; classDiscriminator = "kind" }

    @Test
    fun terminalBotApprovalRoutineAndActivityEventsMapToExactTargets() {
        val bot = map("message_changed", BOT_FINISHED)
        assertEquals(ClientAlertKind.BOT_FINISHED, bot?.kind)
        assertEquals(CHAT_ID, bot?.chatId)
        assertEquals(BOT_ID, bot?.botId)
        assertEquals("homebot://chat/$CHAT_ID?bot=$BOT_ID", bot?.deepLink())

        val approval = map("approval_changed", APPROVAL_REQUIRED)
        assertEquals(ClientAlertKind.APPROVAL_REQUIRED, approval?.kind)
        assertEquals(APPROVAL_ID, approval?.activityId)

        val routine = map("routine_run_changed", ROUTINE_FAILED)
        assertEquals(ClientAlertKind.ERROR, routine?.kind)
        assertEquals(ROUTINE_ID, routine?.routineId)
        assertEquals(RUN_ID, routine?.runId)
        assertEquals("homebot://routine/$ROUTINE_ID?run=$RUN_ID", routine?.deepLink())

        val activity = map("activity_changed", ACTIVITY_FAILED)
        assertEquals(ACTIVITY_ID, activity?.activityId)
        assertEquals(CHAT_ID, activity?.chatId)

        val input = map("activity_changed", INTERACTION_PENDING)
        assertEquals(ClientAlertKind.NEEDS_INPUT, input?.kind)
        assertEquals("Input needed", input?.title)
        assertEquals("homebot://chat/$CHAT_ID?activity=$ACTIVITY_ID", input?.deepLink())
    }

    @Test
    fun runningAndNonBotMessagesDoNotCreateNotifications() {
        assertNull(map("message_changed", USER_MESSAGE))
        assertNull(map("routine_run_changed", ROUTINE_RUNNING))
    }

    private fun map(kind: String, body: String): ClientAlert? = NotificationEventMapper.map(
        kind,
        json.parseToJsonElement(body).jsonObject,
        json,
    )

    private companion object {
        const val CHAT_ID = "00000000-0000-0000-0000-000000000101"
        const val BOT_ID = "00000000-0000-0000-0000-000000000102"
        const val APPROVAL_ID = "00000000-0000-0000-0000-000000000103"
        const val ROUTINE_ID = "00000000-0000-0000-0000-000000000104"
        const val RUN_ID = "00000000-0000-0000-0000-000000000105"
        const val ACTIVITY_ID = "00000000-0000-0000-0000-000000000106"
        const val PREFIX = "\"protocol_version\":1,\"sequence\":2,\"event_id\":\"00000000-0000-0000-0000-000000000100\""
        const val BOT_FINISHED = """{$PREFIX,"message":{"id":"00000000-0000-0000-0000-000000000110","chat_id":"$CHAT_ID","author":"bot","author_bot_id":"$BOT_ID","status":"completed","parts":[],"mentioned_bot_ids":[],"shared_context_message_ids":[],"created_at_ms":1}}"""
        const val USER_MESSAGE = """{$PREFIX,"message":{"id":"00000000-0000-0000-0000-000000000111","chat_id":"$CHAT_ID","author":"user","status":"completed","parts":[],"mentioned_bot_ids":[],"shared_context_message_ids":[],"created_at_ms":1}}"""
        const val APPROVAL_REQUIRED = """{$PREFIX,"approval":{"id":"$APPROVAL_ID","chat_id":"$CHAT_ID","title":"Allow command","detail":"git status","status":"pending","created_at_ms":1}}"""
        const val ROUTINE_FAILED = """{$PREFIX,"run":{"id":"$RUN_ID","routine_id":"$ROUTINE_ID","routine_version_id":"00000000-0000-0000-0000-000000000107","bot_id":"$BOT_ID","status":"failed","trigger":{},"input_metadata":{},"dry_run":false,"results":[],"error_message":"Provider unavailable","attempt_count":1,"started_at_unix_ms":1,"finished_at_unix_ms":2}}"""
        const val ROUTINE_RUNNING = """{$PREFIX,"run":{"id":"$RUN_ID","routine_id":"$ROUTINE_ID","routine_version_id":"00000000-0000-0000-0000-000000000107","bot_id":"$BOT_ID","status":"running","trigger":{},"input_metadata":{},"dry_run":false,"results":[],"attempt_count":1,"started_at_unix_ms":1}}"""
        const val ACTIVITY_FAILED = """{$PREFIX,"activity":{"id":"$ACTIVITY_ID","chat_id":"$CHAT_ID","title":"Command failed","detail":"Exit 1","kind":"terminal","presentation":{"risk":"low","detail":{}},"status":"failed","requires_attention":true,"started_at_ms":1,"finished_at_ms":2}}"""
        const val INTERACTION_PENDING = """{$PREFIX,"activity":{"id":"$ACTIVITY_ID","chat_id":"$CHAT_ID","title":"Choose an account","detail":"Pick one","kind":"interaction","presentation":{"risk":"low","detail":{}},"status":"pending","requires_attention":true,"started_at_ms":1}}"""
    }
}
