package app.porchlight.shared.data.syncmodel

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.live.StreamFrame
import app.porchlight.shared.transport.*
import io.ktor.http.Url
import kotlinx.serialization.json.*
import java.time.LocalDateTime
import java.time.ZoneId
import kotlin.time.Duration.Companion.days
import kotlin.time.Duration.Companion.minutes
import kotlin.time.Instant

/** Independent read oracle: no cache membership, installer or dispatch code is used to derive answers. */
internal class HouseholdModel(val longThread: Boolean = false, val itemCount: Int = 6) {
    companion object {
        val now = Instant.parse("2026-10-06T12:00:00Z")
        val from = now - 180.days
        val to = now + 548.days
        const val zone = "America/New_York"
        fun id(n: Int) = "01999999-1111-7000-8000-${n.toString().padStart(12, '0')}"
        fun local(at: Instant, tz: String = zone) = java.time.Instant.parse(at.toString()).atZone(ZoneId.of(tz)).toLocalDateTime().toString()
    }
    private val base = SyncBundleFixtures.snapshot
    val readers = listOf(id(1), id(2))
    var members = readers.mapIndexed { i, id -> base.members.single().copy(id = id, displayName = if (i == 0) "Sam Rivera" else "Taylor Reed", initials = if (i == 0) "SR" else "TR", role = "member", tzid = zone) }
    var items = (0 until itemCount).map { i -> base.items.single().copy(id = id(100 + i), title = "Task $i", ownerId = readers[i % 2],
        visibility = if (i == 1) "private" else "household", assignees = if (i in listOf(1, 5)) emptyList() else readers,
        reminders = base.items.single().reminders.map { it.copy(id = id(300 + i), itemId = id(100 + i)) }, tzid = zone,
        kind = if (i < 2) "appointment" else "deadline", estMinutes = 60,
        timeMode = if (i == 2) "floating" else "fixed", allDay = i == 3, dueAt = if (i < 4) null else Instant.parse("2026-10-06T13:00:00Z"),
        startsAt = if (i < 2) Instant.parse("2026-10-06T13:00:00Z") else null, endsAt = if (i < 2) Instant.parse("2026-10-06T14:00:00Z") else null,
        localStart = if (i < 2) "2026-10-06T09:00:00" else null, localEnd = if (i < 2) "2026-10-06T10:00:00" else null,
        localDue = if (i == 2) "2026-11-01T01:30:00" else null, dueDate = if (i == 3) kotlinx.datetime.LocalDate.parse("2026-11-01") else null,
        rrule = if (i == 4) "FREQ=DAILY;COUNT=3" else null, recurrenceMode = if (i == 4) "calendar" else null,
        seriesId = if (i >= 4) id(900) else null, splitFromItemId = if (i == 5) id(104) else null, splitFromSeq = if (i == 5) 2 else null) }
    var occurrences = items.flatMapIndexed { i, item -> (0 until if (i == 4) 2 else 1).map { n -> base.occurrences.single().copy(
        id = id(200 + i * 10 + n), itemId = item.id, seq = n, dueAt = item.dueAt, localDue = item.localDue ?: "2026-10-06T09:00:00".takeIf { i >= 4 }, dueDate = item.dueDate,
        isDetached = i == 1, overrideLocalStart = if (i == 1) "2026-10-06T09:00:00" else null, overrideLocalEnd = if (i == 1) "2026-10-06T10:00:00" else null,
        // All-day/date-only rows have no point anchor; floating DST resolves to the earlier fold.
        startsAt = item.startsAt, endsAt = item.endsAt, localStart = item.localStart, localEnd = item.localEnd).let { if (i == 3) it.copy(localDue = null) else it } } }
    var comments = (0 until if (longThread) 205 else 4).map { n -> base.comments.single().copy(id = id(1000 + n), itemId = items[0].id, authorId = readers[n % 2], body = "Comment $n") }
    var attachments = items.mapIndexed { i, item -> base.attachments.single().copy(id = id(400 + i), itemId = item.id, uploaderId = readers[0]) }
    var lists = listOf(base.lists.single().copy(id = id(500), name = "Shared supplies", ownerId = readers[0],
        propertyEntries = base.lists.single().propertyEntries.map { it.copy(id = id(600), listId = id(500)) }, gotIt = emptyList()),
        SyncBundleFixtures.recording<ListResponse>("list-occurrence-template").copy(id = id(501), itemId = id(104), ownerId = readers[0],
            propertyEntries = base.lists.single().propertyEntries.map { it.copy(id = id(601), listId = id(501)) }, gotIt = emptyList()))
    var settings = readers.associateWith { base.memberSettings.copy(memberId = it) }
    var prefs = readers.associateWith { base.notificationPrefs.copy(memberId = it) }
    var availability = readers.associateWith { base.availability.single().copy(memberId = it) }
    var suggestions = readers.mapIndexed { i, member -> SuggestionResponse(id = id(800 + i), memberId = member, itemId = id(104), occurrenceId = id(240),
        proposedStart = now + 1.days, proposedEnd = now + 1.days + 60.minutes, status = SuggestionStatus.proposed, runId = id(850 + i), version = 1,
        createdAt = now, conflicts = emptyList(), occurrenceVersion = 1) }
    var feeds = readers.mapIndexed { i, member -> Source(id = id(900 + i), householdId = base.items.single().householdId, ownerMemberId = member,
        kind = "subscription", visibility = SourceVisibility.`private`, importKey = "fixture-$i", name = "Calendar $i", url = "https://example.invalid/calendar.ics",
        allowHttp = false, refreshSeconds = 3600, enabled = true, nextFetchAt = now + 1.days, lastWarnings = emptyList(), failureCount = 0, version = 1) }
    var checks = setOf<Pair<String, String>>()
    data class NoticeSource(val itemId: String, val actorId: String)
    data class Notice(val id: String, val recipient: String, val digest: Boolean, val sources: List<NoticeSource>, val version: Int = 1, val readAt: Instant? = null)
    var noticeRows = readers.flatMapIndexed { r, reader -> (0..1).map { n ->
        Notice(id(700 + r * 10 + n), reader, n == 1, (if (n == 0) listOf(id(100)) else listOf(id(100), id(101), id(104))).map { NoticeSource(it, readers[1 - r]) })
    } }
    var cursor = 16L
        private set
    private val continuationScopes = mutableMapOf<String, Pair<ItemAgendaWindow, Int>>()

