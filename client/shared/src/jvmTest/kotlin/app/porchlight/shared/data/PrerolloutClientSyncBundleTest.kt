package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.api.models.*
import app.porchlight.shared.db.PorchlightDb
import kotlinx.coroutines.*
import kotlinx.coroutines.test.runTest
import kotlinx.datetime.LocalDate
import kotlin.test.*
import kotlin.time.Duration.Companion.days
import kotlin.time.Instant

class PrerolloutClientSyncBundleTest {
    private val f = SyncBundleFixtures
    private fun normalized(v: CacheView) = v.copy(sync = null, visibilityRevision = 0, evictedItemIds = emptySet(), evictedListIds = emptySet(),
        items = v.items.sortedBy { it.id }, occurrences = v.occurrences.sortedBy { it.id },
        comments = v.comments.sortedBy { it.id }, attachments = v.attachments.sortedBy { it.id },
        lists = v.lists.sortedBy { it.id + it.occurrenceId.orEmpty() }, reminders = v.reminders.sortedBy { it.id })

    @Test fun bundleEqualsFreshFullRefreshForRecurringDetachedDstFloatingAllDayConflictsAndReminders() = runTest {
        for (kind in listOf("recurring", "detached", "dst", "floating", "all-day")) {
            val original = f.snapshot
            val base = original.items.single()
            val occurrence = original.occurrences.single()
            val item = when (kind) {
                "dst" -> base.copy(tzid = "America/New_York")
                "floating" -> base.copy(timeMode = "floating", tzid = null)
                "all-day" -> base.copy(allDay = true)
                else -> base
            }.copy(title = "Reconciled", pendingDerivation = false)
            val row = when (kind) {
                "detached" -> occurrence.copy(isDetached = true, overrideLocalDue = "2026-11-04T16:00:00", dueAt = f.from - 20.days)
                "dst" -> occurrence.copy(startsAt = null, dueAt = null, localDue = "2026-11-01T01:30:00", overrideLocalDue = "2026-11-01T01:45:00")
                "floating" -> occurrence.copy(startsAt = null, dueAt = null, localDue = "2026-10-06T13:00:00")
                "all-day" -> occurrence.copy(dueAt = null, localDue = null, dueDate = LocalDate.parse("2026-10-06"))
                else -> occurrence.copy(seq = 3)
            }
            // Same item/occurrence versions can have different derived representations.
            val old = original.copy(items = listOf(item.copy(title = "Before", pendingDerivation = true)),
                occurrences = listOf(row.copy(id = "removed"), row), scheduledReminders = original.scheduledReminders.map { it.copy(id = "old-reminder") })
            val next = original.copy(items = listOf(item), occurrences = listOf(row),
                scheduledReminders = original.scheduledReminders.map { it.copy(fireAt = f.now) })
            val conflict = ConflictRead(original.memberSettings.memberId, row.id, "Busy", f.now, f.now + 1.days, "hard")
            val agenda = item.copy(agenda = f.agenda.agenda, occurrence = row, conflicts = listOf(conflict))
            val board = item.copy(occurrence = row)
            val targeted = memoryCache { f.now }; val full = memoryCache { f.now }
            try {
                targeted.install(old, projections = Projections(listOf(f.board), AgendaWindow(f.from, f.to, original.memberSettings.memberId, listOf(f.agenda))))
                targeted.installBundle(f.bundle(next, listOf(board), listOf(agenda)))
                full.install(next, projections = Projections(listOf(board), AgendaWindow(f.from, f.to, original.memberSettings.memberId, listOf(agenda))))
                assertEquals(normalized(full.view.value), normalized(targeted.view.value), kind)
                assertEquals(original.cursor, targeted.sync.value?.cursor)
            } finally { targeted.close(); full.close() }
        }
    }

