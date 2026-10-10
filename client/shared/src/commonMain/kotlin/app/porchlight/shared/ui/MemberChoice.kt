package app.porchlight.shared.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.theme.LocalPorchlightPalette
import app.porchlight.shared.theme.PorchlightTokens

/** One labelled member target, rather than an avatar/name plus a second repeated-name button. */
@Composable fun MemberChoice(member: MemberRead, selected: Boolean, description: String, onSelect: () -> Unit,
    trailing: @Composable RowScope.() -> Unit = {}) {
    val p = LocalPorchlightPalette.current
    Surface(Modifier.fillMaxWidth(), shape = MaterialTheme.shapes.small,
        color = if (selected) p.selectedSurface else p.background,
        border = BorderStroke(PorchlightTokens.selectionStroke.dp, if (selected) p.primary else p.outline)) {
        Row(Modifier.fillMaxWidth().heightIn(min = PorchlightTokens.touchTarget.dp)
            .selectable(selected, role = Role.RadioButton, onClick = onSelect)
            .semantics { contentDescription = description }
            .padding(PorchlightTokens.spaceSm.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
            MemberChip(member, Modifier.weight(1f).clearAndSetSemantics { })
            Canvas(Modifier.size(PorchlightTokens.iconSize.dp)) {
                if (selected) {
                    val stroke = PorchlightTokens.selectionStroke.dp.toPx()
                    drawLine(p.primaryText, Offset(size.width * .2f, size.height * .5f), Offset(size.width * .4f, size.height * .7f), stroke)
                    drawLine(p.primaryText, Offset(size.width * .4f, size.height * .7f), Offset(size.width * .8f, size.height * .3f), stroke)
                }
            }
            trailing()
        }
    }
}
