package app.porchlight.shared

import app.porchlight.shared.transport.*
import kotlin.test.*

class ZoneCacheTest {
    @Test fun slashlessTzdbIdsAreReportedVerbatim() {
        assertEquals("CET", ianaZone("CET"))
        assertEquals("UTC", ianaZone("UTC"))
        assertEquals("Etc/GMT-2", ianaZone("GMT+02:00"))
    }
    @Test fun mappingAndWarningRunOnlyOnceUntilDeviceZoneChanges() {
        var raw = "CET"
        var maps = 0
        var warnings = 0
        val cache = DeviceZoneCache({ raw }, { maps++; ianaZone(it) }, { warnings++ })
        repeat(10) { assertEquals("CET", cache.current()) }
        assertEquals(1, maps)
        raw = "bad-zone"
        repeat(10) { assertNull(cache.current()) }
        assertEquals(2, maps)
        assertEquals(1, warnings)
        raw = "UTC"
        assertEquals("UTC", cache.current())
        assertEquals(3, maps)
    }
}
