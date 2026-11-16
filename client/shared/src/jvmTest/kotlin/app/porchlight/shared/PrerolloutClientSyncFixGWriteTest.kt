package app.porchlight.shared

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicLong
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class PrerolloutClientSyncFixGWriteTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun relationshipContinuationCannotUseReplacementIdentityBeforeEpochNotification() {
        val epoch = AtomicLong(0)
        // Keep presentation/cache notification delayed; transport observes the actual replacement immediately.
        val cache = memoryCache { now }
        val session = ScreenSession().also { it.bind(0); it.bindCache(cache) }
        val between = CompletableDeferred<Unit>(); val resume = CompletableDeferred<Unit>()
        val sent = mutableListOf<Pair<String, String?>>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method != "GET") synchronized(sent) { sent += method to session }
                return WireJson.encodeToJsonElement(itemResponse("item").copy(version = 2)).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { if (epoch.get() == 0L) "original" else "replacement" }, epoch = epoch::get), { now })
        data.items.afterWrite = { if (!between.isCompleted) { between.complete(Unit); resume.await() } }
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
        try {
            runBlocking { cache.install(emptySnapshot(1).copy(items = listOf(itemRead("item")))) }
            val retained = session.editor("new", cache.view.value, true)
            retained.current(cache.view.value)!!.apply {
                savedId = "item"; expectedVersion = 1
                baseline = EditorDraft.from(itemRead("item"))
                store(baseline.copy(title = "Frozen edit", assignees = listOf("fixture-member")))
            }
            retained.bindResources(cache.view.value, setOf(CacheResource("Item", "item")))
            runDesktopComposeUiTest(width = 1280, height = 1400) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) {
                    PorchlightTheme(ThemeMode.Daylight, false) { ItemScreen("new", creating = true) }
                } }
                onNodeWithText("Save item").performClick()
                waitUntil { between.isCompleted }
                // Deliberately do NOT notify ScreenSession.bind or cancel the old screen scope.
                epoch.incrementAndGet()
                resume.complete(Unit)
                waitUntil { !session.writes("item:new").state.value.busy }
                assertEquals(listOf("PATCH" to "original"), synchronized(sent) { sent.toList() },
                    "The next frozen relationship step must never acquire replacement credentials")
                assertNotNull(session.writes("item:new").state.value.retryKey)
            }
        } finally { resume.complete(Unit); session.close(); data.close() }
    }

    @Test fun multistepClosureUsesLiveEpochEvenWhenScreenSessionNotificationIsDelayed() = continuation(true)
    @Test fun sameIdentityStorageRecoveryDoesNotInvalidatePendingContinuation() = continuation(false)
    private fun continuation(replaceIdentity: Boolean): Unit = runBlocking {
        val epoch = AtomicLong(0)
        val cache = memoryCache { now }.also { it.sessionEpoch = epoch::get }
        val sent = mutableListOf<String?>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                sent += session
                return WireJson.encodeToJsonElement(itemResponse("item")).jsonObject
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { if (epoch.get() == 0L) "original" else "replacement" }, epoch = epoch::get), { now })
        data.items.afterWrite = {}
        val screen = ScreenSession(Dispatchers.Unconfined).also { it.bind(0); it.bindCache(cache) }
        val between = CompletableDeferred<Unit>(); val resume = CompletableDeferred<Unit>()
        try {
            val writer = screen.writes("continuation")
            supervisorScope {
                val attempt = async { runCatching { writer.submit { key ->
                    data.items.edit("item", 1, ItemEdit().text("title", "Frozen edit"), key = "$key-item")
                    between.complete(Unit); resume.await()
                    data.items.setAssignees("item", 1, listOf("fixture-member"), "$key-assignees").feedback()
                } } }
                between.await()
                if (replaceIdentity) epoch.incrementAndGet() else cache.clearLocalCache()
                resume.complete(Unit)
                val result = attempt.await()
                if (replaceIdentity) {
                    assertIs<CancellationException>(result.exceptionOrNull())
                    assertEquals(listOf<String?>("original"), sent)
                    assertNotNull(writer.state.value.retryKey)
                } else {
                    result.getOrThrow()
                    assertEquals(listOf<String?>("original", "original"), sent)
                    assertTrue(writer.state.value.saved)
                }
            }
        } finally { resume.complete(Unit); screen.close(); data.close() }
    }

    @Test fun singleStepRetryCannotUseReplacementIdentityBeforeEpochNotification() = retry(false)
    @Test fun singleStepRetryCannotUseReplacementSession() = retry(true)
    private fun retry(replaceSession: Boolean) = runBlocking {
        val epoch = AtomicLong(0); val revision = AtomicLong(0)
        val cache = memoryCache { now }.also { it.sessionEpoch = epoch::get }
        val sent = mutableListOf<String?>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                sent += session
                throw ApiProblem("invalid-response", 502, "Reply unavailable")
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { if (revision.get() == 0L) "original" else "replacement" },
            epoch = epoch::get, sessionRevision = revision::get), { now })
        val session = ScreenSession(Dispatchers.Unconfined).also { it.bind(0); it.bindCache(cache) }
        try {
            val writer = session.writes("single")
            writer.submit { data.items.edit("item", 1, ItemEdit().text("title", "Frozen edit"), key = it).feedback() }
            val key = writer.state.value.retryKey
            assertNotNull(key)
            if (!replaceSession) epoch.incrementAndGet()
            revision.incrementAndGet()
            runCatching { writer.retry() }
            assertEquals(listOf<String?>("original"), sent, "A pending single-step request must retain its origin")
            assertEquals(key, writer.state.value.retryKey)
        } finally { session.close(); data.close() }
    }
}
