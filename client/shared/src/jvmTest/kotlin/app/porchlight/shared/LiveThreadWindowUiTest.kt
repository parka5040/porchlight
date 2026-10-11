package app.porchlight.shared

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.*
import androidx.compose.ui.test.v2.runDesktopComposeUiTest
import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import app.porchlight.shared.screens.collaboration.CollaborationSection
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.transport.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.mock.*
import io.ktor.http.*
import kotlinx.coroutines.cancel
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.runTest
import java.util.concurrent.atomic.AtomicInteger
import kotlin.test.*
import kotlin.time.Instant
import kotlin.time.Duration.Companion.seconds

@OptIn(ExperimentalTestApi::class)
class LiveThreadWindowUiTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    private fun services(data: HouseholdData): AppServices = object : AppServices by FakeServices() {
        override val data = data
        override val clock = AppClock { now }
        override val screenSession = ScreenSession()
    }
    private fun transport(handler: suspend MockRequestHandleScope.(io.ktor.client.request.HttpRequestData) -> io.ktor.client.request.HttpResponseData) =
        KtorApiTransport(HttpClient(MockEngine(handler)), listOf(testOrigin()), HeaderPolicy(ClientInfo("1.0.0"), { "UTC" }, { "unused" }), { false })

    @Test fun openThreadUsesLiveCacheTailAndShowOlderDoesNotFetchCachedHistory() = runTest {
        val cache = memoryCache { now }
        val commentRequests = AtomicInteger()
        val api = transport { req ->
            if (req.url.encodedPath.endsWith("comments")) commentRequests.incrementAndGet()
            respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
        val services = services(data)
        try {
            val comments = List(120) { commentFixture().copy(id = "comment-$it", body = "Message $it", createdAt = now + it.seconds) }
            val item = itemRead(collabId("item")).copy(capabilities = listOf("view"))
            cache.install(emptySnapshot(1).copy(items = listOf(item), comments = comments))
            runDesktopComposeUiTest(width = 700, height = 900) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(item) {} }
                } } }
                onNodeWithText("Message 119").assertExists()
                onNodeWithText("Message 69").assertDoesNotExist()
                onNodeWithContentDescription("Show older").performScrollTo().performClick()
                onNodeWithText("Message 20").assertExists()
                onNodeWithText("Message 19").assertDoesNotExist()
                // Inject on the test driver thread, never block the Compose UI thread.
                runBlocking { cache.putComments(listOf(commentFixture().copy(id = "synced", body = "Live from sync", createdAt = now + 121.seconds))) }
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText("Live from sync").fetchSemanticsNodes().isNotEmpty() }
                onNodeWithText("Message 20").assertDoesNotExist() // expanded budget stays at 100
                onNodeWithText("Message 21").assertExists()
                onNodeWithContentDescription("Show latest").performScrollTo().performClick()
                onNodeWithText("Message 70").assertDoesNotExist()
                onNodeWithText("Message 71").assertExists()
                onNodeWithText("Live from sync").assertExists()
                runOnIdle { assertEquals(0, commentRequests.get()) }
            }
        } finally { services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }

    @Test fun thumbnailsAreDemandLoadedAndOlderLoadDoesNotClearAttachmentError() = runTest {
        val cache = memoryCache { now }
        val thumbnails = AtomicInteger(); val pages = AtomicInteger()
        val api = transport { req ->
            when {
                req.url.encodedPath.endsWith("/thumbnail") -> { thumbnails.incrementAndGet(); respond(byteArrayOf(), headers = headersOf(HttpHeaders.ContentType, "image/png")) }
                req.url.encodedPath.endsWith("/comments") -> { pages.incrementAndGet(); respond(WireJson.encodeToString(CommentPage(emptyList(), null)), headers = headersOf(HttpHeaders.ContentType, "application/json")) }
                req.url.encodedPath.contains("/attachments/") -> respond("", HttpStatusCode.ServiceUnavailable)
                else -> respond("""{"items":[],"next_cursor":null}""", headers = headersOf(HttpHeaders.ContentType, "application/json"))
            }
        }
        val data = HouseholdData(cache, GeneratedCall({ api }, { "session" }), { now })
        val services = services(data)
        try {
            cache.putAttachment(attachmentFixture().copy(hasThumbnail = true))
            val item = itemRead(collabId("item")).copy(capabilities = listOf("view"))
            runDesktopComposeUiTest(width = 700, height = 900) {
                setContent { CompositionLocalProvider(LocalScreenServices provides services) { PorchlightTheme(ThemeMode.Daylight, false) {
                    Column(Modifier.verticalScroll(rememberScrollState())) { CollaborationSection(item) {} }
                } } }
                onNodeWithContentDescription("Show thumbnail").performScrollTo().assertExists()
                runOnIdle { assertEquals(0, thumbnails.get()); assertEquals(0, pages.get()) }
                onNodeWithContentDescription("Show thumbnail").performClick()
                waitUntil(timeoutMillis = 10_000) { thumbnails.get() == 1 }
                onNodeWithContentDescription("Hide thumbnail").performClick()
                onNodeWithContentDescription("Open notes.txt").performScrollTo().performClick()
                val error = "Attachment could not open. Refetch or unlock before retrying."
                waitUntil(timeoutMillis = 10_000) { onAllNodesWithText(error).fetchSemanticsNodes().isNotEmpty() }
                onNodeWithContentDescription("Show older").performScrollTo().performClick()
                waitUntil(timeoutMillis = 10_000) { pages.get() == 1 }
                onNodeWithText(error).assertExists()
                runOnIdle { assertEquals(1, thumbnails.get()) }
            }
        } finally { services.screenSession!!.writeScope.cancel(); api.close(); cache.close() }
    }
}
