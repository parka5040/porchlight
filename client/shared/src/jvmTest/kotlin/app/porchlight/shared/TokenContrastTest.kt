package app.porchlight.shared

import java.io.File
import androidx.compose.ui.graphics.toArgb
import app.porchlight.shared.theme.PorchlightTokens
import kotlin.math.pow
import kotlin.test.Test
import kotlin.test.assertTrue
import kotlinx.serialization.json.*

class TokenContrastTest {
    internal fun luminance(hex: String): Double = hex.removePrefix("#").chunked(2)
        .map { it.toInt(16) / 255.0 }
        .map { if (it <= 0.04045) it / 12.92 else ((it + 0.055) / 1.055).pow(2.4) }
        .let { it[0] * 0.2126 + it[1] * 0.7152 + it[2] * 0.0722 }

    @Test fun androidFirstFrameBackgroundsMatchThemeTokens() {
        val themes = Json.parseToJsonElement(File("../../design/tokens.json").readText()).jsonObject.getValue("themes").jsonObject
        for ((directory, theme) in listOf("values" to "Daylight", "values-night" to "Evening")) {
            val resources = javax.xml.parsers.DocumentBuilderFactory.newInstance().newDocumentBuilder()
                .parse(File("../androidApp/src/main/res/$directory/porchlight-theme.xml"))
            val items = resources.getElementsByTagName("item")
            val background = (0 until items.length).map { items.item(it) as org.w3c.dom.Element }
                .single { it.getAttribute("name") == "android:windowBackground" }.textContent.trim()
            kotlin.test.assertEquals(themes.getValue(theme).jsonObject.getValue("background").jsonPrimitive.content.uppercase(), background.uppercase())
        }
    }

    @Test fun eveningFillsStayOnBrandAndTextRolesUseMockupColors() {
        val root = Json.parseToJsonElement(File("../../design/tokens.json").readText()).jsonObject
        val evening = root.getValue("themes").jsonObject.getValue("Evening").jsonObject
        mapOf("primary" to "#FABD2F", "secondary" to "#FE8019", "error" to "#FB4934",
            "success" to "#B8BB26", "info" to "#83A598", "onSurfaceMuted" to "#BDAE93",
            "errorText" to "#FD8D7F").forEach { (role, value) ->
            kotlin.test.assertEquals(value, evening[role]?.jsonPrimitive?.content, role)
        }
    }
    @Test fun everyDeclaredPairPassesWcag22AA() {
        val root = Json.parseToJsonElement(File("../../design/tokens.json").readText()).jsonObject
        val offenders = mutableListOf<String>()
        root.getValue("themes").jsonObject.forEach { (name, theme) ->
            val colors = theme.jsonObject
            val pairs = root.getValue("contrastPairs").jsonArray
            // Coverage follows real C1a drawing roles, not every accent on every surface.
            val usages = mapOf(
                "onSurface" to listOf("background", "surface", "surfaceVariant", "navigationSurface", "selectedSurface"),
                "onSurfaceMuted" to listOf("background", "surface", "navigationSurface"),
                "primaryText" to listOf("selectedSurface"),
                "errorText" to listOf("surface"), "infoText" to listOf("surface"),
                "heading" to listOf("background", "surface", "navigationSurface"))
            usages.forEach { (role, surfaces) -> surfaces.forEach { surface ->
                assertTrue(pairs.any { it.jsonObject["foreground"]?.jsonPrimitive?.content == role &&
                    it.jsonObject["background"]?.jsonPrimitive?.content == surface &&
                    it.jsonObject["kind"]?.jsonPrimitive?.content == "text" }, "Missing usage $role/$surface")
            } }
            assertTrue(pairs.any { it.jsonObject["foreground"]?.jsonPrimitive?.content == "primary" &&
                it.jsonObject["background"]?.jsonPrimitive?.content == "background" &&
                it.jsonObject["kind"]?.jsonPrimitive?.content == "boundary" }, "Missing selected outline")
            pairs.forEach { pair ->
                val p = pair.jsonObject
                val required = when (p.getValue("kind").jsonPrimitive.content) {
                    "text" -> 4.5
                    "largeText", "icon", "boundary" -> 3.0
                    else -> error("Unknown contrast pair kind: $p")
                }
                assertTrue(p.getValue("minimum").jsonPrimitive.double >= required, "Understated AA minimum: $p")
                val fg = p.getValue("foreground").jsonPrimitive.content
                val bg = p.getValue("background").jsonPrimitive.content
                val a = luminance(colors.getValue(fg).jsonPrimitive.content)
                val b = luminance(colors.getValue(bg).jsonPrimitive.content)
                val ratio = (maxOf(a, b) + 0.05) / (minOf(a, b) + 0.05)
                if (ratio < p.getValue("minimum").jsonPrimitive.double) offenders += "$name $fg/$bg: $ratio"
            }
        }
        // MemberChip renders this exact list; no theme-local duplicates may hide failures.
        val members = root.getValue("memberColors").jsonArray
        kotlin.test.assertEquals(members.map { it.jsonPrimitive.content },
            PorchlightTokens.memberColors.map { "#" + (it.toArgb() and 0xFFFFFF).toString(16).padStart(6, '0').uppercase() },
            "The UI must render the checked member-fill list")
        root.getValue("themes").jsonObject.forEach { (theme, values) ->
            val colors = values.jsonObject
            assertTrue(colors.keys.none { it.matches(Regex("member[0-9]+")) }, "Duplicate member list")
            members.forEachIndexed { index, fill ->
                val a = luminance(colors.getValue("memberInk").jsonPrimitive.content)
                val b = luminance(fill.jsonPrimitive.content)
                val ratio = (maxOf(a, b) + 0.05) / (minOf(a, b) + 0.05)
                if (ratio < 4.5) offenders += "$theme memberInk/memberColors[$index]: $ratio"
            }
        }
        assertTrue(offenders.isEmpty(), offenders.joinToString("\n"))
    }
}
