package app.porchlight.shared

import kotlin.test.*

class MemberInitialsTest {
    @Test fun memberInitialsHaveANonColorSignal() {
        assertEquals("AL", app.porchlight.shared.ui.memberInitials("Alex Lee"))
        assertEquals("A", app.porchlight.shared.ui.memberInitials("Alex"))
        assertEquals("?", app.porchlight.shared.ui.memberInitials("  "))
    }
}
