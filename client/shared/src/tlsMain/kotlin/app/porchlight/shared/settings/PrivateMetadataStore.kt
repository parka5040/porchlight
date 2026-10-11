package app.porchlight.shared.settings

import app.porchlight.shared.transport.WireJson
import kotlinx.serialization.encodeToString
import java.io.File
import java.io.FileOutputStream
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** Small synchronous string/int metadata store. Never reset damaged alarm high-water or encrypted trust metadata. */
class PrivateMetadataStore(private val file: File, legacy: () -> Map<String, *>, clearLegacy: () -> Unit,
                           private val forceDirectory: (File) -> Unit = ::forcePrivateDirectory) {
    private var values: Map<String, String>
    init {
        values = if (file.exists()) WireJson.decodeFromString<Map<String, String>>(file.readText()) else {
            legacy().mapValues { (_, value) ->
                require(value is String || value is Int) { "Unsupported private metadata" }
                value.toString()
            }.also { persist(it) }
        }
        // Also force an existing replacement: a previous attempt may have failed after its rename.
        forceDirectory(file.parentFile)
        clearLegacy()
    }
    @Synchronized fun getString(key: String, default: String?): String? = values[key] ?: default
    @Synchronized fun getInt(key: String, default: Int): Int = values[key]?.toInt() ?: default
    @Synchronized fun contains(key: String) = key in values
    val all: Map<String, String> @Synchronized get() = values.toMap()
    fun edit() = Editor()
    private fun persist(next: Map<String, String>) {
        durablePrivateDirectory(file.parentFile, forceDirectory)
        val pending = File(file.path + ".new")
        FileOutputStream(pending).use { stream ->
            stream.write(WireJson.encodeToString(next).toByteArray()); stream.fd.sync()
        }
        Files.move(pending.toPath(), file.toPath(), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE)
        forceDirectory(file.parentFile)
    }
    inner class Editor {
        private val changes = mutableMapOf<String, String?>()
        private var cleared = false
        fun putString(key: String, value: String?) = apply { changes[key] = value }
        fun putInt(key: String, value: Int) = putString(key, value.toString())
        fun remove(key: String) = putString(key, null)
        fun clear() = apply { cleared = true }
        fun commit(): Boolean = synchronized(this@PrivateMetadataStore) {
            val next = (if (cleared) emptyMap() else values).toMutableMap()
            changes.forEach { (key, value) -> if (value == null) next.remove(key) else next[key] = value }
            persist(next); values = next
            true
        }
    }
}

internal fun forcePrivateDirectory(directory: File) {
    java.nio.channels.FileChannel.open(directory.toPath(), java.nio.file.StandardOpenOption.READ).use { it.force(true) }
}
private fun durablePrivateDirectory(directory: File, force: (File) -> Unit) {
    if (!directory.exists()) {
        directory.parentFile?.let { durablePrivateDirectory(it, force) }
        check(directory.mkdir() || directory.isDirectory)
    }
    // Retrying must also commit directory names created by a previous failed attempt.
    directory.parentFile?.let { force(it) }
}

/** DataStore has one owner; migrate before constructing it, keeping the old file intact on failure. */
fun migratePrivateSettings(legacy: File, destination: File, forceDirectory: (File) -> Unit = ::forcePrivateDirectory): File {
    if (legacy.exists()) {
        durablePrivateDirectory(destination.parentFile, forceDirectory)
        if (!destination.exists()) {
            val pending = File(destination.path + ".new")
            FileOutputStream(pending).use { output ->
                legacy.inputStream().use { it.copyTo(output) }
                output.fd.sync()
            }
            Files.move(pending.toPath(), destination.toPath(), StandardCopyOption.ATOMIC_MOVE)
        }
        forceDirectory(destination.parentFile)
        Files.delete(legacy.toPath())
        forceDirectory(legacy.parentFile)
    }
    return destination
}
