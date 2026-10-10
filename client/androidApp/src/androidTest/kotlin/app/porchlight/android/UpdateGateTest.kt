package app.porchlight.android

import android.accessibilityservice.AccessibilityServiceInfo
import android.content.Context
import android.content.Intent
import android.os.Build
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.android.trust.ControllerOnly
import app.porchlight.shared.BuildInfo
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.trust.TrustState
import app.porchlight.shared.update.UpdateState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import org.json.JSONObject
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Fresh disposable emulator only. C4-style run-as stdin JSON; argv contains only a fixed private filename. */
@ControllerOnly
@SdkSuppress(minSdkVersion = 31)
@RunWith(AndroidJUnit4::class)
class UpdateGateTest {
    private fun dumpWindowHierarchy(): List<AccessibilityNodeInfo> {
        val root = InstrumentationRegistry.getInstrumentation().uiAutomation.rootInActiveWindow ?: return emptyList()
        val nodes = mutableListOf<AccessibilityNodeInfo>()
        fun visit(node: AccessibilityNodeInfo) {
            nodes.add(node)
            for (index in 0 until node.childCount) node.getChild(index)?.let(::visit)
        }
        visit(root)
        return nodes
    }

    @Test fun enrollCheckAndCommitUpdateForHostConfirmation() {
        check(Build.VERSION.SDK_INT >= 31)
        check(BuildInfo.VERSION == "9.9.8") { "Update gate requires the scratch 9.9.8 debug build" }
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val automation = instrumentation.uiAutomation
        automation.serviceInfo = automation.serviceInfo.apply { flags = flags or AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS }
        val context = instrumentation.targetContext
        val filename = InstrumentationRegistry.getArguments().getString("UPDATE_GATE_INPUT")
        check(filename == "update-gate.json") { "Required private update gate input missing" }
        val input = File(context.filesDir, filename)
        val config = try { JSONObject(input.readText()) } finally { input.delete() }
        check(config.getString("version") == "9.9.9")
        runBlocking(Dispatchers.IO) {
            withTimeout(180_000) {
                val services = AndroidAppServices.get(context)
                val controller = services.trust.controller
                controller.ensureRestored()
                controller.state.first { it != TrustState.Loading }
                check(controller.state.value == TrustState.Enrollment) { "Fresh unenrolled update gate device required" }
                controller.enroll(config.getString("uri"), config.getString("password"))
                check(controller.state.value == TrustState.PinRequired) { "Scratch enrollment did not finish" }
                // The existing debug-only Compose test activity keeps this UID foreground without a device-PIN
                // dialog covering the installer. No household UI/content is unlocked; updater/receiver are real.
                instrumentation.startActivitySync(Intent(context, androidx.activity.ComponentActivity::class.java)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                check(context.packageManager.canRequestPackageInstalls()) { "Controller must grant unknown-app installation first" }
                val updates = services.updates
                updates.check().join()
                val available = updates.state.first { it is UpdateState.Available || it is UpdateState.Failed || it is UpdateState.Unavailable }
                check(available is UpdateState.Available && available.release.version == "9.9.9") { "Expected Available(9.9.9), got $available" }
                var update = updates.update()
                while (update == null && updates.state.value is UpdateState.Available) {
                    delay(100)
                    update = updates.update()
                }
                checkNotNull(update) { "Update did not start" }.join()
                if (updates.state.value is UpdateState.Failed) {
                    // Diagnose the swallowed failure: repeat the platform step directly and surface its stack trace.
                    val failure = runCatching {
                        app.porchlight.shared.update.AndroidUpdater.get(context)
                            .install(available.release, app.porchlight.shared.update.TrustedUpdateSource(controller)) {}
                    }.exceptionOrNull()
                    throw IllegalStateException("Update failed: ${updates.state.value}; direct install: " +
                        (failure?.stackTraceToString()?.take(4000) ?: "succeeded on retry"))
                }
                check(updates.state.value == UpdateState.Installing) { "Update did not reach PackageInstaller: ${updates.state.value}" }
                val prefs = context.getSharedPreferences("update-install", Context.MODE_PRIVATE)
                // Poll the outcome: the system confirmation must appear; an installer failure arrives asynchronously.
                withTimeout(45_000) {
                    while (dumpWindowHierarchy().none { node ->
                        node.packageName?.toString() in setOf("com.android.packageinstaller", "com.google.android.packageinstaller") &&
                            node.viewIdResourceName in setOf("android:id/button1", "com.android.packageinstaller:id/ok_button") &&
                            node.isEnabled && node.isClickable && node.text?.toString()?.lowercase() in setOf("install", "update")
                    }) {
                        check(prefs.getString("failure", null) == null) { "Installer failed: ${prefs.getString("failure", null)}" }
                        check(updates.state.value == UpdateState.Installing) { "Installer confirmation failed: ${updates.state.value}" }
                        val id = prefs.getInt("session", -1)
                        check(id >= 0) { "Installer session ended without confirmation: ${updates.state.value}" }
                        val session = context.packageManager.packageInstaller.getSessionInfo(id)
                        check(session != null && session.isCommitted) { "PackageInstaller session $id not committed (info=${session != null})" }
                        delay(250)
                    }
                }
                // Do not confirm here: instrumentation must return before the package replacement kills its target.
            }
        }
    }
}
