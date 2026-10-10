package app.porchlight.android

import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.shared.screens.collaboration.*
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File

/**
 * fbcc m6c-astra C5-06 on a device: the real helper Activity reads a returned document and delivers it to the session
 * inbox although the composition that launched the picker was discarded first (as the ON_STOP PIN relock does).
 * The external picker is replaced by an instrumentation monitor; nothing leaves the app's private cache.
 */
class AttachmentPickRelockInstrumentedTest {
    @get:Rule val compose = createComposeRule()

    @Test fun pickReturningAfterTheLaunchingCompositionIsGoneIsKeptForItsTarget() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val file = File(File(context.cacheDir, "attachments").apply { mkdirs() }, "relock-pick.txt").apply { writeText("picked after relock") }
        val uri = Uri.parse("content://${context.packageName}.attachments/attachments/${file.name}")
        val monitor = object : Instrumentation.ActivityMonitor() {
            override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? =
                if (intent.action == Intent.ACTION_OPEN_DOCUMENT) Instrumentation.ActivityResult(Activity.RESULT_OK, Intent().setData(uri)) else null
        }
        instrumentation.addMonitor(monitor)
        val picks = AttachmentPicks()
        val shown = mutableStateOf(true)
        var actions: AttachmentActions? = null
        try {
            compose.setContent { if (shown.value) actions = rememberAttachmentActions(picks, "item") {} }
            // Launch, then discard the launching composition before the helper can deliver anything.
            compose.runOnIdle { actions!!.file("comment"); shown.value = false }
            compose.waitUntil(timeoutMillis = 15_000) { picks.waiting("item").isNotEmpty() }
            val outcome = picks.waiting("item").single()
            assertEquals("comment", outcome.request.commentId)
            assertNull(outcome.error)
            assertEquals("picked after relock", outcome.picked!!.bytes.decodeToString())
            assertEquals("text/plain", outcome.picked!!.mime)
        } finally {
            instrumentation.removeMonitor(monitor)
            file.delete()
        }
    }
}
