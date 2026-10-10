package app.porchlight.shared.live

import android.app.*
import android.content.*
import android.content.pm.ServiceInfo
import android.net.*
import android.os.Build
import android.os.IBinder
import androidx.core.content.ContextCompat
import androidx.work.*
import app.porchlight.shared.data.*
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.trust.TrustController
import app.porchlight.shared.trust.TrustState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.withLock
import java.util.concurrent.TimeUnit
import kotlin.time.Clock
import kotlin.time.Instant
import kotlin.time.Duration.Companion.minutes

/** Local identity generation survives token rotation; explicit identity-end deletes it. */
fun notificationActionOwner(context: Context, controller: TrustController): String? {
    val identity = controller.notificationIdentity() ?: return null
    val prefs = context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE)
    synchronized(actionOwnerLock) {
        val owner = actionIdentityOwner(identity, prefs.getString("identity", null), prefs.getString("owner", null), ::randomOperationKey)
        if (prefs.getString("identity", null) != identity || prefs.getString("owner", null) != owner)
            check(prefs.edit().putString("identity", identity).putString("owner", owner).commit())
        return owner
    }
}
private val actionOwnerLock = Any()
private fun storedOwner(context: Context) = context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).getString("owner", null)
private fun baseAction(context: Context, action: String, id: String, version: Int, key: String = randomOperationKey()) = Intent(context, NotificationActionReceiver::class.java)
    .setAction(action).setData(Uri.parse("porchlight-local://action/$action/$id/$version"))
    .putExtra("id", id).putExtra("version", version).putExtra("key", key).putExtra("owner", storedOwner(context))
