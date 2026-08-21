package dev.homebot.android.connection

import dev.homebot.protocol.ActivitySummary
import dev.homebot.protocol.ApprovalSummary
import dev.homebot.protocol.MessageSummary
import dev.homebot.protocol.RoutineRunSummary
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.decodeFromJsonElement

internal object NotificationEventMapper {
    fun map(kind: String, event: JsonObject, json: Json): ClientAlert? {
        val eventId = event["event_id"]?.toString()?.trim('"') ?: return null
        return when (kind) {
            "message_changed" -> {
                val message = json.decodeFromJsonElement<MessageSummary>(event["message"] ?: return null)
                if (message.author != "bot" || message.status !in setOf("completed", "failed")) return null
                ClientAlert(
                    eventId,
                    if (message.status == "failed") ClientAlertKind.ERROR else ClientAlertKind.BOT_FINISHED,
                    if (message.status == "failed") "Bot needs attention" else "Bot finished",
                    message.error?.message ?: "Open the chat to review the result.",
                    chatId = message.chat_id,
                    botId = message.author_bot_id,
                )
            }
            "approval_changed" -> {
                val approval = json.decodeFromJsonElement<ApprovalSummary>(event["approval"] ?: return null)
                if (approval.status != "pending") return null
                ClientAlert(eventId, ClientAlertKind.APPROVAL_REQUIRED, approval.title, approval.detail, chatId = approval.chat_id, activityId = approval.id)
            }
            "routine_run_changed" -> {
                val run = json.decodeFromJsonElement<RoutineRunSummary>(event["run"] ?: return null)
                if (run.status !in setOf("succeeded", "failed", "cancelled")) return null
                ClientAlert(
                    eventId,
                    if (run.status == "failed") ClientAlertKind.ERROR else ClientAlertKind.ROUTINE_RESULT,
                    if (run.status == "succeeded") "Routine finished" else "Routine ${run.status}",
                    run.error_message ?: "Open HomeBot to review the run.",
                    botId = run.bot_id,
                    routineId = run.routine_id,
                    runId = run.id,
                )
            }
            "activity_changed" -> {
                val activity = json.decodeFromJsonElement<ActivitySummary>(event["activity"] ?: return null)
                if (!activity.requires_attention && activity.status != "failed") return null
                ClientAlert(eventId, ClientAlertKind.ERROR, activity.title, activity.detail, chatId = activity.chat_id, activityId = activity.id)
            }
            else -> null
        }
    }
}
