package app.porchlight.shared.data

import app.cash.sqldelight.db.*
import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import app.porchlight.shared.api.models.CommentPage
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class PrerolloutClientSyncFixBPublicationTest {
    private val f = SyncBundleFixtures

    @Test fun narrowRepairsReadOnlyTouchedPayloadsAndRetainUnrelatedObjects() = runTest {
        for (kind in listOf("comment", "comment-page", "comment-thread", "notification", "inbox", "list", "lists", "list-eviction", "settings", "members", "member", "availability", "prefs")) {
            val delegate = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
            PorchlightDb.Schema.create(delegate)
            var watching = false
            val scans = mutableListOf<String>()
            val driver = object : SqlDriver by delegate {
                override fun <R> executeQuery(identifier: Int?, sql: String, mapper: (SqlCursor) -> QueryResult<R>, parameters: Int, binders: (SqlPreparedStatement.() -> Unit)?): QueryResult<R> {
                    if (watching && Regex("SELECT\\s+\\*\\s+FROM\\s+Cache", RegexOption.IGNORE_CASE).containsMatchIn(sql) && !sql.contains("WHERE", true)) scans += sql
                    return delegate.executeQuery(identifier, sql, mapper, parameters, binders)
                }
            }
            val cache = ReadCache(driver) { f.now }
            try {
                val unrelated = f.snapshot.items.single().copy(id = "unrelated")
                cache.install(f.snapshot.copy(items = f.snapshot.items + unrelated))
                val before = cache.view.value
                watching = true
                cache.inSession { fence -> when (kind) {
                    "comment" -> cache.replaceCommentIds(fence, f.snapshot.items.single().id, setOf(f.snapshot.comments.single().id), emptyList())
                    "comment-page" -> cache.putCommentPage(fence, f.snapshot.items.single().id, null, CommentPage(f.snapshot.comments, null))
                    "comment-thread" -> cache.replaceComments(fence, f.snapshot.items.single().id, emptyList())
                    "notification" -> cache.replaceNotificationIds(fence, setOf("notice"), emptyList())
                    "inbox" -> cache.replaceNotifications(fence, emptyList())
                    "lists" -> cache.replaceLists(fence, null, f.snapshot.lists)
                    "list-eviction" -> cache.evictList(f.snapshot.lists.single().id)
                    "member" -> cache.putMember(fence, f.snapshot.members.single())
                    "availability" -> cache.putAvailability(fence, f.snapshot.availability.single())
                    "list" -> cache.putList(fence, f.snapshot.lists.single().copy(name = "Updated"))
                    "settings" -> cache.putSettings(fence, f.snapshot.memberSettings.copy(version = 11))
                    "members" -> cache.replaceMembers(fence, f.snapshot.members.map { it.copy(displayName = "Updated") })
                    else -> cache.putNotificationPrefs(fence, f.snapshot.notificationPrefs)
                } }
                watching = false
                assertTrue(scans.isEmpty(), "$kind unscoped payload reads: $scans")
                assertSame(before.items.single { it.id == unrelated.id }, cache.view.value.items.single { it.id == unrelated.id }, kind)
                if (kind !in setOf("notification", "inbox")) assertSame(before.notifications, cache.view.value.notifications, kind)
                if (kind !in setOf("members", "member")) assertSame(before.members.single(), cache.view.value.members.single(), kind)
            } finally { cache.close() }
        }
    }
}
