package app.porchlight.shared.data

import app.porchlight.shared.live.StreamFrame
import kotlin.time.Duration.Companion.days

internal fun HouseholdData.itemDemand(id: String, collaboration: Boolean = false): SyncDemand {
    val retained = agenda.retainedWindows()
    val member = cache.view.value.settings?.memberId
    val at = cache.sync.value?.lastFullSync
    val windows = if (retained.isNotEmpty() || member == null || at == null) retained
        else setOf(ItemAgendaWindow(member, at - 180.days, at + 548.days))
    return SyncDemand(SyncResource.Item(id), collaboration || cache.view.value.items.none { it.id == id }, windows)
}

/** True means no scoped coverage was granted and the caller must await the throttled snapshot fallback. */
internal suspend fun HouseholdData.readTargeted(fence: ReadCache.Fence, demand: SyncDemand): Boolean {
    when (val resource = demand.resource) {
        is SyncResource.Item -> {
            // Multiple retained windows may exceed the wire cap; each response still replaces only this item.
            val windows = demand.agenda.toList().chunked(8).ifEmpty { listOf(emptyList()) }
            try {
                for ((index, part) in windows.withIndex()) when (val result = itemBundles.fetch(resource.id, part, demand.collaboration && index == 0)) {
                    is ItemSyncResult.Bundle -> cache.installItemBundle(fence, result.coverage, result.rows)
                    ItemSyncResult.Evicted -> {
                        // Item eviction already invalidates the inbox in the same transaction.
                        cache.evictItem(resource.id)
                        // The scheduler follows up the inbox after releasing its content lane/read slot.
                        return false
                    }
                    ItemSyncResult.Fallback -> return true
                }
            } catch (_: ItemBundleNeedsSnapshot) { return true }
            catch (_: InvalidItemBundleCoverage) { return true }
            catch (e: StaleVisibilityReply) { if (resource.id !in cache.view.value.evictedItemIds) throw e }
        }
        is SyncResource.Notification -> notifications.refetchIds(setOf(resource.id))
        is SyncResource.Comment -> comments.refetchIds(resource.itemId, setOf(resource.id))
        is SyncResource.List -> lists.refetchByIds(setOf(resource.id), mapOf(resource.id to demand.listScopes))
        is SyncResource.Availability -> availability.refresh(resource.memberId)
        SyncResource.Members -> household.refreshMembers()
        SyncResource.Today -> household.refreshSettings()
        SyncResource.NotificationPrefs -> notifications.refreshPrefs()
        SyncResource.Inbox -> notifications.refresh()
        SyncResource.Lists -> { lists.refresh(); lists.refetchByIds(cache.view.value.lists.mapTo(mutableSetOf()) { it.id }) }
        SyncResource.Suggestions -> planning.refresh()
        SyncResource.Feeds -> feeds.refreshSummaries()
    }
    return false
}

internal suspend fun HouseholdData.applyTargeted(frame: StreamFrame.Tickle) {
    val view = cache.view.value
    suspend fun request(vararg resources: SyncResource) = sync.reconcile(*resources.map { SyncDemand(it) }.toTypedArray())
    suspend fun bundle(id: String, hydrate: Boolean = false) = sync.reconcile(itemDemand(id, hydrate))
    when {
        frame.evicted && frame.entityType == "item" -> {
            // Privacy fences install before any queued read; even an older active bundle cannot resurrect children.
            // The eviction also hides the inbox; do not advance its fence a second time.
            sync.evictItem(frame.entityId)
            request(SyncResource.Inbox)
        }
        frame.evicted && frame.entityType == "list" -> cache.evictList(frame.entityId)
        frame.kind in setOf("item.changed", "item.access_changed", "item.invalidated") && frame.entityType == "item" ->
            bundle(frame.entityId, frame.kind != "item.changed")
        frame.kind == "derived" && frame.entityType == "item" ->
            sync.reconcile(itemDemand(frame.entityId), SyncDemand(SyncResource.Members))
        frame.kind == "occurrence.changed" && frame.entityType == "occurrence" -> {
            val former = view.occurrences.find { it.id == frame.entityId }?.itemId
                ?: (view.board + view.agenda).find { it.occurrence?.id == frame.entityId || it.agenda?.occurrenceId == frame.entityId }?.id
            val parents = setOfNotNull(frame.itemId, former)
            if (parents.isEmpty()) sync.refresh(authoritative = true)
            else sync.reconcile(*parents.map { itemDemand(it) }.toTypedArray())
        }
        frame.kind in setOf("notification.created", "notification.updated", "notification.dismissed", "notification.invalidated") && frame.entityType == "notification" ->
            request(SyncResource.Notification(frame.entityId))
        frame.kind == "comment.changed" && frame.entityType == "comment" -> {
            val parent = frame.itemId ?: view.comments.find { it.id == frame.entityId }?.itemId
            if (parent == null) sync.refresh(authoritative = true)
            else if (view.items.none { it.id == parent }) bundle(parent, true)
            else request(SyncResource.Comment(parent, frame.entityId))
        }
        frame.kind == "attachment.changed" && frame.entityType == "attachment" -> {
            val parent = frame.itemId ?: view.attachments.find { it.id == frame.entityId }?.itemId
            if (parent == null) sync.refresh(authoritative = true) else bundle(parent, true)
        }
        frame.kind == "list.changed" && frame.entityType == "list" -> request(SyncResource.List(frame.entityId))
        frame.kind == "list_entry.changed" && frame.entityType == "list_entry" -> {
            val parent = frame.listId ?: view.lists.find { row -> (row.propertyEntries + row.gotIt).any { it.id == frame.entityId } }?.id
            when {
                parent != null -> request(SyncResource.List(parent))
                frame.itemId != null -> bundle(frame.itemId, true)
                else -> request(SyncResource.Lists)
            }
        }
        (frame.kind == "settings.changed" || frame.kind == "member.changed") && frame.entityType == "availability" ->
            request(SyncResource.Availability(frame.entityId))
        frame.kind == "settings.changed" -> when (frame.entityType) {
            "member_settings" -> request(SyncResource.Today)
            "notification_prefs" -> request(SyncResource.NotificationPrefs)
            else -> sync.refresh(authoritative = true)
        }
        frame.kind == "member.changed" && frame.entityType == "member" -> {
            request(SyncResource.Members)
            val after = cache.view.value.members
            val before = view.members.associateBy { it.id }
            if (before.keys != after.mapTo(mutableSetOf()) { it.id } || after.any { before[it.id]?.role != it.role })
                sync.refresh(authoritative = true)
            else {
                val zones = after.filter { before[it.id]?.tzid != it.tzid }.mapTo(mutableSetOf()) { it.id }
                val affected = cache.view.value.items.filter { it.ownerId in zones || it.assignees.any { member -> member in zones } }
                sync.reconcile(*affected.map { itemDemand(it.id) }.toTypedArray())
            }
        }
        frame.kind == "ics_source.changed" && frame.entityType == "ics_source" -> request(SyncResource.Feeds)
        frame.kind == "suggestion.changed" && frame.entityType == "suggestion" -> request(SyncResource.Suggestions)
        else -> sync.refresh(authoritative = true)
    }
}
