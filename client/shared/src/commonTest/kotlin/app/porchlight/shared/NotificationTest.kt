package app.porchlight.shared

import app.porchlight.shared.api.models.*
import app.porchlight.shared.data.*
import app.porchlight.shared.screens.inbox.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

// Schema-allowed safe S6a shape: summary topic is deliberately NOT the delivery channel.
fun notificationFixture() = WireJson.decodeFromString<Notification>("""{"id":"0199be3c-4800-7000-8000-000000000001","kind":"comment","category":"activity","entity_type":"item","entity_id":"0199be3c-4800-7000-8000-000000000002","count":1,"summary":{"text":"New comment","title":"New comment","body":"A household member commented.","category":"comments","local_alarm":false},"created_at":"2026-10-06T10:00:00Z","release_at":"2026-10-06T10:00:30Z","released_at":"2026-10-06T10:00:30Z","version":1}""")
class NotificationTest {
    private val now = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun userFacingTopicAndReachabilityTimeUseMemberZone() {
        assertEquals("Comments", notificationTopicLabel("comments"))
        assertEquals("Calendar conflicts", notificationTopicLabel("conflicts"))
        assertEquals("Household activity", notificationTopicLabel("unknown"))
        assertEquals("2026-10-06 08:00 (America/New_York)", reachabilityTime(now, "America/New_York"))
    }
    @Test fun snapshotRetainsNotificationAndCanonicalPrefsThenClearsOnIdentityEnd() = runTest {
        val cache = memoryCache { now }
        try {
            cache.install(emptySnapshot(1).copy(notifications = listOf(notificationFixture())))
            assertEquals("activity", cache.view.value.notifications.single().category)
            assertEquals("comments", cache.view.value.notifications.single().summary.category)
            assertEquals("self", cache.view.value.notificationPrefs?.memberId)
            cache.initialize()
            assertEquals(1, cache.view.value.notifications.size)
            cache.clear(); assertTrue(cache.view.value.notifications.isEmpty()); assertNull(cache.view.value.notificationPrefs)
        } finally { cache.close() }
    }
    @Test fun inboxUsesMemberZoneAndTopicNotChannel() {
        val n = notificationFixture()
        val groups = inboxGroups(listOf(n, n.copy(id="y", createdAt=Instant.parse("2026-10-05T10:00:00Z")),
            n.copy(id="w", createdAt=Instant.parse("2026-10-03T10:00:00Z"))), now, "America/New_York")
        assertEquals(listOf("Today", "Yesterday", "This week"), groups.map { it.title })
        assertEquals("comments", groups.first().rows.first().summary.category)
    }
}
