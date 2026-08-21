package dev.homebot.android

import android.app.Application
import dev.homebot.android.connection.AndroidKeystoreSessionStore
import dev.homebot.android.connection.HomeBotClient
import dev.homebot.android.settings.AndroidEndpointPreferences
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import okhttp3.OkHttpClient
import dev.homebot.android.notifications.AndroidNotificationCoordinator
import dev.homebot.android.notifications.NetworkReconnectObserver

class HomeBotApplication : Application() {
    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val endpointPreferences by lazy { AndroidEndpointPreferences(this) }
    val sessionStore by lazy { AndroidKeystoreSessionStore(this) }
    val client by lazy {
        HomeBotClient(
            http = OkHttpClient.Builder().retryOnConnectionFailure(true).build(),
            sessions = sessionStore,
            scope = applicationScope,
        )
    }

    override fun onCreate() {
        super.onCreate()
        AndroidNotificationCoordinator(this, client, applicationScope).start()
        NetworkReconnectObserver(this, client).start()
    }
}
