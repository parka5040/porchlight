package app.porchlight.shared

import app.porchlight.shared.backups.*
import app.porchlight.shared.api.models.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import java.nio.file.*
import kotlin.time.Instant
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PrerolloutDesktopBackupsTest {
    private class Source(var admin: Boolean) : app.porchlight.shared.trust.TrustTransportFactory, app.porchlight.shared.transport.ApiTransport {
        @Volatile var requests = 0
        override fun enrollment(uri: app.porchlight.shared.trust.EnrollUri) = this
        override suspend fun authenticated(trust: app.porchlight.shared.trust.DeviceTrust) = this
        override suspend fun request(method: String, path: String, body: kotlinx.serialization.json.JsonObject?, session: String?, idempotencyKey: String?, version: Long?, stepUp: String?): kotlinx.serialization.json.JsonObject {
            requests++
            val json = when (path) {
                "/v1/me" -> """{"member_id":"member","user_id":"user","username":"member","display_name":"Sam Rivera","role":"${if (admin) "super_admin" else "member"}","household_id":"household","device":{"id":"device","label":"Desktop","platform":"desktop"},"session_expires_at":"2028-10-06T12:00:00Z","connection":{"transport":"lan"}}"""
                "/v1/backups" -> "{}"
                else -> error("Unexpected backup request")
            }
            return app.porchlight.shared.transport.WireJson.parseToJsonElement(json) as kotlinx.serialization.json.JsonObject
        }
    }
    @Test fun idleMemberHourMakesZeroRequests() = idle(false)
    @Test fun idleAdminHourWithNothingDueMakesZeroRequests() = idle(true)
    private fun idle(admin: Boolean) = runTest {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "scheduled-pull-")
        val origin = Instant.parse("2026-10-06T05:00:00Z")
        val source = Source(admin)
        var clock = origin
        val controller = app.porchlight.shared.trust.TrustController(AuditFinalClientCFixture().keys, source, "desktop", { clock }, false)
        controller.restore()
        val pull = PassivePull(root, TrustedBackupSource(controller), { clock }, zone = { java.time.ZoneId.of("UTC") })
        try {
            if (admin) { pull.pull(); assertNull(pull.state.value.error); assertEquals(1, root.toFile().walk().count { it.name == ".next-pull" }); pull.resetView() }
            source.requests = 0
            val job = pull.start(backgroundScope, kotlinx.coroutines.flow.MutableStateFlow("household".takeIf { admin }))
            repeat(60) {
                runCurrent()
                withContext(Dispatchers.IO) { Thread.sleep(10) }
                clock = Instant.fromEpochSeconds(clock.epochSeconds + 60)
                advanceTimeBy(60_000)
            }
            runCurrent()
            withContext(Dispatchers.IO) { Thread.sleep(20) }
            assertEquals(0, source.requests, "An idle hour must not poll authorization")
            assertEquals(admin, pull.state.value.allowed, "Cached admin eligibility must keep manual backup actions available without polling")
            job.cancelAndJoin()
        } finally { controller.close(); root.toFile().deleteRecursively() }
    }
}
