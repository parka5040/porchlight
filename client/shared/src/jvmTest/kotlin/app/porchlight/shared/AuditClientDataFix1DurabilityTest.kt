package app.porchlight.shared

import app.porchlight.shared.settings.*
import java.io.IOException
import java.nio.file.*
import kotlin.test.*

class AuditClientDataFix1DurabilityTest {
    @Test fun migrationAndCommitsRequireDurableDirectoryBeforeAcknowledgement() {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-durable-").toFile()
        try {
            val file = dir.resolve("private/metadata.json")
            var legacy: Map<String, Any> = mapOf("nextCode" to 71, "encrypted" to "opaque fixture")
            var fail = true
            var forced = false
            val force: (java.io.File) -> Unit = { directory ->
                if (directory == file.parentFile && file.exists()) {
                    assertEquals("71", app.porchlight.shared.transport.WireJson.decodeFromString<Map<String, String>>(file.readText())["nextCode"])
                    if (fail) throw IOException("directory force unavailable")
                    forced = true
                }
            }
            assertFailsWith<IOException>("migration must force the renamed directory before legacy cleanup") {
                PrivateMetadataStore(file, { legacy }, { legacy = emptyMap() }, force)
            }
            assertEquals(2, legacy.size)
            fail = false
            val store = PrivateMetadataStore(file, { error("authoritative replacement exists") }, {
                assertTrue(forced, "retry must force an existing replacement before clearing legacy")
                legacy = emptyMap()
            }, force)
            assertTrue(legacy.isEmpty())
            fail = true
            assertFailsWith<IOException> { store.edit().putString("owner", "fixture").commit() }
            assertNull(store.getString("owner", null), "failed durability must not acknowledge the commit")
            assertEquals(71, store.getInt("nextCode", 0))
        } finally { dir.deleteRecursively() }
    }

    @Test fun settingsKeepLegacyUntilDestinationDirectoryIsDurable() {
        val dir = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "audit-settings-").toFile()
        try {
            val old = dir.resolve("old.preferences_pb").apply { writeText("settings fixture") }
            val target = dir.resolve("private/settings.preferences_pb")
            assertFailsWith<IOException>("settings relocation must force its destination") {
                migratePrivateSettings(old, target) { throw IOException("directory force unavailable") }
            }
            assertEquals("settings fixture", old.readText())
            assertEquals("settings fixture", migratePrivateSettings(old, target).readText())
            assertFalse(old.exists())
        } finally { dir.deleteRecursively() }
    }
}
