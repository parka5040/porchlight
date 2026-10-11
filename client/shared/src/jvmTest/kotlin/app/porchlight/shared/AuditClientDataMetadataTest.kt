package app.porchlight.shared

import app.porchlight.shared.settings.*
import java.nio.file.*
import kotlin.test.*

class AuditClientDataMetadataTest {
    @Test fun migrationPreservesHighWaterAndEncryptedTrustAndSurvivesInterruptedCleanup() {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-metadata-").toFile()
        try {
            val file = dir.resolve("private.json")
            var legacy: Map<String, Any> = mapOf("nextCode" to 71, "encrypted" to "opaque fixture", "owner" to "fixture owner")
            assertFailsWith<IllegalStateException> { PrivateMetadataStore(file, { legacy }, { error("interrupted cleanup") }) }
            assertTrue(file.exists()); assertEquals(3, legacy.size)
            val store = PrivateMetadataStore(file, { error("must not reload legacy") }, { legacy = emptyMap() })
            assertTrue(legacy.isEmpty()); assertEquals(71, store.getInt("nextCode", 1))
            assertEquals("opaque fixture", store.getString("encrypted", null))
            store.edit().putInt("nextCode", 72).remove("owner").commit()
            val reopened = PrivateMetadataStore(file, { emptyMap<String, String>() }, {})
            assertEquals(72, reopened.getInt("nextCode", 1)); assertFalse(reopened.contains("owner"))
            file.writeText("broken")
            assertFails { PrivateMetadataStore(file, { legacy }, {}) } // never reset a damaged high-water
            val old = dir.resolve("old.preferences_pb").apply { writeText("preserve settings") }
            val destination = dir.resolve("private/device.preferences_pb")
            assertEquals("preserve settings", migratePrivateSettings(old, destination).readText())
            assertFalse(old.exists())
        } finally { dir.deleteRecursively() }
    }
}
