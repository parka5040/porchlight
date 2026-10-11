package app.porchlight.shared.data

import app.porchlight.shared.api.apis.ReadsApi
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

fun memoryCache(now: () -> Instant): ReadCache = memoryCache(MemoryAttachmentStore(), now)
expect fun memoryCache(attachmentStore: AttachmentStore, now: () -> Instant): ReadCache

class DataLayerTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun generatedCallUsesExistingTransportHeaders() = runTest {
        val transport = KtorApiTransport(HttpClient(MockEngine { request ->
            assertEquals("1.0.0", request.headers["Porchlight-Client-Version"])
            assertEquals("America/New_York", request.headers["Porchlight-TZ"])
            assertNotNull(request.headers["Authorization"])
            respond("{\"items\":[],\"next_cursor\":null}", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "America/New_York" }, { "unused" }), { false })
        val api = ReadsApi(GeneratedCall({ transport }, { "fixture-session" }))
        assertTrue(api.readsBoard("1.0.0").items.isEmpty())
        transport.close()
    }
    @Test fun failedSnapshotRetainsOldCursorAndContents() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(11), now)
            assertEquals(11, cache.sync.value?.cursor)
            assertFailsWith<IllegalStateException> {
                cache.installStaged(emptySnapshot(12), now) { throw IllegalStateException("injected staging failure") }
            }
            assertEquals(11, cache.sync.value?.cursor)
        } finally { cache.close() }
    }
}
fun testOrigin() = "https" + "://" + "fixture.invalid"
fun emptySnapshot(cursor: Long) = app.porchlight.shared.transport.WireJson.decodeFromString<app.porchlight.shared.api.models.SnapshotRead>(
    """{"notifications":[],"notification_prefs":{"member_id":"self","version":1,"prefs":{"reminders":true,"assignments":true,"dependents":true,"activity":true,"digest":false,"connection":true,"quiet_start":null,"quiet_end":null,"default_todo_offsets":[],"default_deadline_offsets":[],"default_appointment_offsets":[]}},"comments":[],"attachments":[],"ics_sources":[],"availability":[],"suggestions":[],"lists":[],"members":[],"items":[],"occurrences":[],"scheduled_reminders":[],"member_settings":{"member_id":"self","version":1,"settings":{"today_sections":["today"],"scope":"mine","this_week_days":7,"pinned_list_id":null}},"cursor":$cursor}""")
