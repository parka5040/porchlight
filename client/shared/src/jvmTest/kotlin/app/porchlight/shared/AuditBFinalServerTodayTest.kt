package app.porchlight.shared

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.CacheView
import app.porchlight.shared.screens.today.*
import app.porchlight.shared.transport.WireJson
import kotlinx.datetime.TimeZone
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class AuditBFinalServerTodayTest {
    private fun rows(name: String): List<ItemRead> {
        val wire = WireJson.parseToJsonElement(checkNotNull(javaClass.getResource("/item-fixtures/audit-today-$name.json")).readText()).jsonObject
        val snapshot = WireJson.decodeFromJsonElement<SnapshotRead>(wire.getValue("snapshot"))
        val agenda = WireJson.decodeFromJsonElement<ItemReadPage>(wire.getValue("agenda"))
        val member = wire.getValue("member").jsonPrimitive.content
        val view = CacheView(items = snapshot.items, occurrences = snapshot.occurrences, agenda = agenda.items)
        val settings = TodaySettings(listOf(TodaySection.today), SettingsScope.mine, 7)
        return todaySections(todayRows(view, member), settings, member,
            TimeZone.of(wire.getValue("zone").jsonPrimitive.content),
            Instant.parse(wire.getValue("now").jsonPrimitive.content)).single().rows
    }

    @Test fun laterRecipientKeepsWorkDespiteOwnerExpiry() {
        assertEquals(1, rows("later").size, "eligible member agenda must override global expiry")
    }
    @Test fun earlierRecipientDoesNotResurrectLocallyExpiredWorkFromSnapshot() {
        assertTrue(rows("earlier").isEmpty(), "absent recipient agenda must not fall back to globally open work")
    }
}
