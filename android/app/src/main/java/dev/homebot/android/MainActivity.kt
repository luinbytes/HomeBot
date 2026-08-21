package dev.homebot.android

import android.content.Intent
import android.os.Bundle
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.homebot.android.connection.ConnectionState
import dev.homebot.protocol.*

class MainActivity : ComponentActivity() {
    private val viewModel by viewModels<MainViewModel>()
    private val incomingPairing = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        acceptIntent(intent)
        setContent { HomeBotTheme { HomeBotRoot(viewModel, incomingPairing.value) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        acceptIntent(intent)
    }

    private fun acceptIntent(intent: Intent?) {
        incomingPairing.value = intent?.data?.takeIf { it.scheme == "homebot" && it.host == "pair" }?.toString()
    }
}

@Composable
private fun HomeBotRoot(viewModel: MainViewModel, incomingPairing: String?) {
    val connection by viewModel.connection.collectAsState()
    val product by viewModel.product.collectAsState()
    val live = connection as? ConnectionState.Live
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
                NavButton("Settings", state.destination is ProductDestination.Settings, viewModel::showSettings)
            }
        },
    ) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            when (state.destination) {
                ProductDestination.Bots -> RosterScreen(viewModel, live)
                is ProductDestination.DirectChat -> DirectChatScreen(viewModel, state.directTimeline, state)
                is ProductDestination.GroupChat -> GroupChatScreen(viewModel, state.groupTimeline)
                ProductDestination.Settings -> ConnectedSettings(viewModel, live, state)
            }
            if (state.loading) CircularProgressIndicator(Modifier.align(Alignment.Center), color = Violet)
            state.error?.let { ErrorBanner(it, viewModel::clearError) }
        }
    }
}

