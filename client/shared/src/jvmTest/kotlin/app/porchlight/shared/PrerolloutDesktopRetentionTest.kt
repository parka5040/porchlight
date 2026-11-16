package app.porchlight.shared

import app.porchlight.shared.data.*
import app.porchlight.shared.screens.*
import kotlinx.coroutines.*
import kotlin.test.*

class PrerolloutDesktopRetentionTest {
    private fun retained(session: ScreenSession, name: String): Int =
        (ScreenSession::class.java.getDeclaredField(name).apply { isAccessible = true }.get(session) as Map<*, *>).size

    @Test fun cleanHistoryIsBoundedButUnsavedAndAmbiguousWorkSurvives() = runBlocking {
        val session = ScreenSession(Dispatchers.Unconfined)
        val view = CacheView()
        val draft = session.editor("draft", view, true)
        draft.current(view)!!.apply { present = true; unsaved = true }
        val ambiguous = session.writes("ambiguous")
        ambiguous.submit { throw java.io.IOException("Unconfirmed") }
        val ambiguousUpload = session.upload("pending")
        session.writes("collaboration:pending").submit { throw java.io.IOException("Unconfirmed upload") }
        repeat(200) { session.editor("item-$it", view, false); session.writes("write-$it"); session.upload("upload-$it") }
        assertTrue(retained(session, "editors") <= 65, "Clean editors must be bounded")
        assertTrue(retained(session, "writers") <= 66, "Completed writes must be bounded")
        assertTrue(retained(session, "uploads") <= 65, "Finished uploads must be bounded")
        assertSame(ambiguousUpload, session.upload("pending"))
        assertSame(draft, session.editor("draft", view, true))
        assertSame(ambiguous, session.writes("ambiguous"))
        session.writeScope.cancel()
    }

    @Test fun closeCancelsWritesAndReleasesRetainedState() {
        val session = ScreenSession(Dispatchers.Unconfined)
        session.writes("one"); session.upload("one")
        val close = ScreenSession::class.java.methods.find { it.name == "close" && it.parameterCount == 0 }
        assertNotNull(close, "ScreenSession must expose close()")
        close.invoke(session)
        assertFalse(session.writeScope.isActive)
        session.bind(2)
        assertFalse(session.writeScope.isActive, "A late composition must not resurrect a closed session")
        assertEquals(0, retained(session, "writers")); assertEquals(0, retained(session, "uploads"))
    }
}