    fun visible(item: ItemRead, reader: String) = item.deletedAt == null && (item.visibility == "household" || item.ownerId == reader || reader in item.viewers)
    fun visibleItems(reader: String) = items.filter { visible(it, reader) }
    // Independent implementation of core's earlier-fold/first-valid-minute resolver.
    private fun resolve(wall: String, tz: String): Instant {
        var local = LocalDateTime.parse(wall)
        val rules = ZoneId.of(tz).rules
        while (rules.getValidOffsets(local).isEmpty()) local = local.withSecond(0).withNano(0).plusMinutes(1)
        return Instant.parse(local.toInstant(rules.getValidOffsets(local).first()).toString())
    }
    private fun memberZone(member: String) = members.single { it.id == member }.let { if (it.role == "dependent") zone else it.tzid ?: zone }
    private fun related(item: ItemRead, member: String) = member in item.assignees ||
        (item.assignees.isEmpty() && item.ownerId == member) || item.subjectMemberId == member
    private fun anchors(o: OccurrenceRead, tz: String = zone): Pair<Instant?, Instant?> {
        val item = items.single { it.id == o.itemId }
        val start = (if (o.isDetached) o.overrideLocalStart else null) ?: o.localStart
        val end = (if (o.isDetached) o.overrideLocalEnd else null) ?: o.localEnd
        val due = (if (o.isDetached) o.overrideLocalDue else null) ?: o.localDue
        val date = (if (o.isDetached) o.overrideDueDate else null) ?: o.dueDate
        val fixedZone = item.tzid.takeIf { o.isDetached && item.timeMode == "fixed" }
        fun span(zone: String): Pair<Instant, Instant>? {
            if (start == null || end == null) return null
            val a = resolve(start, zone)
            val b = resolve(end, zone)
            val nominal = java.time.Duration.between(LocalDateTime.parse(start), LocalDateTime.parse(end))
            return a to if (b <= a && !nominal.isNegative && !nominal.isZero) a + kotlin.time.Duration.parse("${nominal.toNanos()}ns") else b
        }
        val fixed = fixedZone?.let(::span)
        val local = span(tz)
        val a = fixed?.first ?: o.startsAt ?: (if (fixedZone != null && due != null) resolve(due, fixedZone) else o.dueAt)
            ?: start?.let { resolve(it, tz) } ?: due?.let { resolve(it, tz) } ?: date?.let { resolve("${it}T00:00:00", tz) }
        val b = fixed?.second ?: o.endsAt ?: local?.second ?: end?.let { resolve(it, tz) }
            ?: date?.let { resolve("${java.time.LocalDate.parse(it.toString()).plusDays(1)}T00:00:00", tz) }
        return a to b
    }
    private fun effective(o: OccurrenceRead): Instant? = anchors(o).first
    private fun covered(o: OccurrenceRead, start: Instant, end: Instant): Boolean {
        val (a, b) = anchors(o)
        return a == null || a < end && (b?.let { it > start } ?: (a >= start))
    }
    private fun agendaAnchors(o: OccurrenceRead, member: String): Pair<Instant?, Instant?> = anchors(o, memberZone(member))
    private fun agendaCovered(o: OccurrenceRead, window: ItemAgendaWindow): Boolean {
        val (a, b) = agendaAnchors(o, window.memberId)
        val date = (if (o.isDetached) o.overrideDueDate else null) ?: o.dueDate
        if (date != null && o.startsAt == null && o.dueAt == null && o.localStart == null && o.localDue == null) {
            // PostgreSQL chooses the later fold and the pre-transition offset in a midnight gap.
            fun midnight(day: java.time.LocalDate): Instant {
                val wall = day.atStartOfDay()
                val rules = ZoneId.of(memberZone(window.memberId)).rules
                val offset = rules.getValidOffsets(wall).lastOrNull() ?: rules.getTransition(wall).offsetBefore
                return Instant.parse(wall.toInstant(offset).toString())
            }
            val day = java.time.LocalDate.parse(date.toString())
            val floor = java.time.Instant.parse(window.from.toString()).atZone(ZoneId.of("UTC")).toLocalDate().minusDays(3)
            return day >= floor && midnight(day) < window.to && midnight(day.plusDays(1)) > window.from
        }
        return a != null && a < window.to && (b?.let { it > window.from } ?: (a >= window.from))
    }
    private fun rows(reader: String) = occurrences.filter { o -> items.any { it.id == o.itemId && visible(it, reader) } }
    fun board(reader: String, scope: String = "household"): List<ItemRead> = visibleItems(reader).filter { scope == "household" || it.ownerId == reader || it.subjectMemberId == reader || reader in it.assignees }.flatMap { item ->
        val os = occurrences.filter { it.itemId == item.id && it.disposition != "expired" }
        val open = os.filter { it.status !in setOf("done", "skipped") }.minWithOrNull(compareBy({ it.seq }, { it.id }))
        os.filter { it.id == open?.id || it.status == "done" && it.completedAt?.let { time -> time >= now - 7.days } == true }
            .map { item.copy(occurrence = it) }
    }.sortedWith(compareBy({ it.occurrence!!.status }, { it.occurrence!!.boardPosition }, { it.occurrence!!.id }))
    fun agenda(reader: String, window: ItemAgendaWindow): List<ItemRead> = rows(reader).filter { o -> agendaCovered(o, window) && related(items.single { it.id == o.itemId }, window.memberId) }.map { o ->
        val item = items.single { it.id == o.itemId }
        val at = agendaAnchors(o, window.memberId).first
        val decoration = occurrences.filter { it.id != o.id && effective(it) == at && it.status != "done" && o.status != "done" && item.kind == "appointment" && items.single { item -> item.id == it.itemId }.let { peer -> peer.kind == "appointment" && peer.deletedAt == null && (peer.ownerId == window.memberId || window.memberId in peer.assignees) } }.map { other ->
            val peer = items.single { it.id == other.itemId }
            ConflictRead(window.memberId, o.id, if (visible(peer, reader)) peer.title else "Busy", at!!, o.endsAt ?: at + 1.days, "hard",
                otherOccurrenceId = other.id.takeIf { visible(peer, reader) }, otherItemId = peer.id.takeIf { visible(peer, reader) })
        }
        val load = if (availability.getValue(window.memberId).windows.isEmpty() && o.status != "done") listOf(
            ConflictRead(window.memberId, o.id, "Busy", at!!, at + 1.days, "load")) else emptyList()
        item.copy(occurrence = o, agenda = SyncBundleFixtures.agenda.agenda!!.copy(memberId = window.memberId, itemId = item.id, occurrenceId = o.id,
            startsAt = at.takeIf { o.startsAt != null || o.localStart != null }, endsAt = agendaAnchors(o, window.memberId).second.takeIf { o.endsAt != null || o.localEnd != null }, dueAt = if (o.startsAt == null && o.localStart == null && o.dueDate == null) at else null, dueDate = o.dueDate), conflicts = decoration + load)
    }.sortedWith(compareBy({ it.agenda?.dueAt?.toString() ?: it.agenda?.dueDate?.toString().orEmpty() }, { it.agenda!!.occurrenceId }))

