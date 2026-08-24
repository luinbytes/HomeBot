package dev.homebot.android

import android.content.Intent
import android.os.Bundle
import android.os.Build
import android.Manifest
import android.net.Uri
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.homebot.android.connection.ConnectionState
import dev.homebot.protocol.*
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive

class MainActivity : ComponentActivity() {
    private val viewModel by viewModels<MainViewModel>()
    private val incomingPairing = mutableStateOf<String?>(null)
    private val incomingNavigation = mutableStateOf<Uri?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        acceptIntent(intent)
        setContent { HomeBotTheme { HomeBotRoot(viewModel, incomingPairing.value, incomingNavigation.value) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        acceptIntent(intent)
    }

    private fun acceptIntent(intent: Intent?) {
        incomingPairing.value = intent?.data?.takeIf { it.scheme == "homebot" && it.host == "pair" }?.toString()
        incomingNavigation.value = intent?.data?.takeIf {
            it.scheme == "homebot" && it.host in setOf("chat", "routine", "settings")
        }
    }
}

@Composable
private fun HomeBotRoot(viewModel: MainViewModel, incomingPairing: String?, incomingNavigation: Uri?) {
    val connection by viewModel.connection.collectAsState()
    val product by viewModel.product.collectAsState()
    val live = connection as? ConnectionState.Live
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    LaunchedEffect(incomingNavigation, live != null) {
        if (live != null && incomingNavigation != null) viewModel.handleDeepLink(incomingNavigation)
    }
    LaunchedEffect(live != null) {
        if (live != null && Build.VERSION.SDK_INT >= 33) permission.launch(Manifest.permission.POST_NOTIFICATIONS)
    }
    if (live == null) PairingScreen(viewModel, connection, incomingPairing)
    else ProductShell(viewModel, live, product)
}

@Composable
private fun ProductShell(viewModel: MainViewModel, live: ConnectionState.Live, state: AndroidProductState) {
    Scaffold(
        containerColor = Canvas,
        topBar = {
            Row(Modifier.fillMaxWidth().background(Color.White).padding(18.dp, 14.dp), verticalAlignment = Alignment.CenterVertically) {
                HomeBotMark()
                Text("HomeBot", fontWeight = FontWeight.Bold, fontSize = 20.sp, modifier = Modifier.padding(start = 10.dp))
                Spacer(Modifier.weight(1f))
                Text("Connected", color = Success, fontSize = 12.sp)
            }
        },
        bottomBar = {
            Row(Modifier.fillMaxWidth().background(Color.White).padding(12.dp, 8.dp), horizontalArrangement = Arrangement.SpaceAround) {
                NavButton("Bots", state.destination is ProductDestination.Bots, viewModel::showBots)
                NavButton("Chats", state.destination is ProductDestination.DirectChat || state.destination is ProductDestination.GroupChat) {
                    live.snapshot.chats.firstOrNull()?.let { viewModel.openDirectChat(it.id) } ?: viewModel.showBots()
                }
                NavButton("Search", state.destination is ProductDestination.Search, viewModel::showSearch)
                NavButton("Settings", state.destination is ProductDestination.Settings, viewModel::showSettings)
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (state.destination) {
                ProductDestination.Bots -> RosterScreen(viewModel, live)
                is ProductDestination.DirectChat -> DirectChatScreen(viewModel, state.directTimeline, state)
                is ProductDestination.GroupChat -> GroupChatScreen(viewModel, state.groupTimeline, live.snapshot.bots)
                ProductDestination.Search -> SearchScreen(viewModel, state)
                ProductDestination.Settings -> ConnectedSettings(viewModel, live, state)
            }
            if (state.loading) CircularProgressIndicator(Modifier.align(Alignment.Center), color = Violet)
            state.error?.let { ErrorBanner(it, viewModel::clearError) }
        }
    }
}

@Composable
private fun SearchScreen(viewModel: MainViewModel, state: AndroidProductState) {
    var query by rememberSaveable { mutableStateOf(state.searchQuery) }
    Column(Modifier.fillMaxSize().padding(18.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Search HomeBot", fontSize = 28.sp, fontWeight = FontWeight.Bold, modifier = Modifier.semantics { heading() })
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(query, { query = it }, label = { Text("Messages, files, links and routines") }, modifier = Modifier.weight(1f))
            Button(onClick = { viewModel.search(query) }, enabled = query.isNotBlank()) { Text("Search") }
        }
        if (state.searchQuery.isNotBlank() && state.searchResults.isEmpty() && !state.loading) {
            Text("No results for “${state.searchQuery}”.", color = Muted)
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(state.searchResults, key = { "${it.kind}:${it.deep_link}" }) { result ->
                Card(Modifier.fillMaxWidth().clickable { viewModel.openSearchResult(result) }, shape = CardShape) {
                    Column(Modifier.padding(14.dp)) {
                        Text(result.title, fontWeight = FontWeight.SemiBold)
                        Text(result.kind.name.lowercase(), color = Violet, fontSize = 12.sp)
                        if (result.snippet.isNotBlank()) Text(result.snippet, color = Muted, maxLines = 3)
                    }
                }
            }
        }
    }
}

@Composable
private fun RosterScreen(viewModel: MainViewModel, live: ConnectionState.Live) {
    var create by remember { mutableStateOf(false) }
    var archived by remember { mutableStateOf(false) }
    var showHidden by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item {
            Row(Modifier.fillMaxWidth().padding(top = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Your Bots", fontSize = 28.sp, fontWeight = FontWeight.Bold, modifier = Modifier.semantics { heading() })
                    Text("Persistent teammates on your HomeBot server.", color = Muted)
                }
                Button(onClick = { create = !create }, colors = ButtonDefaults.buttonColors(containerColor = Violet)) {
                    Text(if (create) "Cancel" else "New Bot")
                }
            }
        }
        if (create) item {
            Card(shape = CardShape) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(name, { name = it }, label = { Text("Name") }, modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(title, { title = it }, label = { Text("Role") }, modifier = Modifier.fillMaxWidth())
                    Button(
                        onClick = { viewModel.createBot(name, title); create = false },
                        enabled = name.isNotBlank() && title.isNotBlank(),
                    ) { Text("Create") }
                }
            }
        }
        items(
            live.snapshot.bots.filter { it.archived == archived && (showHidden || !it.hidden) },
            key = { it.id },
        ) { bot ->
            BotRow(
                bot,
                { viewModel.openBot(bot.id) },
                { viewModel.setBotArchived(bot.id, !bot.archived) },
                { name, role -> viewModel.updateBot(bot, name, role) },
                { viewModel.setBotPinned(bot.id, !bot.pinned) },
                { viewModel.setBotHidden(bot.id, !bot.hidden) },
                { viewModel.duplicateBot(bot.id) },
                { confirmation -> viewModel.deleteBot(bot, confirmation) },
            )
        }
        item {
            TextButton(onClick = { archived = !archived }) { Text(if (archived) "Show active Bots" else "Show archived Bots") }
            TextButton(onClick = { showHidden = !showHidden }) { Text(if (showHidden) "Hide hidden Bots" else "Review hidden Bots") }
            HorizontalDivider()
            Text("Group chats", fontSize = 20.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(vertical = 12.dp))
        }
        items(live.snapshot.group_chats, key = { it.id }) { group ->
            HomeBotCard(group.title, "${group.coordination_turns_used}/${group.coordination_max_turns} coordination turns") {
                viewModel.openGroupChat(group.id)
            }
        }
        if (live.snapshot.group_chats.isEmpty() && live.snapshot.bots.count { !it.archived } >= 2) item {
            OutlinedButton(onClick = {
                viewModel.createGroup("Bot team", live.snapshot.bots.filterNot { it.archived }.take(3).map { it.id })
            }) { Text("Start a group with your first Bots") }
        }
        item { Spacer(Modifier.height(20.dp)) }
    }
}

