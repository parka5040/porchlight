package app.porchlight.shared.data

import app.porchlight.shared.files.FileDialogSlot
import app.porchlight.shared.files.validateCalendarFile
import kotlin.test.*

class FileDialogSlotTest {
    @Test fun duplicateTapIsRejectedAndLostHelperReleasesOnlyItsOwnRequest() {
        val slot = FileDialogSlot<String>()
        var first: String? = "waiting"
        var second: String? = "waiting"
        val owner = slot.begin { first = it }!!
        assertNull(slot.begin { second = it })
        slot.cancel(owner)
        assertNull(first)
        val next = slot.begin { second = it }!!
        slot.cancel(owner) // an obsolete helper must not cancel a new dialog
        slot.complete(next, "chosen")
        assertEquals("chosen", second)
        assertNotNull(slot.begin {})
    }
    @Test fun calendarMimeMayHaveExtensionlessNameButContentStillValidated() {
        val bytes = "BEGIN:VCALENDAR\r\nEND:VCALENDAR".encodeToByteArray()
        assertEquals("School", validateCalendarFile("School", bytes, "text/calendar").name)
        assertFailsWith<IllegalArgumentException> { validateCalendarFile("School", bytes, "text/plain") }
        assertFailsWith<IllegalArgumentException> { validateCalendarFile("School", "not calendar".encodeToByteArray(), "text/calendar") }
    }

}
