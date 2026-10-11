package app.porchlight.shared.settings

import java.io.File
import java.io.IOException
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class AuditClientDataFix2StorageEntryTest {
    @Test fun activityStorageFailuresAreRecoverable() = entry(PrivateStorageEntry.Activity)
    @Test fun receiverStorageFailuresAreRecoverable() = entry(PrivateStorageEntry.Receiver)
    @Test fun workerStorageFailuresAreRecoverable() = entry(PrivateStorageEntry.Worker)
    @Test fun liveStorageFailuresAreRecoverable() = entry(PrivateStorageEntry.Live)
    @Test fun settingsStorageFailuresAreRecoverable() = entry(PrivateStorageEntry.Settings)
    private fun entry(entry: PrivateStorageEntry) = runTest {
        for (fault in listOf("alarm-ledger", "os-delivery", "migration", "settings-relocation")) {
            val dir = File(System.getProperty("porchlight.test.dir"), "audit-fix2-${entry.name}-$fault").apply { mkdirs() }
            val file = dir.resolve("private.json")
            val legacySettings = dir.resolve("legacy.preferences_pb")
            val targetSettings = dir.resolve("private/device.preferences_pb")
            val broken = fault == "alarm-ledger" || fault == "os-delivery"
            var fail = true
            var cleared = false
            val boundary = PrivateStorageBoundary()
            try {
                if (broken) file.writeText("broken private metadata")
                legacySettings.writeText("retained settings")
                suspend fun read(): String {
                    if (fault == "settings-relocation") migratePrivateSettings(legacySettings, targetSettings) {
                        if (fail) throw IOException("injected private directory failure")
                    }
                    return PrivateMetadataStore(file, { mapOf("nextCode" to 81) }, { cleared = true }, {
                        if (fail && fault == "migration") throw IOException("injected directory force failure")
                    }).getString("nextCode", null)!!
                }
                val shortEntry = entry == PrivateStorageEntry.Worker || entry == PrivateStorageEntry.Receiver
                if (shortEntry) {
                    val attempt = boundary.attempt(entry) { read() }
                    assertTrue(attempt.isFailure, "storage errors must return retry, not escape $entry")
                    assertEquals(PRIVATE_STORAGE_NOTICE, boundary.notice)
                } else {
                    var result: Result<String>? = null
                    val job = launch { result = runCatching {
                        if (entry == PrivateStorageEntry.Settings) boundary.observe(entry, flow { emit(read()) }).first()
                        else boundary.retry(entry) { read() }
                    } }
                    runCurrent()
                    assertNull(result, "$entry must retain its collector and retry")
                    assertEquals(PRIVATE_STORAGE_NOTICE, boundary.notice)
                    assertFalse(cleared)
                    if (broken) assertEquals("broken private metadata", file.readText())
                    if (fault == "settings-relocation") assertTrue(legacySettings.exists())
                    fail = false
                    if (broken) file.writeText("{\"nextCode\":\"81\"}")
                    advanceTimeBy(30_000); runCurrent(); job.join()
                    assertEquals("81", result!!.getOrThrow())
                }
                fail = false
                if (broken) file.writeText("{\"nextCode\":\"81\"}")
                assertEquals("81", boundary.attempt(entry) { read() }.getOrThrow())
                assertNull(boundary.notice)
                assertTrue(cleared)
            } finally { dir.deleteRecursively() }
        }
    }
}
