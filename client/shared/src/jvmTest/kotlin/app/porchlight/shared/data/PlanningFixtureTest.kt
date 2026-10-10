package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlin.test.*

/** Real Rust handler recordings, normalized only for volatile UUIDs/creation timestamps. */
class PlanningFixtureTest {
    private fun fixture(name: String) = checkNotNull(javaClass.getResourceAsStream("/planning-fixtures/$name.json")).bufferedReader().use { it.readText() }
    @Test fun realImportAvailabilityAndSuggestionDecode() {
        val preview = WireJson.decodeFromString<ImportSummary>(fixture("preview"))
        val availability = WireJson.decodeFromString<AvailabilityResponse>(fixture("availability"))
        val suggestion = WireJson.decodeFromString<SuggestionResponse>(fixture("suggestion"))
        val device = WireJson.decodeFromString<Device>(fixture("device"))
        val feed = WireJson.decodeFromString<Source>(fixture("feed"))
        assertNotNull(device.certificateExpiresAt)
        assertEquals(3600, feed.refreshSeconds)
        assertEquals(availability.memberId, feed.targetMemberId)
        assertEquals(0, preview.invalidSkipped)
        assertEquals(2, preview.cancelledSkipped) // real Google fixture has two EXDATE/cancelled occurrences
        assertEquals(preview.skipped, preview.invalidSkipped!! + preview.cancelledSkipped!!)
        assertTrue(preview.preview)
        assertEquals(1, preview.created)
        assertTrue(preview.itemIds.isEmpty())
        assertEquals(7, availability.windows.size)
        validateAvailability(availability.windows)
        assertEquals(SuggestionStatus.proposed, suggestion.status)
        assertNotNull(suggestion.occurrenceVersion)
        assertEquals(availability.memberId, suggestion.memberId)
        assertTrue(suggestion.proposedEnd > suggestion.proposedStart)
    }
}