@Composable
private fun BotRow(
    bot: BotSummary,
    onOpen: () -> Unit,
    onArchive: () -> Unit,
    onUpdate: (String, String) -> Unit,
    onPin: () -> Unit,
    onHide: () -> Unit,
    onDuplicate: () -> Unit,
    onDelete: (String) -> Unit,
) {
    var editing by remember(bot.id) { mutableStateOf(false) }
    var menuOpen by remember(bot.id) { mutableStateOf(false) }
    var confirmingDelete by remember(bot.id) { mutableStateOf(false) }
    var deleteConfirmation by remember(bot.id) { mutableStateOf("") }
    var name by remember(bot.id) { mutableStateOf(bot.name) }
    var role by remember(bot.id) { mutableStateOf(bot.title) }
    Card(shape = CardShape) {
        Column(Modifier.fillMaxWidth().padding(15.dp)) {
            Row(Modifier.fillMaxWidth().clickable(onClick = onOpen), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.background(Violet.copy(alpha = .14f), CircleShape).padding(12.dp)) {
                    Text(bot.name.take(1).uppercase(), color = Violet, fontWeight = FontWeight.Black)
                }
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(bot.name + if (bot.unread_count > 0) "  ${bot.unread_count}" else "", fontWeight = FontWeight.Bold)
                    Text(bot.title, color = Muted)
                    Text(bot.provider, color = Muted, fontSize = 12.sp)
                }
                Box {
                    TextButton(onClick = { menuOpen = true }) { Text("More") }
                    DropdownMenu(expanded = menuOpen, onDismissRequest = { menuOpen = false }) {
                        DropdownMenuItem(
                            text = { Text("Edit") },
                            onClick = { editing = true; menuOpen = false },
                        )
                        DropdownMenuItem(
                            text = { Text(if (bot.pinned) "Unpin" else "Pin") },
                            onClick = { onPin(); menuOpen = false },
                        )
                        DropdownMenuItem(
                            text = { Text(if (bot.hidden) "Unhide" else "Hide") },
                            onClick = { onHide(); menuOpen = false },
                        )
                        DropdownMenuItem(
                            text = { Text("Duplicate") },
                            onClick = { onDuplicate(); menuOpen = false },
                        )
                        DropdownMenuItem(
                            text = { Text(if (bot.archived) "Restore" else "Archive") },
                            onClick = { onArchive(); menuOpen = false },
                        )
                        DropdownMenuItem(
                            text = { Text("Delete permanently", color = Danger) },
                            onClick = { confirmingDelete = true; menuOpen = false },
                        )
                    }
                }
            }
            if (editing) {
                OutlinedTextField(name, { name = it }, label = { Text("Name") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(role, { role = it }, label = { Text("Role") }, modifier = Modifier.fillMaxWidth())
                Button(onClick = { onUpdate(name, role); editing = false }, enabled = name.isNotBlank() && role.isNotBlank()) {
                    Text("Save changes")
                }
            }
            if (confirmingDelete) {
                Text("Type ${bot.name} to delete this Bot permanently.", color = Danger)
                OutlinedTextField(
                    deleteConfirmation,
                    { deleteConfirmation = it },
                    label = { Text("Bot name") },
                    modifier = Modifier.fillMaxWidth(),
                )
                Row {
                    TextButton(onClick = { confirmingDelete = false; deleteConfirmation = "" }) { Text("Cancel") }
                    Button(
                        onClick = { onDelete(deleteConfirmation); confirmingDelete = false; deleteConfirmation = "" },
                        enabled = deleteConfirmation == bot.name,
                        colors = ButtonDefaults.buttonColors(containerColor = Danger),
                    ) { Text("Delete permanently") }
                }
            }
        }
    }
}

