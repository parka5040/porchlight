package app.porchlight.android

import android.content.Intent
import android.net.Uri
import android.provider.MediaStore
import androidx.compose.runtime.*
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.unit.Density
import app.porchlight.shared.api.models.*
import app.porchlight.shared.screens.lists.ListContent
import app.porchlight.shared.screens.collaboration.*
import app.porchlight.shared.settings.ThemeMode
import app.porchlight.shared.theme.PorchlightTheme
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import kotlin.time.Instant

/** Managed-device intent contract and scalable content; no live household, sleeps or UI-thread runBlocking. */
class CollaborationInstrumentedTest {
    @get:Rule val compose = createComposeRule()
    private val at = Instant.parse("2026-10-06T12:00:00Z")
    @Test fun pickerAndCameraIntentsUseScopedReadAndPrivateOutput() {
        val photo = photoPickerIntent()
        assertEquals(if (android.os.Build.VERSION.SDK_INT >= 33) MediaStore.ACTION_PICK_IMAGES else Intent.ACTION_OPEN_DOCUMENT, photo.action)
        assertEquals("image/*", photo.type)
        assertEquals(Intent.ACTION_OPEN_DOCUMENT, filePickerIntent().action)
        val output = Uri.parse("content://app.porchlight.attachments/attachments/capture.jpg")
        val camera = cameraIntent(output)
        assertEquals(MediaStore.ACTION_IMAGE_CAPTURE, camera.action)
        assertEquals(output, camera.getParcelableExtra(MediaStore.EXTRA_OUTPUT, Uri::class.java))
        assertEquals(output, camera.clipData?.getItemAt(0)?.uri)
        assertTrue(camera.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0)
    }
    @Test fun bothThemesDoubleScaleListCommentAttachmentRows() {
        val theme = mutableStateOf(ThemeMode.Evening)
        val id = "01999999-0000-7000-8000-000000000010"
        val entry = EntryResponse(id, id, id, "Oat milk", "cartons", "", "a0", 1, at, at, qty = 2.0)
        val list = ListResponse(id = id, householdId = id, ownerId = id, name = "Groceries", mode = ListMode.persistent, visibility = ListVisibility.household, version = 1, createdAt = at, updatedAt = at, viewers = emptyList(), capabilities = listOf("check"), propertyEntries = listOf(entry), gotIt = emptyList())
        val comment = Comment(id, id, "A comment", emptyList(), at, 1, authorId = id)
        val attachment = Attachment(id, id, "notes.txt", at, 1, 5, "text/plain", false)
        compose.setContent { CompositionLocalProvider(LocalDensity provides Density(LocalDensity.current.density, 2f)) { PorchlightTheme(theme.value, false) { Column {
            ListContent(list, emptyList(), { _, _ -> }, {}, { _, _ -> }, {}, {})
            CommentRow(comment, emptyList(), id, false, at, {}, {})
            AttachmentRow(attachment, false, {}, {})
        } } } }
        for (mode in listOf(ThemeMode.Evening, ThemeMode.Daylight)) {
            compose.runOnIdle { theme.value = mode }
            compose.onNodeWithContentDescription("Check Oat milk").assertExists()
            compose.onNodeWithContentDescription("Open notes.txt").assertExists()
            val bounds = compose.onNodeWithContentDescription("Check Oat milk").fetchSemanticsNode().boundsInRoot
            val density = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().targetContext.resources.displayMetrics.density
            assertTrue(bounds.width >= 48 * density && bounds.height >= 48 * density)
        }
    }
}
