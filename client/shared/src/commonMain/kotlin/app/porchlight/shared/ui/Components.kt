package app.porchlight.shared.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.BorderStroke
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.testTag
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.text.style.TextAlign
import app.porchlight.shared.theme.*

fun memberInitials(name: String): String = name.trim().split(Regex("\\s+"))
    .filter { it.isNotEmpty() }.take(2).joinToString("") { it.take(1).uppercase() }.ifEmpty { "?" }

@Composable
fun MemberChip(name: String, colorIndex: Int, modifier: Modifier = Modifier, serverColor: Color? = null, initials: String? = null) {
    val colors = PorchlightTokens.memberColors
    val color = serverColor ?: colors[((colorIndex % colors.size) + colors.size) % colors.size]
    MemberChip(name, color, initials ?: memberInitials(name), modifier)
}

/** Server colours (including admin-set non-palette values) and server initials are authoritative. */
fun memberColor(hex: String, fallback: Int = 0): Color {
    val rgb = hex.takeIf { Regex("#[0-9a-fA-F]{6}").matches(it) }?.drop(1)?.toLongOrNull(16)
    val colors = PorchlightTokens.memberColors
    return rgb?.let { Color(0xFF000000 or it) } ?: colors[((fallback % colors.size) + colors.size) % colors.size]
}

@Composable
fun MemberChip(member: app.porchlight.shared.api.models.MemberRead, modifier: Modifier = Modifier) =
    MemberChip(member.displayName, memberColor(member.color), member.initials, modifier)

@Composable
private fun MemberChip(name: String, color: Color, initials: String, modifier: Modifier) {
    val ink = if (color.luminance() > 0.179f) Color.Black else Color.White
    Row(modifier.semantics(mergeDescendants = true) { contentDescription = "Member $name" },
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        Box(Modifier.background(color, CircleShape).padding(PorchlightTokens.spaceSm.dp), contentAlignment = Alignment.Center) {
            Text(initials, color = ink, style = MaterialTheme.typography.labelLarge)
        }
        Text(name)
    }
}
@Composable
fun StatusPill(label: String, color: Color = LocalPorchlightPalette.current.infoText) {
    Surface(shape = MaterialTheme.shapes.small, color = LocalPorchlightPalette.current.surface) {
        Text(label, Modifier.padding(horizontal = PorchlightTokens.spaceInset.dp, vertical = PorchlightTokens.spacePillVertical.dp), color = color, style = MaterialTheme.typography.labelMedium)
    }
}
@Composable fun PorchlightCard(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    Card(modifier, shape = MaterialTheme.shapes.medium,
        colors = CardDefaults.cardColors(containerColor = LocalPorchlightPalette.current.surface, contentColor = LocalPorchlightPalette.current.onSurface)) {
        Column(Modifier.padding(PorchlightTokens.spaceMd.dp), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp), content = content)
    }
}
@Composable fun DesignPending(screen: String) {
    Text("DESIGN-PENDING: $screen", style = MaterialTheme.typography.bodySmall, color = LocalPorchlightPalette.current.onSurfaceMuted)
}
@Composable fun MockupPlaceholder(screen: String, mockup: String) {
    Text("CONTENT PLACEHOLDER: $screen · Design: $mockup", style = MaterialTheme.typography.bodySmall,
        color = LocalPorchlightPalette.current.onSurfaceMuted)
}
@Composable fun EmptyState(title: String, message: String) {
    PorchlightCard { Text(title, style = MaterialTheme.typography.titleMedium); Text(message) }
}
@Composable fun OfflineState() { StatusPill("Offline — showing saved data") }
@Composable fun ErrorState(message: String, retry: () -> Unit) {
    PorchlightCard {
        Text(message, color = LocalPorchlightPalette.current.errorText)
        ShellButton("Try again", "Try again: retry loading", retry)
    }
}

/** Null selection is an action; only selectable chips reserve an indicator slot. */
@Composable fun ShellButton(label: String, description: String = label, onClick: () -> Unit, modifier: Modifier = Modifier,
    selected: Boolean? = null, enabled: Boolean = true) {
    val p = LocalPorchlightPalette.current
    val isSelected = selected == true
    TextButton(onClick, modifier.heightIn(min = PorchlightTokens.touchTarget.dp)
        .widthIn(min = PorchlightTokens.touchTarget.dp).semantics {
            contentDescription = if (description.contains(label, ignoreCase = true)) description else "$label: $description"
            if (selected != null) this.selected = selected
        }, enabled = enabled, shape = MaterialTheme.shapes.small,
        border = if (isSelected) BorderStroke(PorchlightTokens.selectionStroke.dp, p.primary) else null,
        colors = ButtonDefaults.textButtonColors(containerColor = if (isSelected) p.selectedSurface else Color.Transparent,
            contentColor = if (isSelected) p.primaryText else MaterialTheme.colorScheme.onSurface)) {
        // Borders draw inside the surface; reserving the check in both states prevents shifts.
        // Plain actions retain TextButton's original text + padding geometry and semantics.
        if (selected != null) {
            Box(Modifier.size(PorchlightTokens.iconSize.dp)) {
                if (isSelected) Canvas(Modifier.fillMaxSize().testTag("selection-$label")) {
                    drawLine(p.primaryText, Offset(size.width * .2f, size.height * .5f),
                        Offset(size.width * .4f, size.height * .7f), PorchlightTokens.selectionStroke.dp.toPx())
                    drawLine(p.primaryText, Offset(size.width * .4f, size.height * .7f),
                        Offset(size.width * .8f, size.height * .3f), PorchlightTokens.selectionStroke.dp.toPx())
                }
            }
            Spacer(Modifier.width(PorchlightTokens.spaceXs.dp))
        }
        Text(label, style = MaterialTheme.typography.labelLarge, textAlign = TextAlign.Center)
    }
}