@Composable
private fun DirectChatScreen(viewModel: MainViewModel, timeline: ChatTimelineResponse?, state: AndroidProductState) {
    if (timeline == null) return EmptyLoading("Loading chat…")
    ChatLayout(
        title = timeline.chat.title,
        running = timeline.chat.running,
        messages = timeline.messages,
        activities = timeline.activities,
        approvals = timeline.approvals,
        queue = timeline.queued_prompts.map { "${it.kind.name.lowercase()}: ${it.content}" },
        highlightedActivityId = state.highlightedActivityId,
        onSend = { text, steer, reply -> viewModel.send(text, steer, replyToMessageId = reply) },
        onStop = viewModel::stopWorking,
        onRetry = viewModel::retry,
        onDecision = viewModel::decide,
        onAttachment = viewModel::sendAttachment,
        onReaction = viewModel::setReaction,
        extras = {
            OutlinedButton(onClick = viewModel::loadCodingWorkspace) { Text("Workspace & diff") }
            if (timeline.checkpoints.size >= 2) {
                TextButton(onClick = viewModel::compareRecentCheckpoints) { Text("Compare latest checkpoints") }
            }
            if (timeline.checkpoints.isNotEmpty()) {
                TextButton(onClick = viewModel::restoreLatestCheckpoint) { Text("Restore latest checkpoint safely") }
            }
            state.coding.status?.let { Text("${it.branch ?: "detached"} • ${it.entries.size} changed files", color = Muted, fontSize = 12.sp) }
            state.coding.diff?.let { Text(it.patch.take(1_200), fontSize = 11.sp) }
        },
    )
}

@Composable
private fun GroupChatScreen(viewModel: MainViewModel, timeline: GroupTimelineResponse?, bots: List<BotSummary>) {
    if (timeline == null) return EmptyLoading("Loading group…")
    var mentionAll by remember { mutableStateOf(false) }
    var editingGroup by remember { mutableStateOf(false) }
    var groupTitle by remember(timeline.group.id) { mutableStateOf(timeline.group.title) }
    val mentions = if (mentionAll) timeline.participants.map { it.bot_id } else emptyList()
    ChatLayout(
        title = timeline.group.title,
        running = !timeline.group.stop_requested,
        messages = timeline.messages,
        activities = emptyList(), approvals = emptyList(), queue = emptyList(),
        highlightedActivityId = null,
        onSend = { text, _, reply -> viewModel.send(text, mentions = mentions, replyToMessageId = reply) },
        onStop = viewModel::stopWorking, onRetry = {}, onDecision = { _, _ -> },
        onAttachment = {},
        onReaction = viewModel::setReaction,
        extras = {
            TextButton(onClick = { editingGroup = !editingGroup }) { Text(if (editingGroup) "Close group editor" else "Edit group") }
            if (editingGroup) {
                OutlinedTextField(groupTitle, { groupTitle = it }, label = { Text("Group name") }, modifier = Modifier.fillMaxWidth())
                Button(onClick = { viewModel.renameGroup(groupTitle); editingGroup = false }, enabled = groupTitle.isNotBlank()) { Text("Save group name") }
                Text("Members (${timeline.participants.size}/6)", fontWeight = FontWeight.SemiBold)
                timeline.participants.forEach { participant ->
                    val bot = bots.firstOrNull { it.id == participant.bot_id }
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Text(bot?.name ?: participant.bot_id.take(8), modifier = Modifier.weight(1f))
                        if (participant.bot_id != timeline.group.ownership_bot_id) {
                            TextButton(onClick = { viewModel.removeGroupParticipant(participant.bot_id) }, enabled = timeline.participants.size > 2) { Text("Remove") }
                        }
                    }
                }
                bots.filter { bot -> !bot.archived && timeline.participants.none { it.bot_id == bot.id } }.forEach { bot ->
                    TextButton(onClick = { viewModel.addGroupParticipant(bot.id) }, enabled = timeline.participants.size < 6) { Text("Add ${bot.name}") }
                }
            }
            TextButton(onClick = { mentionAll = !mentionAll }) { Text(if (mentionAll) "@All Bots selected" else "Mention all Bots") }
            val owner = timeline.group.ownership_bot_id
            timeline.participants.firstOrNull { it.bot_id != owner }?.let { next ->
                OutlinedButton(onClick = { viewModel.handoff(owner, next.bot_id) }) { Text("Hand off ownership") }
            }
        },
    )
}

