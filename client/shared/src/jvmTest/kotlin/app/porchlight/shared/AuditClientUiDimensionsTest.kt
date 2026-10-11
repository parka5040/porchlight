package app.porchlight.shared

import java.io.File
import kotlin.test.*

class AuditClientUiDimensionsTest {
    companion object {
        fun hasLiteralDimension(text: String): Boolean {
            val number = "(?:0[xX][0-9a-fA-F_]+|0[bB][01_]+|[0-9][0-9_]*(?:\\.[0-9_]+)?(?:[eE][+-]?[0-9_]+)?[fF]?)"
            fun nonzero(value: String): Boolean {
                val clean = value.replace("_", "")
                return when {
                    clean.startsWith("0x", true) -> clean.drop(2).any { it != '0' }
                    clean.startsWith("0b", true) -> clean.drop(2).any { it != '0' }
                    else -> clean.removeSuffix("f").removeSuffix("F").toDouble() != 0.0
                }
            }
            if (Regex("(?<![A-Za-z0-9_])($number)\\s*\\.dp\\b").findAll(text).any { nonzero(it.groupValues[1]) }) return true
            return Regex("\\bconst\\s+val\\s+(\\w+)(?:\\s*:\\s*\\w+)?\\s*=\\s*($number)\\b").findAll(text).any {
                nonzero(it.groupValues[2]) && Regex("\\b${Regex.escape(it.groupValues[1])}\\s*\\.dp\\b").containsMatchIn(text)
            }
        }
    }
    @Test fun commonUiUsesOnlyTokenDimensions() {
        val violations = File("src/commonMain").walkTopDown().filter { it.extension == "kt" }.flatMap { file ->
            if (hasLiteralDimension(file.readText())) sequenceOf(file.path) else emptySequence()
        }.toList()
        assertTrue(violations.isEmpty(), "Non-zero literal dp dimensions: ${violations.joinToString()}")
    }
}
