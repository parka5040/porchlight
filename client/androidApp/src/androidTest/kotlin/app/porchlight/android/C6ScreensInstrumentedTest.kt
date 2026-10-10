package app.porchlight.android

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.text.BasicText as Text
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.unit.Density
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.shared.files.*
import app.porchlight.shared.screens.planning.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import app.porchlight.shared.ui.ShellButton
import org.junit.After
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test

class C6ScreensInstrumentedTest {
    @get:Rule val compose = createComposeRule()
    /** Stubs only the SAF system activity. An IntentFilter(action) monitor would also match the action-less explicit
     * CalendarDocumentActivity launch (IntentFilter passes intents without an action) and swallow the helper itself. */
    private class SafResultMonitor(private val action: String, private val document: Uri) : Instrumentation.ActivityMonitor() {
        @Volatile var launched: Intent? = null
        override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? {
            if (intent.action != action) return null
            launched = Intent(intent)
            return Instrumentation.ActivityResult(Activity.RESULT_OK, Intent().setData(document))
        }
    }
    /** The dialog slot is process-wide; a failed test must not leave it owned for the next one. */
    @After fun releaseFileDialog() = InstrumentationRegistry.getInstrumentation().runOnMainSync {
        CalendarDocumentActivity.requests.current?.let(CalendarDocumentActivity.requests::discard)
    }
    @Test fun bothThemesDoubleScaleNamedActions() {
        var screen by mutableIntStateOf(0)
        var theme by mutableStateOf(ThemeMode.Evening)
        val labels = listOf("Choose ICS file", "Add feed", "Add weekly window", "Request suggestions", "Export my data", "Refresh devices")
        compose.setContent { CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) { PorchlightTheme(theme,false) {
            when (screen) {
                0 -> ImportContent()
                1 -> FeedContent()
                2 -> AvailabilityContent()
                3 -> PlanningCardsContent()
                4 -> ExportContent()
                else -> DeviceAdminContent()
            }
        } } }
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) for (index in labels.indices) {
            compose.runOnIdle { theme = mode; screen = index }
            val label = labels[index]
            compose.onNodeWithContentDescription(label).performScrollTo().assertExists()
            val node = compose.onNodeWithContentDescription(label).fetchSemanticsNode()
            val density = InstrumentationRegistry.getInstrumentation().targetContext.resources.displayMetrics.density
            assertTrue(node.size.width / density >= 48f)
            assertTrue(node.size.height / density >= 48f)
        }
    }
    /** Production SAF intent/helper/parser with a deterministic system-activity result and debug-only document. */
    @Test fun safCalendarResultReadsOnlyTheChosenDocument() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val uri = Uri.parse("content://${instrumentation.targetContext.packageName}.calendar-fixture/fixture.ics")
        val monitor = SafResultMonitor(Intent.ACTION_OPEN_DOCUMENT, uri).also(instrumentation::addMonitor)
        var result by mutableStateOf<PickedCalendar?>(null)
        compose.setContent {
            val files = rememberCalendarFiles()
            ShellButton("Pick fixture", onClick = { files.pick { result = it.getOrThrow() } })
            result?.let { Text("Picked ${it.name}") }
        }
        try {
            compose.onNodeWithContentDescription("Pick fixture").performClick()
            compose.waitUntil(timeoutMillis = 10_000) { result != null }
            compose.onNodeWithText("Picked fixture.ics").assertExists()
            assertEquals(1, monitor.hits)
            assertTrue(monitor.launched!!.hasCategory(Intent.CATEGORY_OPENABLE))
            assertTrue(result!!.content.contains("BEGIN:VCALENDAR"))
            assertTrue(calendarDocumentIntent(false).hasCategory(Intent.CATEGORY_OPENABLE))
            assertEquals(Intent.ACTION_CREATE_DOCUMENT, calendarDocumentIntent(true).action)
        } finally { instrumentation.removeMonitor(monitor) }
    }
    @Test fun safPrePublishFailureDoesNotDeleteConfirmedOverwriteDocument() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val uri = Uri.parse("content://${context.packageName}.calendar-fixture/existing.json")
        context.contentResolver.openOutputStream(uri, "wt")!!.use { it.write("user data".toByteArray()) }
        val monitor = SafResultMonitor(Intent.ACTION_CREATE_DOCUMENT, uri).also(instrumentation::addMonitor)
        var replied by mutableStateOf(false)
        var failed by mutableStateOf(false)
        compose.setContent {
            val files = rememberCalendarFiles()
            ShellButton("Save fixture", onClick = { files.save(false, { throw IllegalStateException("before publish") }) { failed = it.isFailure; replied = true } })
        }
        try {
            compose.onNodeWithContentDescription("Save fixture").performClick()
            compose.waitUntil(timeoutMillis = 10_000) { replied }
            assertTrue(failed)
            assertEquals(1, monitor.hits)
            assertEquals("application/json", monitor.launched!!.type)
            assertEquals("user data", context.contentResolver.openInputStream(uri)!!.bufferedReader().use { it.readText() })
        } finally { instrumentation.removeMonitor(monitor) }
    }

}
