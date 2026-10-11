package app.porchlight.android

import android.content.Intent
import android.content.pm.ApplicationInfo
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/** Black-box instrumentation: never link against application/library methods that R8 may inline. */
@RunWith(AndroidJUnit4::class)
class AuditApkReleaseSmokeTest {
    @Test fun coldStartKeepsPinSerializersCacheWorkersAndCsrUsable() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        assertEquals("app.porchlight.minified", context.packageName)
        assertEquals(0, context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE)
        fun launch(name: String) = instrumentation.startActivitySync(Intent()
            .setClassName(context.packageName, "app.porchlight.android.$name")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        val main = launch("MainActivity")
        try {
            await("Production PIN gate did not render") {
                val root = instrumentation.uiAutomation.rootInActiveWindow
                root.containsText("Unlock Porchlight") && root.containsText("Unlock with device credential")
            }
            val probe = launch("AuditApkProbeActivity")
            try {
                var result: String? = null
                await("Minified runtime probe did not complete") {
                    instrumentation.runOnMainSync { result = probe.intent.getStringExtra("smoke-result") }
                    result != null
                }
                assertEquals("Minified runtime probe failed", "passed", result)
            } finally { instrumentation.runOnMainSync { probe.finish() } }
        } finally { instrumentation.runOnMainSync { main.finish() } }
    }

    private fun await(message: String, ready: () -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 15_000
        while (SystemClock.uptimeMillis() < deadline) {
            if (ready()) return
            SystemClock.sleep(100)
        }
        fail(message)
    }

    private fun AccessibilityNodeInfo?.containsText(expected: String): Boolean {
        if (this == null) return false
        if (text?.toString() == expected) return true
        return (0 until childCount).any { getChild(it).containsText(expected) }
    }
}
