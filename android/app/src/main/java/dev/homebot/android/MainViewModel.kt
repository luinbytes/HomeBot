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
import dev.homebot.protocol.RecordedAction
import dev.homebot.protocol.RoutineDefinition
import dev.homebot.protocol.RoutineSummary
import dev.homebot.protocol.UpdateBotRequest
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import java.util.UUID

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val homeBot = application as HomeBotApplication
    val connection: StateFlow<ConnectionState> = homeBot.client.state
    private val mutableProduct = MutableStateFlow(AndroidProductState())
    private var pendingPush: PendingRemoteMutation? = null
    private var pendingPullRequest: PendingPullRequest? = null
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
            searchUnavailable = response.status.name == "UNAVAILABLE",
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
        loadServicesNow()
    }

    private suspend fun loadServicesNow() {
        val assistantPacks = homeBot.client.assistantPacks().getOrThrow()
        val skills = homeBot.client.skills().getOrThrow()
        val plugins = homeBot.client.plugins().getOrThrow()
        val memoryProviders = homeBot.client.memoryProviders().getOrThrow()
        val routines = homeBot.client.routines().getOrThrow()
        val secrets = homeBot.client.secrets().getOrThrow()
        val device = homeBot.client.currentDevice().getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            assistantPacks = assistantPacks,
            skills = skills,
            plugins = plugins,
            memoryProviders = memoryProviders,
            routines = routines,
            secrets = secrets,
            currentDevice = device,
        )
    }

    fun takeOverBrowser(sessionId: String, approvalId: String? = null) = perform {
        homeBot.client.takeOverBrowser(sessionId, approvalId).getOrThrow()
    }

    fun returnBrowserToBot(sessionId: String) = perform {
        homeBot.client.returnBrowserToBot(sessionId).getOrThrow()
    }

    fun watchBrowser(sessionId: String) = perform {
        val result = homeBot.client.watchBrowser(sessionId).getOrThrow()
        result.artifact?.let { artifact ->
            mutableProduct.value = mutableProduct.value.copy(
                highlightedActivityId = artifact.activity_id,
            )
        }
    }

    fun selectRoutine(routineId: String) = perform {
        selectRoutineNow(routineId)
    }

    private suspend fun selectRoutineNow(routineId: String) {
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
        selectRoutineNow(routineId)
    }

    fun dryRunRoutine(routineId: String) = perform {
        homeBot.client.dryRunRoutine(routineId).getOrThrow()
        selectRoutineNow(routineId)
    }

    fun toggleRoutine(routineId: String, enabled: Boolean) = perform {
        homeBot.client.mutateRoutine(routineId, enabled).getOrThrow()
        loadServicesNow()
    }

    fun scheduleRoutine(routineId: String) = perform {
        homeBot.client.scheduleRoutineOnce(routineId, System.currentTimeMillis() + 5 * 60_000).getOrThrow()
        selectRoutineNow(routineId)
    }

    fun createRoutine(botId: String, name: String, description: String, prompt: String, requiresApproval: Boolean) = perform {
        homeBot.client.createRoutine(
            botId,
            name.trim(),
            description.trim(),
            promptDefinition(botId, prompt, requiresApproval),
            draft = false,
        ).getOrThrow()
        loadServicesNow()
    }

    fun updateRoutine(routine: RoutineSummary, name: String, description: String, prompt: String, requiresApproval: Boolean) = perform {
        val steps = routine.definition.steps.toMutableList()
        val promptIndex = steps.indexOfFirst {
            it.jsonObject["kind"]?.jsonPrimitive?.contentOrNull == "bot_prompt"
        }
        require(promptIndex >= 0) { "This routine has no Bot instruction to edit on mobile" }
        steps[promptIndex] = botPromptStep(routine.bot_id, prompt, requiresApproval)
        homeBot.client.updateRoutine(
            routine.id,
            name.trim(),
            description.trim(),
            routine.definition.copy(steps = steps),
            draft = false,
        ).getOrThrow()
        loadServicesNow()
        selectRoutineNow(routine.id)
    }

    fun duplicateRoutine(routine: RoutineSummary) = perform {
        homeBot.client.duplicateRoutine(routine.id, "${routine.name} copy").getOrThrow()
        loadServicesNow()
    }

    fun deleteRoutine(routineId: String) = perform {
        homeBot.client.deleteRoutine(routineId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            selectedRoutineId = null,
            routineRuns = emptyList(),
            routineTriggers = emptyList(),
        )
        loadServicesNow()
    }

    fun startRoutineRecording(botId: String, name: String, description: String) = perform {
        val recording = homeBot.client.startRoutineRecording(botId, name.trim(), description.trim()).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(activeRoutineRecording = recording)
    }

    fun appendRoutineRecording(prompt: String, requiresApproval: Boolean) = perform {
        val recording = mutableProduct.value.activeRoutineRecording ?: error("Start recording first")
        val action = RecordedAction(
            actor = "user",
            step = botPromptStep(recording.bot_id, prompt, requiresApproval),
        )
        val updated = homeBot.client.appendRoutineRecording(recording.id, action).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(activeRoutineRecording = updated)
    }

    fun finishRoutineRecording() = perform {
        val recording = mutableProduct.value.activeRoutineRecording ?: error("No routine recording is active")
        val routine = homeBot.client.finishRoutineRecording(recording.id).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            activeRoutineRecording = null,
            selectedRoutineId = routine.id,
        )
        loadServicesNow()
        selectRoutineNow(routine.id)
    }

    fun cancelRoutineRecording() = perform {
        val recording = mutableProduct.value.activeRoutineRecording ?: error("No routine recording is active")
        homeBot.client.cancelRoutineRecording(recording.id).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(activeRoutineRecording = null)
    }

    fun mutatePlugin(pluginId: String, action: String) = perform {
        homeBot.client.mutatePlugin(pluginId, action).getOrThrow()
        loadServicesNow()
    }

    fun authorizeRemoteMcp(pluginId: String) = perform {
        val authorization = homeBot.client.authorizeRemoteMcp(pluginId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(externalAuthorization = authorization)
        loadServicesNow()
    }

    fun togglePlugin(pluginId: String, botId: String, enabled: Boolean) = perform {
        homeBot.client.setPluginAssigned(pluginId, botId, enabled).getOrThrow()
        loadServicesNow()
    }

    fun createMemoryProvider(
        providerId: String,
        name: String,
        endpoint: String?,
        credential: String?,
    ) = perform {
        val secretId = credential?.takeIf(String::isNotBlank)?.let {
            homeBot.client.createSecret("$name credential", it).getOrThrow().id
        }
        val plugin = try {
            homeBot.client.createMemoryProvider(
                providerId,
                name,
                endpoint?.takeIf(String::isNotBlank),
                secretId,
            ).getOrThrow()
        } catch (failure: Throwable) {
            secretId?.let { withContext(NonCancellable) { homeBot.client.deleteSecret(it) } }
            throw failure
        }
        if (plugin.kind != "builtin_memory") {
            homeBot.client.mutatePlugin(plugin.id, "connect").getOrThrow()
        }
        loadServicesNow()
    }

    fun createRemoteMcp(name: String, endpoint: String, bearerToken: String) = perform {
        val secretId = bearerToken.takeIf(String::isNotBlank)?.let {
            homeBot.client.createSecret("$name bearer token", it).getOrThrow().id
        }
        val plugin = try {
            homeBot.client.createRemoteMcp(name, endpoint, secretId).getOrThrow()
        } catch (failure: Throwable) {
            secretId?.let { withContext(NonCancellable) { homeBot.client.deleteSecret(it) } }
            throw failure
        }
        homeBot.client.mutatePlugin(plugin.id, "connect").getOrThrow()
        loadServicesNow()
    }

    fun createComposioConnector(
        name: String,
        toolkit: String,
        apiKey: String,
    ) = perform {
        val secretId = homeBot.client.createSecret("$name Composio API key", apiKey).getOrThrow().id
        val plugin = try {
            homeBot.client.createComposioConnector(name, secretId, listOf(toolkit)).getOrThrow()
        } catch (failure: Throwable) {
            withContext(NonCancellable) { homeBot.client.deleteSecret(secretId) }
            throw failure
        }
        homeBot.client.mutatePlugin(plugin.id, "connect").getOrThrow()
        val authorization = homeBot.client.authorizeComposioToolkit(plugin.id, toolkit).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(externalAuthorization = authorization)
        loadServicesNow()
    }

    fun clearExternalAuthorization() {
        mutableProduct.value = mutableProduct.value.copy(externalAuthorization = null)
    }

    fun revokeComposioAccount(
        pluginId: String,
        toolkit: String,
        reauthorize: Boolean,
    ) = perform {
        homeBot.client.revokeComposioToolkit(pluginId, toolkit).getOrThrow()
        val authorization = if (reauthorize) {
            homeBot.client.authorizeComposioToolkit(pluginId, toolkit).getOrThrow()
        } else {
            null
        }
        mutableProduct.value = mutableProduct.value.copy(externalAuthorization = authorization)
        loadServicesNow()
    }

    fun configureComposioEvents(pluginId: String) = perform {
        homeBot.client.configureComposioEvents(pluginId).getOrThrow()
        loadServicesNow()
    }

    fun installAssistantPack(
        packId: String,
        botId: String,
        timezone: String,
        hour: Int,
        minute: Int,
    ) = perform {
        val installed = homeBot.client.installAssistantPack(
            packId,
            botId,
            timezone.trim(),
            hour,
            minute,
        ).getOrThrow()
        loadServicesNow()
        mutableProduct.value = mutableProduct.value.copy(
            assistantPackNotice = "${installed.routine.name} installed and scheduled",
        )
    }

    fun toggleSkill(skillId: String, botId: String, enabled: Boolean) = perform {
        homeBot.client.setSkillAssigned(skillId, botId, enabled).getOrThrow()
        loadServicesNow()
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
        mutableProduct.value = mutableProduct.value.copy(
            destination = ProductDestination.DirectChat(chatId),
            coding = CodingWorkspaceProjection(),
        )
        refreshSelection()
    }

    fun openGroupChat(chatId: String) {
        mutableProduct.value = mutableProduct.value.copy(
            destination = ProductDestination.GroupChat(chatId),
            coding = CodingWorkspaceProjection(),
        )
        refreshSelection()
    }

    fun openBot(botId: String) = perform {
        val existing = liveSnapshot()?.chats?.firstOrNull { it.bot_id == botId }
        val chat = existing ?: homeBot.client.createDirectChat(botId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(
            destination = ProductDestination.DirectChat(chat.id),
            coding = CodingWorkspaceProjection(),
        )
        refreshSelection()
    }

    fun createBot(name: String, title: String, description: String) = perform {
        val request = CreateBotRequest(
            request_id = id(), idempotency_key = id(), name = name.trim(), title = title.trim(),
            description = description.trim(),
            shape = "circle", color = "violet", permission_profile = "ask_before_changes",
        )
        homeBot.client.createBot(request).getOrThrow()
    }

    fun updateBot(bot: BotSummary, name: String, title: String, description: String) = perform {
        val request = UpdateBotRequest(
            request_id = id(), idempotency_key = id(), name = name.trim(), title = title.trim(),
            description = description.trim(), shape = bot.shape, color = bot.color,
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

    fun respondInteraction(
        interactionId: String,
        confirmed: Boolean? = null,
        choice: String? = null,
        secret: String? = null,
    ) = perform {
        homeBot.client.respondInteraction(interactionId, confirmed, choice, secret).getOrThrow()
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
        val workspace = liveSnapshot()?.chat_workspaces?.firstOrNull { it.chat_id == chat }
            ?: error("Attach a repository before loading source control")
        val status = homeBot.client.vcsStatus(chat).getOrThrow()
        val diff = homeBot.client.workingTreeDiff(chat).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = CodingWorkspaceProjection(workspace, status, diff))
    }

    fun registerAndAttachWorkspace(path: String, name: String) = perform {
        val chat = directChatId()
        val repository = homeBot.client.registerWorkspace(path, name).getOrThrow()
        val workspace = homeBot.client.attachWorkspace(chat, repository.id).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = CodingWorkspaceProjection(workspace = workspace))
        loadCodingWorkspaceNow(chat, workspace)
    }

    fun attachWorkspace(workspaceId: String) = perform {
        val chat = directChatId()
        val workspace = homeBot.client.attachWorkspace(chat, workspaceId).getOrThrow()
        loadCodingWorkspaceNow(chat, workspace)
    }

    fun detachWorkspace() = perform {
        val chat = directChatId()
        homeBot.client.detachWorkspace(chat).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = CodingWorkspaceProjection())
    }

    fun loadStagedDiff() = perform {
        val diff = homeBot.client.workingTreeDiff(directChatId(), staged = true).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = mutableProduct.value.coding.copy(stagedDiff = diff))
    }

    fun commitAll(message: String) = perform {
        val chat = directChatId()
        val result = homeBot.client.commit(chat, message).getOrThrow()
        val workspace = mutableProduct.value.coding.workspace
        loadCodingWorkspaceNow(chat, workspace)
        mutableProduct.value = mutableProduct.value.copy(coding = mutableProduct.value.coding.copy(commit = result))
    }

    fun createBranch(branch: String) = perform {
        val status = homeBot.client.createBranch(directChatId(), branch).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = mutableProduct.value.coding.copy(status = status))
    }

    fun pushCurrentBranch() = perform {
        val chat = directChatId()
        val status = mutableProduct.value.coding.status ?: error("Load source control first")
        val branch = status.branch ?: error("Create or switch to a branch before pushing")
        val remote = status.remotes.firstOrNull { it.push_configured }?.name ?: error("No push remote is configured")
        val pending = pendingPush ?: PendingRemoteMutation(chat, id(), id(), remote, branch)
        val response = homeBot.client.push(
            pending.chatId,
            pending.requestId,
            pending.idempotencyKey,
            pending.remote,
            pending.branch,
            pending.approvalId,
        ).getOrThrow()
        pendingPush = if (response.status == "approval_required") pending.copy(approvalId = response.approval?.id) else null
        mutableProduct.value = mutableProduct.value.copy(
            coding = mutableProduct.value.coding.copy(remoteNotice = response.result?.let { "Pushed ${it.branch} to ${it.remote}" } ?: "Approval required before push"),
        )
    }

    fun loadPullRequest(base: String) = perform {
        val status = mutableProduct.value.coding.status ?: error("Load source control first")
        val remote = status.remotes.firstOrNull { it.push_configured }?.name ?: error("No push remote is configured")
        val head = status.branch ?: error("Create or switch to a branch first")
        val metadata = homeBot.client.pullRequest(directChatId(), remote, head, base.trim()).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = mutableProduct.value.coding.copy(pullRequest = metadata))
    }

    fun createPullRequest(base: String, title: String) = perform {
        val chat = directChatId()
        val status = mutableProduct.value.coding.status ?: error("Load source control first")
        val remote = status.remotes.firstOrNull { it.push_configured }?.name ?: error("No push remote is configured")
        val head = status.branch ?: error("Create or switch to a branch first")
        val pending = pendingPullRequest ?: PendingPullRequest(chat, id(), id(), remote, head, base.trim(), title.trim())
        val response = homeBot.client.createPullRequest(
            pending.chatId,
            pending.requestId,
            pending.idempotencyKey,
            pending.remote,
            pending.head,
            pending.base,
            pending.title,
            pending.approvalId,
        ).getOrThrow()
        pendingPullRequest = if (response.status == "approval_required") pending.copy(approvalId = response.approval?.id) else null
        mutableProduct.value = mutableProduct.value.copy(
            coding = mutableProduct.value.coding.copy(
                remoteNotice = response.result?.let { "PR #${it.number} created" } ?: "Approval required before creating the pull request",
            ),
        )
    }

    fun clearError() {
        mutableProduct.value = mutableProduct.value.copy(error = null)
    }

    private fun refreshSelection(showLoading: Boolean = true) = perform(showLoading) {
        when (val destination = mutableProduct.value.destination) {
            is ProductDestination.DirectChat -> {
                val timeline = homeBot.client.directTimeline(destination.chatId).getOrThrow()
                mutableProduct.value = mutableProduct.value.copy(directTimeline = timeline, groupTimeline = null)
                if (timeline.chat.unread_count > 0) {
                    homeBot.client.markChatRead(destination.chatId).getOrThrow()
                }
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
    private fun directChatId() = (mutableProduct.value.destination as? ProductDestination.DirectChat)?.chatId
        ?: error("Open a direct chat first")

    private suspend fun loadCodingWorkspaceNow(chatId: String, workspace: dev.homebot.protocol.ChatWorkspaceSummary?) {
        val status = homeBot.client.vcsStatus(chatId).getOrThrow()
        val diff = homeBot.client.workingTreeDiff(chatId).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(coding = CodingWorkspaceProjection(workspace, status, diff))
    }
    private fun id(): String = UUID.randomUUID().toString()

    private fun promptDefinition(botId: String, prompt: String, requiresApproval: Boolean) = RoutineDefinition(
        inputs = emptyList(),
        steps = listOf(botPromptStep(botId, prompt, requiresApproval)),
        expected_outputs = emptyList(),
    )

    private fun botPromptStep(botId: String, prompt: String, requiresApproval: Boolean) = buildJsonObject {
        put("kind", "bot_prompt")
        put("bot_id", botId)
        put("prompt_template", prompt.trim())
        put("requires_approval", requiresApproval)
    }

    override fun onCleared() {
        homeBot.client.stop()
        super.onCleared()
    }
}

private data class PendingRemoteMutation(
    val chatId: String,
    val requestId: String,
    val idempotencyKey: String,
    val remote: String,
    val branch: String,
    val approvalId: String? = null,
)

private data class PendingPullRequest(
    val chatId: String,
    val requestId: String,
    val idempotencyKey: String,
    val remote: String,
    val head: String,
    val base: String,
    val title: String,
    val approvalId: String? = null,
)
