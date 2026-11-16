package app.porchlight.shared.data

import app.porchlight.shared.BuildInfo
import app.porchlight.shared.api.apis.NotificationsApi
import app.porchlight.shared.api.models.*
import kotlinx.coroutines.flow.*
import kotlinx.serialization.json.buildJsonObject

/** Typed recipient-private inbox. No optimistic read/dismiss and no automatic version-conflict replay. */
class NotificationRepository(private val api: NotificationsApi, private val cache: ReadCache,
    private val writes: WriteCoordinator, private val version: String = BuildInfo.VERSION) {
    val notifications = cache.view.map { it.notifications }.distinctUntilChanged()
    val prefs = cache.view.map { it.notificationPrefs }.distinctUntilChanged()
    internal var afterWrite: suspend () -> Unit = {}
    internal var scopedAfterWrite: (suspend (SyncResource) -> Boolean)? = null
    private suspend fun follow(resource: SyncResource) { if (scopedAfterWrite?.invoke(resource) != true) afterWrite() }
    suspend fun refresh(): List<Notification> = cache.orderedRead { fence ->
        allPages { cursor -> api.notificationsList(version, unread = false, cursor = cursor, limit = 200).let { it.items to it.nextCursor } }
            .also { cache.replaceNotifications(fence, it) }
    }
    internal suspend fun refetchIds(ids: Set<String>): Unit = cache.orderedRead { fence ->
        for (part in ids.sorted().chunked(100)) {
            val rows = api.notificationsList(version, ids = part.joinToString(",")).items
            cache.replaceNotificationIds(fence, part.toSet(), rows)
        }
    }
    suspend fun refreshPrefs(): PrefsRead = cache.orderedRead { fence -> api.notificationsPrefsGet(version).also { cache.putNotificationPrefs(fence, it) } }
    suspend fun read(id: String, key: String = writes.newKey()): WriteResult<Notification> = cache.inSession { fence ->
        writes.execute(key) { api.notificationsRead(id, version, it, buildJsonObject {}) }.withRefresh { follow(SyncResource.Notification(id)) }
    }
    suspend fun readAll(key: String = writes.newKey()): WriteResult<ReadAllResult> = cache.inSession { fence ->
        writes.execute(key) { api.notificationsReadAll(version, it, buildJsonObject {}) }.withRefresh { follow(SyncResource.Inbox) }
    }
    suspend fun putPrefs(expectedVersion: Int, prefs: NotificationPrefs, key: String = writes.newKey()): WriteResult<PrefsRead> = cache.inSession { fence ->
        writes.execute(key) { api.notificationsPrefsPut(expectedVersion.toString(), version, it, prefs) }
            .also { if (it is WriteResult.Success) cache.putNotificationPrefs(fence, it.value) }.withRefresh { follow(SyncResource.NotificationPrefs) }
    }
}

private suspend fun <T> WriteResult<T>.withRefresh(refresh: suspend () -> Unit): WriteResult<T> {
    if (this !is WriteResult.Success) return this
    return try { refresh(); this } catch (e: kotlinx.coroutines.CancellationException) { throw e }
    catch (_: Exception) { copy(refreshPending = true) }
}
