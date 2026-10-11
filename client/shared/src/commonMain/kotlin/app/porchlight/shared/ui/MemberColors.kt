package app.porchlight.shared.ui

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import app.porchlight.shared.theme.PorchlightTokens

val memberColorChoices = listOf("Yellow", "Orange", "Aqua", "Blue", "Purple", "Green", "Red")
    .zip(PorchlightTokens.memberColors.map { color ->
        "#" + (color.toArgb() and 0xFFFFFF).toString(16).padStart(6, '0').uppercase()
    }).toMap()

/** Token colors have audited memberInk contrast; arbitrary server colors need a contrast fallback. */
fun memberInk(color: Color, tokenInk: Color): Color = if (color in PorchlightTokens.memberColors) tokenInk
    else if (color.luminance() > 0.179f) Color.Black else Color.White
