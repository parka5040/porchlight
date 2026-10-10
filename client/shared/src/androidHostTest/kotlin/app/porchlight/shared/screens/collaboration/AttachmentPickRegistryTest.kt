package app.porchlight.shared.screens.collaboration

import kotlin.test.*

/**
 * fbcc m6c-astra C5-06 (Android side): the helper Activity delivers to the session inbox registered at launch, not to a
 * callback owned by the launching composition (which rotation or the ON_STOP PIN relock disposes while the picker is up).
 */
class AttachmentPickRegistryTest {
    private val photo = PickedAttachment("photo.jpg", "image/jpeg", byteArrayOf(1, 2, 3))

    @Test fun resultOutlivesTheLaunchingCompositionAndKeepsItsTarget() {
        val picks = AttachmentPicks()
        val request = picks.begin("item", "comment")
        AttachmentPickRegistry.register(picks, request)
        // The launching composition is gone now (relock/rotation); nothing about it is needed for delivery.
        assertTrue(AttachmentPickRegistry.complete(request.id, photo, null))
        val parked = picks.waiting("item").single()
        assertEquals(request, parked.request)
        assertSame(photo, parked.picked)
        assertFalse(AttachmentPickRegistry.complete(request.id, photo, null), "delivered once")
        assertTrue(picks.take(parked)); assertTrue(picks.waiting("item").isEmpty())
    }

    @Test fun cancelUnknownAndClearedRequestsDeliverNothing() {
        val picks = AttachmentPicks()
        val cancelled = picks.begin("item", null).also { AttachmentPickRegistry.register(picks, it) }
        assertTrue(AttachmentPickRegistry.complete(cancelled.id, null, null))
        assertTrue(picks.waiting("item").isEmpty())
        assertFalse(AttachmentPickRegistry.complete(null, photo, null))
        assertFalse(AttachmentPickRegistry.complete("unknown", photo, null))
        val stale = picks.begin("item", null).also { AttachmentPickRegistry.register(picks, it) }
        picks.clear() // identity change
        assertFalse(AttachmentPickRegistry.complete(stale.id, photo, null))
        assertTrue(picks.waiting("item").isEmpty())
    }

    @Test fun pickerFailureIsParkedForTheItemToShow() {
        val picks = AttachmentPicks()
        val request = picks.begin("item", null).also { AttachmentPickRegistry.register(picks, it) }
        assertTrue(AttachmentPickRegistry.complete(request.id, null, "Attachments must be at most 25 MiB."))
        assertEquals("Attachments must be at most 25 MiB.", picks.waiting("item").single().error)
        assertTrue(picks.waiting("other").isEmpty())
    }
}
