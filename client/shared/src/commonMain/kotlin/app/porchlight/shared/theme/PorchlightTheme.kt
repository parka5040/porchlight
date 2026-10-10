package app.porchlight.shared.theme

import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.porchlight.shared.resources.*
import app.porchlight.shared.settings.ThemeMode
import org.jetbrains.compose.resources.Font

val LocalPorchlightPalette = staticCompositionLocalOf { PorchlightTokens.Evening }

@Composable
fun PorchlightTheme(themeMode: ThemeMode, systemDark: Boolean = true, content: @Composable () -> Unit) {
    val dark = themeMode == ThemeMode.Evening || themeMode == ThemeMode.System && systemDark
    val p = if (dark) PorchlightTokens.Evening else PorchlightTokens.Daylight
    // Every foreground and container is explicit: no stock Material purple or untested derived surface tints.
    val base = if (dark) darkColorScheme() else lightColorScheme()
    val scheme = base.copy(
        primary = p.primary, onPrimary = p.onPrimary, primaryContainer = p.surfaceVariant, onPrimaryContainer = p.onSurface,
        secondary = p.secondary, onSecondary = p.onSecondary, secondaryContainer = p.surfaceVariant, onSecondaryContainer = p.onSurface,
        tertiary = p.info, onTertiary = p.onInfo, tertiaryContainer = p.surfaceVariant, onTertiaryContainer = p.onSurface,
        background = p.background, onBackground = p.onSurface, surface = p.surface, onSurface = p.onSurface,
        surfaceVariant = p.surfaceVariant, onSurfaceVariant = p.onSurface,
        error = p.error, onError = p.onError, errorContainer = p.surfaceVariant, onErrorContainer = p.onSurface,
        outline = p.outline, outlineVariant = p.outline, surfaceTint = p.surface,
        surfaceDim = p.surface, surfaceBright = p.surface, surfaceContainerLowest = p.background,
        surfaceContainerLow = p.surface, surfaceContainer = p.surface, surfaceContainerHigh = p.surfaceVariant,
        surfaceContainerHighest = p.surfaceVariant, inverseSurface = p.onSurface, inverseOnSurface = p.background,
        inversePrimary = p.background, scrim = p.scrim,
        primaryFixed = p.primary, primaryFixedDim = p.primary, onPrimaryFixed = p.onPrimary, onPrimaryFixedVariant = p.onPrimary,
        secondaryFixed = p.secondary, secondaryFixedDim = p.secondary, onSecondaryFixed = p.onSecondary, onSecondaryFixedVariant = p.onSecondary,
        tertiaryFixed = p.info, tertiaryFixedDim = p.info, onTertiaryFixed = p.onInfo, onTertiaryFixedVariant = p.onInfo,
    )
    val regular = FontWeight(PorchlightTokens.bodyWeights[0])
    val semibold = FontWeight(PorchlightTokens.bodyWeights[1])
    val bold = FontWeight(PorchlightTokens.bodyWeights[2])
    val body = when (PorchlightTokens.bodyFamily) {
        "Source Sans 3" -> FontFamily(Font(Res.font.source_sans_3_regular, regular),
            Font(Res.font.source_sans_3_semibold, semibold), Font(Res.font.source_sans_3_bold, bold))
        else -> error("Bundle font resources for ${PorchlightTokens.bodyFamily}")
    }
    val heading = when (PorchlightTokens.headingFamily) {
        "Literata" -> FontFamily(Font(Res.font.literata_semibold, FontWeight(PorchlightTokens.headingWeight)))
        else -> error("Bundle font resources for ${PorchlightTokens.headingFamily}")
    }
    fun bodyStyle(size: Int, weight: FontWeight = regular) = TextStyle(fontFamily = body,
        fontSize = size.sp, lineHeight = (size * PorchlightTokens.lineHeightFactor).sp, fontWeight = weight)
    fun headingStyle(size: Int) = TextStyle(fontFamily = heading, fontWeight = FontWeight(PorchlightTokens.headingWeight),
        fontSize = size.sp, lineHeight = (size * PorchlightTokens.lineHeightFactor).sp)
    val type = Typography(
        displayLarge = headingStyle(PorchlightTokens.desktopHeadingSize), displayMedium = headingStyle(PorchlightTokens.desktopHeadingSize),
        displaySmall = headingStyle(PorchlightTokens.headingSize), headlineLarge = headingStyle(PorchlightTokens.desktopHeadingSize),
        headlineMedium = headingStyle(PorchlightTokens.headingSize), headlineSmall = headingStyle(PorchlightTokens.sectionHeadingSize),
        titleLarge = headingStyle(PorchlightTokens.brandSize), titleMedium = bodyStyle(PorchlightTokens.bodySize, semibold),
        titleSmall = bodyStyle(PorchlightTokens.labelSize, semibold),
        bodyLarge = bodyStyle(PorchlightTokens.bodySize), bodyMedium = bodyStyle(PorchlightTokens.labelSize),
        bodySmall = bodyStyle(PorchlightTokens.captionSize), labelLarge = bodyStyle(PorchlightTokens.labelSize, bold),
        labelMedium = bodyStyle(PorchlightTokens.navigationSize, semibold), labelSmall = bodyStyle(PorchlightTokens.microSize, semibold),
    )
    val corner = RoundedCornerShape(PorchlightTokens.corner.dp)
    CompositionLocalProvider(LocalPorchlightPalette provides p) {
        MaterialTheme(colorScheme = scheme, typography = type,
            shapes = Shapes(corner, corner, corner, corner, corner), content = content)
    }
}
