package dev.homebot.android

import dev.homebot.protocol.ChatTimelineResponse
import dev.homebot.protocol.GroupTimelineResponse
import dev.homebot.protocol.VcsStatus
import dev.homebot.protocol.WorkingTreeDiffResponse

sealed interface ProductDestination {
    data object Bots : ProductDestination
    data class DirectChat(val chatId: String) : ProductDestination
    data class GroupChat(val chatId: String) : ProductDestination
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
    val loading: Boolean = false,
    val error: String? = null,
)
