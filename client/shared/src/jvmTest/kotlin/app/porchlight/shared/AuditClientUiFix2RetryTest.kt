package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.item.ItemScreen
import app.porchlight.shared.screens.today.defaultTodaySettings
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import app.porchlight.shared.trust.*
import kotlinx.coroutines.*
import kotlinx.serialization.json.*
import java.util.concurrent.CopyOnWriteArrayList
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditClientUiFix2RetryTest {
    @Test fun assigneesReplayIdenticallyAfterPinRelock() = replay("assignees")
    @Test fun viewersReplayIdenticallyAfterPinRelock() = replay("viewers")
    @Test fun remindersReplayIdenticallyAfterPinRelock() = replay("reminders")
    private data class Request(val method: String, val path: String, val body: JsonObject?, val version: Long?, val key: String?)
    private fun replay(lockStep: String) = runBlocking {
        val now = Instant.parse("2026-10-11T12:00:00Z")
        val cache = memoryCache { now }; val release = CompletableDeferred<Unit>()
        val requests = CopyOnWriteArrayList<Request>()
        val committed = mutableMapOf<String?, Pair<Request, JsonObject>>()
        val member = MemberRead(false, "self", "home", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
        val caps = listOf("view", "edit", "reassign", "change_visibility", "edit_reminders")
        var server = itemResponse("item").copy(capabilities = caps.map(ItemCapability::valueOf))
        lateinit var controller: TrustController
        var locked = false
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (method == "GET") return WireJson.encodeToJsonElement(server).jsonObject
                val request = Request(method, path, body, version, idempotencyKey)
                requests += request
                committed[idempotencyKey]?.let { (original, reply) -> assertEquals(original, request); return reply }
                if (requests.size == 1) release.await()
                if (version != null && version != server.version.toLong()) throw ApiProblem("version-conflict", 409, "Stale")
                val fields = checkNotNull(body)
                server = server.copy(notes = fields["notes"]?.jsonPrimitive?.content ?: server.notes,
                    title = fields["title"]?.jsonPrimitive?.content ?: server.title,
                    assignees = fields["assignees"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.assignees,
                    viewers = fields["viewers"]?.jsonArray?.map { it.jsonPrimitive.content } ?: server.viewers,
                    reminderOffsets = fields["reminder_offsets"]?.jsonArray?.map { it.jsonPrimitive.int } ?: server.reminderOffsets,
                    version = server.version + 1)
                val reply = WireJson.encodeToJsonElement(server).jsonObject
                committed[idempotencyKey] = request to reply
                if (!locked && path.endsWith("/$lockStep")) { locked = true; controller.lockPin() }
                return reply
            }
        }
        val keys = object : DeviceKeyStore {
            var stored: DeviceTrust? = DeviceTrust("device", "fixture", "fixture", "session", "2027-01-01T00:00:00Z", listOf("https://example.invalid"), "2027-01-01T00:00:00Z")
            override suspend fun load() = stored
            override suspend fun save(trust: DeviceTrust, expectedPin: String?) { stored = trust }
            override suspend fun clear() { stored = null }
            override suspend fun csr() = "unused"
            override fun certificateExpiry(pem: String) = "2027-01-01T00:00:00Z"
        }
        controller = TrustController(keys, object : TrustTransportFactory {
            override fun enrollment(uri: EnrollUri) = api
            override suspend fun authenticated(trust: DeviceTrust) = api
        }, "android", { now }, true)
        controller.restore(); controller.unlockPin()
        val data = HouseholdData(cache, GeneratedCall.trusted(controller), { now }).also { it.items.afterWrite = {} }
        val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
        try {
            cache.inSession { cache.install(it, emptySnapshot(1).copy(members = listOf(member), memberSettings = SettingsRead("self", 1, defaultTodaySettings))) }
            runDesktopComposeUiTest(width = 320, height = 1800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    val gate by controller.state.collectAsState()
                    if (gate == TrustState.Ready) ItemScreen("new", creating = true)
                } } }
                onNodeWithContentDescription("Title").performTextReplacement("Laundry")
                onNodeWithContentDescription("Save item").performClick(); waitUntil { requests.isNotEmpty() }
                onNodeWithContentDescription("Notes").performScrollTo().performTextReplacement("Newer notes")
                onNodeWithContentDescription("Assignees: Sam Rivera").performScrollTo().performClick()
                onNodeWithContentDescription("Viewers: Sam Rivera").performScrollTo().performClick()
                onNodeWithContentDescription("Reminder offsets (minutes)").performScrollTo().performTextReplacement("15,30")
                val writer = services.screenSession.writes("item:new")
                release.complete(Unit); waitUntil { !writer.state.value.busy }
                onNodeWithContentDescription("Save item").performClick()
                waitUntil { controller.state.value == TrustState.PinRequired && !writer.state.value.busy }
                assertNotNull(writer.state.value.retryKey)
                waitForIdle() // The real trust gate removes the screen before it is restored.
                runOnIdle { controller.unlockPin() }
                onNodeWithContentDescription("Retry identical request").performClick()
                waitUntil { requests.count { it.path.endsWith("/$lockStep") } == 2 && !writer.state.value.busy }
                val replayed = requests.filter { it.path.endsWith("/$lockStep") }
                assertEquals(replayed[0], replayed[1], "relock retry must preserve method, body, version and step key")
                assertTrue(writer.state.value.saved, "identical replay must continue remaining plan steps")
                assertEquals(1, requests.count { it.method == "POST" })
                assertEquals(1, requests.count { it.method == "PATCH" })
                for (part in listOf("assignees", "viewers", "reminders")) assertEquals(if (part == lockStep) 2 else 1, requests.count { it.path.endsWith("/$part") })
                assertEquals(listOf("self"), server.assignees); assertEquals(listOf("self"), server.viewers)
                assertEquals(listOf(15, 30), server.reminderOffsets)
            }
        } finally { release.complete(Unit); services.screenSession.writeScope.cancel(); controller.close(); cache.close() }
    }
}