    @Test fun partialPagesReplaceOnlyTheirKeyIntervalsAndNeverOtherItemMemberOrWindow() = runTest {
        val snapshot = f.snapshot
        val item = snapshot.items.single()
        val member = snapshot.memberSettings.memberId
        val comment = snapshot.comments.single()
        fun comment(id: String) = comment.copy(id = id)
        fun agenda(id: String, at: Instant, who: String = member) = item.copy(agenda = f.agenda.agenda!!.copy(occurrenceId = id, memberId = who, dueAt = at, startsAt = null, endsAt = null))
        val before = listOf(agenda("gone", f.now - 1.days), agenda("returned", f.now), agenda("tail", f.now + 1.days),
            agenda("outside", f.to + 1.days), agenda("other-member", f.now, "other"))
        val outside = snapshot.occurrences.single().copy(id = "outside", dueAt = f.to + 10.days, localDue = null)
        val otherItem = item.copy(id = "other-item")
        val cache = memoryCache { f.now }
        try {
            cache.install(snapshot.copy(items = listOf(item, otherItem), occurrences = snapshot.occurrences + outside,
                comments = listOf(comment("001"), comment("002"), comment("900"))),
                projections = Projections(emptyList(), AgendaWindow(f.from, f.to + 20.days, null, before)))
            val page = f.bundle(collaboration = false).let { p -> p.copy(
                coverage = p.coverage.copy(itemSections = false, suggestions = false, comments = true, commentsComplete = false,
                    agenda = listOf(AgendaCoverage(member, f.from, f.to, false, nextCursor = "agenda-tail"))),
                items = null, occurrences = null, scheduledReminders = null, board = null, suggestions = null,
                comments = listOf(comment("002")), commentsNextCursor = "comments-tail",
                agenda = listOf(SyncAgenda(member, f.from, f.to, listOf(before[1])))) }
            cache.installBundle(page)
            assertEquals(setOf("returned", "tail", "outside", "other-member"), cache.view.value.agenda.map { it.agenda!!.occurrenceId }.toSet())
            assertEquals(setOf("002", "900"), cache.view.value.comments.map { it.id }.toSet())
            assertEquals(setOf(item.id, otherItem.id), cache.view.value.items.map { it.id }.toSet())
            assertTrue(cache.view.value.occurrences.any { it.id == outside.id })
            assertEquals(snapshot.attachments, cache.view.value.attachments)
            assertEquals(snapshot.lists, cache.view.value.lists)
        } finally { cache.close() }
    }

    @Test fun emptyIncompletePagesGrantNoOmissionCoverage() = runTest {
        val cache = memoryCache { f.now }
        try {
            cache.install(f.snapshot, projections = Projections(listOf(f.board), AgendaWindow(f.from, f.to, null, listOf(f.agenda))))
            val before = cache.view.value
            val p = f.bundle(collaboration = false)
            cache.installBundle(p.copy(coverage = p.coverage.copy(itemSections = false, suggestions = false, comments = true,
                commentsComplete = false, agenda = p.coverage.agenda.map { it.copy(complete = false, nextCursor = "tail") }),
                comments = emptyList(), commentsNextCursor = "comments", agenda = p.agenda!!.map { it.copy(items = emptyList()) }))
            assertEquals(before.agenda, cache.view.value.agenda)
            assertEquals(before.comments, cache.view.value.comments)
            assertEquals(before.board, cache.view.value.board)
        } finally { cache.close() }
    }

    @Test fun omissionOfCompleteCollaborationPurgesFilesAndFencesLateChildren() = runTest {
        val store = MemoryAttachmentStore()
        val cache = memoryCache(store) { f.now }
        val snapshot = f.snapshot.let { it.copy(lists = it.lists.map { list -> list.copy(itemId = it.items.single().id) }) }
        try {
            cache.install(snapshot)
            val attachment = snapshot.attachments.single()
            store.write(attachmentKey(cache.attachmentNamespace, attachment, false), byteArrayOf(1))
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val late = async {
                cache.inSession { fence -> started.complete(Unit); release.await()
                    assertFailsWith<StaleVisibilityReply> { cache.putComments(fence, snapshot.comments) }
                    assertFailsWith<StaleVisibilityReply> { cache.putAttachment(fence, attachment) }
                    assertFailsWith<StaleVisibilityReply> { cache.putList(fence, snapshot.lists.single()) }
                }
            }
            started.await()
            val p = f.bundle().copy(comments = emptyList(), attachments = emptyList(), lists = emptyList())
            cache.installBundle(p)
            assertTrue(cache.view.value.comments.isEmpty()); assertTrue(cache.view.value.attachments.isEmpty()); assertTrue(cache.view.value.lists.isEmpty())
            assertNull(store.read(attachmentKey(cache.attachmentNamespace, attachment, false)))
            release.complete(Unit); late.await()
        } finally { cache.close() }
    }

