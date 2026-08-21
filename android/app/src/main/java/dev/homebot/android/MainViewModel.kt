package dev.homebot.android

import android.app.Application
import android.net.Uri
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.homebot.android.connection.ConnectionState
import dev.homebot.android.settings.EndpointSettings
import dev.homebot.protocol.ApprovalSummary
import dev.homebot.protocol.BotSummary
import dev.homebot.protocol.CreateBotRequest
import dev.homebot.protocol.CreateGroupChatRequest
import dev.homebot.protocol.MessageSummary
import dev.homebot.protocol.UpdateBotRequest
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import java.util.UUID

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val homeBot = application as HomeBotApplication
    val connection: StateFlow<ConnectionState> = homeBot.client.state
    private val mutableProduct = MutableStateFlow(AndroidProductState())
    val product: StateFlow<AndroidProductState> = mutableProduct.asStateFlow()
    val settings: StateFlow<EndpointSettings> = homeBot.endpointPreferences.settings.stateIn(
        viewModelScope,
        SharingStarted.WhileSubscribed(5_000),
        EndpointSettings(),
    )

    init {
        homeBot.client.start()
        viewModelScope.launch {
            connection.filterIsInstance<ConnectionState.Live>().collect {
                refreshSelection(showLoading = false)
            }
        }
    }

    fun pair(deepLink: String, deviceName: String, onResult: (String?) -> Unit) {
        viewModelScope.launch {
            homeBot.client.pair(deepLink, deviceName).fold(
                onSuccess = {
                    val endpoint = homeBot.sessionStore.load()?.endpoint.orEmpty()
                    homeBot.endpointPreferences.update(endpoint, deviceName)
                    homeBot.client.start()
                    onResult(null)
                },
                onFailure = { onResult(it.message ?: "Pairing failed") },
            )
        }
    }

    fun updateEndpoint(endpoint: String, deviceName: String, onResult: (String?) -> Unit) {
        viewModelScope.launch {
            homeBot.client.updateEndpoint(endpoint).fold(
                onSuccess = { normalized ->
                    homeBot.endpointPreferences.update(normalized, deviceName)
                    onResult(null)
                },
                onFailure = { onResult(it.message ?: "Endpoint update failed") },
            )
        }
    }

    fun showBots() {
        mutableProduct.value = mutableProduct.value.copy(
            destination = ProductDestination.Bots,
            directTimeline = null,
            groupTimeline = null,
            error = null,
        )
    }

    fun handleDeepLink(uri: Uri) {
        when (uri.host) {
            "chat" -> uri.pathSegments.firstOrNull()?.let { chatId ->
                val activity = uri.getQueryParameter("activity")
                val message = uri.getQueryParameter("message")
                mutableProduct.value = mutableProduct.value.copy(
                    highlightedActivityId = activity,
                    highlightedMessageId = message,
                )
                if (liveSnapshot()?.group_chats?.any { it.id == chatId } == true) openGroupChat(chatId)
                else openDirectChat(chatId)
            }
            "routine" -> uri.pathSegments.firstOrNull()?.let { routineId ->
                showSettings()
                selectRoutine(routineId)
            }
            "settings" -> showSettings()
        }
    }

    fun showSearch() {
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.Search, error = null)
    }

    fun search(query: String) = perform {
        val response = homeBot.client.search(query).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            destination = ProductDestination.Search,
            searchQuery = response.query,
            searchResults = response.results,
        )
    }

    fun openSearchResult(result: dev.homebot.protocol.SearchResultSummary) {
        handleDeepLink(Uri.parse(result.deep_link))
    }

    fun showSettings() {
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.Settings, error = null)
        loadServices()
    }

    fun loadServices() = perform {
        val skills = homeBot.client.skills().getOrThrow()
        val plugins = homeBot.client.plugins().getOrThrow()
        val routines = homeBot.client.routines().getOrThrow()
        val secrets = homeBot.client.secrets().getOrThrow()
        val device = homeBot.client.currentDevice().getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            skills = skills,
            plugins = plugins,
            routines = routines,
            secrets = secrets,
            currentDevice = device,
        )
    }

    fun selectRoutine(routineId: String) = perform {
        val runs = homeBot.client.routineRuns(routineId).getOrThrow()
        val triggers = homeBot.client.routineTriggers(routineId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            selectedRoutineId = routineId,
            routineRuns = runs,
            routineTriggers = triggers,
        )
    }

    fun runRoutine(routineId: String) = perform {
        homeBot.client.runRoutine(routineId).getOrThrow()
        selectRoutine(routineId)
    }

    fun toggleRoutine(routineId: String, enabled: Boolean) = perform {
        homeBot.client.mutateRoutine(routineId, enabled).getOrThrow()
        loadServices()
    }

    fun scheduleRoutine(routineId: String) = perform {
        homeBot.client.scheduleRoutineOnce(routineId, System.currentTimeMillis() + 5 * 60_000).getOrThrow()
        selectRoutine(routineId)
    }

    fun mutatePlugin(pluginId: String, action: String) = perform {
        homeBot.client.mutatePlugin(pluginId, action).getOrThrow()
        loadServices()
    }

    fun toggleSkill(skillId: String, botId: String, enabled: Boolean) = perform {
        homeBot.client.setSkillAssigned(skillId, botId, enabled).getOrThrow()
        loadServices()
    }

    fun testSkill(skillId: String) = perform {
        val result = homeBot.client.testSkill(skillId).getOrThrow()
        require(result.capability_policy_enforced) { "Skill test did not preserve capability policy" }
        mutableProduct.value = mutableProduct.value.copy(skillTestPreview = result.prompt_preview)
    }

    fun revokeThisDevice() = perform {
        homeBot.client.revokeCurrentDevice().getOrThrow()
    }

    fun openDirectChat(chatId: String) {
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.DirectChat(chatId))
        refreshSelection()
    }

    fun openGroupChat(chatId: String) {
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.GroupChat(chatId))
        refreshSelection()
    }

    fun openBot(botId: String) = perform {
        val existing = liveSnapshot()?.chats?.firstOrNull { it.bot_id == botId }
        val chat = existing ?: homeBot.client.createDirectChat(botId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.DirectChat(chat.id))
        refreshSelection()
    }

    fun createBot(name: String, title: String) = perform {
        val request = CreateBotRequest(
            request_id = id(), idempotency_key = id(), name = name.trim(), title = title.trim(),
            shape = "circle", color = "violet", permission_profile = "ask_before_changes",
        )
        homeBot.client.createBot(request).getOrThrow()
    }

    fun updateBot(bot: BotSummary, name: String, title: String) = perform {
        val request = UpdateBotRequest(
            request_id = id(), idempotency_key = id(), name = name.trim(), title = title.trim(),
            description = bot.description, shape = bot.shape, color = bot.color,
            provider_profile_id = bot.advanced.provider_profile_id,
            permission_profile = bot.advanced.permission_profile,
        )
        homeBot.client.updateBot(bot.id, request).getOrThrow()
    }

    fun setBotArchived(botId: String, archived: Boolean) = perform {
        homeBot.client.setBotArchived(botId, archived).getOrThrow()
    }

    fun setBotPinned(botId: String, pinned: Boolean) = perform {
        homeBot.client.setBotPinned(botId, pinned).getOrThrow()
    }

    fun setBotHidden(botId: String, hidden: Boolean) = perform {
        homeBot.client.setBotHidden(botId, hidden).getOrThrow()
    }

    fun duplicateBot(botId: String) = perform {
        homeBot.client.duplicateBot(botId).getOrThrow()
    }

    fun deleteBot(bot: BotSummary, confirmation: String) = perform {
        require(confirmation == bot.name) { "Type the Bot name exactly to delete it" }
        homeBot.client.deleteBot(bot.id, confirmation).getOrThrow()
    }

    fun createGroup(title: String, botIds: List<String>) = perform {
        require(botIds.size >= 2) { "Choose at least two Bots" }
        val response = homeBot.client.createGroup(
            CreateGroupChatRequest(id(), id(), title.trim(), botIds, botIds.first(), 12, 3),
        ).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.GroupChat(response.group.id))
        refreshSelection()
    }

    fun send(content: String, steering: Boolean = false, mentions: List<String> = emptyList(), replyToMessageId: String? = null) = perform {
        when (val destination = mutableProduct.value.destination) {
            is ProductDestination.DirectChat -> homeBot.client.sendMessage(destination.chatId, content, steering, replyToMessageId = replyToMessageId).getOrThrow()
            is ProductDestination.GroupChat -> homeBot.client.sendGroupMessage(destination.chatId, content, mentions, replyToMessageId).getOrThrow()
            else -> error("Open a chat before sending a message")
        }
        refreshSelection(showLoading = false)
    }

    fun sendAttachment(uri: Uri) = perform {
        val resolver = getApplication<Application>().contentResolver
        val mediaType = resolver.getType(uri) ?: "application/octet-stream"
        var filename = "attachment"
        resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) filename = cursor.getString(0) ?: filename
        }
        val bytes = resolver.openInputStream(uri)?.use { it.readBytes() }
            ?: error("Android could not read this attachment")
        val attachment = homeBot.client.uploadAttachment(filename, mediaType, bytes).getOrThrow()
        val chat = (mutableProduct.value.destination as? ProductDestination.DirectChat)?.chatId
            ?: error("Attachments are currently supported in direct chats")
        homeBot.client.sendMessage(chat, "", attachmentIds = listOf(attachment.id)).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun setReaction(messageId: String, emoji: String, active: Boolean) = perform {
        homeBot.client.setReaction(messageId, emoji, active).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun compareRecentCheckpoints() = perform {
        val timeline = mutableProduct.value.directTimeline ?: error("Open a direct chat first")
        require(timeline.checkpoints.size >= 2) { "At least two checkpoints are required for an exact diff" }
        val from = timeline.checkpoints[timeline.checkpoints.lastIndex - 1]
        val to = timeline.checkpoints.last()
        val diff = homeBot.client.checkpointDiff(timeline.chat.id, from.id, to.id).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            coding = mutableProduct.value.coding.copy(
                diff = dev.homebot.protocol.WorkingTreeDiffResponse(false, diff.patch, diff.files),
            ),
        )
    }

    fun restoreLatestCheckpoint() = perform {
        val checkpoint = mutableProduct.value.directTimeline?.checkpoints?.lastOrNull()
            ?: error("No checkpoint is available to restore")
        homeBot.client.restoreCheckpoint(checkpoint.id).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun stopWorking() = perform {
        when (val destination = mutableProduct.value.destination) {
            is ProductDestination.DirectChat -> homeBot.client.stop(destination.chatId, false).getOrThrow()
            is ProductDestination.GroupChat -> homeBot.client.stop(destination.chatId, true).getOrThrow()
            else -> error("Open a chat before stopping work")
        }
    }

    fun retry(message: MessageSummary) = perform {
        homeBot.client.retry(message.chat_id, message.id).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun decide(approval: ApprovalSummary, allow: Boolean) = perform {
        homeBot.client.decideApproval(approval.id, allow).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun handoff(fromBotId: String, toBotId: String) = perform {
        val group = mutableProduct.value.destination as? ProductDestination.GroupChat
            ?: error("Open a group chat before handing off ownership")
        homeBot.client.handoff(group.chatId, fromBotId, toBotId).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun renameGroup(title: String) = perform {
        val group = mutableProduct.value.destination as? ProductDestination.GroupChat
            ?: error("Open a group chat before renaming it")
        homeBot.client.renameGroup(group.chatId, title.trim()).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun addGroupParticipant(botId: String) = perform {
        val group = mutableProduct.value.destination as? ProductDestination.GroupChat
            ?: error("Open a group chat before adding a Bot")
        homeBot.client.addGroupParticipant(group.chatId, botId).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun removeGroupParticipant(botId: String) = perform {
        val group = mutableProduct.value.destination as? ProductDestination.GroupChat
            ?: error("Open a group chat before removing a Bot")
        homeBot.client.removeGroupParticipant(group.chatId, botId).getOrThrow()
        refreshSelection(showLoading = false)
    }

    fun loadCodingWorkspace() = perform {
        val chat = (mutableProduct.value.destination as? ProductDestination.DirectChat)?.chatId
            ?: error("Coding workspace details are available in direct chats")
        val status = homeBot.client.vcsStatus(chat).getOrThrow()
        val diff = homeBot.client.workingTreeDiff(chat).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = CodingWorkspaceProjection(status, diff))
    }

    fun clearError() {
        mutableProduct.value = mutableProduct.value.copy(error = null)
    }

    private fun refreshSelection(showLoading: Boolean = true) = perform(showLoading) {
        when (val destination = mutableProduct.value.destination) {
            is ProductDestination.DirectChat -> {
                val timeline = homeBot.client.directTimeline(destination.chatId).getOrThrow()
                mutableProduct.value = mutableProduct.value.copy(directTimeline = timeline, groupTimeline = null)
                homeBot.client.markChatRead(destination.chatId)
            }
            is ProductDestination.GroupChat -> {
                val timeline = homeBot.client.groupTimeline(destination.chatId).getOrThrow()
                mutableProduct.value = mutableProduct.value.copy(groupTimeline = timeline, directTimeline = null)
            }
            else -> Unit
        }
    }

    private fun perform(showLoading: Boolean = true, operation: suspend () -> Unit) {
        viewModelScope.launch {
            if (showLoading) mutableProduct.value = mutableProduct.value.copy(loading = true, error = null)
            runCatching { operation() }.onFailure { failure ->
                mutableProduct.value = mutableProduct.value.copy(error = failure.message ?: "HomeBot request failed")
            }
            if (showLoading) mutableProduct.value = mutableProduct.value.copy(loading = false)
        }
    }

    private fun liveSnapshot() = (connection.value as? ConnectionState.Live)?.snapshot
    private fun id(): String = UUID.randomUUID().toString()

    override fun onCleared() {
        homeBot.client.stop()
        super.onCleared()
    }
}