    fun list(reader: String, listId: String, occurrence: String? = null): ListResponse {
        val row = lists.singleOrNull { it.id == listId && (it.itemId == null || items.any { i -> i.id == it.itemId && visible(i, reader) }) } ?: missing()
        if (occurrence == null || row.mode == ListMode.persistent) return row
        val o = rows(reader).singleOrNull { it.id == occurrence } ?: missing()
        require(o.itemId == row.itemId || items.single { it.id == o.itemId }.splitFromItemId == row.itemId)
        val entries = (row.propertyEntries + row.gotIt).map { it.copy(occurrenceId = occurrence, checkedAt = if (it.id to occurrence in checks) now else null,
            checkedBy = readers[0].takeIf { _ -> it.id to occurrence in checks }) }
        // Per-occurrence checks remain in entries; got_it belongs only to persistent lists.
        return row.copy(occurrenceId = occurrence, propertyEntries = entries, gotIt = emptyList())
    }
    private fun notices(reader: String): List<Notification> = noticeRows.filter { it.recipient == reader }.mapNotNull { row ->
        val sources = row.sources.filter { source -> items.any { it.id == source.itemId && visible(it, reader) } }
        if (sources.isEmpty()) null else {
            val actors = sources.map { source -> members.single { it.id == source.actorId }.displayName }.distinct()
            Notification(row.id, if (row.digest) "digest" else "comment", if (row.digest) "digest" else "activity", "item", sources.first().itemId,
                sources.size, NotificationSummary("${sources.size} updates by ${actors.joinToString()}", "Household activity", "${sources.size} updates", "activity", false),
                now, now, row.version, itemId = sources.first().itemId, actorMemberId = sources.first().actorId, actorLabel = actors.first(), releasedAt = now, readAt = row.readAt)
        }
    }
    fun snapshot(reader: String): SnapshotRead {
        val visible = visibleItems(reader).mapTo(mutableSetOf()) { it.id }
        val os = rows(reader).filter { covered(it, from, to) }
        return base.copy(cursor = cursor, members = members, items = visibleItems(reader), occurrences = os,
            comments = comments.filter { it.itemId in visible }, attachments = attachments.filter { it.itemId in visible },
            lists = lists.filter { it.itemId == null || it.itemId in visible }, memberSettings = settings.getValue(reader), notificationPrefs = prefs.getValue(reader),
            icsSources = feeds.filter { it.ownerMemberId == reader }.map { SourceSnapshot(it.id, it.name, it.enabled, it.version, "example.invalid", it.lastStatus, it.lastFetchAt) },
            suggestions = suggestions.filter { it.memberId == reader && it.itemId in visible }.map { s -> s.copy(occurrenceVersion = occurrences.singleOrNull { it.id == s.occurrenceId }?.version) },
            notifications = notices(reader), availability = listOf(availability.getValue(reader)), scheduledReminders = os.filter { o ->
                o.status !in setOf("done", "skipped") && o.disposition != "expired" && related(items.single { it.id == o.itemId }, reader)
            }.mapNotNull { o ->
                val anchor = agendaAnchors(o, reader).first ?: return@mapNotNull null
                val fire = anchor - 15.minutes
                if (fire < from || fire >= to || fire <= now) return@mapNotNull null
                base.scheduledReminders.single().copy(id = id(3000 + occurrences.indexOf(o)), itemId = o.itemId, occurrenceId = o.id, memberId = reader,
                    reminderId = items.single { it.id == o.itemId }.reminders.single().id, fireAt = fire) })
    }
    private fun parseWindow(s: String): ItemAgendaWindow {
        val parts = s.split(':', limit = 2)
        // RFC3339 strings can contain colons; the boundary between the two is the terminating Z.
        val rest = parts[1]; val boundary = rest.indexOf("Z:")
        val window = ItemAgendaWindow(parts[0], Instant.parse(rest.substring(0, boundary + 1)), Instant.parse(rest.substring(boundary + 2)))
        if (window.to <= window.from || window.to - window.from > 730.days) throw ApiProblem("validation-failed", 422, "Agenda window exceeds the wire limit")
        return window
    }
    fun bundle(reader: String, itemId: String, url: Url): ItemSyncRead {
        val snap = snapshot(reader)
        if (snap.items.none { it.id == itemId }) missing()
        val continued = url.parameters["comments_cursor"] != null || url.parameters["agenda_cursor"] != null
        val windows = if (continued) url.parameters.getAll("agenda_cursor").orEmpty().map { continuationScopes.getValue(it) }
            else url.parameters.getAll("agenda").orEmpty().map { parseWindow(it) to 0 }
        val covers = mutableListOf<AgendaCoverage>()
        val agendas = windows.map { (w, offset) ->
            val rows = agenda(reader, w).filter { it.id == itemId }
            val part = rows.drop(offset).take(1)
            val input = if (offset == 0) null else "${w.memberId}/${w.from}/${w.to}/$offset"
            val next = if (offset + part.size < rows.size) "${w.memberId}/${w.from}/${w.to}/${offset + part.size}" else null
            if (next != null) continuationScopes[next] = w to offset + part.size
            covers += AgendaCoverage(w.memberId, w.from, w.to, next == null, zone, local(w.from), local(w.to), cursor = input, nextCursor = next)
            SyncAgenda(w.memberId, w.from, w.to, part, zone, local(w.from), local(w.to))
        }
        val collaboration = !continued && url.parameters["include"] == "collaboration"
        val input = url.parameters["comments_cursor"]
        val hasComments = collaboration || input != null
        val cs = if (hasComments) snap.comments.filter { it.itemId == itemId && (input == null || it.id > input) }.sortedBy { it.id } else emptyList()
        val part = cs.take(200)
        val next = part.lastOrNull()?.id.takeIf { cs.size > part.size }
        val proto = SyncBundleFixtures.bundle(snap.copy(items = snap.items.sortedBy { it.id != itemId }), board(reader).filter { it.id == itemId }, emptyList(), collaboration)
        val inherited = snap.lists.filter { it.itemId == itemId || (it.mode == ListMode.per_occurrence && it.itemId == snap.items.single { row -> row.id == itemId }.splitFromItemId) }
        return proto.copy(windowFrom = from, windowTo = to, coverage = proto.coverage.copy(continued = continued, itemSections = !continued,
            occurrences = SyncWindow(from, to, zone, local(from), local(to)), scheduledReminders = SyncWindow(from, to), agenda = covers,
            suggestions = !continued, collaboration = collaboration, comments = hasComments, commentsCursor = input, commentsComplete = next == null),
            items = if (continued) null else snap.items.filter { it.id == itemId }, occurrences = if (continued) null else snap.occurrences.filter { it.itemId == itemId },
            scheduledReminders = if (continued) null else snap.scheduledReminders.filter { it.itemId == itemId }, board = if (continued) null else board(reader).filter { it.id == itemId },
            agenda = agendas, suggestions = if (continued) null else snap.suggestions.filter { it.itemId == itemId }, comments = if (hasComments) part else null, commentsNextCursor = next,
            attachments = if (collaboration) snap.attachments.filter { it.itemId == itemId } else null, lists = if (collaboration) inherited else null)
    }
    fun response(reader: String, url: Url): JsonObject {
        val path = url.encodedPath
        val snap by lazy { snapshot(reader) }
        return when {
            path == "/v1/snapshot" -> encode(snap)
            path == "/v1/board" -> paged(board(reader, url.parameters["scope"] ?: "household"), url)
            path == "/v1/agenda" -> {
                val window = ItemAgendaWindow(url.parameters["member_id"] ?: reader, Instant.parse(url.parameters["from"]!!), Instant.parse(url.parameters["to"]!!))
                if (window.to <= window.from || window.to - window.from > 730.days) throw ApiProblem("validation-failed", 422, "Agenda window exceeds the wire limit")
                paged(agenda(reader, window), url)
            }
            path.endsWith("/sync") -> encode(bundle(reader, path.split('/')[3], url))
            path.endsWith("/occurrences") -> {
                val parent = path.split('/')[3]
                if (snap.items.none { it.id == parent }) missing()
                page(snap.occurrences.filter { it.itemId == parent }.map { o ->
                    JsonObject(encode(o) + mapOf("capabilities" to JsonArray(snap.items.single { it.id == parent }.capabilities.map(::JsonPrimitive)),
                        "pending_derivation" to JsonPrimitive(false), "conflicts" to JsonArray(emptyList())))
                })
            }
            path.startsWith("/v1/items/") && !path.endsWith("/comments") -> {
                val item = snap.items.singleOrNull { it.id == path.split('/')[3] } ?: missing()
                JsonObject(encode(SyncBundleFixtures.recording<ItemResponse>("item")) + encode(item))
            }
            path.endsWith("/comments") -> paged(snap.comments.filter { it.itemId == path.split('/')[3] && matchesIds(it.id, url) }, url, ascending = true)
            path == "/v1/notifications" -> paged(snap.notifications.filter { matchesIds(it.id, url) }, url)
            path == "/v1/members" -> page(members.map { encode(it) })
            path == "/v1/me/settings" -> encode(settings.getValue(reader))
            path == "/v1/me/notification-prefs" -> encode(prefs.getValue(reader))
            path.endsWith("/availability") -> encode(availability.getValue(path.split('/')[3]))
            path == "/v1/lists" -> page(snap.lists.map { encode(it) })
            path.startsWith("/v1/lists/") -> encode(list(reader, path.split('/')[3], url.parameters["occurrence_id"]))
            path == "/v1/feeds" -> paged(feeds.filter { it.ownerMemberId == reader }, url)
            path == "/v1/suggestions" -> paged(snap.suggestions, url)
            else -> error("Unexpected model route $path")
        }
    }
    private fun matchesIds(id: String, url: Url) = url.parameters.getAll("ids")?.flatMap { it.split(',') }?.contains(id) ?: true
    private inline fun <reified T> paged(rows: List<T>, url: Url, ascending: Boolean = false): JsonObject {
        val offset = url.parameters["cursor"]?.toInt() ?: 0
        val limit = if (ascending) 200 else 3
        return page(rows.drop(offset).take(limit).map { encode(it) }, (offset + limit).toString().takeIf { offset + limit < rows.size })
    }
    private fun missing(): Nothing = throw ApiProblem("not-found", 404, "Not found")
    private inline fun <reified T> encode(value: T) = WireJson.encodeToJsonElement(value).jsonObject
    private fun page(items: List<JsonObject>, next: String? = null) = buildJsonObject { put("items", JsonArray(items)); put("next_cursor", next?.let(::JsonPrimitive) ?: JsonNull) }

