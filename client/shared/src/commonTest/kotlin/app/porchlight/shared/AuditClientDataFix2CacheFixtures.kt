package app.porchlight.shared

import app.porchlight.shared.api.models.Notification
import app.porchlight.shared.api.models.SnapshotRead
import app.porchlight.shared.data.ReadCache
import app.porchlight.shared.data.CacheOwner
import app.porchlight.shared.data.cacheCaFingerprint

// These live fixtures belong to the same pinned root/device as their fake trust store, not an anonymous legacy cache.
suspend fun ReadCache.install(snapshot: SnapshotRead) {
    if (!ownerBindingRequired) initialize(CacheOwner(cacheCaFingerprint("root"), null, "self"))
    inSession { install(it, snapshot) }
}
suspend fun ReadCache.replaceNotifications(rows: List<Notification>) = inSession { replaceNotifications(it, rows) }
