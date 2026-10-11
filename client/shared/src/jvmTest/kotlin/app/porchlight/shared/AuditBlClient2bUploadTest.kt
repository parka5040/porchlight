package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant

@OptIn(ExperimentalTestApi::class)
class AuditBlClient2bUploadTest {
    @Test fun retriedUploadRetainsProgressAndCancellationAcrossRecomposition() = runTest {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val attempts = AtomicInteger()
        val cancelled = CompletableDeferred<Unit>()
        val keys = mutableListOf<String>()
        val api = object : ApiTransport {
            override suspend fun request(method: String, path: String, body: JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?) =
                buildJsonObject { put("items", JsonArray(emptyList())); put("next_cursor", JsonNull) }
            override suspend fun upload(path: String, name: String, mime: String, bytes: ByteArray, session: String?, key: String, progress: (Long, Long) -> Unit): JsonObject {
                keys += key
                if (attempts.incrementAndGet() == 1) throw ConnectFailure()
                progress(2, 4)
                try { awaitCancellation() } finally { cancelled.complete(Unit) }
            }
        }
        val cache = memoryCache { now }
        val data = HouseholdData(cache, GeneratedCall({ api }, { null }), { now })
        val session = ScreenSession()
        val services = object : AppServices by FakeServices() {
            override val data = data
            override val screenSession = session
            override val clock = AppClock { now }
        }
        try {
            val item = itemRead("item").copy(capabilities = listOf("view", "comment"))
            cache.install(emptySnapshot(1).copy(items = listOf(item)))
            runDesktopComposeUiTest(width = 320, height = 1500) {
                var generation by mutableIntStateOf(0)
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    key(generation) { Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(item) {} } }
                } } }
                waitForIdle()
                runOnIdle {
                    val request = session.attachmentPicks.begin(item.id, null)
                    session.attachmentPicks.complete(request.id, PickedAttachment("fixture.txt", "text/plain", byteArrayOf(1, 2, 3, 4)), null)
                }
                waitUntil { session.writes("collaboration:${item.id}").state.value.retryKey != null }
                onNodeWithContentDescription("Retry identical request").performScrollTo().performClick()
                waitUntil { attempts.get() == 2 }
                onNodeWithContentDescription("Cancel upload").assertExists()
                runOnIdle { generation++ }
                onNodeWithText("Uploading 2 / 4 bytes").assertExists()
                onNodeWithContentDescription("Cancel upload").performScrollTo().performClick()
                waitUntil { cancelled.isCompleted }
                assertEquals(2, keys.size)
                assertTrue(keys[0] == keys[1], "Retry must retain its request key")
            }
        } finally { session.writeScope.cancel(); data.close() }
    }
}
