plugins {
    alias(libs.plugins.kotlin.multiplatform)
    alias(libs.plugins.android.kmp.library)
    id("com.android.lint")
    alias(libs.plugins.compose.multiplatform)
    alias(libs.plugins.compose.compiler)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.sqldelight)
    alias(libs.plugins.openapi.generator)
}

// c2b: standard model generator on the authoritative spec (C2a's conventions); the API template delegates directly
// to the existing trust transport.
val generatedApi = layout.buildDirectory.dir("generated/openapi")
openApiGenerate {
    generatorName = "kotlin"
    library = "multiplatform"
    inputSpec = rootProject.layout.projectDirectory.file("../docs/openapi.json")
    outputDir = generatedApi
    cleanupOutput = true
    packageName = "app.porchlight.shared.api"
    templateDir = rootProject.layout.projectDirectory.dir("openapi-templates")
    configOptions = OpenApiClientConventions.CONFIG_OPTIONS
    globalProperties = mapOf("models" to "", "apis" to "", "supportingFiles" to "false", "apiDocs" to "false", "modelDocs" to "false")
    schemaMappings = OpenApiClientConventions.SHARED_SCHEMA_MAPPINGS
    typeMappings = OpenApiClientConventions.TYPE_MAPPINGS
    importMappings = OpenApiClientConventions.IMPORT_MAPPINGS
}
tasks.named("openApiGenerate") {
    inputs.dir(rootProject.layout.projectDirectory.dir("openapi-templates"))
}
tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompilationTask<*>>().configureEach { dependsOn("openApiGenerate") }
// Release-only: AGP's ART-profile task scans <source dir>/../baselineProfiles of the generated sources.
tasks.matching { it.name == "prepareAndroidMainArtProfile" }.configureEach { dependsOn("openApiGenerate") }

val generatePorchlightTokens = rootProject.tasks.named<GeneratePorchlightTokens>("generatePorchlightTokens")

kotlin {
    android {
        namespace = "app.porchlight.shared"
        compileSdk = libs.versions.android.compileSdk.get().toInt()
        minSdk = libs.versions.android.minSdk.get().toInt()
        lint { abortOnError = true; checkDependencies = true }
        androidResources { enable = true } // Compose resources (fonts) are packaged as Android assets
        withHostTest { } // commonTest also runs on the JVM-hosted Android target: :shared:testAndroidHostTest
    }
    jvm()

    sourceSets {
        commonMain {
            kotlin.srcDir(generatePorchlightTokens.map { it.outputDirectory })
            kotlin.srcDir(generatedApi.map { it.dir("src/commonMain/kotlin") })
        }
        // c1b: JCA/JSSE/OkHttp adapters compile on both Java platforms, without widening commonMain.
        androidMain { kotlin.srcDir("src/tlsMain/kotlin") }
        jvmMain { kotlin.srcDir("src/tlsMain/kotlin") }
        commonMain.dependencies {
            implementation(libs.compose.runtime)
            implementation(libs.compose.foundation)
            implementation(libs.compose.ui)
            implementation(libs.compose.material3)
            implementation(libs.compose.components.resources)
            implementation(libs.ktor.client.core)
            implementation(libs.kotlinx.serialization.json)
            implementation(libs.kotlinx.coroutines.core)
            implementation(libs.kotlinx.datetime)
            implementation(libs.sqldelight.runtime)
            implementation(libs.androidx.datastore.preferences.core)
            implementation(libs.androidx.datastore.core.okio)
            implementation(libs.jetbrains.navigation.compose)
            implementation(libs.jetbrains.lifecycle.viewmodel.compose)
            implementation(libs.jetbrains.lifecycle.runtime.compose)
        }
        androidMain.dependencies {
            implementation(libs.ktor.client.okhttp)
            implementation(libs.sqldelight.android.driver)
            implementation(libs.androidx.biometric)
            implementation(libs.androidx.work.runtime.ktx)
            implementation(libs.androidx.core.ktx)
            implementation(libs.androidx.lifecycle.service)
            implementation(libs.bouncycastle.bcpkix)
        }
        jvmMain.dependencies {
            implementation(libs.ktor.client.okhttp)
            implementation(libs.sqldelight.sqlite.driver)
            implementation(libs.java.keyring)
            implementation(libs.dbus.java.core)
            implementation(libs.dbus.java.transport.native.unixsocket)
            implementation(libs.bouncycastle.bcpkix)
            implementation(libs.kotlinx.coroutines.swing)
        }
        commonTest.dependencies {
            implementation(libs.kotlin.test)
            implementation(libs.kotlinx.coroutines.test)
            implementation(libs.turbine)
            implementation(libs.ktor.client.mock)
            implementation(libs.ktor.client.content.negotiation)
            implementation(libs.ktor.serialization.kotlinx.json)
        }
        getByName("androidHostTest").dependencies { implementation(libs.sqldelight.sqlite.driver) }
        jvmTest.dependencies {
            implementation(libs.compose.ui.test)
            implementation(libs.compose.ui.test.junit4)
            implementation(compose.desktop.currentOs)
        }
    }
}