    enum class Kind { TITLE, COMPLETE, COMMENT, COMMENT_EDIT, COMMENT_DELETE, ATTACHMENT, LIST, CHECK, ACL, REPARENT, PENDING, DERIVED, RENAME, SETTINGS, PREFS, AVAILABILITY, NOTICE_DELETE, NOTICE_READ, FEED, SUGGESTION, PURGE, RESET }
    data class Mutation(val kind: Kind, val actor: Int, val target: Int)
    /** Domain tickles plus conservative read-projection dependency closure from the documented producers. */
    fun mutate(m: Mutation): Map<String, List<StreamFrame>> {
        val before = readers.associateWith { snapshot(it) }
        val beforeAgendas = readers.associateWith { reader -> readers.flatMap { member -> agenda(reader, ItemAgendaWindow(member, from, to)) }.groupBy { it.id } }
        val index = Math.floorMod(m.target, items.size)
        val target = items[index]
        val actor = readers[m.actor % readers.size]
        when (m.kind) {
            Kind.TITLE -> items = items.map { if (it.id == target.id) it.copy(title = "Edit ${cursor}", version = it.version + 1) else it }
            Kind.COMPLETE -> occurrences = occurrences.map { if (it.itemId == target.id) it.copy(status = if (it.status == "done") "todo" else "done", completedAt = now.takeIf { _ -> it.status != "done" }, completedBy = actor.takeIf { _ -> it.status != "done" }, closedAt = now.takeIf { _ -> it.status != "done" }, version = it.version + 1) else it }
            Kind.COMMENT -> {
                comments = comments + base.comments.single().copy(id = id(10000 + cursor.toInt()), itemId = target.id, authorId = actor, body = "Edit $cursor")
                readers.filter { it != actor && visible(target, it) && prefs.getValue(it).prefs.activity }.forEach { recipient ->
                    val source = NoticeSource(target.id, actor)
                    noticeRows = noticeRows.map { if (it.recipient == recipient && it.digest) it.copy(sources = it.sources + source, version = it.version + 1) else it } +
                        Notice(id(20000 + cursor.toInt()), recipient, false, listOf(source))
                }
            }
            Kind.COMMENT_EDIT, Kind.COMMENT_DELETE -> {
                val comment = comments.lastOrNull { it.itemId == target.id }
                if (comment != null) comments = comments.map { if (it.id == comment.id) it.copy(body = if (m.kind == Kind.COMMENT_DELETE) "" else "Edited $cursor",
                    deletedAt = now.takeIf { m.kind == Kind.COMMENT_DELETE }, editedAt = now, version = it.version + 1) else it }
            }
            Kind.ATTACHMENT -> attachments = if (attachments.any { it.itemId == target.id }) attachments.filterNot { it.itemId == target.id } else attachments + base.attachments.single().copy(id = id(400 + index), itemId = target.id)
            Kind.LIST -> lists = lists.map { if (it.id == id(501)) it.copy(name = "List $cursor", version = it.version + 1) else it }
            Kind.CHECK -> { val key = id(601) to id(250); checks = if (key in checks) checks - key else checks + key }
            Kind.ACL -> items = items.map { if (it.id == target.id) it.copy(visibility = if (it.visibility == "household") "private" else "household", assignees = emptyList(), version = it.version + 1) else it }
            Kind.REPARENT -> occurrences = occurrences.map { if (it.id == id(241)) it.copy(itemId = if (it.itemId == id(104)) id(105) else id(104), version = it.version + 1) else it }
            Kind.PENDING, Kind.DERIVED -> items = items.map { if (it.id == target.id) it.copy(pendingDerivation = m.kind == Kind.PENDING) else it }
            Kind.RENAME -> members = members.map { if (it.id == actor) it.copy(displayName = "Member $cursor", version = it.version + 1) else it }
            Kind.SETTINGS -> settings = settings + (actor to settings.getValue(actor).let { it.copy(version = it.version + 1, settings = it.settings.copy(thisWeekDays = if (it.settings.thisWeekDays == 7) 5 else 7)) })
            Kind.PREFS -> prefs = prefs + (actor to prefs.getValue(actor).let { it.copy(version = it.version + 1, prefs = it.prefs.copy(activity = !it.prefs.activity)) })
            Kind.AVAILABILITY -> availability = availability + (actor to availability.getValue(actor).let { it.copy(version = it.version + 1, windows = if (it.windows.isEmpty()) listOf(Window(2, "08:00:00", "18:00:00")) else emptyList()) })
            Kind.NOTICE_DELETE -> noticeRows = noticeRows.filterNot { it.id == id(700 + readers.indexOf(actor) * 10) }
            Kind.NOTICE_READ -> noticeRows = noticeRows.map { if (it.recipient == actor && it.readAt == null) it.copy(readAt = now, version = it.version + 1) else it }
            Kind.FEED -> feeds = feeds.map { if (it.ownerMemberId == actor) it.copy(enabled = !it.enabled, version = it.version + 1) else it }
            Kind.SUGGESTION -> suggestions = suggestions.map { if (it.memberId == actor) it.copy(status = if (it.status == SuggestionStatus.proposed) SuggestionStatus.dismissed else SuggestionStatus.proposed, version = it.version + 1) else it }
            Kind.PURGE -> { items = items.map { if (it.id == target.id) it.copy(deletedAt = now) else it }; comments = comments.filterNot { it.itemId == target.id }; attachments = attachments.filterNot { it.itemId == target.id } }
            Kind.RESET -> {
                cursor += 2
                if (itemCount > 200) items = items.map { it.copy(notes = "Bulk projection edit $cursor", version = it.version + 1) }
            }
        }
        fun tick(kind: String, type: String, id: String, item: String? = null, list: String? = null, evicted: Boolean = false): StreamFrame.Tickle {
            cursor += 2 // Filtered rows and publication gaps are legal.
            return StreamFrame.Tickle(cursor, kind, type, id, evicted, item, list)
        }
        return readers.associateWith { reader ->
            val frames = mutableListOf<StreamFrame>()
            val old = before.getValue(reader)
            val after = snapshot(reader)
            val wasVisible = old.items.any { it.id == target.id }
            val isVisible = after.items.any { it.id == target.id }
            when (m.kind) {
                Kind.TITLE, Kind.PENDING -> if (isVisible) frames += tick("item.changed", "item", target.id)
                Kind.DERIVED -> if (isVisible) frames += tick("derived", "item", target.id)
                Kind.COMPLETE -> if (isVisible) occurrences.filter { it.itemId == target.id }.forEach { frames += tick("occurrence.changed", "occurrence", it.id, target.id) }
                Kind.COMMENT -> if (isVisible) frames += tick("comment.changed", "comment", comments.last().id, target.id)
                Kind.COMMENT_EDIT, Kind.COMMENT_DELETE -> comments.lastOrNull { it.itemId == target.id }?.let { if (isVisible) frames += tick("comment.changed", "comment", it.id, target.id) }
                Kind.ATTACHMENT -> if (isVisible) frames += tick("attachment.changed", "attachment", id(400 + index), target.id)
                Kind.LIST -> if (after.lists.any { it.id == id(501) }) frames += tick("list.changed", "list", id(501), id(104))
                Kind.CHECK -> if (after.lists.any { it.id == id(501) }) frames += tick("list_entry.changed", "list_entry", id(601), id(104), id(501))
                Kind.ACL, Kind.PURGE -> if (isVisible || wasVisible) frames += tick("item.access_changed", "item", target.id, evicted = !isVisible)
                Kind.REPARENT -> occurrences.single { it.id == id(241) }.let { o -> if (after.items.any { it.id == o.itemId }) frames += tick("occurrence.changed", "occurrence", o.id, o.itemId) }
                Kind.RENAME -> frames += tick("member.changed", "member", actor)
                Kind.SETTINGS -> if (reader == actor) frames += tick("settings.changed", "member_settings", actor)
                Kind.PREFS -> if (reader == actor) frames += tick("settings.changed", "notification_prefs", actor)
                Kind.AVAILABILITY -> if (reader == actor) frames += tick("settings.changed", "availability", actor)
                Kind.NOTICE_DELETE, Kind.NOTICE_READ -> Unit
                Kind.FEED -> if (reader == actor) frames += tick("ics_source.changed", "ics_source", feeds.single { it.ownerMemberId == actor }.id)
                Kind.SUGGESTION -> {
                    if (reader == actor) frames += tick("suggestion.changed", "suggestion", suggestions.single { it.memberId == actor }.id)
                    if (after.items.any { it.id == id(104) }) frames += tick("item.invalidated", "item", id(104))
                }
                Kind.RESET -> frames += StreamFrame.ProjectionChanged()
            }
            // Compare model read projections to enumerate exactly which peers/notifications the producer must invalidate.
            if (m.kind in setOf(Kind.TITLE, Kind.COMPLETE, Kind.ACL, Kind.PURGE, Kind.AVAILABILITY, Kind.REPARENT)) {
                val previous = beforeAgendas.getValue(reader)
                val current = readers.flatMap { agenda(reader, ItemAgendaWindow(it, from, to)) }.groupBy { it.id }
                current.keys.filter { current[it] != previous[it] }.forEach { id ->
                    frames += tick("item.invalidated", "item", id)
                }
            }
            if (m.kind in setOf(Kind.ACL, Kind.PURGE, Kind.LIST)) after.items.filter { it.splitFromItemId == target.id || (m.kind == Kind.LIST && it.splitFromItemId == id(104)) }
                .forEach { frames += tick("item.invalidated", "item", it.id) }
            val oldNotices = old.notifications.associateBy { it.id }
            val newNotices = after.notifications.associateBy { it.id }
            (oldNotices.keys + newNotices.keys).filter { oldNotices[it] != newNotices[it] }.forEach { id ->
                val kind = when {
                    id !in oldNotices && m.kind == Kind.COMMENT -> "notification.created"
                    m.kind == Kind.COMMENT -> "notification.updated"
                    m.kind == Kind.NOTICE_READ -> "notification.dismissed"
                    else -> "notification.invalidated"
                }
                frames += tick(kind, "notification", id)
            }
            frames
        }
    }
}
