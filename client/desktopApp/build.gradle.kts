import java.nio.file.Files
import java.nio.file.StandardCopyOption.REPLACE_EXISTING

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

// Keep the installer/desktop/autostart entry point, but decide AWT's WM mode before the JVM starts.
val wrapDesktopLauncher by tasks.registering {
    val wrapperSource = layout.projectDirectory.file("launcher/porchlight.sh")
    val imageDirectory = layout.buildDirectory.dir("compose/binaries/main/app/porchlight")
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
tasks.matching { it.name == "createDistributable" }.configureEach { finalizedBy(wrapDesktopLauncher) }