@Composable
private fun ChatLayout(
    title: String,
    running: Boolean,
    messages: List<MessageSummary>,
    activities: List<ActivitySummary>,
    approvals: List<ApprovalSummary>,
    queue: List<String>,
    highlightedActivityId: String?,
    onSend: (String, Boolean, String?) -> Unit,
    onStop: () -> Unit,
    onRetry: (MessageSummary) -> Unit,
    onDecision: (ApprovalSummary, Boolean) -> Unit,
    onAttachment: (android.net.Uri) -> Unit,
    onReaction: (String, String, Boolean) -> Unit,
    extras: @Composable () -> Unit,
) {
    var composer by remember { mutableStateOf("") }
    var steering by remember { mutableStateOf(false) }
    var replyToMessageId by remember { mutableStateOf<String?>(null) }
    val attachmentPicker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) onAttachment(uri)
    }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(title, fontSize = 22.sp, fontWeight = FontWeight.Bold)
                Text(if (running) "Bot is working" else "Ready", color = if (running) Violet else Success, fontSize = 12.sp)
            }
            if (running) OutlinedButton(onClick = onStop) { Text("Stop") }
        }
        LazyColumn(Modifier.weight(1f).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            items(messages, key = { it.id }) {
                MessageCard(it, onRetry, onReaction, onReply = { replyToMessageId = it })
            }
            items(activities, key = { it.id }) {
                HomeBotCard(it.title, "${it.detail}\n${it.status}", if (it.id == highlightedActivityId) Violet else Color.Unspecified)
            }
            items(approvals.filter { it.status == "pending" }, key = { it.id }) { ApprovalCard(it, onDecision) }
            items(queue) { HomeBotCard("Queued", it) }
            item { extras(); Spacer(Modifier.height(8.dp)) }
        }
        Column(Modifier.background(Color.White).padding(12.dp)) {
            replyToMessageId?.let { reply ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Replying to ${reply.take(8)}", modifier = Modifier.weight(1f), color = Muted)
                    TextButton(onClick = { replyToMessageId = null }) { Text("Cancel") }
                }
            }
            OutlinedTextField(
                composer, { composer = it }, modifier = Modifier.fillMaxWidth(), minLines = 2,
                placeholder = { Text(if (steering) "Steer this Bot…" else "Message HomeBot…") },
            )
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                TextButton(onClick = { steering = !steering }, enabled = running) {
                    Text(if (steering) "Steering" else if (running) "Queue follow-up" else "Message")
                }
                TextButton(onClick = { attachmentPicker.launch("*/*") }) { Text("Attach") }
                Spacer(Modifier.weight(1f))
                Button(
                    onClick = { onSend(composer.trim(), steering, replyToMessageId); composer = ""; replyToMessageId = null }, enabled = composer.isNotBlank(),
                    colors = ButtonDefaults.buttonColors(containerColor = Violet),
                ) { Text("Send") }
            }
        }
    }
}

@Composable
private fun MessageCard(
    message: MessageSummary,
    onRetry: (MessageSummary) -> Unit,
    onReaction: (String, String, Boolean) -> Unit,
    onReply: (String) -> Unit,
) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (message.author == "user") Arrangement.End else Arrangement.Start) {
        Card(shape = RoundedCornerShape(18.dp), modifier = Modifier.fillMaxWidth(if (message.author == "user") .84f else .96f)) {
            Column(Modifier.padding(14.dp)) {
                Text(if (message.author == "user") "You" else "Bot", color = Violet, fontSize = 12.sp, fontWeight = FontWeight.Bold)
                message.parts.forEach { part ->
                    when (part) {
                        is MessagePart.Text -> Text(part.text, modifier = Modifier.padding(top = 5.dp))
                        is MessagePart.Notice -> Text(part.text, color = Muted, modifier = Modifier.padding(top = 5.dp))
                        is MessagePart.AttachmentPart -> Text("Attachment • ${part.attachment.filename}", color = Violet)
                    }
                }
                if (message.status == "failed") TextButton(onClick = { onRetry(message) }) { Text("Retry") }
                message.error?.let { Text(it.message, color = Danger, fontSize = 12.sp) }
                message.reply_to_message_id?.let { Text("Reply to ${it.take(8)}", color = Muted, fontSize = 11.sp) }
                if (message.references.isNotEmpty()) {
                    Text(message.references.joinToString("  ") { "@${it.label}" }, color = Violet, fontSize = 12.sp)
                }
                if (message.applied_skills.isNotEmpty()) {
                    Text(message.applied_skills.joinToString("  ") { "/${it.name} v${it.version}" }, color = Violet, fontSize = 12.sp)
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    message.reactions.forEach { reaction ->
                        TextButton(onClick = { onReaction(message.id, reaction.emoji, !reaction.reacted_by_user) }) {
                            Text("${reaction.emoji} ${reaction.count}")
                        }
                    }
                    if (message.reactions.none { it.emoji == "👍" }) {
                        TextButton(onClick = { onReaction(message.id, "👍", true) }) { Text("React") }
                    }
                    TextButton(onClick = { onReply(message.id) }) { Text("Reply") }
                }
            }
        }
    }
}

@Composable
private fun ApprovalCard(approval: ApprovalSummary, onDecision: (ApprovalSummary, Boolean) -> Unit) {
    Card(shape = CardShape) {
        Column(Modifier.padding(14.dp)) {
            Text("Approval required", color = Warning, fontWeight = FontWeight.Bold)
            Text(approval.title, fontWeight = FontWeight.SemiBold)
            Text(approval.detail, color = Muted)
            Row {
                OutlinedButton(onClick = { onDecision(approval, false) }) { Text("Deny") }
                Button(onClick = { onDecision(approval, true) }, modifier = Modifier.padding(start = 8.dp)) { Text("Allow") }
            }
        }
    }
}

