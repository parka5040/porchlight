package app.porchlight.shared

import kotlin.test.*

class AuditClientUiFix1DimensionsTest {
    @Test fun rejectsNonzeroNumericDimensions() {
        for (value in listOf("48.dp", "48f.dp", "12.5F.dp", "0.5.dp", "1_024.dp", "1e2.dp", "0x20.dp", "const val size = 48f\nsize.dp")) {
            assertTrue(AuditClientUiDimensionsTest.hasLiteralDimension(value), "Missed dimension: $value")
        }
    }
    @Test fun acceptsZeroAndTokens() {
        for (value in listOf("0.dp", "0f.dp", "0.0F.dp", "0_0.dp", "0e2.dp", "PorchlightTokens.spaceMd.dp")) {
            assertFalse(AuditClientUiDimensionsTest.hasLiteralDimension(value), "Not a nonzero literal: $value")
        }
    }
}