    @Test fun itemEvictionAndChildEvictionWinEvenAfterFreshReadRestoresVisibility() = runTest {
        val cache = memoryCache { f.now }
        try {
            cache.install(f.snapshot)
            val started = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val late = async {
                cache.inSession { fence -> started.complete(Unit); release.await()
                    assertFailsWith<StaleVisibilityReply> { cache.installItemBundle(fence, ItemBundleCoverage(listOf(f.bundle().coverage)), ItemBundleRows(listOf(f.bundle()))) }
                }
            }
            started.await(); cache.evictItem(f.snapshot.items.single().id)
            cache.installBundle(f.bundle())
            release.complete(Unit); late.await()
            assertEquals(f.snapshot.items, cache.view.value.items)
            val old = CompletableDeferred<Unit>(); val go = CompletableDeferred<Unit>()
            val child = async { cache.inSession { fence -> old.complete(Unit); go.await()
                assertFailsWith<StaleVisibilityReply> { cache.installItemBundle(fence, ItemBundleCoverage(listOf(f.bundle().coverage)), ItemBundleRows(listOf(f.bundle()))) }
            } }
            old.await(); cache.evictComment(f.snapshot.comments.single().id); go.complete(Unit); child.await()
            assertTrue(cache.view.value.comments.isEmpty())
        } finally { cache.close() }
    }

    @Test fun epochAndForeignFenceCannotInstallBundle() = runTest {
        val cache = memoryCache { f.now }; val other = memoryCache { f.now }
        var epoch = 1L; cache.sessionEpoch = { epoch }
        try {
            cache.install(f.snapshot)
            cache.inSession { fence ->
                epoch++
                assertFailsWith<CancellationException> { cache.installItemBundle(fence, ItemBundleCoverage(listOf(f.bundle().coverage)), ItemBundleRows(listOf(f.bundle()))) }
                other.inSession {
                    assertFailsWith<IllegalStateException> { other.installItemBundle(fence, ItemBundleCoverage(listOf(f.bundle().coverage)), ItemBundleRows(listOf(f.bundle()))) }
                }
            }
        } finally { cache.close(); other.close() }
    }

    @Test fun inheritedTemplatesAndRetainedChecksMatchFullRefreshWithoutDeletingUnrelatedLists() = runTest {
        val original = f.snapshot
        val child = original.items.single().copy(splitFromItemId = "ancestor")
        val parent = child.copy(id = "ancestor", splitFromItemId = null)
        val template = f.recording<ListResponse>("list-occurrence-template").copy(itemId = parent.id)
        val check = f.recording<ListResponse>("list-occurrence-check").copy(itemId = parent.id, occurrenceId = original.occurrences.single().id)
        val unrelated = original.lists.single()
        val old = original.copy(items = listOf(child, parent), lists = listOf(template, unrelated))
        val changed = template.copy(name = "Updated inherited template", propertyEntries = template.propertyEntries.map { it.copy(label = "Updated entry") })
        val next = old.copy(lists = listOf(changed, unrelated))
        val cache = memoryCache { f.now }; val full = memoryCache { f.now }
        try {
            for (c in listOf(cache, full)) {
                c.install(old, projections = Projections(emptyList(), AgendaWindow(f.from, f.to, null, emptyList())))
                c.putList(check)
            }
            cache.installBundle(f.bundle(next, emptyList(), emptyList()).copy(lists = listOf(changed)))
            full.install(next, projections = Projections(emptyList(), AgendaWindow(f.from, f.to, null, emptyList())))
            assertEquals(normalized(full.view.value), normalized(cache.view.value))
            assertEquals(check.propertyEntries.first().checkedAt, cache.view.value.lists.single { it.occurrenceId != null }.propertyEntries.first().checkedAt)
            cache.installBundle(f.bundle(next, emptyList(), emptyList()).copy(lists = emptyList()))
            assertEquals(listOf(unrelated.id), cache.view.value.lists.map { it.id })
        } finally { cache.close(); full.close() }
    }

