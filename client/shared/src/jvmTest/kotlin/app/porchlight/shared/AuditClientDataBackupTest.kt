package app.porchlight.shared

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.w3c.dom.Element
import kotlin.test.*

class AuditClientDataBackupTest {
    @Test fun mergedManifestExcludesEveryBackupAndTransferDomain() {
        val app = File("../androidApp")
        val manifest = app.resolve("build/intermediates/merged_manifests/debug/processDebugManifest/AndroidManifest.xml")
        assertTrue(manifest.isFile, "assembleDebug must generate merged manifest")
        val factory = DocumentBuilderFactory.newInstance().apply { isNamespaceAware = true }
        val application = factory.newDocumentBuilder().parse(manifest).getElementsByTagName("application").item(0) as Element
        val ns = "http://schemas.android.com/apk/res/android"
        assertEquals("false", application.getAttributeNS(ns, "allowBackup"))
        fun rules(attribute: String): Element {
            val ref = application.getAttributeNS(ns, attribute)
            assertTrue(ref.startsWith("@xml/"), "merged manifest missing $attribute")
            return factory.newDocumentBuilder().parse(app.resolve("build/intermediates/packaged_res/debug/packageDebugResources/xml/${ref.substringAfter('/')}.xml")).documentElement
        }
        val domains = setOf("root", "file", "database", "sharedpref", "external", "device_root", "device_file", "device_database", "device_sharedpref")
        fun check(parent: Element) {
            val nodes = parent.getElementsByTagName("exclude")
            assertEquals(domains, (0 until nodes.length).map { (nodes.item(it) as Element).getAttribute("domain") }.toSet())
            repeat(nodes.length) { assertEquals(".", (nodes.item(it) as Element).getAttribute("path")) }
        }
        check(rules("fullBackupContent"))
        val extraction = rules("dataExtractionRules")
        for (branch in listOf("cloud-backup", "device-transfer")) check(extraction.getElementsByTagName(branch).item(0) as Element)
    }
}
