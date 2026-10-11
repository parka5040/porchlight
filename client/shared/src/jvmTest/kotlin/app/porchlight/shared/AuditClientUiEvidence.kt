package app.porchlight.shared

import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.*
import java.awt.image.BufferedImage
import java.io.File
import javax.imageio.ImageIO
import kotlin.test.assertTrue

@OptIn(ExperimentalTestApi::class)
internal fun ComposeUiTest.auditEvidence(name: String) {
    val pixels = onRoot().captureToImage().toPixelMap()
    val image = BufferedImage(pixels.width, pixels.height, BufferedImage.TYPE_INT_ARGB)
    for (y in 0 until pixels.height) for (x in 0 until pixels.width) image.setRGB(x, y, pixels[x, y].toArgb())
    val directory = File("build/test-evidence/audit-client-ui").apply { mkdirs() }
    assertTrue(ImageIO.write(image, "png", directory.resolve("$name.png")))
}
