package app.porchlight.shared.data

import java.lang.reflect.Modifier
import kotlin.test.*

class AuditClientDataFix2StructureTest {
    @Test fun everyCacheInstallRequiresAnExplicitRequestFence() {
        val nonInstall = setOf("inSession", "initialize", "awaitInitialized", "itemWrite", "receiptMark", "snapshotMark",
            "cleanup", "clear", "acknowledgeCursor", "evictOccurrence", "evictItem", "evictList", "evictListScope",
            "evictComment", "evictAttachment", "withLiveAttachment", "keepListScope", "close")
        val writes = ReadCache::class.java.declaredMethods.filter {
            Modifier.isPublic(it.modifiers) && !it.isSynthetic &&
                !it.name.startsWith("get") && !it.name.startsWith("set") && it.name.substringBefore('$') !in nonInstall
        }
        assertTrue(writes.size >= 25, "must enumerate all cache install entry points")
        val unfenced = writes.filter { method -> method.parameterTypes.none { it.simpleName == "Fence" } }
        assertTrue(unfenced.isEmpty(), "Unfenced cache installs: ${unfenced.map { it.name }.sorted()}")
    }
}
