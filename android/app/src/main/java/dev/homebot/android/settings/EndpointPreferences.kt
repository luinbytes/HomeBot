package dev.homebot.android.settings

import android.content.Context
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map

data class EndpointSettings(val endpoint: String = "", val deviceName: String = "Android")

interface EndpointPreferences {
    val settings: Flow<EndpointSettings>
    suspend fun update(endpoint: String, deviceName: String)
}

private val Context.homeBotDataStore by preferencesDataStore("homebot_preferences")

class AndroidEndpointPreferences(private val context: Context) : EndpointPreferences {
    override val settings: Flow<EndpointSettings> = context.homeBotDataStore.data.map { values ->
        EndpointSettings(
            endpoint = values[ENDPOINT].orEmpty(),
            deviceName = values[DEVICE_NAME] ?: "Android",
        )
    }

    override suspend fun update(endpoint: String, deviceName: String) {
        context.homeBotDataStore.edit { values ->
            values[ENDPOINT] = endpoint
            values[DEVICE_NAME] = deviceName
        }
    }

    private companion object {
        val ENDPOINT = stringPreferencesKey("endpoint")
        val DEVICE_NAME = stringPreferencesKey("device_name")
    }
}
