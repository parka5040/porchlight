package app.porchlight.shared.ui

import androidx.compose.foundation.layout.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.porchlight.shared.theme.PorchlightTokens

/** Shared Today/Board controls. ShellButton's other callers deliberately retain their existing look. */
@Composable
fun CollectionHeader(
    options: List<ScopeOption>,
    selectedId: String,
    onScopeChange: (String) -> Unit,
    onNewItem: () -> Unit,
    onRefresh: () -> Unit,
    modifier: Modifier = Modifier,
    additionalScopes: @Composable () -> Unit = {},
) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(PorchlightTokens.spaceSm.dp)) {
        SegmentedChoice(options, selectedId, onScopeChange)
        additionalScopes()
        ActionRow {
            PrimaryAction("New item", onNewItem, Modifier.weight(1f, fill = false).fillMaxHeight())
            SecondaryAction("Refresh", onRefresh, Modifier.weight(1f, fill = false).fillMaxHeight())
        }
    }
}
