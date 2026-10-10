package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

/** Companion of the real-handler Rust equality gate; no synthetic wire documents here. */
class ItemFixtureTest {
    private fun recording(name: String) = checkNotNull(javaClass.getResourceAsStream("/item-fixtures/$name.json")).bufferedReader().use { it.readText() }
    @Test fun realServerItemAgendaBoardSnapshotDecodeAndPromote() = runTest {
        val item = WireJson.decodeFromString<ItemResponse>(recording("item"))
        val agenda = WireJson.decodeFromString<ItemRead>(recording("agenda"))
        val board = WireJson.decodeFromString<ItemRead>(recording("board"))
        val snapshot = WireJson.decodeFromString<SnapshotRead>(recording("snapshot"))
        assertTrue(snapshot.cursor >= 0)
        assertTrue(item.capabilities.isNotEmpty())
        assertNotNull(agenda.agenda); assertNotNull(board.occurrence)
        assertEquals(snapshot.items.single().id, snapshot.occurrences.single().itemId)
        // S7b sections (C4/C5 own their repositories): the real rows decode with the generated models.
        assertEquals("Recorded comment", snapshot.comments.single().body)
        assertEquals(snapshot.items.single().id, snapshot.comments.single().itemId)
        assertEquals("notes.txt", snapshot.attachments.single().originalName)
        assertEquals(snapshot.items.single().id, snapshot.attachments.single().itemId)
        // S6a sections (C4 owns notifications): real prefs decode; this single-member recording has no notification.
        assertEquals(snapshot.memberSettings.memberId, snapshot.notificationPrefs.memberId)
        assertTrue(snapshot.notifications.isEmpty())
        // Sync installs it through the same staged path as production.
        val cache = memoryCache { Instant.parse("2026-10-06T12:00:00Z") }
        try {
            cache.install(snapshot)
            assertEquals(snapshot.cursor, cache.sync.value?.cursor)
            assertEquals(snapshot.items, cache.view.value.items)
            assertEquals(snapshot.memberSettings, cache.view.value.settings)
            cache.putItem(item)
            assertEquals(item.reminderOffsets, cache.itemWrite(item.id)?.reminderOffsets)
        } finally { cache.close() }
    }
}
