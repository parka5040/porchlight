package app.porchlight.shared

import androidx.compose.runtime.*
import androidx.compose.material3.Text
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.files.*
import app.porchlight.shared.navigation.SettingsSection
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.launch
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class C6PickerRelockTest {
    @Test fun pickerReplyWhileGatedRetainsDraftAndPreviewsOnlyOnUnlock() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        var previewCalls = 0
        var callback: ((Result<PickedCalendar?>) -> Unit)? = null
        val visible = mutableStateOf(true)
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                assertTrue(visible.value, "Never preview while gated")
                assertEquals(true, body!!["preview"]!!.jsonPrimitive.boolean)
                previewCalls++
                return WireJson.parseToJsonElement("""{"created":1,"updated":0,"unchanged":0,"removed":0,"deleted":0,"skipped":0,"warnings":[],"item_ids":[],"conflicts":[],"preview":true,"removal_suppressed":false}""").jsonObject
            }
        }
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex","user","#FABD2F","AX","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { callback = onResult }
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) = error("Not used")
            }
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) {
                    if (visible.value) PlanningScreen(services, SettingsSection.CalendarData, files) else Text("Locked")
                } } }
                onNodeWithContentDescription("Choose ICS file").performClick()
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { callback!!(Result.success(PickedCalendar("school.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR"))) }
                assertEquals(0, previewCalls)
                runOnIdle { visible.value = true }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Commit import").fetchSemanticsNodes().isNotEmpty() }
                assertEquals(1, previewCalls)
                onNodeWithText("school.ics").assertExists()
            }
        } finally { cache.close() }
    }
    @Test fun saveReplyWhileGatedDownloadsOnlyOnUnlockAndDisablesSecondTap() = runTest {
        for (household in listOf(false, true)) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val visible = mutableStateOf(true)
        var chosenWrite: (suspend (ExportSink) -> Unit)? = null
        var completed: ((Result<Boolean>) -> Unit)? = null
        var calls = 0
        var finished = false
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {}
            override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
                assertTrue(visible.value, "Never download while gated")
                assertEquals(if (household) "/v1/admin/export/household" else "/v1/export/member", path)
                assertEquals(if (household) "fixture-step" else null, stepUp)
                calls++
                consume("export".encodeToByteArray())
            }
        }
        val screenSession = ScreenSession()
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex",if (household) "super_admin" else "user","#FABD2F","AX","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = screenSession; override suspend fun stepUp(password: String, action: String): String { assertEquals("household.export", action); return "fixture-step" } }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) = error("Not used")
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) {
                    check(chosenWrite == null) { "Double tap launched another dialog" }
                    chosenWrite = write; completed = onResult
                }
            }
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) {
                    if (visible.value) PlanningScreen(services, SettingsSection.Export, files) else Text("Locked")
                } } }
                if (household) {
                    onNodeWithContentDescription("Export household").performClick()
                } else onNodeWithContentDescription("Export my data").performClick()
                onNodeWithContentDescription(if (household) "Export household" else "Export my data").assertIsNotEnabled()
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { screenSession.writeScope.launch {
                    val sink = object : ExportSink {
                        override suspend fun write(bytes: ByteArray) {}
                        override suspend fun finish(checkGate: () -> Unit) { checkGate(); finished = true }
                        override suspend fun abort() {}
                    }
                    try { chosenWrite!!(sink); completed!!(Result.success(true)) }
                    catch (e: Exception) { completed!!(Result.failure(e)) }
                } }
                runOnIdle { assertEquals(0, calls); visible.value = true }
                if (household) {
                    waitUntil { onAllNodes(hasText("Password") and hasSetTextAction()).fetchSemanticsNodes().size == 1 }
                    onNode(hasText("Password") and hasSetTextAction()).performTextInput("fixture password")
                    onNodeWithContentDescription("Confirm household export").performClick()
                }
                // finish() precedes the platform completion callback; wait for its visible acknowledgement.
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Export saved").fetchSemanticsNodes().isNotEmpty() }
                assertTrue(finished)
                assertEquals(1, calls)
                onNodeWithText("Export saved").assertExists()
            }
        } finally { cache.close() }
        }
    }

}
