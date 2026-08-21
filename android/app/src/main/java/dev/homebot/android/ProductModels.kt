package dev.homebot.android

import dev.homebot.protocol.ChatTimelineResponse
import dev.homebot.protocol.GroupTimelineResponse
import dev.homebot.protocol.VcsStatus
import dev.homebot.protocol.WorkingTreeDiffResponse
import dev.homebot.protocol.DeviceSessionSummary
import dev.homebot.protocol.PluginSummary
import dev.homebot.protocol.RoutineRunSummary
import dev.homebot.protocol.RoutineSummary
import dev.homebot.protocol.RoutineTriggerSummary
import dev.homebot.protocol.SecretSummary
import dev.homebot.protocol.SkillSummary
import dev.homebot.protocol.SearchResultSummary

sealed interface ProductDestination {
    data object Bots : ProductDestination
    data class DirectChat(val chatId: String) : ProductDestination
    data class GroupChat(val chatId: String) : ProductDestination
    data object Search : ProductDestination
    data object Settings : ProductDestination
}

data class CodingWorkspaceProjection(
    val status: VcsStatus? = null,
    val diff: WorkingTreeDiffResponse? = null,
)

data class AndroidProductState(
    val destination: ProductDestination = ProductDestination.Bots,
    val directTimeline: ChatTimelineResponse? = null,
    val groupTimeline: GroupTimelineResponse? = null,
    val coding: CodingWorkspaceProjection = CodingWorkspaceProjection(),
    val skills: List<SkillSummary> = emptyList(),
    val plugins: List<PluginSummary> = emptyList(),
    val routines: List<RoutineSummary> = emptyList(),
    val routineRuns: List<RoutineRunSummary> = emptyList(),
    val routineTriggers: List<RoutineTriggerSummary> = emptyList(),
    val secrets: List<SecretSummary> = emptyList(),
    val currentDevice: DeviceSessionSummary? = null,
    val selectedRoutineId: String? = null,
    val highlightedActivityId: String? = null,
    val highlightedMessageId: String? = null,
    val searchQuery: String = "",
    val searchResults: List<SearchResultSummary> = emptyList(),
    val loading: Boolean = false,
    val error: String? = null,
)
