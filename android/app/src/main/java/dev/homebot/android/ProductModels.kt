package dev.homebot.android

import dev.homebot.protocol.ChatTimelineResponse
import dev.homebot.protocol.ChatSummary
import dev.homebot.protocol.BotSummary
import dev.homebot.protocol.AssistantPackSummary
import dev.homebot.protocol.GroupTimelineResponse
import dev.homebot.protocol.VcsStatus
import dev.homebot.protocol.WorkingTreeDiffResponse
import dev.homebot.protocol.DeviceSessionSummary
import dev.homebot.protocol.PluginSummary
import dev.homebot.protocol.RoutineRunSummary
import dev.homebot.protocol.RoutineRecordingSummary
import dev.homebot.protocol.RoutineSummary
import dev.homebot.protocol.RoutineTriggerSummary
import dev.homebot.protocol.SecretSummary
import dev.homebot.protocol.SkillSummary
import dev.homebot.protocol.SearchResultSummary
import dev.homebot.protocol.ChatWorkspaceSummary
import dev.homebot.protocol.PullRequestMetadata
import dev.homebot.protocol.VcsCommitResult

sealed interface ProductDestination {
    data object Bots : ProductDestination
    data class DirectChat(val chatId: String) : ProductDestination
    data class GroupChat(val chatId: String) : ProductDestination
    data object Search : ProductDestination
    data object Settings : ProductDestination
}

data class CodingWorkspaceProjection(
    val workspace: ChatWorkspaceSummary? = null,
    val status: VcsStatus? = null,
    val diff: WorkingTreeDiffResponse? = null,
    val stagedDiff: WorkingTreeDiffResponse? = null,
    val commit: VcsCommitResult? = null,
    val pullRequest: PullRequestMetadata? = null,
    val remoteNotice: String? = null,
)

data class BotConversation(val bot: BotSummary, val chat: ChatSummary?)

internal fun botConversations(
    bots: List<BotSummary>,
    chats: List<ChatSummary>,
    archived: Boolean = false,
    showHidden: Boolean = false,
): List<BotConversation> {
    val chatsByBot = chats.associateBy(ChatSummary::bot_id)
    return bots
        .asSequence()
        .filter { it.archived == archived && (showHidden || !it.hidden) }
        .map { BotConversation(it, chatsByBot[it.id]) }
        .sortedWith(
            compareByDescending<BotConversation> { it.bot.pinned }
                .thenByDescending { it.chat?.last_sequence ?: -1 }
                .thenBy { it.bot.name.lowercase() },
        )
        .toList()
}

data class AndroidProductState(
    val destination: ProductDestination = ProductDestination.Bots,
    val directTimeline: ChatTimelineResponse? = null,
    val groupTimeline: GroupTimelineResponse? = null,
    val coding: CodingWorkspaceProjection = CodingWorkspaceProjection(),
    val assistantPacks: List<AssistantPackSummary> = emptyList(),
    val skills: List<SkillSummary> = emptyList(),
    val plugins: List<PluginSummary> = emptyList(),
    val routines: List<RoutineSummary> = emptyList(),
    val routineRuns: List<RoutineRunSummary> = emptyList(),
    val routineTriggers: List<RoutineTriggerSummary> = emptyList(),
    val activeRoutineRecording: RoutineRecordingSummary? = null,
    val secrets: List<SecretSummary> = emptyList(),
    val currentDevice: DeviceSessionSummary? = null,
    val selectedRoutineId: String? = null,
    val highlightedActivityId: String? = null,
    val highlightedMessageId: String? = null,
    val searchQuery: String = "",
    val searchResults: List<SearchResultSummary> = emptyList(),
    val skillTestPreview: String? = null,
    val assistantPackNotice: String? = null,
    val loading: Boolean = false,
    val error: String? = null,
)
