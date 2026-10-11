package app.porchlight.shared.data

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlin.test.*
import kotlin.time.Instant

class AuditBlClient2bReceiptsTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")

    @Test fun collectionAbsenceDoesNotDeleteInterveningListOrEntryWrites() = runTest {
        val cache = memoryCache { now }
        try {
            val list = listFixture()
            cache.putList(list)
            cache.inSession { fence ->
                val mark = cache.receiptMark()
                cache.putList(list.copy(id = "created", version = 2))
                cache.putEntry(list.propertyEntries.first().copy(version = 2, label = "Changed"))
                cache.replaceLists(fence, null, emptyList(), mark)
            }
            assertEquals(setOf(list.id, "created"), cache.view.value.lists.map { it.id }.toSet())
            assertEquals("Changed", cache.view.value.lists.first { it.id == list.id }.propertyEntries.first().label)
            cache.replaceLists(null, emptyList(), cache.receiptMark())
            assertTrue(cache.view.value.lists.isEmpty(), "A later authoritative absence still deletes")
        } finally { cache.close() }
    }

    @Test fun olderListCollectionKeepsInterveningCheckInGotIt() = runTest {
        val cache = memoryCache { now }
        try {
            val list = listFixture()
            cache.putList(list)
            cache.inSession { fence ->
                val mark = cache.receiptMark()
                cache.putEntry(list.propertyEntries.first().copy(version = 2, checkedAt = now, checkedBy = "self"))
                cache.replaceLists(fence, null, listOf(list), mark)
            }
            val row = cache.view.value.lists.single()
            assertTrue(row.propertyEntries.none { it.id == list.propertyEntries.first().id })
            assertEquals(now, row.gotIt.first { it.id == list.propertyEntries.first().id }.checkedAt)
        } finally { cache.close() }
    }

    @Test fun completeCommentPageRetainsInterveningCreateAndEdit() = runTest {
        val cache = memoryCache { now }
        try {
            val comment = commentFixture()
            cache.putComments(listOf(comment))
            cache.inSession { fence ->
                cache.putComments(listOf(comment.copy(version = 2, body = "Edited"), comment.copy(id = "created")))
                cache.replaceComments(fence, comment.itemId, listOf(comment))
            }
            assertEquals(setOf(comment.id, "created"), cache.view.value.comments.map { it.id }.toSet())
            assertEquals("Edited", cache.view.value.comments.first { it.id == comment.id }.body)
            cache.replaceComments(comment.itemId, emptyList())
            assertTrue(cache.view.value.comments.isEmpty())
        } finally { cache.close() }
    }

    @Test fun restoredMemberReceiptCannotBeRestampedByANewWrite() = runTest {
        val cache = memoryCache { now }
        val member = WireJson.decodeFromString<MemberRead>("""{"id":"self","household_id":"household","display_name":"Sam Rivera","role":"member","color":"red","initials":"SR","tzid":"UTC","tz_source":"manual","version":2,"pending_derivation":false}""")
        try {
            cache.install(emptySnapshot(10).copy(members = listOf(member)))
            cache.putMember(member.copy(version = 9))
            cache.replaceMembers(listOf(member), receivedAfter = cache.receiptMark())
            val mark = cache.receiptMark()
            cache.putMember(member.copy(version = 3))
            cache.replaceMembers(listOf(member), receivedAfter = mark)
            assertEquals(3, cache.view.value.members.single().version, "Pre-restore v9 is not a new receipt")
        } finally { cache.close() }
    }

    @Test fun snapshotGhostChildCannotRetainReceiptWhenParentDisappears() = runTest {
        val cache = memoryCache { now }
        try {
            val parent = itemRead("hidden")
            val child = occurrenceRead("ghost", parent.id)
            cache.install(emptySnapshot(1).copy(items = listOf(parent), occurrences = listOf(child)))
            val mark = cache.receiptMark()
            cache.putOccurrence(occurrenceResponse(child.id, parent.id).copy(version = 2))
            cache.installStaged(emptySnapshot(2).copy(occurrences = listOf(child)), now, receivedAfter = mark)
            assertTrue(cache.view.value.items.isEmpty())
            assertTrue(cache.view.value.occurrences.isEmpty(), "Snapshot contains ghost child but no parent")
        } finally { cache.close() }
    }

    @Test fun staleAndAbsentDeletionReceiptsDoNotDeleteSnapshotRows() = runTest {
        val cache = memoryCache { now }
        try {
            val snapshot = emptySnapshot(2).copy(items = listOf(itemRead("same").copy(version = 3), itemRead("keep")))
            cache.install(emptySnapshot(1).copy(items = snapshot.items + itemRead("absent")))
            val mark = cache.receiptMark()
            cache.putItem(itemResponse("same").copy(version = 2, deletedAt = now))
            cache.putItem(itemResponse("absent").copy(version = 2, deletedAt = now))
            cache.installStaged(snapshot, now, receivedAfter = mark)
            assertEquals(setOf("same", "keep"), cache.view.value.items.map { it.id }.toSet())
        } finally { cache.close() }
    }
}