val resourcesDirectory = layout.buildDirectory.dir("generated/porchlight-resources")
val preparePorchlightResources by tasks.registering(Sync::class) {
    from(layout.projectDirectory.dir("src/commonMain/composeResources"))
    from(rootProject.layout.projectDirectory.file("../design/icon/porchlight-icon-512.png")) {
        into("drawable")
        rename { "porchlight_icon.png" }
    }
    into(resourcesDirectory)
}

compose.resources {
    customDirectory(sourceSetName = "commonMain", directoryProvider = preparePorchlightResources.map { resourcesDirectory.get() })
    packageOfResClass = "app.porchlight.shared.resources"
}

sqldelight {
    databases {
        create("PorchlightDb") {
            packageName = "app.porchlight.shared.db"
        }
    }
}

tasks.named<Test>("jvmTest") {
    // Audit the actual merged backup policy, not just the source application manifest.
    dependsOn(":androidApp:processDebugManifest", ":androidApp:packageDebugResources")
    inputs.files(rootProject.layout.projectDirectory.file("androidApp/build/intermediates/merged_manifests/debug/processDebugManifest/AndroidManifest.xml"))
    inputs.dir(rootProject.layout.projectDirectory.dir("androidApp/build/intermediates/packaged_res/debug/packageDebugResources/xml"))
    // Compose UI tests render offscreen: no display needed (sandboxed agents have none).
    systemProperty("java.awt.headless", "true")
    // Skiko unpacks its native library to ~/.skiko by default; sandboxed agents have a read-only home.
    systemProperty("skiko.data.path", layout.buildDirectory.dir("skiko").get().asFile.absolutePath)
    jvmArgs("--enable-native-access=ALL-UNNAMED") // Skiko loads its native library
}

// Run existing headless smoke tests with only the shipping app jars and bundled JVM. The
// jpackage runtime intentionally has no java executable, so the harness reuses its native launcher.
tasks.register<Exec>("packagedDesktopSmoke") {
    val main = kotlin.targets.getByName("jvm").compilations.getByName("main")
    val tests = kotlin.targets.getByName("jvm").compilations.getByName("test")
    val desktopRuntime = project(":desktopApp").configurations.named("runtimeClasspath").get()
    val testOnly = tests.output.allOutputs + (checkNotNull(tests.runtimeDependencyFiles) - desktopRuntime - main.output.allOutputs)
    val image = rootProject.layout.projectDirectory.dir("desktopApp/build/compose/binaries/main/app/porchlight").asFile
    val harness = rootProject.layout.projectDirectory.file("../deploy/tests/audit-desktop-smoke.py").asFile
    dependsOn(tests.compileTaskProvider, ":desktopApp:createDistributable")
    inputs.files(testOnly)
    inputs.dir(image)
    inputs.file(harness)
    doFirst {
        commandLine(listOf("python3", "-I", harness.absolutePath, image.absolutePath) + testOnly.files.map { it.absolutePath })
    }
}

// Applies to both desktop and Android-host JVM tests (ART needs neither flag).
tasks.withType<Test>().configureEach {
    // Persisted restart fixtures and generated test trust belong in private runtime storage on both JVM targets.
    systemProperty("porchlight.test.dir", rootProject.layout.projectDirectory.dir("../var/trust-tests").asFile.absolutePath)
    jvmArgs("--sun-misc-unsafe-memory-access=allow", "-Dslf4j.internal.verbosity=ERROR")
}
