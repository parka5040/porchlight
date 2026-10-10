package app.porchlight.android

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import java.io.File

/** Debug-only deterministic SAF result source; never included in release. */
class CalendarFixtureProvider : ContentProvider() {
    override fun onCreate() = true
    override fun getType(uri: Uri) = "text/calendar"
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor =
        MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME)).apply { addRow(arrayOf("fixture.ics")) }
    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        val file = fixtureFile(uri)
        if (!file.exists()) file.writeText(if (uri.lastPathSegment == "existing.json") "user data" else "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n")
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.parseMode(mode))
    }
    private fun fixtureFile(uri: Uri) = File(context!!.cacheDir, if (uri.lastPathSegment == "existing.json") "saf-existing.json" else "saf-fixture.ics")
    @Suppress("DEPRECATION")
    override fun call(method: String, arg: String?, extras: Bundle?): Bundle? {
        if (method == "android:deleteDocument") {
            extras?.getParcelable<Uri>("uri")?.let { fixtureFile(it).delete() }
            return Bundle()
        }
        return super.call(method, arg, extras)
    }
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
}
