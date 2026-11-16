package app.porchlight.shared.data

import app.porchlight.shared.live.*
import kotlin.test.*

class PrerolloutClientSyncFramesTest {
    private fun fixture(name: String): List<StreamFrame> {
        val bytes = checkNotNull(javaClass.getResourceAsStream("/sync-fixtures/$name.sse")).use { it.readBytes() }
        val parser = SseParser()
        return bytes.decodeToString().split('\n').mapNotNull(parser::line)
    }
    @Test fun negotiatedByteCatalogueIncludesInvalidationsAvailabilityAndProjectionReset() {
        val frames = fixture("negotiated")
        val tickles = frames.filterIsInstance<StreamFrame.Tickle>()
        assertEquals(listOf("list_entry.changed", "item.invalidated", "notification.invalidated", "settings.changed"), tickles.map { it.kind })
        assertEquals("availability", tickles.last().entityType)
        assertNotNull(tickles.first().itemId); assertNotNull(tickles.first().listId)
        assertEquals(StreamFrame.ProjectionChanged(), frames.last())
    }
    @Test fun legacyByteCatalogueKeepsIdenticalFramesWithoutHintsOrNewResetType() {
        val frames = fixture("legacy")
        assertEquals(StreamFrame.Heartbeat, frames.first()); assertEquals(StreamFrame.Ready, frames[1])
        assertEquals(StreamFrame.Resnapshot, frames.last())
        val tickles = frames.filterIsInstance<StreamFrame.Tickle>()
        assertEquals(listOf(42L, 44L), tickles.map { it.seq })
        assertTrue(tickles.last().evicted)
        tickles.forEach { assertNull(it.itemId); assertNull(it.listId) }
    }
    @Test fun malformedKnownFieldsStillFailClosedButUnknownKindsRemainParseable() {
        val parser = SseParser()
        val bytes = "id: 5\nevent: future.changed\ndata: {\"seq\":5,\"kind\":\"future.changed\",\"entity_type\":\"future\",\"entity_id\":\"opaque\",\"item_id\":null,\"future\":true}\n\n"
        assertEquals("future.changed", bytes.split('\n').mapNotNull(parser::line).filterIsInstance<StreamFrame.Tickle>().single().kind)
        assertFailsWith<InvalidStream> {
            "id: 6\nevent: item.changed\ndata: {\"seq\":7,\"kind\":\"item.changed\",\"entity_type\":\"item\",\"entity_id\":\"opaque\"}\n\n".split('\n').forEach(parser::line)
        }
    }
    @Test fun recordedNegotiatedFramesRetainAuthoritativeParentHints() {
        val parser = SseParser()
        val wire = "id: 42\nevent: list_entry.changed\ndata: {\"seq\":42,\"kind\":\"list_entry.changed\",\"entity_type\":\"list_entry\",\"entity_id\":\"entry\",\"item_id\":\"parent\",\"list_id\":\"list\",\"future\":{\"ignored\":true}}\n\n".encodeToByteArray()
        val frames = wire.decodeToString().split('\n').mapNotNull(parser::line)
        assertEquals(1, frames.size)
        assertTrue(frames.single().toString().contains("itemId=parent"), "Negotiated parent hint must survive decoding")
        assertTrue(frames.single().toString().contains("listId=list"))
    }
}
