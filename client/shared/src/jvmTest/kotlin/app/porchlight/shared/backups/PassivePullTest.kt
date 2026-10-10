package app.porchlight.shared.backups

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.ConnectFailure
import kotlinx.coroutines.test.runTest
import java.nio.file.Files
import java.security.MessageDigest
import kotlin.test.*
import kotlin.time.Instant

class PassivePullTest {
    private val now = Instant.parse("2026-10-06T05:00:00Z")
    private fun bundle(bytes: ByteArray, id: String = "one", time: Instant = now) = Bundle(id, "porchlight-$id.tar.age", bytes.size.toLong(),
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }, time, time, time)
    private class Fake(var metadata: Bundle, var bytes: ByteArray) : BackupSource {
        var allowed = true; var statusCalls = 0; val offsets = mutableListOf<Long>(); var interrupt = false; var full = false; var badRange = false; var endSession = false
        var offline = false
        override suspend fun authorizedHousehold(): String? { if (offline) throw ConnectFailure(); return if (allowed) "household" else null }
        override fun ensureAuthorized() { if (!allowed) throw app.porchlight.shared.data.HouseholdLocked() }
        override suspend fun status(): BackupStatus { statusCalls++; return BackupStatus(latest = metadata) }
        override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
            offsets += offset
            val start = if (full) 0L else offset
            begin(DownloadHead(if (start == 0L) 200 else 206, "\"${metadata.sha256}\"", bytes.size.toLong() - start,
                if (start == 0L) null else if (badRange) "bytes 0-1/2" else "bytes $start-${bytes.lastIndex}/${bytes.size}"))
            if (interrupt) { interrupt = false; chunk(bytes.copyOfRange(offset.toInt(), offset.toInt() + 2)); throw ConnectFailure() }
            chunk(bytes.copyOfRange(start.toInt(), bytes.size))
            if (endSession) allowed = false
        }
    }
    private suspend fun fixture(test: suspend (java.nio.file.Path) -> Unit) {
        val root = Files.createTempDirectory(Files.createDirectories(java.nio.file.Path.of(System.getProperty("porchlight.test.dir"))), "pull-")
        try { test(root) } finally { root.toFile().deleteRecursively() }
    }
    @Test fun deniedDoesNotFetchStatusOrBytes() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray()).apply { allowed = false }
        val pull = PassivePull(root, fake, { now }, 2, { java.time.ZoneId.of("UTC") })
        pull.pull(); assertEquals(0, fake.statusCalls); assertTrue(fake.offsets.isEmpty()); assertFalse(pull.state.value.allowed)
    } }
    @Test fun interruptionResumesByRangeAndVerifiesBeforePublishing() = runTest { fixture { root ->
        val bytes = "encrypted backup".toByteArray(); val fake = Fake(bundle(bytes), bytes).apply { interrupt = true }
        val pull = PassivePull(root, fake, { now }, 2, { java.time.ZoneId.of("UTC") })
        pull.pull(); assertTrue(pull.state.value.cantReachHome); assertEquals(0, root.toFile().walk().count { it.extension == "age" })
        // Reconstruct to prove checkpoint survives restart.
        val restarted = PassivePull(root, fake, { now }, 2, { java.time.ZoneId.of("UTC") }); restarted.pull()
        assertEquals(listOf(0L, 2L), fake.offsets); assertFalse(restarted.state.value.cantReachHome)
        assertContentEquals(bytes, root.toFile().walk().single { it.extension == "age" }.readBytes())
    } }
    @Test fun checksumMismatchKeepsCheckpointUntilValidatedReplacement() = runTest { fixture { root ->
        val bytes = "correct".toByteArray(); val fake = Fake(bundle(bytes), "corrupt".toByteArray())
        val pull = PassivePull(root, fake, { now }, 2, { java.time.ZoneId.of("UTC") }); pull.pull()
        assertNotNull(pull.state.value.error); assertEquals(0, root.toFile().walk().count { it.extension == "age" })
        assertEquals("corrupt", root.toFile().walk().single { it.extension == "part" }.readText())
        fake.bytes = bytes; pull.pull(); assertEquals(listOf(0L, 0L), fake.offsets)
    } }
    @Test fun changedLatestRestartsAndRetentionKeepsNewestN() = runTest { fixture { root ->
        val fake = Fake(bundle("one".toByteArray()), "one".toByteArray()).apply { interrupt = true }
        val pull = PassivePull(root, fake, { now }, 2, { java.time.ZoneId.of("UTC") }); pull.pull()
        for (i in 2..4) { fake.bytes = "bundle$i".toByteArray(); fake.metadata = bundle(fake.bytes, "id$i", Instant.fromEpochSeconds(now.epochSeconds + i)); pull.pull() }
        assertEquals(listOf(0L, 0L, 0L, 0L), fake.offsets)
        val files = root.toFile().walk().filter { it.extension == "age" }.toList()
        assertEquals(2, files.size); assertEquals(setOf("bundle3", "bundle4"), files.map { it.readText() }.toSet())
    } }
    @Test fun endedSessionCannotPublishCompletedBytesOrStatus() = runTest { fixture { root ->
        val bytes = "data".toByteArray(); val fake = Fake(bundle(bytes), bytes).apply { endSession = true }
        val pull = PassivePull(root, fake, { now }, 2); pull.pull()
        assertEquals(0, root.toFile().walk().count { it.extension == "age" }); assertFalse(pull.state.value.allowed)
    } }
    @Test fun fullResponseRestartsPartialAndWrongRangeNeverAppends() = runTest { fixture { root ->
        val bytes = "backup data".toByteArray(); val fake = Fake(bundle(bytes), bytes).apply { interrupt = true }
        val pull = PassivePull(root, fake, { now }, 2); pull.pull()
        fake.badRange = true; pull.pull(); assertNotNull(pull.state.value.error)
        assertEquals(2L, root.toFile().walk().single { it.extension == "part" }.length())
        fake.full = true; pull.pull(); assertContentEquals(bytes, root.toFile().walk().single { it.extension == "age" }.readBytes())
        assertEquals(listOf(0L, 2L, 2L), fake.offsets)
    } }
    @Test fun symlinkStorageIsRejectedWithoutTouchingOtherDirectory() = runTest { fixture { root ->
        val outside = root.resolve("outside"); Files.createDirectory(outside)
        val link = root.resolve("backups"); Files.createSymbolicLink(link, outside)
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        val pull = PassivePull(link, fake, { now }, 2); pull.pull()
        assertNotNull(pull.state.value.error); assertEquals(0, fake.statusCalls); assertEquals(0, outside.toFile().listFiles()!!.size)
        Files.delete(link)
    } }
    @Test fun recoveredNonDueOutageDoesNotStartADownloadLater() = runTest { fixture { root ->
        var clock = now; val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        val pull = PassivePull(root, fake, { clock }, jitter = { 1.0 }); pull.tick()
        fake.offline = true; pull.tick(); fake.offline = false
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 60); pull.tick()
        assertFalse(pull.state.value.cantReachHome); assertEquals(1, fake.statusCalls)
    } }
    @Test fun completedOrphanIsPrunedButUnknownFilesAreUntouched() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray()); val pull = PassivePull(root, fake, { now })
        pull.pull()
        val old = root.toFile().walk().single { it.extension == "age" }
        root.toFile().walk().single { it.extension == "json" }.delete()
        val unknown = old.parentFile.toPath().resolve("operator.tar.age"); Files.writeString(unknown, "not managed")
        fake.metadata = bundle(fake.bytes, "new"); pull.pull()
        assertFalse(old.exists()); assertEquals("not managed", Files.readString(unknown))
    } }
    @Test fun cleanupPreservesUnownedFilesEvenWithManagedNamesAndBundleReceipts() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        PassivePull(root, fake, { now }, 1).pull()
        val dir = root.toFile().walk().single { it.extension == "age" }.toPath().parent
        val old = bundle("foreign".toByteArray(), "foreign", Instant.fromEpochSeconds(now.epochSeconds - 10))
        val stem = MessageDigest.getInstance("SHA-256").digest(old.id.toByteArray()).joinToString("") { "%02x".format(it) } + "-" + old.sha256
        val unknown = mapOf(
            "notes.part" to "operator checkpoint",
            "$stem.part" to "foreign checkpoint",
            "operator.json" to app.porchlight.shared.transport.WireJson.encodeToString(old),
            "operator.tar.age" to "operator ciphertext",
            "$stem.json" to app.porchlight.shared.transport.WireJson.encodeToString(old),
            "$stem.tar.age" to "foreign ciphertext",
            "${"a".repeat(64)}-${"b".repeat(64)}.tar.age" to "foreign orphan")
        unknown.forEach { (name, bytes) -> Files.writeString(dir.resolve(name), bytes) }
        fake.metadata = bundle(fake.bytes, "new")
        PassivePull(root, fake, { now }, 1).pull() // reload ownership after restart
        unknown.forEach { (name, bytes) ->
            val original = dir.resolve(name)
            val saved = if (Files.exists(original)) original else Files.list(dir).use { files ->
                files.filter { it.fileName.toString().startsWith("$name.legacy-") }.toList().single()
            }
            assertEquals(bytes, Files.readString(saved), name)
        }
    } }
    @Test fun replacingAnOwnedBundleDoesNotTransferOwnershipToTheReplacement() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        PassivePull(root, fake, { now }, 1).pull()
        val old = root.toFile().walk().single { it.extension == "age" }.toPath()
        Files.move(old, old.resolveSibling("operator-saved.age"))
        Files.writeString(old, "operator replacement") // distinct file identity, same recorded name
        fake.metadata = bundle(fake.bytes, "new", Instant.fromEpochSeconds(now.epochSeconds + 1))
        val restarted = PassivePull(root, fake, { now }, 1); restarted.pull()
        assertNull(restarted.state.value.error)
        assertFalse(Files.exists(old))
        val saved = Files.list(old.parent).use { files -> files.filter { it.fileName.toString().startsWith("${old.fileName}.legacy-") }.toList().single() }
        assertEquals("operator replacement", Files.readString(saved))
        assertTrue(restarted.state.value.notice!!.contains(saved.toString()))
    } }
    @Test fun unownedCurrentCheckpointIsNeitherResumedNorOverwritten() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        PassivePull(root, fake, { now }).pull()
        val dir = root.toFile().walk().single { it.extension == "age" }.toPath().parent
        fake.metadata = bundle(fake.bytes, "new")
        val stem = MessageDigest.getInstance("SHA-256").digest(fake.metadata.id.toByteArray()).joinToString("") { "%02x".format(it) } + "-" + fake.metadata.sha256
        val foreign = dir.resolve("$stem.part"); Files.writeString(foreign, "xy")
        val restarted = PassivePull(root, fake, { now }); restarted.pull()
        assertNull(restarted.state.value.error)
        assertEquals(listOf(0L, 0L), fake.offsets) // foreign bytes were not resumed
        assertFalse(Files.exists(foreign))
        val saved = Files.list(dir).use { files -> files.filter { it.fileName.toString().startsWith("${foreign.fileName}.legacy-") }.toList().single() }
        assertEquals("xy", Files.readString(saved))
        assertEquals("Preserved unverified backup file ${foreign.fileName} as $saved", restarted.state.value.notice)
    } }
    @Test fun manualProcessLockReportsBusyWithoutDownloading() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray()); val pull = PassivePull(root, fake, { now }); pull.pull()
        val lock = root.toFile().walk().single { it.name == ".lock" }.toPath()
        java.nio.channels.FileChannel.open(lock, java.nio.file.StandardOpenOption.WRITE).use { channel ->
            channel.lock().use { pull.pull(); assertTrue(pull.state.value.error!!.contains("Another desktop process")); assertEquals(1, fake.offsets.size) }
        }
    } }
    @Test fun oneSuccessfulNonDueCheckClearsOutage() = runTest { fixture { root ->
        val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        val pull = PassivePull(root, fake, { now }); pull.tick()
        fake.offline = true; pull.tick(); assertTrue(pull.state.value.cantReachHome)
        fake.offline = false; pull.tick()
        assertFalse(pull.state.value.cantReachHome); assertNull(pull.state.value.error)
        assertEquals(1, fake.statusCalls) // recovery did not trigger an unnecessary bundle hash/download
    } }
    @Test fun persistentFailureBacksOffWithCapJitterAndResetsOnSuccess() = runTest { fixture { root ->
        var clock = now
        val bytes = "correct".toByteArray(); val fake = Fake(bundle(bytes), "corrupt".toByteArray())
        val pull = PassivePull(root, fake, { clock }, jitter = { 1.0 })
        pull.tick(); assertEquals(1, fake.statusCalls)
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 59); pull.tick(); assertEquals(1, fake.statusCalls)
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 1); pull.tick(); assertEquals(2, fake.statusCalls)
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 119); pull.tick(); assertEquals(2, fake.statusCalls)
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 1); pull.tick(); assertEquals(3, fake.statusCalls)
        for (seconds in listOf(240L, 480L, 960L, 1920L, 3600L, 3600L)) {
            val before = fake.statusCalls
            clock = Instant.fromEpochSeconds(clock.epochSeconds + seconds - 1); pull.tick(); assertEquals(before, fake.statusCalls)
            clock = Instant.fromEpochSeconds(clock.epochSeconds + 1); pull.tick(); assertEquals(before + 1, fake.statusCalls)
        }
        fake.bytes = bytes; pull.pull(); assertNull(pull.state.value.error)
        fake.bytes = "corrupt".toByteArray(); fake.metadata = bundle(bytes, "new")
        pull.pull(); val before = fake.statusCalls
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 60); pull.tick(); assertEquals(before + 1, fake.statusCalls)
    } }
    @Test fun jitterDelaysRetryAndInterruptedPartialResumesAfterBackoff() = runTest { fixture { root ->
        var clock = now; val bytes = "encrypted".toByteArray()
        val fake = Fake(bundle(bytes), bytes).apply { interrupt = true }
        val pull = PassivePull(root, fake, { clock }, jitter = { 1.2 })
        pull.tick(); clock = Instant.fromEpochSeconds(clock.epochSeconds + 71); pull.tick(); assertEquals(listOf(0L), fake.offsets)
        clock = Instant.fromEpochSeconds(clock.epochSeconds + 1); pull.tick(); assertEquals(listOf(0L, 2L), fake.offsets)
        assertNull(pull.state.value.error)
    } }
    @Test fun nightlyTickUsesInjectedClockAndPersistsDueTime() = runTest { fixture { root ->
        var clock = now; val fake = Fake(bundle("data".toByteArray()), "data".toByteArray())
        PassivePull(root, fake, { clock }, 2, { java.time.ZoneId.of("UTC") }).tick(); assertEquals(1, fake.statusCalls)
        val restarted = PassivePull(root, fake, { clock }, 2, { java.time.ZoneId.of("UTC") }); restarted.tick(); assertEquals(1, fake.statusCalls)
        clock = Instant.fromEpochSeconds(now.epochSeconds + 86401); restarted.tick(); assertEquals(2, fake.statusCalls)
    } }
}
