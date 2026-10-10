package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.files.calendarImportName
import app.porchlight.shared.files.calendarImportScope
import app.porchlight.shared.screens.ScreenSession
import app.porchlight.shared.screens.planning.*
import kotlinx.coroutines.*
import kotlinx.coroutines.test.*
import kotlin.test.*

@OptIn(ExperimentalCoroutinesApi::class)
class PlanningFix2Test {
    private fun pending() = PendingUiExport(object : ExportSink {
        override suspend fun write(bytes: ByteArray) = Unit
        override suspend fun finish(checkGate: () -> Unit) = checkGate()
        override suspend fun abort() = Unit
    }, "old-step-up")

    @Test fun expiredDeferredExportCannotStartWithOldToken() = runTest {
        val pending = pending()
        val waiting = async { runCatching { pending.awaitCompletion() } }
        runCurrent()
        advanceTimeBy(60_001)
        runCurrent()
        assertTrue(waiting.await().isFailure)
        assertFalse(pending.begin())
        assertTrue(pending.result.isCompleted)
    }

    @Test fun orphanedDraftIsCancelledRatherThanResurrected() = runTest {
        val session = ScreenSession(StandardTestDispatcher(testScheduler))
        val pending = pending()
        session.planningDrafts["c6:Export:dependent"] = PlanningDraft().also { it.export = pending }
        val waiting = async { runCatching { pending.awaitCompletion() } }
        runCurrent()
        session.cancelOrphanedPlanningExports("c6:Export:own")
        runCurrent()
        assertTrue(waiting.await().isFailure)
        assertFalse(pending.begin())
    }

    @Test fun deferredExportCanOnlyStartOnceAndDownloadHasNoPickerDeadline() = runTest {
        val pending = pending()
        val waiting = async { pending.awaitCompletion() }
        runCurrent()
        assertTrue(pending.begin())
        assertFalse(pending.begin())
        advanceTimeBy(120_000)
        runCurrent()
        assertFalse(waiting.isCompleted, "Deadline bounds waiting for unlock, not active downloads")
        pending.result.complete(Unit)
        waiting.await()
    }

    @Test fun filenameScopesAreStableBoundedAndKeepLongNamesDistinct() {
        assertEquals("file:school.ics", calendarImportScope(" School.ICS "))
        val prefix = "x".repeat(220)
        val first = calendarImportScope("${prefix}a.ics")
        assertTrue(first.length <= 200)
        assertEquals(first, calendarImportScope("${prefix}a.ics"))
        assertNotEquals(first, calendarImportScope("${prefix}b.ics"))
        assertTrue(calendarImportScope("😀".repeat(220) + ".ics").length <= 200)
    }

    @Test fun importNamesFitTheServerBound() {
        assertEquals("School.ics", calendarImportName(" School.ics "))
        assertEquals("calendar.ics", calendarImportName("\u0000"))
        val long = calendarImportName("x".repeat(300) + ".ics")
        assertEquals(196, long.length)
        assertTrue(long.endsWith(".ics"))
        val emoji = calendarImportName("😀".repeat(150) + ".ics")
        assertTrue(emoji.length <= 196 && emoji.endsWith(".ics") && !emoji.dropLast(4).last().isHighSurrogate())
    }

    @Test fun dependentFeedDefaultsHouseholdButOrdinaryMemberDefaultsPrivate() {
        val dependent = MemberRead(false, "dependent", "household", "Sam", "dependent", "#FABD2F", "SM", "UTC", "manual", 1)
        val ordinary = dependent.copy(id = "ordinary", role = "user")
        assertEquals(SourceVisibility.household, defaultFeedVisibility(null))
        assertEquals(SourceVisibility.household, defaultFeedVisibility(dependent))
        assertEquals(SourceVisibility.private, defaultFeedVisibility(ordinary))
    }
}
