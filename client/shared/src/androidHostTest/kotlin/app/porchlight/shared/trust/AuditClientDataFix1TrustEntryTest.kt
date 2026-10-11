package app.porchlight.shared.trust

import app.porchlight.shared.settings.PrivateMetadataStore
import app.porchlight.shared.transport.*
import java.io.File
import java.io.IOException
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditClientDataFix1TrustEntryTest {
    @Test fun constructionSurvivesFailedMigrationAndMalformedTrustAndRestoreCanRetry() = runTest {
        for (malformed in listOf(false, true)) {
            val dir = File(System.getProperty("porchlight.test.dir"), "audit-trust-entry-$malformed").apply { mkdirs() }
            val file = dir.resolve("trust.json")
            try {
                if (malformed) file.writeText("broken metadata")
                var failMigration = !malformed
                var cleared = false
                val holder = AppTrustHolder({
                    AndroidDeviceKeyStore("fixture-device", "fixture-trust") {
                        PrivateMetadataStore(file, {
                            if (failMigration) throw IOException("storage temporarily unavailable")
                            emptyMap<String, String>()
                        }, { cleared = true })
                    }
                }) { keys ->
                    TrustController(keys, object : TrustTransportFactory {
                        override fun enrollment(uri: EnrollUri): ApiTransport = error("not enrolled")
                        override suspend fun authenticated(trust: DeviceTrust): ApiTransport = error("not enrolled")
                    }, "android", { Instant.parse("2026-10-06T12:00:00Z") }, true)
                }
                val controller = holder.controller // the same entry boundary used by the Activity
                try {
                    controller.restore()
                    assertEquals(TrustState.LocalTrustLost(true), controller.state.value)
                    assertFalse(cleared, "failed load must preserve legacy state")
                    if (malformed) assertEquals("broken metadata", file.readText())
                    failMigration = false
                    if (malformed) file.writeText("{}") // explicit storage repair, never an automatic reset
                    controller.restore()
                    assertEquals(TrustState.Enrollment, controller.state.value)
                    assertTrue(cleared)
                } finally { controller.close() }
            } finally { dir.deleteRecursively() }
        }
    }
}
