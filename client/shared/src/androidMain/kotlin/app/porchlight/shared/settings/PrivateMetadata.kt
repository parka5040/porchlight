package app.porchlight.shared.settings

import android.content.Context
import java.io.File

/** One owner per metadata file, shared by cold receivers, workers and the application. */
fun privateMetadata(context: Context, name: String): PrivateMetadataStore = synchronized(stores) {
    val file = File(context.applicationContext.noBackupFilesDir, "$name.json")
    stores.getOrPut(file.absolutePath) {
        val legacy = context.applicationContext.getSharedPreferences(name, Context.MODE_PRIVATE)
        PrivateMetadataStore(file, { legacy.all }, { check(legacy.edit().clear().commit()) })
    }
}
private val stores = mutableMapOf<String, PrivateMetadataStore>()

/** Cold Android entry points share recovery state even before AppServices has been initialized. */
fun privateStorageBoundary(context: Context): PrivateStorageBoundary = synchronized(boundaries) {
    boundaries.getOrPut(context.applicationContext.noBackupFilesDir.absolutePath) { PrivateStorageBoundary() }
}
private val boundaries = mutableMapOf<String, PrivateStorageBoundary>()