    @Test fun cleanupOnlyListDescriptorPurgesRetainedScopesAndPublishesItsVisibilityFence() = runTest {
        val snapshot = f.snapshot
        val id = snapshot.items.single().id
        val template = f.recording<ListResponse>("list-occurrence-template").copy(itemId = id)
        val checked = f.recording<ListResponse>("list-occurrence-check").copy(itemId = id, occurrenceId = snapshot.occurrences.single().id)
        val descriptor = f.recording<ListResponse>("list-orphan-redacted").copy(id = template.id, itemId = id)
        val cache = memoryCache { f.now }
        try {
            cache.install(snapshot.copy(lists = listOf(template))); cache.putList(checked)
            cache.installBundle(f.bundle(snapshot, emptyList(), emptyList()).copy(lists = listOf(descriptor)))
            val row = cache.view.value.lists.single()
            assertTrue(row.redacted == true); assertTrue(row.propertyEntries.isEmpty()); assertNull(row.occurrenceId)
            assertTrue(template.id in cache.view.value.evictedListIds)
            cache.initialize()
            assertEquals(row, cache.view.value.lists.single())
        } finally { cache.close() }
    }

    @Test fun callerReminderAndOccurrenceWindowCoverageDoesNotDeleteOutsideScopeButBoardCoversOutsideWindow() = runTest {
        val original = f.snapshot
        val item = original.items.single()
        val occurrence = original.occurrences.single()
        val outside = occurrence.copy(id = "outside-occurrence", dueAt = f.to + 10.days, localDue = null)
        val reminder = original.scheduledReminders.single()
        val otherMember = reminder.copy(id = "other-member", memberId = "other")
        val outsideReminder = reminder.copy(id = "outside-reminder", occurrenceId = outside.id, fireAt = f.to + 10.days)
        val snapshot = original.copy(occurrences = original.occurrences + outside,
            scheduledReminders = original.scheduledReminders + otherMember + outsideReminder)
        val cache = memoryCache { f.now }
        try {
            cache.install(snapshot, projections = Projections(listOf(item.copy(occurrence = outside)), AgendaWindow(f.from, f.to, null, emptyList())))
            val page = f.bundle(original, emptyList(), emptyList(), false).copy(scheduledReminders = emptyList())
            cache.installBundle(page)
            assertEquals(setOf(otherMember.id, outsideReminder.id), cache.view.value.reminders.map { it.id }.toSet())
            assertTrue(cache.view.value.occurrences.any { it.id == outside.id })
            assertTrue(cache.view.value.board.isEmpty(), "Board coverage is not limited by the occurrence window")
        } finally { cache.close() }
    }

    @Test fun mineBoardUsesAuthoritativeRelationshipsRatherThanAllHouseholdCards() = runTest {
        val snapshot = f.snapshot
        val item = snapshot.items.single()
        val member = snapshot.memberSettings.memberId
        val cache = memoryCache { f.now }
        try {
            cache.install(snapshot)
            cache.inSession { cache.replaceBoard(it, emptyList(), "mine") }
            for (relationship in listOf("none", "owner", "subject", "assignee")) {
                val changed = item.copy(ownerId = if (relationship == "owner") member else "another-member",
                    subjectMemberId = member.takeIf { relationship == "subject" }, assignees = if (relationship == "assignee") listOf(member) else emptyList())
                val page = f.bundle(snapshot.copy(items = listOf(changed)), listOf(changed.copy(occurrence = f.board.occurrence)), emptyList())
                cache.installBundle(page)
                assertEquals(if (relationship == "none") 0 else 1, cache.view.value.board.size, relationship)
            }
        } finally { cache.close() }
    }

