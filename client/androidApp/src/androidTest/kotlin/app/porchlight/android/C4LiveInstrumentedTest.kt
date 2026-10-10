package app.porchlight.android

import android.app.Notification
import android.app.NotificationManager
import android.content.ComponentName
import android.content.pm.ServiceInfo
import android.os.Build
import android.service.notification.NotificationListenerService.Ranking
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.work.workDataOf
import app.porchlight.shared.live.*
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Safe managed-device checks: no enrollment, permission mutations, network or wall-clock waits. */
@RunWith(AndroidJUnit4::class)
class C4LiveInstrumentedTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    @Test fun sixChannelsArePrivateAndPostingUsesGenericPublicSummary() {
        createNotificationChannels(context)
        val manager = context.getSystemService(NotificationManager::class.java)
        assertEquals(6, deliveryChannels.size)
        deliveryChannels.forEach { id ->
            val channel = manager.getNotificationChannel(id)
            assertNotNull(channel)
            // Channel lock-screen visibility is system/user-owned: Android stores app-created channels as NO_OVERRIDE.
            // The channel must never force public content; privacy comes from each notification (asserted below).
            assertTrue(channel.lockscreenVisibility in setOf(Ranking.VISIBILITY_NO_OVERRIDE, Notification.VISIBILITY_PRIVATE, Notification.VISIBILITY_SECRET))
            assertEquals(if (id == "connection") NotificationManager.IMPORTANCE_MIN else NotificationManager.IMPORTANCE_DEFAULT, channel.importance)
        }
        val notification = privateNotification(context, "reminders", "Private household title", "Private household body").build()
        assertEquals(Notification.VISIBILITY_PRIVATE, notification.visibility)
        assertEquals("Porchlight: 1 reminder", notification.publicVersion.extras.getCharSequence(Notification.EXTRA_TITLE).toString())
        assertFalse(notification.publicVersion.extras.toString().contains("Private household"))
    }
    @Test fun immutableActionsAndExpeditedRequestRetainOriginalInputs() {
        val complete = actionIntent(context, "complete", "occurrence", 7)
        val snooze = localAlarmActionIntent(context, "snooze", 41, 3)
        if (Build.VERSION.SDK_INT >= 31) { assertTrue(complete.isImmutable); assertTrue(snooze.isImmutable) }
        assertNotEquals(snooze, localAlarmActionIntent(context, "snooze", 41, 4))
        assertNotEquals(complete, actionIntent(context, "read", "occurrence", 7))
        val input = workDataOf("action" to "snooze", "id" to "alarm:41", "code" to 41, "generation" to 3L,
            "at" to 2100000000000L, "owner" to "non-secret-owner", "key" to "original-operation", "version" to 7)
        val request = notificationActionRequest(input)
        assertTrue(request.workSpec.expedited)
        assertEquals(input, request.workSpec.input)
        assertEquals("original-operation", request.workSpec.input.getString("key"))
        assertEquals(3L, request.workSpec.input.getLong("generation", -1))
        assertEquals(2100000000000L, request.workSpec.input.getLong("at", -1))
    }
    @Test fun workerForegroundFallbackAndBothServiceDeclarationsArePresent() {
        val info = notificationWorkerForegroundInfo(context)
        assertEquals(2, info.notificationId)
        assertEquals("connection", info.notification.channelId)
        assertEquals(Notification.VISIBILITY_PRIVATE, info.notification.visibility)
        if (Build.VERSION.SDK_INT >= 34) assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE, info.foregroundServiceType)
        val packageManager = context.packageManager
        for (name in listOf("app.porchlight.shared.live.HouseholdStreamService", "androidx.work.impl.foreground.SystemForegroundService")) {
            val service = packageManager.getServiceInfo(ComponentName(context, name), 0)
            assertFalse(service.exported)
            if (Build.VERSION.SDK_INT >= 34) assertEquals(ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE, service.foregroundServiceType)
        }
    }
}
