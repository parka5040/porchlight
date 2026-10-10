package app.porchlight.shared.transport

import io.ktor.utils.io.ByteReadChannel
import kotlinx.coroutines.test.runTest
import kotlin.test.*

class AttachmentStreamTest {
    @Test fun streamingReadsTinyEmptyAndExactLimitWithoutFixedMaximumAllocation() = runTest {
        assertContentEquals(byteArrayOf(), readAttachmentBytes(ByteReadChannel(byteArrayOf()), 16))
        assertContentEquals(byteArrayOf(1, 2, 3), readAttachmentBytes(ByteReadChannel(byteArrayOf(1, 2, 3)), 16))
        val bytes = ByteArray(20_000) { (it % 127).toByte() }
        assertContentEquals(bytes, readAttachmentBytes(ByteReadChannel(bytes), bytes.size))
    }
    @Test fun streamingRejectsOversizeEvenWithoutLengthHeader() = runTest {
        val problem = assertFailsWith<ApiProblem> { readAttachmentBytes(ByteReadChannel(ByteArray(16_385)), 16_384) }
        assertEquals(413, problem.status)
    }
}
