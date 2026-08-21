package dev.homebot.android.notifications

import android.Manifest
import android.app.Activity
import android.app.Application
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.PackageManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.Uri
import android.os.Bundle
import dev.homebot.android.MainActivity
import dev.homebot.android.connection.ClientAlert
import dev.homebot.android.connection.ClientAlertKind
import dev.homebot.android.connection.HomeBotClient
import dev.homebot.android.connection.deepLink
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch

class AndroidNotificationCoordinator(
    private val application: Application,
    private val client: HomeBotClient,
    private val scope: CoroutineScope,
) : Application.ActivityLifecycleCallbacks {
    private val manager = application.getSystemService(NotificationManager::class.java)
    private var startedActivities = 0

    fun start() {
        createChannels()
        application.registerActivityLifecycleCallbacks(this)
        scope.launch {
            client.alerts.collect { alert ->
                if (startedActivities == 0) notify(alert)
            }
        }
    }

    private fun createChannels() {
        listOf(
            NotificationChannel(WORK_CHANNEL, "Bot work", NotificationManager.IMPORTANCE_DEFAULT),
            NotificationChannel(APPROVAL_CHANNEL, "Approvals", NotificationManager.IMPORTANCE_HIGH),
            NotificationChannel(ROUTINE_CHANNEL, "Routines", NotificationManager.IMPORTANCE_DEFAULT),
            NotificationChannel(ERROR_CHANNEL, "HomeBot errors", NotificationManager.IMPORTANCE_HIGH),
        ).forEach(manager::createNotificationChannel)
    }

    private fun notify(alert: ClientAlert) {
        if (application.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        val intent = Intent(Intent.ACTION_VIEW, Uri.parse(alert.deepLink()), application, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val pending = PendingIntent.getActivity(
            application,
            alert.eventId.hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = Notification.Builder(application, alert.channel())
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(alert.title)
            .setContentText(alert.detail)
            .setStyle(Notification.BigTextStyle().bigText(alert.detail))
            .setContentIntent(pending)
            .setAutoCancel(true)
            .setCategory(if (alert.kind == ClientAlertKind.APPROVAL_REQUIRED) Notification.CATEGORY_RECOMMENDATION else Notification.CATEGORY_STATUS)
            .build()
        manager.notify(alert.eventId.hashCode(), notification)
    }

    override fun onActivityStarted(activity: Activity) { startedActivities += 1 }
    override fun onActivityStopped(activity: Activity) { startedActivities = (startedActivities - 1).coerceAtLeast(0) }
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit

    private fun ClientAlert.channel(): String = when (kind) {
        ClientAlertKind.BOT_FINISHED -> WORK_CHANNEL
        ClientAlertKind.APPROVAL_REQUIRED -> APPROVAL_CHANNEL
        ClientAlertKind.ROUTINE_RESULT -> ROUTINE_CHANNEL
        ClientAlertKind.ERROR -> ERROR_CHANNEL
    }

    private companion object {
        const val WORK_CHANNEL = "homebot_work"
        const val APPROVAL_CHANNEL = "homebot_approvals"
        const val ROUTINE_CHANNEL = "homebot_routines"
        const val ERROR_CHANNEL = "homebot_errors"
    }
}

class NetworkReconnectObserver(
    application: Application,
    private val client: HomeBotClient,
) {
    private val connectivity = application.getSystemService(ConnectivityManager::class.java)
    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = client.nudgeReconnect()
    }

    fun start() {
        connectivity.registerDefaultNetworkCallback(callback)
    }
}