fun actionIntent(context: Context, action: String, id: String, version: Int, key: String = randomOperationKey()): PendingIntent = PendingIntent.getBroadcast(context, 0,
    baseAction(context, action, id, version, key), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
fun localAlarmActionIntent(context: Context, action: String, code: Int, generation: Long): PendingIntent = PendingIntent.getBroadcast(context, code,
    baseAction(context, action, "alarm:$code", 0).setData(Uri.parse("porchlight-local://alarm-action/$action/$code/$generation"))
        .putExtra("code", code).putExtra("generation", generation), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
fun notificationActionRequest(input: Data) = OneTimeWorkRequestBuilder<NotificationActionWorker>().setInputData(input)
    .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST).build()
private fun enqueueAction(context: Context, input: Data) {
    val key = input.getString("key") ?: return
    WorkManager.getInstance(context).enqueueUniqueWork("notification-action:$key", ExistingWorkPolicy.KEEP, notificationActionRequest(input))
}
fun enqueueLocalSnooze(context: Context, ticket: AlarmTicket, at: Instant) {
    enqueueAction(context, workDataOf("action" to "snooze", "id" to "alarm:${ticket.code}", "code" to ticket.code,
        "generation" to ticket.generation, "at" to at.toEpochMilliseconds(), "key" to randomOperationKey(), "owner" to storedOwner(context)))
}
class NotificationActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra("id") ?: return
        val action = intent.action ?: return
        if (action !in setOf("complete", "read", "dismiss", "dismiss-local", "snooze")) return
        if (action == "dismiss" || action == "dismiss-local") AndroidDelivery(context).dismissed(id)
        if (action == "dismiss-local") return
        // Persist original body/version/key/owner and snooze timestamp BEFORE execution; never network in a receiver.
        enqueueAction(context, workDataOf("action" to if (action == "dismiss") "read" else action, "id" to id,
            "version" to intent.getIntExtra("version", -1), "key" to intent.getStringExtra("key"), "owner" to intent.getStringExtra("owner"),
            "code" to intent.getIntExtra("code", -1), "generation" to intent.getLongExtra("generation", -1),
            "at" to (Clock.System.now() + 15.minutes).toEpochMilliseconds()))
    }
}
fun notificationWorkerForegroundInfo(context: Context): ForegroundInfo {
    createNotificationChannels(context)
    val notification = privateNotification(context, "connection", "Porchlight notification action", "Finishing your household notification action.")
        .setOngoing(true).setContentIntent(openAppIntent(context)).build()
    return if (Build.VERSION.SDK_INT >= 34) ForegroundInfo(2, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else ForegroundInfo(2, notification)
}
class NotificationActionWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    // WorkManager requires this for expedited fallback on minSdk 30 (pre Android 12).
    override suspend fun getForegroundInfo() = notificationWorkerForegroundInfo(applicationContext)
    override suspend fun doWork(): Result {
        val services = AndroidAppServices.get(applicationContext)
        val controller = services.trust.controller
        controller.ensureRestored(); controller.state.first { it != TrustState.Loading }
        val owner = inputData.getString("owner") ?: return Result.failure()
        if (owner != notificationActionOwner(applicationContext, controller)) return failed(services) // old identity never gets new credentials
        val id = inputData.getString("id") ?: return Result.failure()
        val key = inputData.getString("key") ?: return Result.failure()
        services.data.cache.initialize()
        if (inputData.getString("action") == "snooze") {
            controller.sessionCommitMutex.withLock {
                if (owner != notificationActionOwner(applicationContext, controller)) return failed(services)
                AndroidAlarms(applicationContext).reconcile(services.data.cache.view.value)
                AndroidAlarms(applicationContext).snooze(inputData.getInt("code", -1), inputData.getLong("generation", -1),
                Instant.fromEpochMilliseconds(inputData.getLong("at", -1)))
                services.data.firedOccurrences.value = firedOccurrences(AndroidAlarms(applicationContext).tickets())
            }
            return Result.success() // stale/cancelled generation is a safe no-op
        }
        if (!controller.backgroundSessionUsable()) return Result.retry()
        return try { withBackgroundSession(controller) {
            if (owner != notificationActionOwner(applicationContext, controller)) return@withBackgroundSession failed(services)
            val outcome = when (inputData.getString("action")) {
                "complete" -> services.data.occurrences.complete(id, inputData.getInt("version", -1), key)
                "read" -> dismissDeliveredNotification(services.data, id, key)
                else -> return@withBackgroundSession Result.failure()
            }
            if (outcome is WriteResult.VersionConflict) failed(services) else Result.success()
        } } catch (e: CancellationException) { throw e }
        catch (p: app.porchlight.shared.transport.ApiProblem) { if (p.status in 400..499 && p.status != 429) failed(services) else Result.retry() }
        catch (_: Exception) { Result.retry() }
    }
    private fun failed(services: AndroidAppServices): Result {
        val notice = "Notification action could not complete. Unlock Porchlight and check Inbox or the item before retrying."
        applicationContext.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).edit().putString("action-failure", notice).commit()
        services.runtimeNotice.value = notice
        return Result.failure() // no silent version-conflict replay/new operation key
    }
}
class HouseholdSyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val services = AndroidAppServices.get(applicationContext)
        services.trust.controller.ensureRestored(); services.trust.controller.state.first { it != TrustState.Loading }
        services.data.cache.initialize()
        recoverLivePermissions(applicationContext, services)
        if (!services.trust.controller.backgroundSessionUsable()) return Result.success()
        return try {
            val completed = withBackgroundSession(services.trust.controller) {
                liveSafetyNet(services.settings.settings.first().stayConnected,
                    services.data.live?.hasRecentStreamFrame() == true,
                    { services.data.notifications.refresh() }, { services.data.sync.refresh() })
            }
            if (!completed) return Result.retry()
            recoverLivePermissions(applicationContext, services)
            Result.success()
        }
        catch (e: CancellationException) { throw e } catch (_: Exception) { Result.retry() }
    }
}
/** Re-check on Activity resume, periodic refresh and exact-permission broadcasts; pending OS inventory retries too. */
fun recoverLivePermissions(context: Context, services: AndroidAppServices) {
    val view = services.data.cache.view.value
    if (view.sync == null) AndroidAlarms(context).reschedule()
    else { AndroidAlarms(context).reconcile(view); AndroidAlarms(context).reschedule(); AndroidDelivery(context).reconcile(view) }
    services.runtimeNotice.value = livePermissionNotice(context)
        ?: context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).getString("action-failure", null)
}
fun livePermissionNotice(context: Context): String? = when {
    !androidx.core.app.NotificationManagerCompat.from(context).areNotificationsEnabled() -> "Notifications are disabled. Enable them in Settings → Notifications; unread alerts will be delivered when allowed."
    Build.VERSION.SDK_INT >= 31 && !context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms() -> "Exact alarms are disabled. Reminders may be late; enable Alarms & reminders in Android settings."
    else -> null
}
/** Application-owned observers; a foreground service protects their process when explicitly enabled. */
fun bindAndroidLive(context: Context, data: HouseholdData, scope: CoroutineScope) {
    createNotificationChannels(context)
    // Application-owned, not tied to the FGS. Android unregisters this callback on process death.
    context.getSystemService(ConnectivityManager::class.java).registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) { data.live?.networkChanged() }
        override fun onLost(network: Network) { data.live?.networkChanged() }
    })
    data.localSnooze = { occurrenceId -> withContext(Dispatchers.IO) { AndroidAlarms(context).snoozeOccurrence(occurrenceId) } }
    WorkManager.getInstance(context).enqueueUniquePeriodicWork("household-sync", ExistingPeriodicWorkPolicy.KEEP,
        PeriodicWorkRequestBuilder<HouseholdSyncWorker>(15, TimeUnit.MINUTES).build())
    scope.launch {
        val controller = AndroidAppServices.get(context).trust.controller
        controller.ensureRestored(); controller.state.first { it != TrustState.Loading }
        data.cache.view.dropWhile { it.sync == null }.collect { view ->
            val services = AndroidAppServices.get(context)
            notificationActionOwner(context, services.trust.controller)?.let { owner ->
                check(context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).edit().putString("owner", owner).commit())
            }
            AndroidAlarms(context).reconcile(view)
            data.firedOccurrences.value = firedOccurrences(AndroidAlarms(context).tickets())
            AndroidDelivery(context).reconcile(view)
            services.runtimeNotice.value = livePermissionNotice(context)
                ?: context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).getString("action-failure", null)
        }
    }
}
class HouseholdStreamService : Service() {
    private var foregroundStarted = false
    private val serviceScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    override fun onCreate() {
        super.onCreate(); createNotificationChannels(this)
        val notification = privateNotification(this, "connection", "Porchlight live updates", "Household live updates are enabled on this device.")
            .setOngoing(true).setContentIntent(openAppIntent(this)).build()
        try {
            if (Build.VERSION.SDK_INT >= 34) startForeground(1, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
            else startForeground(1, notification)
        } catch (e: RuntimeException) {
            if (Build.VERSION.SDK_INT >= 31 && e is ForegroundServiceStartNotAllowedException) {
                enqueueSafetyNet(this); stopSelf(); return
            }
            throw e
        }
        foregroundStarted = true
        val services = AndroidAppServices.get(this)
        services.trust.controller.ensureRestored(); services.data

    }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (!foregroundStarted) return START_NOT_STICKY
        serviceScope.launch {
            if (!AndroidAppServices.get(this@HouseholdStreamService).settings.settings.first().stayConnected) stopSelf()
        }
        return START_STICKY
    }
    override fun onDestroy() { serviceScope.cancel(); super.onDestroy() }
    override fun onBind(intent: Intent?): IBinder? = null
}
/** Call only from a foreground Activity (Android forbids arbitrary background FGS starts). */
fun updateForegroundConnection(context: Context, enabled: Boolean) {
    val intent = Intent(context, HouseholdStreamService::class.java)
    if (enabled) try { ContextCompat.startForegroundService(context, intent) }
    catch (e: RuntimeException) {
        if (Build.VERSION.SDK_INT >= 31 && e is ForegroundServiceStartNotAllowedException) enqueueSafetyNet(context)
        else throw e
    }
    else context.stopService(intent)
}

private fun enqueueSafetyNet(context: Context) {
    WorkManager.getInstance(context).enqueueUniqueWork("household-fgs-fallback", ExistingWorkPolicy.KEEP,
        OneTimeWorkRequestBuilder<HouseholdSyncWorker>().build())
}
