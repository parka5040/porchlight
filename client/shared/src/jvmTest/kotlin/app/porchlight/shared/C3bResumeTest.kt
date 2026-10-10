package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.navigation.*
import kotlinx.coroutines.test.runTest
import java.io.File
import java.nio.file.Files
import kotlin.test.*
import kotlin.time.Instant

class C3bResumeTest {
    @Test fun cacheOpenFailureFailsClosedButCancellationPropagates() = runTest {
        for (failure in listOf(IllegalStateException("Unreadable cache"), kotlinx.coroutines.CancellationException("Canceled"))) {
            val fixture = FakeServices()
            val cache = ReadCache({ throw failure }, fixture.clock::now)
            val data = HouseholdData(cache, GeneratedCall({ error("No HTTP") }, { error("No credentials") }), fixture.clock::now)
            val services = object : AppServices {
                override val settings = fixture.settings
                override val clock = fixture.clock
                override val platform = fixture.platform
                override val data = data
            }
            if (failure is kotlinx.coroutines.CancellationException) assertFailsWith<kotlinx.coroutines.CancellationException> { services.routeAvailable(AppRoute.Item("saved")) }
            else assertFalse(services.routeAvailable(AppRoute.Item("saved")))
            data.close()
        }
    }
    @Test fun productionDefaultResumeInitializesColdDiskWithoutCallingTransport() = runTest {
        val root = File(checkNotNull(System.getProperty("porchlight.test.dir"))).apply { mkdirs() }
        val directory = Files.createTempDirectory(root.toPath(), "c3b-resume-").toFile()
        val file = File(directory, "read-cache.db")
        val now = Instant.parse("2026-10-06T12:00:00Z")
        try {
            ReadCache(openDesktopCache(file), { now }).let { cache ->
                cache.install(emptySnapshot(7).copy(items = listOf(itemRead("saved"))))
                cache.close()
            }
            val cache = ReadCache({ openDesktopCache(file) }, { now })
            val data = HouseholdData(cache, GeneratedCall({ error("Resume must never access HTTP") }, { error("Resume must never access credentials") }), { now })
            val fixture = FakeServices()
            val services = object : AppServices {
                override val settings = fixture.settings
                override val clock = fixture.clock
                override val platform = fixture.platform
                override val data = data
            }
            assertTrue(cache.view.value.items.isEmpty())
            assertTrue(services.routeAvailable(AppRoute.Item("saved")))
            assertFalse(services.routeAvailable(AppRoute.Item("invisible")))
            data.close()
        } finally { directory.deleteRecursively() }
    }
}
