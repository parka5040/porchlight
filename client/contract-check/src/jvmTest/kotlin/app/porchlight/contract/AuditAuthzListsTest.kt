package app.porchlight.contract

import app.porchlight.contract.models.ListResponse
import kotlinx.serialization.json.Json
import kotlin.test.*

class AuditAuthzListsTest {
    @Test fun generatedListContractDecodesRealRedactedCleanup() {
        val source = java.io.File("../shared/src/jvmTest/resources/item-fixtures/list-orphan-redacted.json").readText()
        val row = Json.decodeFromString<ListResponse>(source)
        assertEquals(true, row.redacted)
        assertNull(row.name)
        assertNull(row.householdId)
        assertNull(row.visibility)
        assertEquals(listOf("delete"), row.capabilities)
        assertTrue(row.propertyEntries.isEmpty() && row.gotIt.isEmpty() && row.viewers.isEmpty())
    }
}
