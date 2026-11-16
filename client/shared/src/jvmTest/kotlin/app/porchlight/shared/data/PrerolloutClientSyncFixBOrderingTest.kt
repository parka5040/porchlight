package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import io.ktor.http.Url
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncFixBOrderingTest {
    private val f = SyncBundleFixtures
    private inline fun <reified T> json(value: T) = WireJson.encodeToJsonElement(value).jsonObject
    private fun page(rows: List<JsonObject>) = buildJsonObject { put("items", JsonArray(rows)); put("next_cursor", JsonNull) }

    @Test fun heldScreenEntryReadsCannotUndoAcknowledgedTargetedRepairs() = runTest {
        for (install in listOf("targeted", "full")) for (collection in listOf("board", "agenda", "inbox", "list", "comments", "settings", "members")) {
            val cache = memoryCache { f.now }
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            var hold = true
            val item = f.snapshot.items.single().id
            val comment = f.snapshot.comments.single()
            val list = f.snapshot.lists.single()
            val notice = Notification("notice", "activity", "activity", "item", item, 1,
                NotificationSummary("Old title", "Old title", "Old title", "activity", false), f.now, f.now, 1, itemId = item, releasedAt = f.now)
            val member = f.snapshot.members.single()
            val transport = object : ApiTransport {
                override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                    val url = Url(testOrigin() + path)
                    val old = hold
                    if (hold) { hold = false; started.complete(Unit); release.await() }
                    return when (url.encodedPath) {
                        "/v1/snapshot" -> json(f.snapshot.copy(
                            comments = listOf(comment.copy(body = "New title")), lists = listOf(list.copy(name = "New title")),
                            members = listOf(member.copy(displayName = "New title")), memberSettings = f.snapshot.memberSettings.copy(version = 11)))
                        "/v1/items/$item/sync" -> json(f.bundle(board = listOf(f.board.copy(title = "New title")), agenda = listOf(f.agenda.copy(title = "New title"))))
                        "/v1/board" -> page(listOf(json(f.board.copy(title = if (old) "Old title" else "New title"))))
                        "/v1/agenda" -> page(listOf(json(f.agenda.copy(title = if (old) "Old title" else "New title"))))
                        "/v1/notifications" -> page(if (old) listOf(json(notice)) else emptyList())
                        "/v1/lists/${list.id}" -> json(list.copy(name = if (old) "Old title" else "New title"))
                        "/v1/items/$item/comments" -> page(listOf(json(comment.copy(body = if (old) "Old title" else "New title"))))
                        "/v1/me/settings" -> json(f.snapshot.memberSettings.copy(version = if (old) 10 else 11))
                        "/v1/members" -> page(listOf(json(member.copy(displayName = if (old) "Old title" else "New title"))))
                        else -> error("Unexpected route ${url.encodedPath}")
                    }
                }
            }
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }, jsonDispatcher = StandardTestDispatcher(testScheduler)),
                { f.now }, syncDispatcher = StandardTestDispatcher(testScheduler))
            data.itemSyncCapability = { true }; data.targetedSyncVerified = true
            try {
                cache.install(f.snapshot.copy(notifications = listOf(notice)), projections = Projections(listOf(f.board), AgendaWindow(f.from, f.to, member.id, listOf(f.agenda))))
                val late = async { runCatching {
                    when (collection) {
                        "board" -> data.board.refresh()
                        "agenda" -> data.agenda.refresh(f.from, f.to, member.id)
                        "inbox" -> data.notifications.refresh()
                        "list" -> data.lists.get(list.id)
                        "comments" -> data.comments.refresh(item)
                        "settings" -> data.household.refreshSettings()
                        else -> data.household.refreshMembers()
                    }
                } }
                started.await()
                val (kind, type, id) = when (collection) {
                    "board", "agenda" -> Triple("item.invalidated", "item", item)
                    "inbox" -> Triple("notification.invalidated", "notification", notice.id)
                    "list" -> Triple("list.changed", "list", list.id)
                    "comments" -> Triple("comment.changed", "comment", comment.id)
                    "settings" -> Triple("settings.changed", "member_settings", member.id)
                    else -> Triple("member.changed", "member", member.id)
                }
                if (install == "full") data.sync.refresh()
                else data.applyStreamFrame(StreamFrame.Tickle(100, kind, type, id, false, itemId = item))
                cache.acknowledgeCursor(100)
                release.complete(Unit); late.await()
                val v = cache.view.value
                when (collection) {
                    "board" -> assertEquals("New title", v.board.single().title, collection)
                    "agenda" -> assertEquals("New title", v.agenda.single().title, collection)
                    "inbox" -> assertTrue(v.notifications.isEmpty(), "stale inbox resurrected removed notification")
                    "list" -> assertEquals("New title", v.lists.single().name, collection)
                    "comments" -> assertEquals("New title", v.comments.single().body, collection)
                    "settings" -> assertEquals(11, v.settings?.version)
                    else -> assertEquals("New title", v.members.single().displayName)
                }
                assertEquals(100, cache.sync.value?.cursor)
            } finally { release.complete(Unit); data.close() }
        }
    }
}
