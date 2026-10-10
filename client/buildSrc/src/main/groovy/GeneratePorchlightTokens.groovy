import groovy.json.JsonSlurper
import org.gradle.api.DefaultTask
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.tasks.*

/** Pure, cacheable generator: design tokens and VERSION are the only inputs. */
@CacheableTask
abstract class GeneratePorchlightTokens extends DefaultTask {
    @InputFile @PathSensitive(PathSensitivity.RELATIVE)
    abstract RegularFileProperty getTokensFile()
    @InputFile @PathSensitive(PathSensitivity.RELATIVE)
    abstract RegularFileProperty getVersionFile()
    @OutputDirectory abstract DirectoryProperty getOutputDirectory()
    @OutputDirectory abstract DirectoryProperty getIconResourceDirectory()

    @TaskAction void generate() {
        def tokens = new JsonSlurper().parse(tokensFile.get().asFile)
        def version = versionFile.get().asFile.text.trim()
        if (!(version ==~ /\d+\.\d+\.\d+(-[A-Za-z0-9.-]+)?/)) throw new IllegalArgumentException('Invalid VERSION')
        def color = { String hex ->
            if (!(hex ==~ /#[0-9A-Fa-f]{6}/)) throw new IllegalArgumentException("Invalid color: $hex")
            "Color(0xFF${hex.substring(1)}L)"
        }
        def names = tokens.themes.Evening.keySet().toList()
        if (names.toSet() != tokens.themes.Daylight.keySet().toSet()) throw new IllegalArgumentException('Theme roles differ')
        def output = outputDirectory.get().asFile
        output.deleteDir()
        iconResourceDirectory.get().asFile.deleteDir()
        def themeDir = new File(output, 'app/porchlight/shared/theme'); themeDir.mkdirs()
        new File(themeDir, 'PorchlightTokens.kt').text = '''// Generated from design/tokens.json. Do not edit.
package app.porchlight.shared.theme
import androidx.compose.ui.graphics.Color

data class PorchlightPalette(
''' + names.collect { "    val $it: Color" }.join(',\n') + '\n)\n\nobject PorchlightTokens {\n' +
            tokens.themes.collect { name, values ->
                "    val $name = PorchlightPalette(\n" + names.collect { "        $it = ${color(values[it])}" }.join(',\n') + '\n    )'
            }.join('\n') + '\n    val memberColors = listOf(' + tokens.memberColors.collect(color).join(', ') + ')\n' +
            tokens.dimensions.collect { k, v -> "    const val $k = $v" }.join('\n') + '\n' +
            "    const val headingFamily = \"${tokens.typography.headingFamily}\"\n" +
            "    const val bodyFamily = \"${tokens.typography.bodyFamily}\"\n" +
            "    const val headingWeight = ${tokens.typography.headingWeight}\n" +
            "    val bodyWeights = listOf(${tokens.typography.bodyWeights.join(', ')})\n" +
            "    const val lineHeightFactor = ${tokens.typography.lineHeightFactor}\n" +
            tokens.typography.sizes.collect { k, v -> "    const val ${k}Size = $v" }.join('\n') + '\n}\n'
        def iconValues = new File(iconResourceDirectory.get().asFile, 'values'); iconValues.mkdirs()
        new File(iconValues, 'porchlight_icon_colors.xml').text = "<resources><color name=\"porchlight_icon_background\">${tokens.themes.Evening.navigationSurface}</color></resources>\n"
        def shared = new File(output, 'app/porchlight/shared'); shared.mkdirs()
        def parts = version ==~ /\d+\.\d+\.\d+/ ? version.split(/\./).collect { new BigInteger(it) } : null
        def code = parts == null ? 0 : parts[0] * 10000 + parts[1] * 100 + parts[2]
        if (code > Integer.MAX_VALUE) code = 0
        new File(shared, 'BuildInfo.kt').text = "// Generated from VERSION. Do not edit.\npackage app.porchlight.shared\nobject BuildInfo { const val VERSION = \"$version\"; const val VERSION_CODE = $code }\n"
    }
}
