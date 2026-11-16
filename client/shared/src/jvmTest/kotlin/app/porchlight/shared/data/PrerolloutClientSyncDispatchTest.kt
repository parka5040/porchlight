package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import io.ktor.http.Url
import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Duration.Companion.milliseconds

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutClientSyncDispatchTest {
    private val f = SyncBundleFixtures
    internal inner class Fixture(scope: TestScope, targeted: Boolean = true) {
        val requests = mutableListOf<Pair<String, Url>>()
        var snapshotCursor = f.snapshot.cursor
        var itemResponse = f.recording<ItemResponse>("item")
        var scopedList = false
        var bundleResponse: ItemSyncRead? = null
        var before: suspend (String, Url) -> Unit = { _, _ -> }
        val cache = memoryCache { f.now + scope.testScheduler.currentTime.milliseconds }
        private val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                val url = Url(testOrigin() + path)
                requests += method to url
                before(method, url)
                val route = url.encodedPath
                return when {
                    route.endsWith("/sync") -> json(bundleResponse ?: f.bundle(collaboration = url.parameters["include"] == "collaboration"))
                    method == "PATCH" && route.startsWith("/v1/items/") -> json(itemResponse)
                    method == "PATCH" && route.startsWith("/v1/occurrences/") -> throw NoContentSuccess()
                    route == "/v1/snapshot" -> json(f.snapshot.copy(cursor = snapshotCursor))
                    route == "/v1/board" -> page(listOf(json(f.board)))
                    route == "/v1/agenda" -> page(listOf(json(f.agenda)))
                    route == "/v1/members" -> page(f.snapshot.members.map { json(it) })
                    route == "/v1/me/settings" -> json(f.snapshot.memberSettings)
                    route == "/v1/me/notification-prefs" -> json(f.snapshot.notificationPrefs)
                    route.endsWith("/availability") -> json(f.snapshot.availability.single())
                    route.endsWith("/comments") -> page(f.snapshot.comments.filter { it.id == url.parameters["ids"] }.map { json(it) })
                    route.startsWith("/v1/lists/") -> json(f.snapshot.lists.single().let {
                        if (scopedList) it.copy(mode = ListMode.per_occurrence, occurrenceId = url.parameters["occurrence_id"]) else it
                    })
                    route == "/v1/lists" -> page(f.snapshot.lists.map { json(it) })
                    route in setOf("/v1/notifications", "/v1/feeds", "/v1/suggestions") -> page(emptyList())
                    else -> error("Unexpected fixture route: $route")
                }
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ transport }, { "fixture-session" }, jsonDispatcher = StandardTestDispatcher(scope.testScheduler)),
            { f.now + scope.testScheduler.currentTime.milliseconds }, syncDispatcher = StandardTestDispatcher(scope.testScheduler))
        init { data.itemSyncCapability = { targeted }; data.targetedSyncVerified = true }
        suspend fun seed() = cache.install(f.snapshot, f.now, Projections(listOf(f.board), AgendaWindow(f.from, f.to, f.snapshot.memberSettings.memberId, listOf(f.agenda))))
        fun getPaths() = requests.filter { it.first == "GET" }.map { it.second.encodedPath }
        fun close() = data.close()
    }
    private inline fun <reified T> json(value: T): JsonObject = WireJson.encodeToJsonElement(value).jsonObject
    private fun page(rows: List<JsonObject>) = buildJsonObject { put("items", JsonArray(rows)); put("next_cursor", JsonNull) }
    private fun frame(kind: String, type: String, id: String, item: String? = null, list: String? = null, evicted: Boolean = false) =
        StreamFrame.Tickle(100, kind, type, id, evicted, item, list)

    @Test fun eachRecordedKindUsesOnlyItsAuthoritativeScopedRequests() = runTest {
        val h = Fixture(this); h.seed()
        val item = f.snapshot.items.single().id; val member = f.snapshot.members.single().id
        val list = f.snapshot.lists.single().id; val comment = f.snapshot.comments.single().id
        val cases = listOf(
            frame("item.changed", "item", item) to listOf("/v1/items/$item/sync"),
            frame("item.access_changed", "item", item) to listOf("/v1/items/$item/sync"),
            frame("item.invalidated", "item", item) to listOf("/v1/items/$item/sync"),
            frame("derived", "item", item) to listOf("/v1/items/$item/sync", "/v1/members"),
            frame("occurrence.changed", "occurrence", f.snapshot.occurrences.single().id, item) to listOf("/v1/items/$item/sync"),
            frame("list.changed", "list", list) to listOf("/v1/lists/$list"),
            frame("list_entry.changed", "list_entry", "entry", item, list) to listOf("/v1/lists/$list"),
            frame("comment.changed", "comment", comment, item) to listOf("/v1/items/$item/comments"),
            frame("attachment.changed", "attachment", f.snapshot.attachments.single().id, item) to listOf("/v1/items/$item/sync"),
            frame("notification.created", "notification", "notice") to listOf("/v1/notifications"),
            frame("notification.updated", "notification", "notice") to listOf("/v1/notifications"),
            frame("notification.dismissed", "notification", "notice") to listOf("/v1/notifications"),
            frame("notification.invalidated", "notification", "notice") to listOf("/v1/notifications"),
            frame("settings.changed", "member_settings", member) to listOf("/v1/me/settings"),
            frame("settings.changed", "notification_prefs", member) to listOf("/v1/me/notification-prefs"),
            frame("settings.changed", "availability", member) to listOf("/v1/members/$member/availability"),
            frame("member.changed", "availability", member) to listOf("/v1/members/$member/availability"),
            frame("member.changed", "member", member) to listOf("/v1/members"),
            frame("ics_source.changed", "ics_source", "feed") to listOf("/v1/feeds"),
            frame("suggestion.changed", "suggestion", "suggestion") to listOf("/v1/suggestions"),
        )
        try {
            for ((event, expected) in cases) {
                h.requests.clear(); h.data.applyStreamFrame(event)
                assertEquals(expected.sorted(), h.getPaths().sorted(), event.kind)
                if (event.kind.startsWith("notification.")) assertEquals("notice", h.requests.single().second.parameters["ids"])
                if (event.kind == "comment.changed") assertEquals(comment, h.requests.single().second.parameters["ids"])
                if (event.kind in setOf("item.invalidated", "item.access_changed", "attachment.changed"))
                    assertEquals("collaboration", h.requests.single().second.parameters["include"])
            }
        } finally { h.close() }
    }

    @Test fun recordedFramesBatchItemIdsAndAcknowledgeOnlyAfterAllReplacementsCommit() = runTest {
        val h = Fixture(this); h.seed()
        val admissions = Channel<Unit>(Channel.UNLIMITED); val release = CompletableDeferred<Unit>()
        h.data.sync.targetedDemandAdmitted = { admissions.trySend(Unit) }
        h.data.sync.waitForTargetedBatch = { delay(it); release.await() }
        val parser = app.porchlight.shared.live.SseParser()
        val frames = checkNotNull(javaClass.getResourceAsStream("/sync-fixtures/dispatch-b.sse"))
            .bufferedReader().use { it.readLines() }.mapNotNull { parser.line(it) }
        val live = app.porchlight.shared.live.LiveSync(app.porchlight.shared.live.StreamSource { _, receive -> frames.forEach { receive(it) } },
            { h.cache.sync.value?.cursor }, { h.data.applyStreamFrame(it) }, { h.cache.acknowledgeCursor(it) }, {}, { f.now }, concurrentApply = { true })
        try {
            val connection = async { live.connectOnce() }
            repeat(4) { admissions.receive() }
            assertTrue(h.requests.isEmpty())
            assertEquals(f.snapshot.cursor, h.cache.sync.value?.cursor)
            release.complete(Unit); connection.await()
            val item = f.snapshot.items.single().id
            assertEquals(listOf("/v1/items/$item/sync", "/v1/items/$item/comments", "/v1/notifications").sorted(), h.getPaths().sorted())
            assertEquals(110L, h.cache.sync.value?.cursor)
        } finally { h.close() }
    }

    @Test fun missingParentHintsUseCachedParentsOrDocumentedFallbacks() = runTest {
        val h = Fixture(this); h.seed()
        val item = f.snapshot.items.single().id
        try {
            h.data.applyStreamFrame(frame("occurrence.changed", "occurrence", f.snapshot.occurrences.single().id))
            assertEquals(listOf("/v1/items/$item/sync"), h.getPaths())
            h.requests.clear()
            h.data.applyStreamFrame(frame("comment.changed", "comment", f.snapshot.comments.single().id))
            assertEquals(listOf("/v1/items/$item/comments"), h.getPaths())
            h.requests.clear()
            h.data.applyStreamFrame(frame("occurrence.changed", "occurrence", "uncached"))
            assertTrue("/v1/snapshot" in h.getPaths())
        } finally { h.close() }
    }

    @Test fun firstVisibilityHydratesCollaborationAndEvictionWinsOverInflightBundle() = runTest {
        val h = Fixture(this); h.seed(); val item = f.snapshot.items.single().id
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        h.before = { _, url -> if (url.encodedPath.endsWith("/sync")) { started.complete(Unit); release.await() } }
        try {
            h.cache.evictItem(item)
            val gained = async { h.data.applyStreamFrame(frame("item.changed", "item", item)) }
            started.await()
            assertEquals("collaboration", h.requests.single().second.parameters["include"])
            val revision = h.cache.view.value.visibilityRevision
            val eviction = async { h.data.applyStreamFrame(frame("item.access_changed", "item", item, evicted = true)) }
            h.cache.view.first { it.visibilityRevision > revision }
            assertTrue(h.cache.view.value.items.isEmpty())
            release.complete(Unit); gained.await(); eviction.await()
            assertTrue(h.cache.view.value.items.isEmpty()); assertTrue(h.cache.view.value.comments.isEmpty())
            assertTrue(h.cache.view.value.attachments.isEmpty())
        } finally { h.close() }
    }

    @Test fun lateItemFrameStartsAnotherBundleAndFallbackCannotAdvanceGlobalCursor() = runTest {
        val h = Fixture(this); h.seed(); val item = f.snapshot.items.single().id
        val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
        val admissions = Channel<Unit>(Channel.UNLIMITED)
        h.data.sync.targetedDemandAdmitted = { admissions.trySend(Unit) }
        h.before = { _, url -> if (url.encodedPath.endsWith("/sync") && !started.isCompleted) { started.complete(Unit); release.await() } }
        try {
            val first = async { h.data.applyStreamFrame(frame("item.changed", "item", item)) }
            admissions.receive(); started.await()
            val late = async { h.data.applyStreamFrame(frame("item.invalidated", "item", item)) }
            admissions.receive(); release.complete(Unit); first.await(); late.await()
            assertEquals(2, h.getPaths().size)
            h.cache.acknowledgeCursor(2)
            val old = h.cache.sync.value!!.cursor
            h.snapshotCursor = old + 200
            // A fallback snapshot is not an acknowledgement of a different, still-unapplied delivered frame.
            h.data.sync.refresh(authoritative = true)
            assertEquals(old, h.cache.sync.value?.cursor)
            h.data.sync.resnapshot()
            assertEquals(old + 200, h.cache.sync.value?.cursor)
        } finally { h.close() }
    }

    @Test fun ordinaryEditUsesOneBundleButLegacyUsesSnapshotAndProjectionPages() = runTest {
        for (targeted in listOf(true, false)) {
            val h = Fixture(this, targeted); h.seed(); val item = f.snapshot.items.single()
            try {
                val result = h.data.items.edit(item.id, item.version, ItemEdit().text("title", "Updated task"))
                assertFalse(assertIs<WriteResult.Success<*>>(result).refreshPending)
                if (targeted) assertEquals(listOf("/v1/items/${item.id}/sync"), h.getPaths())
                else assertEquals(listOf("/v1/snapshot", "/v1/board", "/v1/agenda").sorted(), h.getPaths().sorted())
            } finally { h.close() }
        }
    }

    @Test fun resnapshotsWithinThirtySecondsWaitWithoutBlockingTargetedReads() = runTest {
        val h = Fixture(this); h.seed(); val waits = Channel<Long>(Channel.UNLIMITED)
        val releaseThrottle = CompletableDeferred<Unit>()
        h.data.sync.waitForReadRetry = { waits.send(it); releaseThrottle.await(); delay(it) }
        try {
            h.data.sync.refresh(authoritative = true)
            val again = async { h.data.sync.refresh(authoritative = true) }
            assertTrue(waits.receive() in 29_000..30_000)
            assertEquals(1, h.getPaths().count { it == "/v1/snapshot" })
            h.data.applyStreamFrame(frame("item.changed", "item", f.snapshot.items.single().id))
            assertEquals(1, h.getPaths().count { it == "/v1/snapshot" })
            releaseThrottle.complete(Unit); again.await()
            assertEquals(2, h.getPaths().count { it == "/v1/snapshot" })
        } finally { h.close() }
    }

    @Test fun failedItemDoesNotFailNotificationAndExplicitRetryNeedsNoSnapshot() = runTest {
        val h = Fixture(this); h.seed(); val item = f.snapshot.items.single().id
        var fail = true
        h.before = { _, url -> if (fail && url.encodedPath.endsWith("/sync")) throw ApiProblem("unavailable", 503, "Try later") }
        try {
            val bad = async { runCatching { h.data.applyStreamFrame(frame("item.changed", "item", item)) } }
            val good = async { h.data.applyStreamFrame(frame("notification.invalidated", "notification", "notice")) }
            good.await(); assertTrue(bad.await().isFailure)
            assertEquals(listOf("/v1/items/$item/sync", "/v1/notifications").sorted(), h.getPaths().sorted())
            fail = false; h.data.applyStreamFrame(frame("item.changed", "item", item))
            assertEquals(2, h.getPaths().count { it.endsWith("/sync") })
        } finally { h.close() }
    }

    @Test fun boundaryClassificationFailureAwaitsAuthoritativeFallbackWithoutAcknowledgingSnapshotWatermark() = runTest {
        val h = Fixture(this)
        val floating = f.snapshot.items.single().copy(timeMode = "floating", tzid = null)
        val boundary = f.snapshot.occurrences.single().copy(dueAt = null, localDue = "2026-04-09T12:00:00")
        h.cache.install(f.snapshot.copy(items = listOf(floating), occurrences = listOf(boundary)))
        h.bundleResponse = f.bundle(f.snapshot.copy(items = listOf(floating), occurrences = emptyList()))
        h.snapshotCursor = f.snapshot.cursor + 200
        try {
            h.data.applyStreamFrame(frame("item.changed", "item", floating.id))
            assertTrue("/v1/snapshot" in h.getPaths())
            assertEquals(f.snapshot.items.single().timeMode, h.cache.view.value.items.single().timeMode)
            assertEquals(f.snapshot.cursor, h.cache.sync.value?.cursor)
        } finally { h.close() }
    }

    @Test fun terminalResetAdoptsSnapshotCursorAndThrottleIsScopedToStorageAdmission() = runTest {
        val h = Fixture(this); h.seed(); var identity = 1L
        h.cache.sessionEpoch = { identity }
        val starts = mutableListOf<Long>()
        h.before = { _, url -> if (url.encodedPath == "/v1/snapshot") starts += testScheduler.currentTime }
        try {
            h.snapshotCursor = f.snapshot.cursor + 200
            h.data.sync.resnapshot(); assertEquals(h.snapshotCursor, h.cache.sync.value?.cursor)
            h.data.sync.clear(); h.data.sync.refresh()
            assertTrue(starts[1] - starts[0] < 30_000, "Replacement storage must not inherit the old snapshot cooldown")
            identity++
            val before = testScheduler.currentTime
            h.data.sync.resnapshot()
            assertTrue(testScheduler.currentTime - before < 30_000)
        } finally { h.close() }
    }

    @Test fun snapshotRetryAfterIsNotCappedAndStillHonoursThirtySecondStartLimit() = runTest {
        val h = Fixture(this); h.seed(); val starts = mutableListOf<Long>()
        h.before = { _, url -> if (url.encodedPath == "/v1/snapshot") {
            starts += testScheduler.currentTime
            if (starts.size == 1) throw ApiProblem("rate-limited", 429, "Try later", retryAfterMillis = 60_000)
        } }
        try {
            h.data.sync.refresh(authoritative = true)
            assertEquals(2, starts.size); assertTrue(starts[1] - starts[0] >= 60_000)
        } finally { h.close() }
    }
}