@Composable
private fun ConnectedSettings(viewModel: MainViewModel, live: ConnectionState.Live, state: AndroidProductState) {
    val endpointSettings by viewModel.settings.collectAsState()
    var endpoint by remember(endpointSettings.endpoint) { mutableStateOf(endpointSettings.endpoint.ifBlank { live.endpoint }) }
    var endpointError by remember { mutableStateOf<String?>(null) }
    var routineComposerOpen by rememberSaveable { mutableStateOf(false) }
    var routineName by rememberSaveable { mutableStateOf("") }
    var routineDescription by rememberSaveable { mutableStateOf("") }
    var routinePrompt by rememberSaveable { mutableStateOf("") }
    var routineBotId by rememberSaveable { mutableStateOf("") }
    var routineRequiresApproval by rememberSaveable { mutableStateOf(true) }
    var recordingName by rememberSaveable { mutableStateOf("") }
    var recordingDescription by rememberSaveable { mutableStateOf("") }
    var recordingPrompt by rememberSaveable { mutableStateOf("") }
    var recordingBotId by rememberSaveable { mutableStateOf("") }
    var recordingRequiresApproval by rememberSaveable { mutableStateOf(true) }
    var deleteRoutineId by rememberSaveable { mutableStateOf<String?>(null) }
    var configuringPackId by rememberSaveable { mutableStateOf<String?>(null) }
    var packBotId by rememberSaveable { mutableStateOf("") }
    var packTimezone by rememberSaveable { mutableStateOf("UTC") }
    var packHour by rememberSaveable { mutableStateOf("8") }
    var packMinute by rememberSaveable { mutableStateOf("0") }
    val selectedRoutine = state.routines.firstOrNull { it.id == state.selectedRoutineId }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item {
            Text("Settings & automations", fontSize = 28.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(top = 18.dp))
            Text("All status and mutations come from your HomeBot server.", color = Muted)
        }
        item {
            Text("Connection", fontSize = 20.sp, fontWeight = FontWeight.Bold)
            OutlinedTextField(endpoint, { endpoint = it }, Modifier.fillMaxWidth(), label = { Text("HTTPS or loopback endpoint") })
            Button(onClick = { viewModel.updateEndpoint(endpoint, endpointSettings.deviceName) { endpointError = it } }) {
                Text("Save and reconnect")
            }
            endpointError?.let { Text(it, color = Danger) }
            Text("Sequence ${live.cursor} • authenticated device session", color = Muted, fontSize = 12.sp)
        }
        item {
            SectionTitle("Computer access policy")
            Text(
                "Rules are enforced by the HomeBot server. Paired devices can monitor them; only the owner desktop can change them.",
                color = Muted,
            )
        }
        items(live.snapshot.capability_rules, key = { it.id }) { rule ->
            HomeBotCard(
                rule.capability.name.lowercase().replace('_', ' '),
                "${rule.effect.name.lowercase().replace('_', ' ')}${rule.action_prefix?.let { " • $it" } ?: ""}",
            )
        }
        item {
            SectionTitle("Shared browser")
            Text("Login state stays on your HomeBot server. Watch, take over, and return control without exposing cookies or credentials.", color = Muted)
        }
        items(live.snapshot.browser_sessions, key = { it.id }) { session ->
            Card(shape = CardShape) {
                Column(Modifier.fillMaxWidth().padding(14.dp)) {
                    Text(session.profile_name, fontWeight = FontWeight.Bold)
                    Text("${session.status.name.lowercase().replace('_', ' ')} • ${session.controller.name.lowercase()} control", color = Muted)
                    session.current_url?.let { Text(it, maxLines = 1) }
                    Row {
                        TextButton(onClick = { viewModel.watchBrowser(session.id) }) { Text("Watch") }
                        if (session.controller.name == "BOT") {
                            TextButton(onClick = { viewModel.takeOverBrowser(session.id, session.pending_approval_id) }) { Text(if (session.pending_approval_id == null) "Take over" else "Resume takeover") }
                        } else {
                            TextButton(onClick = { viewModel.returnBrowserToBot(session.id) }) { Text("Return to Bot") }
                        }
                    }
                }
            }
        }
        item {
            SectionTitle("Assistant Packs")
            Text("Install a useful Skill and scheduled routine onto one Bot.", color = Muted)
            state.assistantPackNotice?.let { Text(it, color = Violet) }
        }
        items(state.assistantPacks, key = { "assistant-pack-${it.id}" }) { pack ->
            Card(shape = CardShape) {
                Column(Modifier.fillMaxWidth().padding(14.dp)) {
                    Text(pack.name, fontWeight = FontWeight.Bold)
                    Text(pack.description, color = Muted)
                    Text(
                        "${pack.schedule.cadence.name.lowercase().replaceFirstChar { it.uppercase() }} • default ${pack.schedule.default_hour.toString().padStart(2, '0')}:${pack.schedule.default_minute.toString().padStart(2, '0')}",
                        color = Muted,
                        fontSize = 12.sp,
                    )
                    TextButton(
                        onClick = {
                            configuringPackId = if (configuringPackId == pack.id) null else pack.id
                            packBotId = live.snapshot.bots.firstOrNull { !it.archived }?.id.orEmpty()
                            packHour = pack.schedule.default_hour.toString()
                            packMinute = pack.schedule.default_minute.toString()
                        },
                    ) { Text(if (configuringPackId == pack.id) "Cancel" else "Configure") }
                    if (configuringPackId == pack.id) {
                        Text("Run with", fontWeight = FontWeight.SemiBold)
                        live.snapshot.bots.filterNot { it.archived }.forEach { bot ->
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                RadioButton(selected = packBotId == bot.id, onClick = { packBotId = bot.id })
                                Text(bot.name)
                            }
                        }
                        OutlinedTextField(
                            packTimezone,
                            { packTimezone = it },
                            label = { Text("Timezone, for example Europe/London") },
                            modifier = Modifier.fillMaxWidth(),
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedTextField(
                                packHour,
                                { packHour = it.filter(Char::isDigit).take(2) },
                                label = { Text("Hour") },
                                modifier = Modifier.weight(1f),
                            )
                            OutlinedTextField(
                                packMinute,
                                { packMinute = it.filter(Char::isDigit).take(2) },
                                label = { Text("Minute") },
                                modifier = Modifier.weight(1f),
                            )
                        }
                        val hour = packHour.toIntOrNull()
                        val minute = packMinute.toIntOrNull()
                        Button(
                            onClick = {
                                viewModel.installAssistantPack(
                                    pack.id,
                                    packBotId,
                                    packTimezone,
                                    hour ?: 0,
                                    minute ?: 0,
                                )
                                configuringPackId = null
                            },
                            enabled = packBotId.isNotBlank() && packTimezone.isNotBlank() && hour != null && hour in 0..23 && minute != null && minute in 0..59,
                        ) { Text("Install and enable") }
                    }
                }
            }
        }
        item {
            SectionTitle("Routines")
            Text("Create, test, record and schedule server-owned Bot workflows.", color = Muted)
            TextButton(onClick = { routineComposerOpen = !routineComposerOpen }) {
                Text(if (routineComposerOpen) "Close routine creator" else "Create routine")
            }
            if (routineComposerOpen) {
                OutlinedTextField(routineName, { routineName = it }, label = { Text("Routine name") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(routineDescription, { routineDescription = it }, label = { Text("Description") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(routinePrompt, { routinePrompt = it }, label = { Text("Bot instruction") }, minLines = 3, modifier = Modifier.fillMaxWidth())
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(routineRequiresApproval, { routineRequiresApproval = it })
                    Text("Ask before running this instruction")
                }
                Text("Run with", fontWeight = FontWeight.SemiBold)
                live.snapshot.bots.filterNot { it.archived }.forEach { bot ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = routineBotId == bot.id, onClick = { routineBotId = bot.id })
                        Text(bot.name)
                    }
                }
                Button(
                    onClick = {
                        viewModel.createRoutine(routineBotId, routineName, routineDescription, routinePrompt, routineRequiresApproval)
                        routineComposerOpen = false
                        routineName = ""
                        routineDescription = ""
                        routinePrompt = ""
                    },
                    enabled = routineBotId.isNotBlank() && routineName.isNotBlank() && routinePrompt.isNotBlank(),
                ) { Text("Create routine") }
            }
        }
        item {
            val recording = state.activeRoutineRecording
            if (recording == null) {
                Text("Record a demonstration", fontWeight = FontWeight.SemiBold)
                OutlinedTextField(recordingName, { recordingName = it }, label = { Text("Recording name") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(recordingDescription, { recordingDescription = it }, label = { Text("Description") }, modifier = Modifier.fillMaxWidth())
                live.snapshot.bots.filterNot { it.archived }.forEach { bot ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = recordingBotId == bot.id, onClick = { recordingBotId = bot.id })
                        Text(bot.name)
                    }
                }
                OutlinedButton(
                    onClick = { viewModel.startRoutineRecording(recordingBotId, recordingName, recordingDescription) },
                    enabled = recordingBotId.isNotBlank() && recordingName.isNotBlank(),
                ) { Text("Start recording") }
            } else {
                HomeBotCard("Recording ${recording.name}", "${recording.actions.size} structured actions captured")
                OutlinedTextField(recordingPrompt, { recordingPrompt = it }, label = { Text("Next Bot instruction") }, minLines = 2, modifier = Modifier.fillMaxWidth())
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(recordingRequiresApproval, { recordingRequiresApproval = it })
                    Text("Preserve an approval boundary")
                }
                Row {
                    TextButton(
                        onClick = { viewModel.appendRoutineRecording(recordingPrompt, recordingRequiresApproval); recordingPrompt = "" },
                        enabled = recordingPrompt.isNotBlank(),
                    ) { Text("Append action") }
                    Button(onClick = viewModel::finishRoutineRecording, enabled = recording.actions.isNotEmpty()) { Text("Finish as draft") }
                }
                TextButton(onClick = viewModel::cancelRoutineRecording) { Text("Cancel recording") }
            }
        }
        items(state.routines, key = { it.id }) { routine ->
            var editing by rememberSaveable(routine.id) { mutableStateOf(false) }
            var editName by rememberSaveable(routine.id) { mutableStateOf(routine.name) }
            var editDescription by rememberSaveable(routine.id) { mutableStateOf(routine.description) }
            var editPrompt by rememberSaveable(routine.id) {
                mutableStateOf(
                    routine.definition.steps.firstOrNull()
                        ?.jsonObject?.get("prompt_template")?.jsonPrimitive?.contentOrNull.orEmpty(),
                )
            }
            var editRequiresApproval by rememberSaveable(routine.id) {
                mutableStateOf(
                    routine.definition.steps.firstOrNull {
                        it.jsonObject["kind"]?.jsonPrimitive?.contentOrNull == "bot_prompt"
                    }?.jsonObject?.get("requires_approval")?.jsonPrimitive?.booleanOrNull ?: true,
                )
            }
            val hasBotPrompt = routine.definition.steps.any {
                it.jsonObject["kind"]?.jsonPrimitive?.contentOrNull == "bot_prompt"
            }
            Card(shape = CardShape) {
                Column(Modifier.fillMaxWidth().padding(14.dp)) {
                    Text(routine.name, fontWeight = FontWeight.Bold)
                    Text("v${routine.version} • ${if (routine.enabled) "enabled" else "disabled"}", color = Muted)
                    Row {
                        TextButton(onClick = { viewModel.selectRoutine(routine.id) }) { Text("Details & history") }
                        Button(onClick = { viewModel.runRoutine(routine.id) }, enabled = !routine.draft) { Text("Run now") }
                    }
                    Row {
                        TextButton(onClick = { viewModel.dryRunRoutine(routine.id) }) { Text("Dry run") }
                        TextButton(onClick = { viewModel.toggleRoutine(routine.id, !routine.enabled) }) {
                            Text(if (routine.enabled) "Disable" else "Enable")
                        }
                    }
                    Row {
                        TextButton(onClick = { viewModel.scheduleRoutine(routine.id) }, enabled = !routine.draft) {
                            Text("Schedule in 5 minutes")
                        }
                        TextButton(onClick = { editing = !editing }, enabled = hasBotPrompt) { Text(if (editing) "Close editor" else "Edit") }
                    }
                    Row {
                        TextButton(onClick = { viewModel.duplicateRoutine(routine) }) { Text("Duplicate") }
                        TextButton(
                            onClick = {
                                if (deleteRoutineId == routine.id) {
                                    viewModel.deleteRoutine(routine.id)
                                    deleteRoutineId = null
                                } else deleteRoutineId = routine.id
                            },
                        ) { Text(if (deleteRoutineId == routine.id) "Confirm delete" else "Delete") }
                    }
                    if (editing) {
                        OutlinedTextField(editName, { editName = it }, label = { Text("Routine name") }, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(editDescription, { editDescription = it }, label = { Text("Description") }, modifier = Modifier.fillMaxWidth())
                        OutlinedTextField(editPrompt, { editPrompt = it }, label = { Text("Bot instruction") }, minLines = 3, modifier = Modifier.fillMaxWidth())
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(editRequiresApproval, { editRequiresApproval = it })
                            Text("Ask before running this instruction")
                        }
                        Button(
                            onClick = { viewModel.updateRoutine(routine, editName, editDescription, editPrompt, editRequiresApproval); editing = false },
                            enabled = editName.isNotBlank() && editPrompt.isNotBlank(),
                        ) { Text("Save routine") }
                    }
                }
            }
        }
        selectedRoutine?.let { routine ->
            item {
                Text("${routine.name} run history", fontWeight = FontWeight.SemiBold)
                Text("${state.routineTriggers.size} schedules/triggers", color = Muted)
            }
            items(state.routineRuns, key = { it.id }) { run ->
                HomeBotCard(run.status, "Attempt ${run.attempt_count}${run.error_message?.let { error -> " • $error" } ?: ""}")
            }
        }
        item { SectionTitle("Skills") }
        items(state.skills, key = { it.id }) { skill ->
            Card(shape = CardShape) {
                Row(Modifier.fillMaxWidth().padding(14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(skill.name, fontWeight = FontWeight.Bold)
                        Text("v${skill.version} • ${skill.bot_ids.size} Bots", color = Muted)
                    }
                    live.snapshot.bots.firstOrNull()?.let { bot ->
                        val assigned = bot.id in skill.bot_ids
                        TextButton(onClick = { viewModel.toggleSkill(skill.id, bot.id, !assigned) }) {
                            Text(if (assigned) "Unassign" else "Assign to ${bot.name}")
                        }
                    }
                    TextButton(onClick = { viewModel.testSkill(skill.id) }) { Text("Test safely") }
                }
            }
        }
        state.skillTestPreview?.let { preview ->
            item { HomeBotCard("Skill test preview", preview) }
        }
        item { SectionTitle("Plugins & MCP") }
        items(state.plugins, key = { it.id }) { plugin ->
            Card(shape = CardShape) {
                Column(Modifier.fillMaxWidth().padding(14.dp)) {
                    Text(plugin.name, fontWeight = FontWeight.Bold)
                    Text("${plugin.connection_state} • ${plugin.auth_state} • ${plugin.tools.size} tools", color = Muted)
                    plugin.error_message?.let { Text(it, color = Danger) }
                    Row {
                        TextButton(onClick = { viewModel.mutatePlugin(plugin.id, "health") }) { Text("Check") }
                        TextButton(onClick = { viewModel.mutatePlugin(plugin.id, if (plugin.enabled) "disable" else "enable") }) {
                            Text(if (plugin.enabled) "Disable" else "Enable")
                        }
                    }
                }
            }
        }
        item { SectionTitle("Provider status") }
        items(live.snapshot.bots, key = { "provider-${it.id}" }) { bot ->
            HomeBotCard(bot.name, "${bot.provider} • profile ${bot.advanced.provider_profile_id?.take(8) ?: "not configured"}")
        }
        item { SectionTitle("Secret references") }
        if (state.secrets.isEmpty()) item { Text("No secret references configured.", color = Muted) }
        items(state.secrets, key = { it.id }) { secret ->
            HomeBotCard(secret.label, "${secret.status} • value is never displayed")
        }
        item { SectionTitle("This paired device") }
        state.currentDevice?.let { device ->
            item {
                Card(shape = CardShape) {
                    Column(Modifier.fillMaxWidth().padding(14.dp)) {
                        Text(device.name, fontWeight = FontWeight.Bold)
                        Text("${device.endpoint_kind.name.lowercase()} • session ${device.id.take(8)}…", color = Muted)
                        OutlinedButton(onClick = viewModel::revokeThisDevice) { Text("Revoke this device") }
                    }
                }
            }
        }
        item { Spacer(Modifier.height(20.dp)) }
    }
}

