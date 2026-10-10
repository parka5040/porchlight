package app.porchlight.shared.screens

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.settings.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*

class SettingsRecoveryTest {
    @Test fun nonAmbiguousErrorRetriesSameKeyAndCanBeDiscarded() = runTest {
        val keys = mutableListOf<String>()
        val writer = HouseholdSettingsController({ "key-${keys.size}" }, { _, _, key ->
            keys += key; throw ApiProblem("unavailable", 503, "Unavailable")
        }, { _, _, _, _ -> error("unused") })
        writer.saveToday(settingsRead(), settingsRead().settings)
        assertEquals(SettingsWriteStatus.Error, writer.state.value.status)
        writer.retry()
        assertEquals(listOf("key-0", "key-0"), keys)
        writer.discard()
        assertFalse(writer.unresolved)
        assertEquals(SettingsWriteStatus.Idle, writer.state.value.status)
    }
    @Test fun conflictReapplyPreservesLatestPinnedListAndDiscardIsExplicit() = runTest {
        val latest = settingsRead(8).copy(settings = settingsRead().settings.copy(pinnedListId = "new-list"))
        val bodies = mutableListOf<TodaySettings>()
        val writer = HouseholdSettingsController({ "key-${bodies.size}" }, { _, body, _ ->
            bodies += body
            if (bodies.size == 1) WriteResult.VersionConflict(WireJson.encodeToJsonElement(SettingsRead.serializer(), latest), ApiProblem("version-conflict", 409, "Changed"))
            else WriteResult.Success(latest)
        }, { _, _, _, _ -> error("unused") })
        writer.saveToday(settingsRead(), settingsRead().settings.copy(thisWeekDays = 14))
        writer.reapply()
        assertEquals("new-list", bodies.last().pinnedListId)
        assertEquals(14, bodies.last().thisWeekDays)
        val conflicting = HouseholdSettingsController({ "key" }, { _, _, _ -> WriteResult.VersionConflict(WireJson.encodeToJsonElement(SettingsRead.serializer(), latest), ApiProblem("version-conflict", 409, "Changed")) }, { _, _, _, _ -> error("unused") })
        conflicting.saveToday(settingsRead(), settingsRead().settings)
        conflicting.discard()
        assertFalse(conflicting.unresolved)
    }
    @Test fun cancellationKeepsAmbiguousBodyAndKeyAndCannotDiscard() = runTest {
        val keys = mutableListOf<String>()
        val writer = HouseholdSettingsController({ "key-${keys.size}" }, { _, _, key ->
            keys += key
            if (keys.size == 1) throw CancellationException("leaving screen")
            WriteResult.Success(settingsRead())
        }, { _, _, _, _ -> error("unused") })
        assertFailsWith<CancellationException> { writer.saveToday(settingsRead(), settingsRead().settings) }
        assertEquals(SettingsWriteStatus.Ambiguous, writer.state.value.status)
        writer.discard()
        assertTrue(writer.unresolved)
        writer.retry()
        assertEquals(listOf("key-0", "key-0"), keys)
    }
}
