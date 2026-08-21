package dev.homebot.android

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.homebot.android.connection.ConnectionState

class MainActivity : ComponentActivity() {
    private val viewModel by viewModels<MainViewModel>()
    private val incomingPairing = mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        acceptIntent(intent)
        setContent {
            HomeBotTheme {
                HomeBotRoot(viewModel, incomingPairing.value)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        acceptIntent(intent)
    }

    private fun acceptIntent(intent: Intent?) {
        incomingPairing.value = intent?.data?.takeIf {
            it.scheme == "homebot" && it.host == "pair"
        }?.toString()
    }
}

@Composable
private fun HomeBotRoot(viewModel: MainViewModel, incomingPairing: String?) {
    val connection by viewModel.connection.collectAsState()
    val storedSettings by viewModel.settings.collectAsState()
    var pairingLink by remember { mutableStateOf("") }
    var endpoint by remember { mutableStateOf("") }
    var deviceName by remember { mutableStateOf("Android") }
    var error by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(incomingPairing) {
        if (incomingPairing != null) pairingLink = incomingPairing
    }
    LaunchedEffect(storedSettings) {
        if (endpoint.isBlank()) endpoint = storedSettings.endpoint
        deviceName = storedSettings.deviceName
    }

    Surface(modifier = Modifier.fillMaxSize(), color = Color(0xFFF7F6F9)) {
        Column(
            modifier = Modifier.fillMaxSize().padding(horizontal = 24.dp, vertical = 48.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(
                    Modifier.background(Color(0xFF7657FF), RoundedCornerShape(14.dp))
                        .padding(horizontal = 13.dp, vertical = 9.dp),
                ) {
                    Text("H", color = Color.White, fontWeight = FontWeight.Black, fontSize = 20.sp)
                }
                Column(Modifier.padding(start = 12.dp)) {
                    Text("HomeBot", fontSize = 25.sp, fontWeight = FontWeight.Bold)
                    Text("Your AI team. On your computer.", color = Color(0xFF6E6978))
                }
            }
            ConnectionCard(connection)
            if (connection is ConnectionState.Unpaired || connection is ConnectionState.Pairing) {
                Text("Pair this device", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                OutlinedTextField(
                    value = pairingLink,
                    onValueChange = { pairingLink = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("HomeBot pairing link") },
                    minLines = 2,
                )
                OutlinedTextField(
                    value = deviceName,
                    onValueChange = { deviceName = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Device name") },
                )
                Button(
                    onClick = { viewModel.pair(pairingLink, deviceName) { error = it } },
                    enabled = pairingLink.isNotBlank() && connection !is ConnectionState.Pairing,
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF7657FF)),
                ) { Text("Connect to HomeBot") }
            } else {
                Text("Server", fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                OutlinedTextField(
                    value = endpoint,
                    onValueChange = { endpoint = it },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("HomeBot endpoint") },
                    singleLine = true,
                )
                Button(
                    onClick = { viewModel.updateEndpoint(endpoint, deviceName) { error = it } },
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF302C38)),
                ) { Text("Save and reconnect") }
            }
            error?.let { Text(it, color = Color(0xFFB3261E)) }
            Spacer(Modifier.height(4.dp))
            Text(
                "Session credentials are encrypted with Android Keystore. HomeBot keeps product state on your server.",
                color = Color(0xFF6E6978),
                fontSize = 13.sp,
            )
        }
    }
}

@Composable
private fun ConnectionCard(connection: ConnectionState) {
    val (title, detail, color) = when (connection) {
        ConnectionState.Unpaired -> Triple("Not paired", "Open a link from HomeBot desktop.", Color(0xFF6E6978))
        ConnectionState.Pairing -> Triple("Pairing", "Exchanging the one-time credential…", Color(0xFF7657FF))
        is ConnectionState.Connecting -> Triple("Connecting", connection.endpoint, Color(0xFF7657FF))
        is ConnectionState.Hydrating -> Triple("Syncing", "Loading authoritative HomeBot state…", Color(0xFF7657FF))
        is ConnectionState.Live -> Triple("Connected", "${connection.snapshot.bots.size} Bots • sequence ${connection.cursor}", Color(0xFF198754))
        is ConnectionState.Reconnecting -> Triple("Reconnecting", "Attempt ${connection.attempt}; your last safe view is retained.", Color(0xFFE07A00))
        is ConnectionState.VersionIncompatible -> Triple("Update required", "This client and server protocol do not overlap.", Color(0xFFB3261E))
        ConnectionState.Revoked -> Triple("Device revoked", "Pair again from an owner device.", Color(0xFFB3261E))
        is ConnectionState.Offline -> Triple("Offline", connection.failure.toString(), Color(0xFFE07A00))
    }
    Card(shape = RoundedCornerShape(18.dp), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(18.dp)) {
            Text(title, color = color, fontWeight = FontWeight.Bold)
            Text(detail, color = Color(0xFF6E6978), modifier = Modifier.padding(top = 4.dp))
        }
    }
}

@Composable
private fun HomeBotTheme(content: @Composable () -> Unit) {
    MaterialTheme(content = content)
}