@Composable private fun SectionTitle(title: String) { Text(title, fontSize = 20.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(top = 12.dp).semantics { heading() }) }

@Composable
private fun PairingScreen(viewModel: MainViewModel, connection: ConnectionState, incomingPairing: String?) {
    val storedSettings by viewModel.settings.collectAsState()
    var pairingLink by remember { mutableStateOf("") }
    var endpoint by remember { mutableStateOf("") }
    var deviceName by remember { mutableStateOf("Android") }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(incomingPairing) { if (incomingPairing != null) pairingLink = incomingPairing }
    LaunchedEffect(storedSettings) { if (endpoint.isBlank()) endpoint = storedSettings.endpoint; deviceName = storedSettings.deviceName }
    Surface(Modifier.fillMaxSize(), color = Canvas) {
        Column(Modifier.fillMaxSize().padding(24.dp, 48.dp), verticalArrangement = Arrangement.spacedBy(18.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                HomeBotMark()
                Column(Modifier.padding(start = 12.dp)) {
                    Text("HomeBot", fontSize = 25.sp, fontWeight = FontWeight.Bold)
                    Text("Your AI team. On your computer.", color = Muted)
                }
            }
            ConnectionCard(connection)
            if (connection is ConnectionState.Unpaired || connection is ConnectionState.Pairing) {
                Text("Pair this device", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                OutlinedTextField(pairingLink, { pairingLink = it }, Modifier.fillMaxWidth(), label = { Text("HomeBot pairing link") }, minLines = 2)
                OutlinedTextField(deviceName, { deviceName = it }, Modifier.fillMaxWidth(), label = { Text("Device name") })
                Button(
                    onClick = { viewModel.pair(pairingLink, deviceName) { error = it } },
                    enabled = pairingLink.isNotBlank() && connection !is ConnectionState.Pairing,
                    colors = ButtonDefaults.buttonColors(containerColor = Violet),
                ) { Text("Connect to HomeBot") }
            } else if (endpoint.isNotBlank()) {
                OutlinedTextField(endpoint, { endpoint = it }, Modifier.fillMaxWidth(), label = { Text("HomeBot endpoint") })
                Button(onClick = { viewModel.updateEndpoint(endpoint, deviceName) { error = it } }) { Text("Save and reconnect") }
            }
            error?.let { Text(it, color = Danger) }
            Text("Session credentials are encrypted with Android Keystore. Product state remains on your server.", color = Muted, fontSize = 13.sp)
        }
    }
}

@Composable
private fun ConnectionCard(connection: ConnectionState) {
    val values = when (connection) {
        ConnectionState.Unpaired -> Triple("Not paired", "Open a pairing link from HomeBot desktop.", Muted)
        ConnectionState.Pairing -> Triple("Pairing", "Exchanging the one-time credential…", Violet)
        is ConnectionState.Connecting -> Triple("Connecting", connection.endpoint, Violet)
        is ConnectionState.Hydrating -> Triple("Syncing", "Loading authoritative HomeBot state…", Violet)
        is ConnectionState.Live -> Triple("Connected", "${connection.snapshot.bots.size} Bots", Success)
        is ConnectionState.Reconnecting -> Triple("Reconnecting", "Attempt ${connection.attempt}; retaining the last safe view.", Warning)
        is ConnectionState.VersionIncompatible -> Triple("Update required", "Client and server protocols do not overlap.", Danger)
        ConnectionState.Revoked -> Triple("Device revoked", "Pair again from an owner device.", Danger)
        is ConnectionState.Offline -> Triple("Offline", connection.failure.toString(), Warning)
    }
    Box(Modifier.semantics { liveRegion = LiveRegionMode.Polite }) {
        HomeBotCard(values.first, values.second, values.third)
    }
}

@Composable
private fun HomeBotCard(title: String, detail: String, color: Color = Color.Unspecified, onClick: (() -> Unit)? = null) {
    val modifier = if (onClick == null) {
        Modifier.fillMaxWidth()
    } else {
        Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick)
    }
    Card(shape = CardShape, modifier = modifier) {
        Column(Modifier.padding(16.dp)) {
            Text(title, color = color, fontWeight = FontWeight.Bold)
            Text(detail, color = Muted, modifier = Modifier.padding(top = 4.dp))
        }
    }
}

