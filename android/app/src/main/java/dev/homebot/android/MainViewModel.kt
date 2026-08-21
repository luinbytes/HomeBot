package dev.homebot.android

import android.app.Application
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

    fun showSettings() {
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.Settings, error = null)
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

    fun createGroup(title: String, botIds: List<String>) = perform {
        require(botIds.size >= 2) { "Choose at least two Bots" }
        val response = homeBot.client.createGroup(
            CreateGroupChatRequest(id(), id(), title.trim(), botIds, botIds.first(), 12, 3),
        ).getOrThrow()
        mutableProduct.value = mutableProduct.value.copy(destination = ProductDestination.GroupChat(response.group.id))
        refreshSelection()
    }

    fun send(content: String, steering: Boolean = false, mentions: List<String> = emptyList()) = perform {
        when (val destination = mutableProduct.value.destination) {
            is ProductDestination.DirectChat -> homeBot.client.sendMessage(destination.chatId, content, steering).getOrThrow()
            is ProductDestination.GroupChat -> homeBot.client.sendGroupMessage(destination.chatId, content, mentions).getOrThrow()
            else -> error("Open a chat before sending a message")
        }
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
