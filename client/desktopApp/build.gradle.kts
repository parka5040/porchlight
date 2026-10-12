import java.nio.file.Files
import java.nio.file.StandardCopyOption.REPLACE_EXISTING
import java.util.zip.ZipEntry
import java.util.zip.ZipFile
import java.util.zip.ZipOutputStream

plugins {
    alias(libs.plugins.kotlin.jvm)
    alias(libs.plugins.compose.multiplatform)
    alias(libs.plugins.compose.compiler)
}

dependencies {
    implementation(project(":shared"))
    implementation(compose.desktop.currentOs)
    implementation(libs.kotlinx.coroutines.swing)
}

// The shipping image is glibc Linux x86_64. Filter only the packaging input: never mutate the
// dependency cache or the JVM test/runtime classpath, and preserve every non-native entry verbatim.
val sqliteJdbc = configurations.runtimeClasspath.map { files ->
    files.single { it.name.startsWith("sqlite-jdbc-") && it.extension == "jar" }
}
val packageSqliteJdbc by tasks.registering(Zip::class) {
    archiveFileName.set(sqliteJdbc.map { it.name })
    destinationDirectory.set(layout.buildDirectory.dir("desktop-sqlite"))
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
    includeEmptyDirs = false
    duplicatesStrategy = DuplicatesStrategy.FAIL
    from(sqliteJdbc.map { zipTree(it) }) {
        include {
            it.isDirectory || !it.path.startsWith("org/sqlite/native/") ||
                it.path == "org/sqlite/native/Linux/x86_64/libsqlitejdbc.so"
        }
    }
    doFirst {
        check(System.getProperty("os.name") == "Linux" && System.getProperty("os.arch") in listOf("amd64", "x86_64")) {
            "Desktop packaging supports glibc Linux x86_64 only"
        }
    }
}

compose.desktop {
    application {
        disableDefaultConfiguration()
        fromFiles(tasks.named("jar"), packageSqliteJdbc, configurations.runtimeClasspath.get().filter {
            !it.name.startsWith("sqlite-jdbc-")
        })
        mainClass = "app.porchlight.desktop.MainKt"
        mainJar.set(tasks.named<Jar>("jar").flatMap { it.archiveFile })
        jvmArgs += listOf("--enable-native-access=ALL-UNNAMED", "--sun-misc-unsafe-memory-access=allow", "-Dslf4j.internal.verbosity=ERROR") // Skiko loads its native library
        // Release images are shrunk (shrink only, names kept); createDistributable stays unshrunk for dev demos.
        buildTypes.release.proguard {
            isEnabled.set(true)
            obfuscate.set(false)
            optimize.set(false)
            configurationFiles.from(project.file("proguard-rules.pro"))
        }
        nativeDistributions {
            packageName = "porchlight"
            packageVersion = rootProject.file("../VERSION").readText().trim()
            // suggestModules discovers instrument + unsupported; include TLS crypto for reflection-loaded providers,
            // java.sql for the SQLite read cache, jdk.security.auth for dbus-java's reflective Unix auth (tray, notifications,
            // portal theme, Secret Service) and java.management for ktor-utils.
            modules("java.instrument", "jdk.unsupported", "jdk.crypto.ec", "java.sql", "jdk.security.auth", "java.management")
            linux { iconFile.set(rootProject.layout.projectDirectory.file("../design/icon/porchlight-icon-512.png")) }
        }
    }
}