@Composable private fun EmptyLoading(message: String) { Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(message, color = Muted) } }
@Composable private fun ErrorBanner(message: String, dismiss: () -> Unit) { Card(Modifier.fillMaxWidth().padding(12.dp).semantics { liveRegion = LiveRegionMode.Assertive }, shape = CardShape) { Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) { Text(message, color = Danger, modifier = Modifier.weight(1f)); TextButton(onClick = dismiss) { Text("Dismiss") } } } }
@Composable private fun HomeBotMark() { Box(Modifier.background(Violet, RoundedCornerShape(14.dp)).padding(13.dp, 9.dp).clearAndSetSemantics { contentDescription = "HomeBot" }) { Text("H", color = Color.White, fontWeight = FontWeight.Black, fontSize = 20.sp) } }
@Composable private fun NavButton(label: String, selected: Boolean, onClick: () -> Unit) { TextButton(onClick = onClick, modifier = Modifier.semantics { role = Role.Tab; this.selected = selected }) { Text(label, color = if (selected) Violet else Muted, fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal) } }
@Composable private fun HomeBotTheme(content: @Composable () -> Unit) { MaterialTheme(content = content) }

private val Canvas = Color(0xFFF7F6F9)
private val Violet = Color(0xFF7657FF)
private val Muted = Color(0xFF6E6978)
private val Success = Color(0xFF198754)
private val Warning = Color(0xFFE07A00)
private val Danger = Color(0xFFB3261E)
private val CardShape = RoundedCornerShape(18.dp)