@Composable
private fun RosterScreen(viewModel: MainViewModel, live: ConnectionState.Live) {
    var create by remember { mutableStateOf(false) }
    var archived by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var title by remember { mutableStateOf("") }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = 18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item {
            Row(Modifier.fillMaxWidth().padding(top = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Your Bots", fontSize = 28.sp, fontWeight = FontWeight.Bold)
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
        items(live.snapshot.bots.filter { it.archived == archived }, key = { it.id }) { bot ->
            BotRow(
                bot,
                { viewModel.openBot(bot.id) },
                { viewModel.setBotArchived(bot.id, !bot.archived) },
                { name, role -> viewModel.updateBot(bot, name, role) },
            )
        }
        item {
            TextButton(onClick = { archived = !archived }) { Text(if (archived) "Show active Bots" else "Show archived Bots") }
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
private fun BotRow(bot: BotSummary, onOpen: () -> Unit, onArchive: () -> Unit, onUpdate: (String, String) -> Unit) {
    var editing by remember(bot.id) { mutableStateOf(false) }
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
                TextButton(onClick = { editing = !editing }) { Text("Edit") }
                TextButton(onClick = onArchive) { Text(if (bot.archived) "Restore" else "Archive") }
            }
            if (editing) {
                OutlinedTextField(name, { name = it }, label = { Text("Name") }, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(role, { role = it }, label = { Text("Role") }, modifier = Modifier.fillMaxWidth())
                Button(onClick = { onUpdate(name, role); editing = false }, enabled = name.isNotBlank() && role.isNotBlank()) {
                    Text("Save changes")
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
        onSend = { text, steer -> viewModel.send(text, steer) },
        onStop = viewModel::stopWorking,
        onRetry = viewModel::retry,
        onDecision = viewModel::decide,
        onAttachment = viewModel::sendAttachment,
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
private fun GroupChatScreen(viewModel: MainViewModel, timeline: GroupTimelineResponse?) {
    if (timeline == null) return EmptyLoading("Loading group…")
    var mentionAll by remember { mutableStateOf(false) }
    val mentions = if (mentionAll) timeline.participants.map { it.bot_id } else emptyList()
    ChatLayout(
        title = timeline.group.title,
        running = !timeline.group.stop_requested,
        messages = timeline.messages,
        activities = emptyList(), approvals = emptyList(), queue = emptyList(),
        onSend = { text, _ -> viewModel.send(text, mentions = mentions) },
        onStop = viewModel::stopWorking, onRetry = {}, onDecision = { _, _ -> },
        onAttachment = {},
        extras = {
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
    onSend: (String, Boolean) -> Unit,
    onStop: () -> Unit,
    onRetry: (MessageSummary) -> Unit,
    onDecision: (ApprovalSummary, Boolean) -> Unit,
    onAttachment: (android.net.Uri) -> Unit,
    extras: @Composable () -> Unit,
) {
    var composer by remember { mutableStateOf("") }
    var steering by remember { mutableStateOf(false) }
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
            items(messages, key = { it.id }) { MessageCard(it, onRetry) }
            items(activities, key = { it.id }) { HomeBotCard(it.title, "${it.detail}\n${it.status}") {} }
            items(approvals.filter { it.status == "pending" }, key = { it.id }) { ApprovalCard(it, onDecision) }
            items(queue) { HomeBotCard("Queued", it) {} }
            item { extras(); Spacer(Modifier.height(8.dp)) }
        }
        Column(Modifier.background(Color.White).padding(12.dp)) {
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
                    onClick = { onSend(composer.trim(), steering); composer = "" }, enabled = composer.isNotBlank(),
                    colors = ButtonDefaults.buttonColors(containerColor = Violet),
                ) { Text("Send") }
            }
        }
    }
}

@Composable
private fun MessageCard(message: MessageSummary, onRetry: (MessageSummary) -> Unit) {
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
        item { SectionTitle("Routines") }
        items(state.routines, key = { it.id }) { routine ->
            Card(shape = CardShape) {
                Column(Modifier.fillMaxWidth().padding(14.dp)) {
                    Text(routine.name, fontWeight = FontWeight.Bold)
                    Text("v${routine.version} • ${if (routine.enabled) "enabled" else "disabled"}", color = Muted)
                    Row {
                        TextButton(onClick = { viewModel.selectRoutine(routine.id) }) { Text("Details & history") }
                        Button(onClick = { viewModel.runRoutine(routine.id) }, enabled = !routine.draft) { Text("Run now") }
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
                HomeBotCard(run.status, "Attempt ${run.attempt_count}${run.error_message?.let { error -> " • $error" } ?: ""}") {}
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
                }
            }
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
        item { SectionTitle("Secret references") }
        if (state.secrets.isEmpty()) item { Text("No secret references configured.", color = Muted) }
        items(state.secrets, key = { it.id }) { secret ->
            HomeBotCard(secret.label, "${secret.status} • value is never displayed") {}
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

@Composable private fun SectionTitle(title: String) { Text(title, fontSize = 20.sp, fontWeight = FontWeight.Bold, modifier = Modifier.padding(top = 12.dp)) }

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
    HomeBotCard(values.first, values.second, values.third) {}
}

@Composable
private fun HomeBotCard(title: String, detail: String, color: Color = Color.Unspecified, onClick: () -> Unit) {
    Card(shape = CardShape, modifier = Modifier.fillMaxWidth().clickable(onClick = onClick)) {
        Column(Modifier.padding(16.dp)) {
            Text(title, color = color, fontWeight = FontWeight.Bold)
            Text(detail, color = Muted, modifier = Modifier.padding(top = 4.dp))
        }
    }
}

@Composable private fun EmptyLoading(message: String) { Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { Text(message, color = Muted) } }
@Composable private fun ErrorBanner(message: String, dismiss: () -> Unit) { Card(Modifier.fillMaxWidth().padding(12.dp), shape = CardShape) { Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) { Text(message, color = Danger, modifier = Modifier.weight(1f)); TextButton(onClick = dismiss) { Text("Dismiss") } } } }
@Composable private fun HomeBotMark() { Box(Modifier.background(Violet, RoundedCornerShape(14.dp)).padding(13.dp, 9.dp)) { Text("H", color = Color.White, fontWeight = FontWeight.Black, fontSize = 20.sp) } }
@Composable private fun NavButton(label: String, selected: Boolean, onClick: () -> Unit) { TextButton(onClick = onClick) { Text(label, color = if (selected) Violet else Muted, fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal) } }
@Composable private fun HomeBotTheme(content: @Composable () -> Unit) { MaterialTheme(content = content) }

private val Canvas = Color(0xFFF7F6F9)
private val Violet = Color(0xFF7657FF)
private val Muted = Color(0xFF6E6978)
private val Success = Color(0xFF198754)
private val Warning = Color(0xFFE07A00)
private val Danger = Color(0xFFB3261E)
private val CardShape = RoundedCornerShape(18.dp)
