package dev.homebot.android

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import dev.homebot.android.connection.ConnectionState
import dev.homebot.android.settings.EndpointSettings
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch

class MainViewModel(application: Application) : AndroidViewModel(application) {
    private val homeBot = application as HomeBotApplication
    val connection: StateFlow<ConnectionState> = homeBot.client.state
    val settings: StateFlow<EndpointSettings> = homeBot.endpointPreferences.settings.stateIn(
        viewModelScope,
        SharingStarted.WhileSubscribed(5_000),
        EndpointSettings(),
    )

    init {
        homeBot.client.start()
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

    override fun onCleared() {
        homeBot.client.stop()
        super.onCleared()
    }
}