    @Test fun ambiguousFloatingWindowBoundaryRequiresSnapshotInsteadOfGuessingHouseholdZone() = runTest {
        val snapshot = f.snapshot
        val item = snapshot.items.single().copy(timeMode = "floating", tzid = null)
        val boundary = snapshot.occurrences.single().copy(dueAt = null, localDue = "2026-04-09T12:00:00")
        val cache = memoryCache { f.now }
        try {
            cache.install(snapshot.copy(items = listOf(item), occurrences = listOf(boundary)))
            val before = cache.view.value
            assertFailsWith<ItemBundleNeedsSnapshot> { cache.installBundle(f.bundle(snapshot.copy(items = listOf(item), occurrences = emptyList()))) }
            assertEquals(before, cache.view.value)
        } finally { cache.close() }
    }

    @Test fun ownerReplacementRejectsLateBundleBeforeItCanRepopulateDisk() = runTest {
        val cache = memoryCache { f.now }
        try {
            cache.initialize(CacheOwner("root-fingerprint", "first-member", "first-device"))
            cache.install(f.snapshot)
            val start = CompletableDeferred<Unit>(); val go = CompletableDeferred<Unit>()
            val late = async { cache.inSession { fence -> start.complete(Unit); go.await()
                assertFailsWith<CancellationException> { cache.installItemBundle(fence, ItemBundleCoverage(listOf(f.bundle().coverage)), ItemBundleRows(listOf(f.bundle()))) }
            } }
            start.await(); cache.initialize(CacheOwner("root-fingerprint", "second-member", "second-device")); go.complete(Unit); late.await()
            assertTrue(cache.view.value.items.isEmpty())
            cache.initialize(); assertTrue(cache.view.value.items.isEmpty())
        } finally { cache.close() }
    }

    @Test fun failedSqlCommitPublishesNothingAndRollsBackEveryScopedRemoval() = runTest {
        val delegate = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(delegate)
        var fail = false
        val driver = object : SqlDriver by delegate {
            override fun execute(identifier: Int?, sql: String, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<Long> {
                if (fail && sql.contains("INSERT OR REPLACE INTO CacheReminder")) throw IllegalStateException("Injected reminder write failure")
                return delegate.execute(identifier, sql, parameters, binders)
            }
        }
        val cache = ReadCache(driver) { f.now }
        try {
            cache.install(f.snapshot)
            val before = cache.view.value
            fail = true
            assertFailsWith<IllegalStateException> { cache.installBundle(f.bundle().copy(items = f.snapshot.items.map { it.copy(title = "Must roll back") })) }
            assertEquals(before, cache.view.value)
            fail = false
            cache.initialize()
            assertEquals(normalized(before), normalized(cache.view.value))
        } finally { cache.close() }
    }

    @Test fun targetedInstallDoesNotReadUnrelatedPayloadsOrAdvanceCursor() = runTest {
        val delegate = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
        PorchlightDb.Schema.create(delegate)
        var watching = false
        val scans = mutableListOf<String>()
        val driver = object : SqlDriver by delegate {
            override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> {
                if (watching && Regex("SELECT \\* FROM Cache", RegexOption.IGNORE_CASE).containsMatchIn(sql) && !sql.contains("WHERE", true)) scans += sql
                return delegate.executeQuery(identifier, sql, mapper, parameters, binders)
            }
        }
        val cache = ReadCache(driver) { f.now }
        try {
            cache.install(f.snapshot.copy(items = f.snapshot.items + itemRead("unrelated")))
            val untouched = cache.view.value.items.single { it.id == "unrelated" }
            cache.acknowledgeCursor(f.snapshot.cursor + 50)
            watching = true
            cache.installBundle(f.bundle())
            watching = false
            assertTrue(scans.isEmpty(), "Unscoped payload reads: $scans")
            assertSame(untouched, cache.view.value.items.single { it.id == "unrelated" })
            assertEquals(f.snapshot.cursor + 50, cache.sync.value?.cursor)
        } finally { cache.close() }
    }
}
