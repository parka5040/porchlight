package app.porchlight.shared.files

import java.io.File
import java.io.IOException
import java.nio.file.Files
import kotlin.test.*

class AuditBlClient3aFix1LinuxFlagsTest {
    @Test fun x86AliasesUseLinuxAmd64OpenFlags() {
        for (arch in listOf("x86_64", "amd64")) {
            // O_RDONLY | O_NONBLOCK | O_NOFOLLOW | O_CLOEXEC from Linux UAPI.
            assertEquals(0x0 or 0x800 or 0x20000 or 0x80000, LinuxCalendarInput.openFlags(arch), arch)
        }
    }

    @Test fun armAliasesUseLinuxArm64OpenFlags() {
        for (arch in listOf("aarch64", "arm64")) {
            assertEquals(0x0 or 0x800 or 0x8000 or 0x80000, LinuxCalendarInput.openFlags(arch), arch)
        }
    }

    @Test fun unknownArchitecturesRefuseImportWithAClearMessage() {
        val root = File(System.getProperty("porchlight.test.dir"), "calendar-abi-audit").apply { mkdirs() }
        val dir = Files.createTempDirectory(root.toPath(), "io-").toFile()
        try {
            val file = File(dir, "calendar.ics").apply { writeText("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n") }
            for (arch in listOf("riscv64", "ppc64le", "i386", "arm", "", "unknown")) {
                val failure = assertFailsWith<IOException>("Unsupported architecture $arch must refuse import") {
                    LinuxCalendarInput.read(file, arch)
                }
                assertEquals("Calendar import supports only x86_64 and aarch64 Linux", failure.message)
            }
        } finally { dir.deleteRecursively() }
    }
}
