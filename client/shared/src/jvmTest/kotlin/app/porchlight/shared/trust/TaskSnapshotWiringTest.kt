package app.porchlight.shared.trust

import java.nio.file.Path
import kotlin.test.*
import javax.xml.parsers.DocumentBuilderFactory

/** Manifest opts the entire app into lifecycle callbacks, not a per-activity source substring check.
 * Actual Android callback/window dispatch is additionally covered by TaskSnapshotInstrumentedTest. */
class TaskSnapshotWiringTest {
    @Test fun applicationPolicyCoversEveryDeclaredActivityIncludingAttachmentPicker() {
        val root = Path.of(System.getProperty("porchlight.test.dir")).parent.parent
        val factory = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
        val manifest = factory.newDocumentBuilder().parse(root.resolve("client/androidApp/src/main/AndroidManifest.xml").toFile())
        val android = "http://schemas.android.com/apk/res/android"
        val application = manifest.getElementsByTagName("application").item(0) as org.w3c.dom.Element
        assertEquals("app.porchlight.shared.trust.PorchlightApplication", application.getAttributeNS(android, "name"))
        assertEquals("false", application.getAttributeNS(android, "allowBackup"))
        val activities = manifest.getElementsByTagName("activity")
        val names = (0 until activities.length).map { (activities.item(it) as org.w3c.dom.Element).getAttributeNS(android, "name") }
        assertTrue(names.contains("app.porchlight.shared.screens.collaboration.AttachmentPickerActivity"))
        assertTrue(names.contains("app.porchlight.shared.trust.TrustCredentialActivity"))
        assertTrue(names.contains("app.porchlight.shared.onboarding.TrustNotificationPermissionActivity"))
        assertTrue(names.contains(".MainActivity"))
        for (name in names) for (api in listOf(30, 35, 36)) {
            var secured = false; var recentsDisabled = false
            val window = object : SnapshotWindow {
                override fun disableRecentsScreenshots() { recentsDisabled = true }
                override fun setSecure(secure: Boolean) { secured = secure }
            }
            val policy = TaskSnapshotPolicy(api)
            policy.apply(SnapshotLifecycle.CREATED, window)
            assertEquals(api >= 33, recentsDisabled, name)
            assertTrue(secured, name)
            policy.apply(SnapshotLifecycle.RESUMED, window)
            assertFalse(secured, name)
            policy.apply(SnapshotLifecycle.PAUSED, window)
            assertTrue(secured, "$name underneath a system sheet")
        }
    }
}
