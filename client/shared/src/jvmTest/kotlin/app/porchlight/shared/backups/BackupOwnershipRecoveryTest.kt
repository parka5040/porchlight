package app.porchlight.shared.backups

import app.porchlight.shared.api.models.*
import app.porchlight.shared.transport.WireJson
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assume.assumeTrue
import java.io.IOException
import java.nio.file.*
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.PosixFilePermissions
import java.security.MessageDigest
import kotlin.test.*
import kotlin.time.Instant

class BackupOwnershipRecoveryTest {
    private val now = Instant.parse("2026-10-06T05:00:00Z")
    private val bytes = "encrypted backup".toByteArray()
    private fun hash(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private val bundle = Bundle("one", "porchlight-one.tar.age", bytes.size.toLong(), hash(bytes), now, now, now)
    private val stem = hash(bundle.id.toByteArray()) + "-" + bundle.sha256
    private val receiptJson = WireJson.encodeToString(bundle)
    // Content-verified legacy adoption can never adopt this receipt (wrong size), so only a recorded intent can.
    private val interruptedReceipt = WireJson.encodeToString(bundle.copy(propertySize = bundle.propertySize + 1))
    // An unrelated owned bundle plus operator files that every recovery must leave exactly as they are.
    private val earlier = Instant.parse("2026-10-05T05:00:00Z")
    private val olderBytes = "older encrypted backup".toByteArray()
    private val older = Bundle("zero", "porchlight-zero.tar.age", olderBytes.size.toLong(), hash(olderBytes), earlier, earlier, earlier)
    private val olderStem = hash(older.id.toByteArray()) + "-" + older.sha256
    private val foreignTemp = ".pull-00000000-0000-0000-0000-000000000000.tmp" // our temp grammar, but never recorded
    private class Crash : Error("simulated process death")
    private class Source(private val metadata: Bundle, private val bytes: ByteArray) : BackupSource {
        var downloads = 0
        override suspend fun authorizedHousehold() = "household"
        override suspend fun status() = BackupStatus(latest = metadata)
        override suspend fun download(bundle: Bundle, offset: Long, begin: suspend (DownloadHead) -> Unit, chunk: suspend (ByteArray) -> Unit) {
            downloads++
            begin(DownloadHead(200, "\"${bundle.sha256}\"", bytes.size.toLong(), null))
            chunk(bytes)
        }
    }
    private suspend fun fixture(test: suspend (Path, Path) -> Unit) {
        val root = Files.createTempDirectory(Files.createDirectories(Path.of(System.getProperty("porchlight.test.dir"))), "ownership-")
        try { test(root, Files.createDirectory(root.resolve(hash("household".toByteArray())))) }
        finally { root.toFile().deleteRecursively() }
    }
    private fun identity(path: Path): String = Files.readAttributes(path, BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
        .let { "${it.fileKey()}:${it.creationTime()}" }
    /** Every file in the household directory except the index, lock and schedule, as name -> content. */
    private fun files(dir: Path): Map<String, String> = Files.list(dir).use { paths ->
        paths.filter { it.fileName.toString() !in setOf(".owned-files", ".lock", ".next-pull") }.toList()
    }.associate { it.fileName.toString() to Files.readAllBytes(it).toString(Charsets.UTF_8) } // lenient: corrupt fixtures
    private class Saved(val entries: Map<String, String>, val pending: Set<String>)
    private fun saved(dir: Path): Saved {
        val json = WireJson.parseToJsonElement(Files.readString(dir.resolve(".owned-files"))).jsonObject
        return Saved(json.getValue("entries").jsonObject.mapValues { it.value.jsonPrimitive.content }, json.getValue("pending").jsonObject.keys)
    }
    /** The durable index records exactly [owned], each at its current on-disk identity, and no unresolved intent. */
    private fun assertRecorded(dir: Path, owned: Set<String>, label: String) {
        val index = saved(dir)
        assertEquals(emptySet(), index.pending, "$label: unresolved intents")
        assertEquals(owned.associateWith { identity(dir.resolve(it)) }, index.entries, "$label: recorded identities")
    }

    @Test fun validLegacyPairIsAdoptedWithoutDownloadAndParticipatesInRetention() = runTest { fixture { root, dir ->
        val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json")
        Files.write(target, bytes); Files.writeString(receipt, WireJson.encodeToString(bundle))
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }, 1)
        pull.pull(); assertNull(pull.state.value.error); assertNull(pull.state.value.notice)
        assertEquals(0, source.downloads); assertTrue(OwnedBackupFiles(dir).owns(target)); assertTrue(OwnedBackupFiles(dir).owns(receipt))
        val next = bundle.copy(id = "two", finishedAt = Instant.fromEpochSeconds(now.epochSeconds + 1))
        val later = PassivePull(root, Source(next, bytes), { now }, 1); later.pull()
        assertNull(later.state.value.error); assertFalse(Files.exists(target)); assertFalse(Files.exists(receipt))
    } }
    @Test fun invalidLegacyCollisionIsRenamedWithOnePreciseNoticeAndNeverDeleted() = runTest { fixture { root, dir ->
        val target = dir.resolve("$stem.tar.age"); Files.writeString(target, "operator bytes")
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now })
        pull.pull(); assertNull(pull.state.value.error)
        val saved = Files.list(dir).use { it.filter { path -> path.fileName.toString().startsWith("$stem.tar.age.legacy-") }.toList() }.single()
        assertEquals("operator bytes", Files.readString(saved)); assertContentEquals(bytes, Files.readAllBytes(target))
        assertEquals("Preserved unverified backup file ${target.fileName} as $saved", pull.state.value.notice)
        pull.tick() // a non-due minute poll must not hide the operator's notice
        assertEquals("Preserved unverified backup file ${target.fileName} as $saved", pull.state.value.notice)
        pull.pull(); assertNull(pull.state.value.error); assertNull(pull.state.value.notice)
        assertEquals("operator bytes", Files.readString(saved)); assertEquals(1, source.downloads)
    } }

    /**
     * Crashes [stage] (an `OwnedBackupFiles` step hook) and recovers with a fresh instance, as the next locked poll
     * would. Every expectation is derived from the scenario, never from the recovered directory: the exact files and
     * contents (nothing renamed aside, our recorded temp gone, foreign files and an unrecorded `.pull-*.tmp` untouched),
     * the exact durable identities with no pending intent, no notice, and a next pull that reuses what survived.
     * [existing]: an owned copy of this bundle's files already exists (stale ciphertext, or a valid pair for receipts).
     */
    private suspend fun crashAt(stage: String, existing: Boolean) = fixture { root, dir ->
        val label = "$stage (existing=$existing)"
        val kind = stage.substringBefore('.'); val step = stage.substringAfter('.')
        val target = "$stem.tar.age"; val partial = "$stem.part"; val receipt = "$stem.json"
        val olderTarget = "$olderStem.tar.age"; val olderReceipt = "$olderStem.json"
        val initial = OwnedBackupFiles(dir)
        initial.write(dir.resolve(olderTarget), olderBytes); initial.write(dir.resolve(olderReceipt), WireJson.encodeToString(older).toByteArray())
        Files.writeString(dir.resolve("operator-notes.txt"), "operator notes"); Files.writeString(dir.resolve(foreignTemp), "operator temp")
        val unrelated = files(dir)
        val expected = unrelated.toMutableMap(); val owned = mutableSetOf(olderTarget, olderReceipt)
        if (existing) {
            val old = if (kind == "write") String(bytes) else "stale bytes"
            initial.write(dir.resolve(target), old.toByteArray()); initial.write(dir.resolve(receipt), receiptJson.toByteArray())
            expected[target] = old; expected[receipt] = receiptJson; owned += setOf(target, receipt)
        }
        if (kind == "publish") { initial.checkpoint(dir.resolve(partial)); Files.write(dir.resolve(partial), bytes) }
        val interrupted = OwnedBackupFiles(dir, step = { if (it == stage) throw Crash() })
        assertFailsWith<Crash>(label) {
            when (kind) {
                "checkpoint" -> interrupted.checkpoint(dir.resolve(partial))
                "publish" -> interrupted.publish(dir.resolve(partial), dir.resolve(target))
                else -> interrupted.write(dir.resolve(receipt), interruptedReceipt.toByteArray())
            }
        }
        when (kind) {
            // The recorded CREATE_NEW intent adopts the surviving (empty) checkpoint.
            "checkpoint" -> if (step != "intent") { expected[partial] = ""; owned += partial }
            // Before the rename the owned checkpoint stays; after it the target carries the recorded identity.
            "publish" -> if (step == "intent") { expected[partial] = String(bytes); owned += partial } else { expected[target] = String(bytes); owned += target }
            // Before the rename the recorded temp is deleted and the old receipt kept; after it the new receipt is ours.
            else -> if (step in setOf("move", "committed")) { expected[receipt] = interruptedReceipt; owned += receipt }
        }
        val notices = mutableListOf<String>()
        val fresh = OwnedBackupFiles(dir, notice = { notices += it })
        assertEquals(expected, files(dir), "$label: recovered files")
        assertEquals(emptyList(), notices, "$label: our own files must not be reported as unverified")
        assertRecorded(dir, owned, label)
        for (name in owned) assertTrue(fresh.owns(dir.resolve(name)), "$label: $name owned")
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }); pull.pull()
        assertNull(pull.state.value.error, label); assertNull(pull.state.value.notice, label)
        assertEquals(target, pull.state.value.localBundle, label)
        val reused = kind == "publish" && step != "intent" || kind == "write" && existing
        assertEquals(if (reused) 0 else 1, source.downloads, "$label: downloads")
        assertEquals(unrelated + mapOf(target to String(bytes), receipt to receiptJson), files(dir), "$label: after the next pull")
        assertRecorded(dir, setOf(olderTarget, olderReceipt, target, receipt), "$label: after the next pull")
    }
    /** Runs every window and reports all failing ones at once. */
    private suspend fun crashWindows(kind: String, steps: List<String>, existing: Boolean) {
        val failures = steps.mapNotNull { step ->
            try { crashAt("$kind.$step", existing); null } catch (e: AssertionError) { "$kind.$step: ${e.message}" }
        }
        assertEquals(emptyList(), failures, "$kind crash windows (existing=$existing)")
    }
    @Test fun firstCheckpointCrashWindows() = runTest { crashWindows("checkpoint", listOf("intent", "create", "committed"), existing = false) }
    @Test fun newReceiptCrashWindows() = runTest {
        crashWindows("write", listOf("intent", "create", "content", "ready", "move", "committed"), existing = false)
    }
    @Test fun newPublicationCrashWindows() = runTest { crashWindows("publish", listOf("intent", "move", "committed"), existing = false) }
    @Test fun checkpointIntentCrash() = runTest { crashAt("checkpoint.intent", existing = true) }
    @Test fun checkpointCreateCrash() = runTest { crashAt("checkpoint.create", existing = true) }
    @Test fun checkpointCommittedCrash() = runTest { crashAt("checkpoint.committed", existing = true) }
    @Test fun publishIntentCrash() = runTest { crashAt("publish.intent", existing = true) }
    @Test fun publishMoveCrash() = runTest { crashAt("publish.move", existing = true) }
    @Test fun publishCommittedCrash() = runTest { crashAt("publish.committed", existing = true) }
    @Test fun receiptIntentCrash() = runTest { crashAt("write.intent", existing = true) }
    @Test fun receiptCreateCrash() = runTest { crashAt("write.create", existing = true) }
    @Test fun receiptContentCrash() = runTest { crashAt("write.content", existing = true) }
    @Test fun receiptReadyCrash() = runTest { crashAt("write.ready", existing = true) }
    @Test fun receiptMoveCrash() = runTest { crashAt("write.move", existing = true) }
    @Test fun receiptCommittedCrash() = runTest { crashAt("write.committed", existing = true) }

    @Test fun v1InterruptedReceiptReplacementIsRepairedFromVerifiedSurvivingPair() = runTest { fixture { root, dir ->
        val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json")
        val initial = OwnedBackupFiles(dir)
        initial.write(target, bytes); initial.write(receipt, WireJson.encodeToString(bundle).toByteArray())
        val old = WireJson.parseToJsonElement(Files.readString(dir.resolve(".owned-files"))).toString()
        // A v1 write persisted the temp identity before replacing the still-valid receipt.
        val json = kotlinx.serialization.json.Json.parseToJsonElement(old) as kotlinx.serialization.json.JsonObject
        val identities = WireJson.decodeFromString<Map<String, String>>(json.getValue("entries").toString()).toMutableMap()
        identities[receipt.fileName.toString()] = "non-surviving temporary identity"
        Files.writeString(dir.resolve(".owned-files"), WireJson.encodeToString(identities))
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }); pull.pull()
        assertNull(pull.state.value.error); assertNull(pull.state.value.notice); assertEquals(0, source.downloads)
        assertTrue(OwnedBackupFiles(dir).owns(receipt)); assertTrue(OwnedBackupFiles(dir).owns(target))
    } }
    @Test fun failedMoveIntentPreservesOldReceiptAndReportsWhichFileAndAction() = runTest { fixture { root, dir ->
        val receipt = dir.resolve("$stem.json"); val target = dir.resolve("$stem.tar.age")
        val initial = OwnedBackupFiles(dir)
        initial.write(target, bytes); initial.write(receipt, receiptJson.toByteArray())
        val broken = OwnedBackupFiles(dir, step = { if (it == "write.ready") throw IOException("move unavailable") })
        val failure = assertFailsWith<BackupStorageFailure> { broken.write(receipt, "new content".toByteArray()) }
        assertEquals("Backup storage problem at $receipt; check permissions and free space on $dir, then refresh", failure.message)
        assertEquals(receiptJson, Files.readString(receipt))
        // The failed operation leaves its recorded temp and intent for recovery, not speculative cleanup...
        assertTrue(files(dir).keys.single { it.startsWith(".pull-") }.endsWith(".tmp"))
        assertEquals(setOf(receipt.fileName.toString()), saved(dir).pending)
        // ...and the next instance deletes exactly that temp and keeps the old receipt owned.
        val fresh = OwnedBackupFiles(dir)
        assertEquals(mapOf(target.fileName.toString() to String(bytes), receipt.fileName.toString() to receiptJson), files(dir))
        assertRecorded(dir, setOf(target.fileName.toString(), receipt.fileName.toString()), "after recovery")
        assertTrue(fresh.owns(receipt))
        val pull = PassivePull(root, Source(bundle, bytes), { now }); pull.pull(); assertNull(pull.state.value.error)
    } }
    @Test fun pendingTempReplacedByOperatorIsNotDeleted() = runTest { fixture { root, dir ->
        val receipt = dir.resolve("$stem.json")
        val broken = OwnedBackupFiles(dir, step = { if (it == "write.ready") throw Crash() })
        assertFailsWith<Crash> { broken.write(receipt, "new receipt".toByteArray()) }
        val temp = Files.list(dir).use { files -> files.filter { it.fileName.toString().startsWith(".pull-") }.toList().single() }
        Files.move(temp, dir.resolve("operator-saved.tmp")); Files.writeString(temp, "operator replacement")
        OwnedBackupFiles(dir)
        // The intent is resolved (dropped) without deleting either file: the replacement is no longer ours.
        assertEquals(mapOf(temp.fileName.toString() to "operator replacement", "operator-saved.tmp" to "new receipt"), files(dir))
        assertRecorded(dir, emptySet(), "replaced temp")
        val pull = PassivePull(root, Source(bundle, bytes), { now }); pull.pull(); assertNull(pull.state.value.error)
        assertEquals("operator replacement", Files.readString(temp))
    } }
    @Test fun nonVerifyingReceiptPairIsPreservedAndNotAdopted() = runTest { fixture { root, dir ->
        val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json")
        Files.write(target, bytes); Files.writeString(receipt, WireJson.encodeToString(bundle.copy(propertySize = bytes.size.toLong() + 1)))
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }); pull.pull()
        assertNull(pull.state.value.error); assertEquals(1, source.downloads)
        val messages = pull.state.value.notice!!.lines(); assertEquals(2, messages.size)
        for (path in listOf(target, receipt)) {
            val saved = Files.list(dir).use { files -> files.filter { it.fileName.toString().startsWith("${path.fileName}.legacy-") }.toList().single() }
            assertTrue(messages.contains("Preserved unverified backup file ${path.fileName} as $saved"))
            if (path == target) assertContentEquals(bytes, Files.readAllBytes(saved))
            else assertEquals(WireJson.encodeToString(bundle.copy(propertySize = bytes.size.toLong() + 1)), Files.readString(saved))
        }
    } }

    // ---- Failure classes: an idle poll never writes; storage errors keep every record and name the storage fix;
    // ---- only a corrupt index is advised to be renamed aside.

    @Test fun idlePollsNeverRewriteTheIndexSoAFullDiskIsNotAnError() = runTest { fixture { root, dir ->
        val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }); pull.pull()
        assertNull(pull.state.value.error)
        val index = dir.resolve(".owned-files"); val text = Files.readString(index); val key = identity(index)
        var writes = 0
        pull.ownershipStep = { if (it == "index.persist") { writes++; throw IOException("No space left on device") } }
        repeat(3) { pull.tick() } // non-due minute polls: nothing pending, nothing to reconcile
        assertNull(pull.state.value.error); assertEquals(0, writes)
        assertEquals(text, Files.readString(index)); assertEquals(key, identity(index))
        OwnedBackupFiles(dir, step = { if (it == "index.persist") fail("an idle instance rewrote the index") })
        assertEquals(key, identity(index)); assertEquals(1, source.downloads)
    } }
    /** JDK-shaped faults for a full disk, a quota, a permission problem and a read-only mount, with their advice. */
    private fun storageFaults(dir: Path): List<Pair<Exception, String>> {
        val index = dir.resolve(".owned-files").toString()
        val full = "(disk full); free space on the disk holding $dir, then refresh"
        return listOf(
            IOException("No space left on device") to full, // FileChannel.write
            java.nio.file.FileSystemException(index, null, "No space left on device") to full, // createTempFile / move
            java.nio.file.FileSystemException(index, null, "Disk quota exceeded") to full,
            java.nio.file.AccessDeniedException(index) to "(permission denied); make $dir and its files readable and writable by your user, then refresh",
            java.nio.file.FileSystemException(index, null, "Read-only file system") to "(read-only file system); make the disk holding $dir writable, then refresh")
    }
    @Test fun recoveryStorageFailuresKeepEveryRecordAndNameTheStorageFix() = runTest {
        for (scenario in listOf("write.ready", "checkpoint.create")) for (fault in 0 until 5) fixture { _, dir ->
            val (exception, advice) = storageFaults(dir)[fault]
            val label = "$scenario / $exception"
            val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json"); val partial = dir.resolve("$stem.part")
            val initial = OwnedBackupFiles(dir)
            initial.write(target, bytes); initial.write(receipt, receiptJson.toByteArray())
            val crashed = OwnedBackupFiles(dir, step = { if (it == scenario) throw Crash() })
            assertFailsWith<Crash>(label) {
                if (scenario == "write.ready") crashed.write(receipt, interruptedReceipt.toByteArray()) else crashed.checkpoint(partial)
            }
            val index = Files.readString(dir.resolve(".owned-files"))
            val failure = assertFailsWith<BackupStorageFailure>(label) { OwnedBackupFiles(dir, step = { if (it == "index.persist") throw exception }) }
            assertEquals("Backup storage problem at $dir $advice", failure.message, label)
            assertEquals(index, Files.readString(dir.resolve(".owned-files")), "$label: records and intent kept")
            // Legacy handling never runs over unresolved intents: our checkpoint is not renamed aside.
            assertTrue(files(dir).keys.none { ".legacy-" in it }, label)
            assertEquals(scenario == "checkpoint.create", Files.exists(partial), label)
            // With the storage fixed the next instance finishes recovery; nothing had to be moved or deleted.
            val notices = mutableListOf<String>()
            OwnedBackupFiles(dir, notice = { notices += it })
            assertEquals(emptyList(), notices, label)
            val expected = mapOf(target.fileName.toString() to String(bytes), receipt.fileName.toString() to receiptJson) +
                (if (scenario == "checkpoint.create") mapOf(partial.fileName.toString() to "") else emptyMap())
            assertEquals(expected, files(dir), label)
            assertRecorded(dir, expected.keys, label)
        }
    }
    @Test fun unwritableDirectoryDuringRecoveryIsAPermissionProblemNotACorruptIndex() = runTest { fixture { _, dir ->
        val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json")
        val initial = OwnedBackupFiles(dir)
        initial.write(target, bytes); initial.write(receipt, receiptJson.toByteArray())
        assertFailsWith<Crash> { OwnedBackupFiles(dir, step = { if (it == "write.ready") throw Crash() }).write(receipt, interruptedReceipt.toByteArray()) }
        val temp = files(dir).keys.single { it.startsWith(".pull-") }; val index = Files.readString(dir.resolve(".owned-files"))
        Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("r-x------"))
        try {
            assumeTrue("directory permissions must apply to this user", !Files.isWritable(dir))
            val failure = assertFailsWith<BackupStorageFailure> { OwnedBackupFiles(dir) } // real EACCES deleting the temp
            assertEquals("Backup storage problem at $dir (permission denied); make $dir and its files readable and writable by your user, then refresh", failure.message)
        } finally { Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("rwx------")) }
        assertEquals(index, Files.readString(dir.resolve(".owned-files"))); assertTrue(Files.exists(dir.resolve(temp)))
        OwnedBackupFiles(dir)
        assertEquals(mapOf(target.fileName.toString() to String(bytes), receipt.fileName.toString() to receiptJson), files(dir))
        assertRecorded(dir, setOf(target.fileName.toString(), receipt.fileName.toString()), "after permissions were fixed")
    } }
    @Test fun indexReadFailureIsAStorageProblemNotCorruption() = runTest { fixture { _, dir ->
        val target = dir.resolve("$stem.tar.age"); OwnedBackupFiles(dir).write(target, bytes)
        val index = dir.resolve(".owned-files")
        val failure = assertFailsWith<BackupStorageFailure> {
            OwnedBackupFiles(dir, step = { if (it == "index.read") throw java.nio.file.AccessDeniedException(index.toString()) })
        }
        assertEquals("Backup storage problem at $index (permission denied); make $dir and its files readable and writable by your user, then refresh", failure.message)
        assertTrue(OwnedBackupFiles(dir).owns(target))
    } }
    @Test fun corruptIndexTouchesNothingAndRenamingItAsideRecovers() = runTest {
        val corruptions: List<Pair<String, (Path) -> Unit>> = listOf(
            "garbage" to { index: Path -> Files.writeString(index, "{ not json") },
            "invalid UTF-8" to { index: Path -> Files.write(index, byteArrayOf(0xff.toByte(), 0xfe.toByte(), 0x7b)) },
            "unsupported version" to { index: Path -> Files.writeString(index, """{"version":3,"entries":{},"pending":{}}""") },
            "unmanaged entry name" to { index: Path -> Files.writeString(index, """{"entries":{"operator-notes.txt":"x"},"pending":{}}""") },
            "symbolic link" to { index: Path -> Files.delete(index); Files.createSymbolicLink(index, index.resolveSibling("operator-notes.txt")) },
        )
        for ((kind, corrupt) in corruptions) fixture { root, dir ->
            val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json"); val partial = dir.resolve("$stem.part")
            val initial = OwnedBackupFiles(dir)
            initial.write(target, bytes); initial.write(receipt, receiptJson.toByteArray()); initial.checkpoint(partial)
            Files.writeString(dir.resolve("operator-notes.txt"), "operator notes")
            val index = dir.resolve(".owned-files"); corrupt(index)
            val before = files(dir); val link = Files.isSymbolicLink(index); val raw = if (link) null else Files.readAllBytes(index)
            val failure = assertFailsWith<BackupStorageFailure>(kind) { OwnedBackupFiles(dir) }
            assertEquals("Backup ownership index $index is corrupt; quit Porchlight, rename it aside within $dir (do not delete it), then refresh",
                failure.message, kind)
            assertEquals(before, files(dir), "$kind: no file renamed, created or deleted")
            assertEquals(link, Files.isSymbolicLink(index), kind); if (raw != null) assertContentEquals(raw, Files.readAllBytes(index), kind)
            // Following the advice ends the error: verified bundles are adopted, the unverifiable checkpoint preserved aside.
            Files.move(index, dir.resolve(".owned-files.corrupt"))
            val notices = mutableListOf<String>(); val fresh = OwnedBackupFiles(dir, notice = { notices += it })
            assertTrue(fresh.owns(target), kind); assertTrue(fresh.owns(receipt), kind)
            val kept = files(dir).keys.single { it.startsWith("${partial.fileName}.legacy-") }
            assertEquals(listOf("Preserved unverified backup file ${partial.fileName} as ${dir.resolve(kept)}"), notices, kind)
            val source = Source(bundle, bytes); val pull = PassivePull(root, source, { now }); pull.pull()
            assertNull(pull.state.value.error, kind); assertEquals(0, source.downloads, kind)
            assertTrue(Files.exists(dir.resolve(".owned-files.corrupt"), LinkOption.NOFOLLOW_LINKS), kind)
        }
    }
    @Test fun diskFullWhileFinishingAnInterruptedPullAdvisesFreeingSpaceAndClearsOnceFreed() = runTest { fixture { root, dir ->
        var clock = now; val source = Source(bundle, bytes); val pull = PassivePull(root, source, { clock }); pull.pull()
        val target = dir.resolve("$stem.tar.age"); val receipt = dir.resolve("$stem.json")
        assertFailsWith<Crash> { OwnedBackupFiles(dir, step = { if (it == "write.ready") throw Crash() }).write(receipt, interruptedReceipt.toByteArray()) }
        val index = dir.resolve(".owned-files"); val recorded = Files.readString(index)
        var attempts = 0
        pull.ownershipStep = { if (it == "index.persist") { attempts++; throw IOException("No space left on device") } }
        pull.tick() // not due, but an intent is pending
        val advice = "Backup storage problem at $dir (disk full); free space on the disk holding $dir, then refresh"
        assertEquals(advice, pull.state.value.error); assertEquals(recorded, Files.readString(index)); assertEquals(1, attempts)
        pull.tick() // backoff: no new attempt, no different advice
        assertEquals(advice, pull.state.value.error); assertEquals(1, attempts)
        pull.ownershipStep = {} // the user freed space; nothing is moved or deleted
        clock = Instant.fromEpochSeconds(now.epochSeconds + 120); pull.tick()
        assertNull(pull.state.value.error); assertNull(pull.state.value.notice); assertEquals(1, source.downloads)
        assertEquals(mapOf(target.fileName.toString() to String(bytes), receipt.fileName.toString() to receiptJson), files(dir))
        assertRecorded(dir, setOf(target.fileName.toString(), receipt.fileName.toString()), "after space was freed")
    } }
    @Test fun corruptIndexOnAPollAdvisesRenamingItAsideAndRecoversOnceRenamed() = runTest { fixture { root, dir ->
        var clock = now; val source = Source(bundle, bytes); val pull = PassivePull(root, source, { clock }); pull.pull()
        val index = dir.resolve(".owned-files"); Files.writeString(index, "{ not json"); val before = files(dir)
        pull.tick()
        assertEquals("Backup ownership index $index is corrupt; quit Porchlight, rename it aside within $dir (do not delete it), then refresh",
            pull.state.value.error)
        assertEquals(before, files(dir)); assertEquals("{ not json", Files.readString(index))
        Files.move(index, dir.resolve(".owned-files.corrupt"))
        clock = Instant.fromEpochSeconds(now.epochSeconds + 120); pull.tick()
        assertNull(pull.state.value.error); assertNull(pull.state.value.notice); assertEquals(1, source.downloads)
        assertEquals(before + (".owned-files.corrupt" to "{ not json"), files(dir))
        assertRecorded(dir, setOf("$stem.tar.age", "$stem.json"), "re-adopted after the index was renamed aside")
    } }
    @Test fun unreadableIndexOnAPollIsAPermissionProblemAndClearsOnceFixed() = runTest { fixture { root, dir ->
        var clock = now; val source = Source(bundle, bytes); val pull = PassivePull(root, source, { clock }); pull.pull()
        val index = dir.resolve(".owned-files"); val recorded = Files.readString(index)
        Files.setPosixFilePermissions(index, PosixFilePermissions.fromString("---------"))
        try {
            assumeTrue("file permissions must apply to this user", !Files.isReadable(index))
            pull.tick() // real EACCES reading the index
            assertEquals("Backup storage problem at $index (permission denied); make $dir and its files readable and writable by your user, then refresh",
                pull.state.value.error)
        } finally { Files.setPosixFilePermissions(index, PosixFilePermissions.fromString("rw-------")) }
        assertEquals(recorded, Files.readString(index))
        clock = Instant.fromEpochSeconds(now.epochSeconds + 120); pull.tick()
        assertNull(pull.state.value.error); assertEquals(1, source.downloads)
        assertRecorded(dir, setOf("$stem.tar.age", "$stem.json"), "after permissions were fixed")
    } }
}
