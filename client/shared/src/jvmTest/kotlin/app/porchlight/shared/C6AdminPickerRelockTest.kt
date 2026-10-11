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
import kotlinx.coroutines.awaitCancellation
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class C6AdminPickerRelockTest {
    @Test fun adminSelectionThenSectionSwitchImportRelockCompletesOnce() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        var previewCalls = 0
        var callback: ((Result<PickedCalendar?>) -> Unit)? = null
        val visible = mutableStateOf(true)
        val section = mutableStateOf(SettingsSection.Availability)
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): JsonObject {
                if (path.contains("availability")) return buildJsonObject { put("member_id", path.substringAfterLast("/")); put("version", 1); put("windows", buildJsonArray {}) }
                assertTrue(visible.value, "Never preview while gated")
                assertEquals(true, body!!["preview"]!!.jsonPrimitive.boolean)
                previewCalls++
                return WireJson.parseToJsonElement("""{"created":1,"updated":0,"unchanged":0,"removed":0,"deleted":0,"skipped":0,"warnings":[],"item_ids":[],"conflicts":[],"preview":true,"removal_suppressed":false}""").jsonObject
            }
        }
        try {
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex","super_admin","#FABD2F","AX","UTC","manual",1), MemberRead(false,"01999999-0000-7000-8000-000000000002","01999999-0000-7000-8000-000000000006","Sam","dependent","#FABD2F","SM","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = ScreenSession() }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) { callback = onResult }
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) = error("Not used")
            }
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) {
                    if (visible.value) PlanningScreen(services, section.value, files) else Text("Locked")
                } } }
                onNodeWithText("Sam").performClick().assertIsSelected()
                runOnIdle { section.value = SettingsSection.CalendarData }
                onNodeWithContentDescription("Choose ICS file").performClick()
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { callback!!(Result.success(PickedCalendar("school.ics", "BEGIN:VCALENDAR\r\nEND:VCALENDAR"))) }
                assertEquals(0, previewCalls)
                runOnIdle { visible.value = true }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithContentDescription("Commit import").fetchSemanticsNodes().isNotEmpty() }
                assertEquals(1, previewCalls)
                onNodeWithText("school.ics").assertExists()
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { visible.value = true }
                onNodeWithText("school.ics").assertExists()
                assertEquals(1, previewCalls)
            }
        } finally { cache.close() }
    }
    @Test fun adminSelectionThenSectionSwitchExportRelockCompletesOnce() = runTest {
        for (household in listOf(false, true)) {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val visible = mutableStateOf(true)
        val section = mutableStateOf(SettingsSection.Availability)
        var chosenWrite: (suspend (ExportSink) -> Unit)? = null
        var completed: ((Result<Boolean>) -> Unit)? = null
        var warned: ((String) -> Unit)? = null
        var calls = 0
        var finished = false
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = if (path.contains("availability")) buildJsonObject { put("member_id", path.substringAfterLast("/")); put("version", 1); put("windows", buildJsonArray {}) } else buildJsonObject {}
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
            cache.install(emptySnapshot(1).copy(memberSettings = emptySnapshot(1).memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex","super_admin","#FABD2F","AX","UTC","manual",1), MemberRead(false,"01999999-0000-7000-8000-000000000002","01999999-0000-7000-8000-000000000006","Sam","dependent","#FABD2F","SM","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = screenSession; override suspend fun stepUp(password: String, action: String): String { assertEquals("household.export", action); return "fixture-step" } }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) = error("Not used")
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) =
                    save(household, write, {}, onResult)
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onWarning: (String) -> Unit, onResult: (Result<Boolean>) -> Unit) {
                    check(chosenWrite == null) { "Double tap launched another dialog" }
                    chosenWrite = write; completed = onResult; warned = onWarning
                }
            }
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) {
                    if (visible.value) PlanningScreen(services, section.value, files) else Text("Locked")
                } } }
                onNodeWithText("Sam").performClick().assertIsSelected()
                runOnIdle { section.value = SettingsSection.Export }
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
                    try { chosenWrite!!(sink); if (household) warned!!(EXPORT_NOT_PRIVATE_WARNING); completed!!(Result.success(true)) }
                    catch (e: Exception) { completed!!(Result.failure(e)) }
                } }
                runOnIdle { assertEquals(0, calls); visible.value = true }
                if (household) {
                    waitUntil { onAllNodes(hasText("Password") and hasSetTextAction()).fetchSemanticsNodes().size == 1 }
                    onNode(hasText("Password") and hasSetTextAction()).performTextInput("fixture password")
                    onNodeWithContentDescription("Confirm household export").performClick()
                }
                waitUntil(timeoutMillis = 10_000) { finished }
                assertEquals(1, calls)
                onNodeWithText(if (household) EXPORT_NOT_PRIVATE_WARNING else "Export saved").assertExists()
                runOnIdle { visible.value = false }
                onNodeWithText("Locked").assertExists()
                runOnIdle { visible.value = true }
                onNodeWithText(if (household) EXPORT_NOT_PRIVATE_WARNING else "Export saved").assertExists()
                assertEquals(1, calls)
            }
        } finally { cache.close() }
        }
    }
    @Test fun navigationInterruptsActiveExportWithNeutralMessage() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val cache = memoryCache { now }
        val section = mutableStateOf(SettingsSection.Export)
        var downloads = 0
        var aborted = 0
        val session = ScreenSession()
        val transport = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) = buildJsonObject {
                put("member_id", path.substringAfterLast("/")); put("version", 1); put("windows", buildJsonArray {})
            }
            override suspend fun download(path: String, session: String?, stepUp: String?, consume: suspend (ByteArray) -> Unit) {
                downloads++
                awaitCancellation()
            }
        }
        try {
            val snapshot = emptySnapshot(1)
            cache.install(snapshot.copy(memberSettings = snapshot.memberSettings.copy(memberId = "01999999-0000-7000-8000-000000000001"), members = listOf(MemberRead(false,"01999999-0000-7000-8000-000000000001","01999999-0000-7000-8000-000000000006","Alex","super_admin","#FABD2F","AX","UTC","manual",1))))
            val data = HouseholdData(cache, GeneratedCall({ transport }, { "session" }), { now })
            val services = object : AppServices by FakeServices() { override val data = data; override val screenSession = session }
            val files = object : CalendarFiles {
                override fun pick(onResult: (Result<PickedCalendar?>) -> Unit) = error("Not used")
                override fun save(household: Boolean, write: suspend (ExportSink) -> Unit, onResult: (Result<Boolean>) -> Unit) {
                    session.writeScope.launch {
                        val sink = object : ExportSink {
                            override suspend fun write(bytes: ByteArray) = Unit
                            override suspend fun finish(checkGate: () -> Unit) = error("Interrupted export must not publish")
                            override suspend fun abort() { aborted++ }
                        }
                        try { write(sink); onResult(Result.success(true)) }
                        catch (e: Exception) { onResult(Result.failure(e)) }
                    }
                }
            }
            runDesktopComposeUiTest(width = 700, height = 1600) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight,false) { PlanningScreen(services, section.value, files) } } }
                onNodeWithContentDescription("Export my data").performClick()
                waitUntil(timeoutMillis = 10_000) { downloads == 1 }
                runOnIdle { section.value = SettingsSection.Availability }
                waitUntil(timeoutMillis = 10_000) { aborted == 1 }
                runOnIdle { section.value = SettingsSection.Export }
                onNodeWithText("Export interrupted before completion. Try again.").assertExists()
                onNodeWithContentDescription("Export my data").assertIsEnabled()
                assertEquals(1, downloads)
            }
        } finally { cache.close() }
    }

}