// ProGuard rewrites classes but copies upstream signature files (BouncyCastle ships signed jars). A signature that no
// longer matches its entries makes the JVM reject classes from that jar, so shrunk jars are published unsigned. Service
// descriptors may also name classes shrinking removed (BouncyCastle's PQC provider); ServiceLoader would then fail with
// ServiceConfigurationError, so such lines are dropped. Untouched descriptors stay byte-identical.
tasks.matching { it.name == "proguardReleaseJars" }.configureEach {
    val shrunk = layout.buildDirectory.dir("compose/tmp/main-release/proguard")
    doLast {
        val signature = Regex("META-INF/[^/]+\\.(SF|RSA|DSA|EC)", RegexOption.IGNORE_CASE)
        val jars = shrunk.get().asFile.listFiles { file -> file.extension == "jar" }.orEmpty().sortedBy { it.name }
        val contents = jars.associateWith { jar -> ZipFile(jar).use { zip -> zip.entries().toList().map { it to zip.getInputStream(it).readBytes() } } }
        val classes = contents.values.flatten().map { it.first.name }.filter { it.endsWith(".class") }
            .map { it.removeSuffix(".class").replace('/', '.') }.toSet()
        contents.forEach { (jar, entries) ->
            val output = entries.mapNotNull { (entry, bytes) ->
                when {
                    signature.matches(entry.name) -> null
                    entry.name.startsWith("META-INF/services/") && !entry.isDirectory -> {
                        val lines = String(bytes, Charsets.UTF_8).lines()
                        val kept = lines.filter { line -> line.substringBefore('#').trim().let { it.isEmpty() || it in classes } }
                        when {
                            kept.size == lines.size -> entry to bytes
                            kept.none { it.substringBefore('#').isNotBlank() } -> null
                            else -> entry to kept.joinToString("\n").toByteArray(Charsets.UTF_8)
                        }
                    }
                    else -> entry to bytes
                }
            }
            if (output.size == entries.size && output.zip(entries).all { (a, b) -> a.second === b.second }) return@forEach
            val rewritten = File(jar.path + ".rewritten")
            ZipOutputStream(rewritten.outputStream()).use { out ->
                output.forEach { (entry, bytes) ->
                    out.putNextEntry(ZipEntry(entry.name).apply { time = entry.time })
                    out.write(bytes); out.closeEntry()
                }
            }
            Files.move(rewritten.toPath(), jar.toPath(), REPLACE_EXISTING)
        }
    }
}

// Keep the installer/desktop/autostart entry point, but decide AWT's WM mode before the JVM starts.
fun wrapLauncher(name: String, image: String) = tasks.register(name) {
    val wrapperSource = layout.projectDirectory.file("launcher/porchlight.sh")
    val imageDirectory = layout.buildDirectory.dir(image)
    inputs.file(wrapperSource)
    // No up-to-date shortcut: jpackage may have recreated the native launcher since the last wrap.
    doLast {
        val image = imageDirectory.get().asFile.toPath()
        val launcher = image.resolve("bin/porchlight")
        val native = image.resolve("bin/porchlight-app")
        val wrapperBytes = wrapperSource.asFile.readBytes()
        // Move only jpackage's binary: an older wrapper (any "#!" script) must never replace porchlight-app.
        val current = Files.readAllBytes(launcher)
        if (!(current.size >= 2 && current[0] == '#'.code.toByte() && current[1] == '!'.code.toByte())) {
            Files.move(launcher, native, REPLACE_EXISTING)
        }
        check(Files.isExecutable(native)) { "Missing executable jpackage launcher: $native" }
        val originalConfig = image.resolve("lib/app/porchlight.cfg")
        val nativeConfig = image.resolve("lib/app/porchlight-app.cfg")
        if (Files.exists(originalConfig)) Files.move(originalConfig, nativeConfig, REPLACE_EXISTING)
        check(Files.isRegularFile(nativeConfig)) { "Missing renamed jpackage config: $nativeConfig" }
        Files.write(launcher, wrapperBytes)
        check(launcher.toFile().setExecutable(true, false)) { "Cannot make launcher wrapper executable" }
    }
}
val wrapDesktopLauncher = wrapLauncher("wrapDesktopLauncher", "compose/binaries/main/app/porchlight")
val wrapReleaseDesktopLauncher = wrapLauncher("wrapReleaseDesktopLauncher", "compose/binaries/main-release/app/porchlight")
tasks.matching { it.name == "createDistributable" }.configureEach { finalizedBy(wrapDesktopLauncher) }
tasks.matching { it.name == "createReleaseDistributable" }.configureEach { finalizedBy(wrapReleaseDesktopLauncher) }
