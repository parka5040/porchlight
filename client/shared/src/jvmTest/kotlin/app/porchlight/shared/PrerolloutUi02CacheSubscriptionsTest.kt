package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class PrerolloutUi02CacheSubscriptionsTest {
    private val now = Instant.parse("2026-10-09T12:00:00Z")
    private val member = MemberRead(false, "self", "household", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
    private val row = itemRead("b").copy(title = "Item B", assignees = listOf("self"))

    private fun screen(test: ComposeUiTest.(ReadCache, HouseholdData, ScreenWrites, Map<String, Int>) -> Unit) {
        val cache = memoryCache { now }
        runBlocking { cache.install(emptySnapshot(1).copy(items = listOf(itemRead("a"), row),
            members = listOf(member, member.copy(id = "other", tzid = "Europe/Helsinki")))) }
        val data = HouseholdData(cache, GeneratedCall({ error("No requests expected") }, { "fixture-session" }), { now })
        val services = object : AppServices by FakeServices() { override val data = data }
        val writer = ScreenWrites().also { it.bindCache(cache) }
        val counts = mutableMapOf<String, Int>()
        try {
            runDesktopComposeUiTest(width = 600, height = 800) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services,
                    LocalScreenSupportCompositionObserver provides { key -> counts[key] = counts.getOrDefault(key, 0) + 1 }) {
                    PorchlightTheme(ThemeMode.Daylight) { Column {
                        Summary(cache)
                        WriteStatus(writer)
                        DataStatus(data)
                    } }
                } }
                waitForIdle()
                test(cache, data, writer, counts)
            }
        } finally { data.close() }
    }

    @Composable private fun Summary(cache: ReadCache) {
        val members by remember(cache) { cache.view.map { it.members }.distinctUntilChanged() }
            .collectAsState(cache.view.value.members)
        ItemSummary(row, members)
    }

    @Test fun itemAPublicationDoesNotRecomposeItemB() = screen { cache, _, _, counts ->
        val before = counts.getValue("summary:b")
        runOnIdle { runBlocking { cache.inSession { cache.putItem(it, itemResponse("a").copy(version = 2)) } } }
        waitForIdle()
        assertEquals(before, counts.getValue("summary:b"), "Item A must not recompose item B")
        runOnIdle { runBlocking { cache.inSession { cache.putSettings(it, cache.view.value.settings!!.copy(version = 2)) } } }
        waitForIdle()
        assertEquals(before, counts.getValue("summary:b"), "Unchanged member selection must not recompose B")

        runOnIdle { runBlocking { cache.inSession { cache.putMember(it, member.copy(version = 2, displayName = "Updated member", tzid = "America/New_York")) } } }
        waitForIdle()
        assertTrue(counts.getValue("summary:b") > before, "Member presentation changes must update B")
        onNodeWithText("Updated member", substring = true).assertExists()
        onNodeWithText("Fixed · displayed in America/New_York").assertExists()

        val afterMember = counts.getValue("summary:b")
        runOnIdle { runBlocking { cache.inSession { cache.putSettings(it, cache.view.value.settings!!.copy(memberId = "other", version = 3)) } } }
        waitForIdle()
        assertTrue(counts.getValue("summary:b") > afterMember, "Settings member changes must update B's zone")
        onNodeWithText("Fixed · displayed in Europe/Helsinki").assertExists()
    }

    @Test fun writeStatusIgnoresPayloadButUpdatesFeedbackAndVisibility() = screen { cache, _, writer, counts ->
        val before = counts.getValue("write")
        runOnIdle { runBlocking { cache.inSession { cache.putItem(it, itemResponse("a").copy(version = 2)) } } }
        waitForIdle()
        assertEquals(before, counts.getValue("write"), "Unrelated payload must not recompose write feedback")

        runOnIdle { runBlocking { writer.submit { WriteResult.Success(buildJsonObject {
            put("conflicts", buildJsonArray { add(buildJsonObject { put("other_title", "Private warning") }) })
        }, false) } } }
        waitForIdle()
        assertTrue(counts.getValue("write") > before)
        onNodeWithText("Saved").assertExists()
        onNodeWithText("Conflict warning: Private warning").assertExists()
        runOnIdle { runBlocking { cache.evictItem("b") } }
        waitForIdle()
        onNodeWithText("Conflict warning: Private warning").assertDoesNotExist()
        onNodeWithText("Saved").assertExists()
    }

    @Test fun dataStatusIgnoresPayloadButUpdatesSyncAvailability() = screen { cache, _, _, counts ->
        val before = counts.getValue("data")
        runOnIdle { runBlocking { cache.inSession { cache.putItem(it, itemResponse("a").copy(version = 2)) } } }
        waitForIdle()
        assertEquals(before, counts.getValue("data"), "Payload must not recompose sync availability")
        runOnIdle { runBlocking { cache.clear() } }
        waitForIdle()
        assertTrue(counts.getValue("data") > before)
        onNodeWithText("Waiting for initial sync…").assertExists()
    }
}
