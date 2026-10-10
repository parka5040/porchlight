plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

val porchlightVersion = rootProject.file("../VERSION").readText().trim()
require(Regex("(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)").matches(porchlightVersion))
val versionParts = porchlightVersion.split('.').map(String::toLong)
require(versionParts[0] <= 210000 && versionParts[1] <= 99 && versionParts[2] <= 99) // bound before arithmetic
val porchlightVersionCode = versionParts[0] * 10000 + versionParts[1] * 100 + versionParts[2]
require(versionParts[1] <= 99 && versionParts[2] <= 99 && porchlightVersionCode in 1..2100000000L)
fun releaseSetting(name: String): String? = providers.gradleProperty(name).orNull
    ?: providers.environmentVariable(name).orNull
val releaseKeystore = releaseSetting("PORCHLIGHT_KEYSTORE")
val releasePasswordFile = releaseSetting("PORCHLIGHT_KEYSTORE_PASSWORD_FILE")
require((releaseKeystore == null) == (releasePasswordFile == null)) {
    "Both PORCHLIGHT_KEYSTORE and PORCHLIGHT_KEYSTORE_PASSWORD_FILE are required for signing"
}

android {
    namespace = "app.porchlight.android"
    compileSdk = libs.versions.android.compileSdk.get().toInt()

    defaultConfig {
        applicationId = "app.porchlight"
        minSdk = libs.versions.android.minSdk.get().toInt()
        targetSdk = libs.versions.android.targetSdk.get().toInt()
        versionCode = porchlightVersionCode.toInt()
        versionName = porchlightVersion
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        testInstrumentationRunnerArguments["notAnnotation"] = providers.gradleProperty(
            "android.testInstrumentationRunnerArguments.notAnnotation").orNull
            ?: "app.porchlight.android.trust.ControllerOnly"
    }

    if (releaseKeystore != null && releasePasswordFile != null) {
        check(!gradle.startParameter.isConfigurationCacheRequested) {
            "Signed builds require --no-configuration-cache so the signing password is not serialized"
        }
        signingConfigs {
            create("release") {
                storeFile = file(releaseKeystore)
                // Password remains in an operator-owned 0600 file, not argv or checked-in properties.
                storePassword = file(releasePasswordFile).readText().trimEnd('\n', '\r')
                keyAlias = "porchlight"
                keyPassword = storePassword
            }
        }
        buildTypes.getByName("release").signingConfig = signingConfigs.getByName("release")
    }
    // Missing both settings intentionally produces an unsigned CI release APK.

    lint { checkDependencies = true } // shared androidMain API guards must be scanned too.

    packaging {
        resources {
            pickFirsts += "META-INF/LICENSE.md" // identical licence in bcprov, bcutil and bcpkix (Bouncy Castle)
        }
    }

    testOptions {
        // Headless instrumented tests on AOSP x86_64 images: ./gradlew :androidApp:phonesGroupDebugAndroidTest
        managedDevices {
            localDevices {
                create("api35") {
                    device = "Pixel 6"
                    apiLevel = 35
                    systemImageSource = "aosp"
                }
                create("api36") {
                    device = "Pixel 6"
                    apiLevel = 36
                    systemImageSource = "aosp"
                }
            }
            groups {
                create("phones") {
                    targetDevices.add(localDevices["api35"])
                    targetDevices.add(localDevices["api36"])
                }
            }
        }
    }
}

// Real wrong-key APK for PackageManager signer tests; all key material stays in repository var/.
abstract class GenerateUpdateSignerFixture : Exec() {
    @get:OutputDirectory abstract val outputDirectory: DirectoryProperty
}
val generateUpdateSignerFixture by tasks.registering(GenerateUpdateSignerFixture::class) {
    outputDirectory.set(layout.buildDirectory.dir("generated/update-signer-assets"))
    val script = rootProject.file("../deploy/scripts/update-signer-fixture.sh")
    inputs.file(script)
    inputs.property("compileSdk", android.compileSdk!!)
    inputs.property("buildTools", android.buildToolsVersion)
    commandLine("bash", script.absolutePath, androidComponents.sdkComponents.sdkDirectory.get().asFile.absolutePath,
        androidComponents.sdkComponents.bootClasspath.get().first { it.asFile.name == "android.jar" }.asFile.absolutePath,
        android.buildToolsVersion, System.getProperty("java.home"),
        outputDirectory.get().asFile.absolutePath)
}
androidComponents.onVariants { variant ->
    variant.androidTest?.sources?.assets?.addGeneratedSourceDirectory(generateUpdateSignerFixture) { it.outputDirectory }
}

// c1a: AGP carries the generator dependency to every resource-processing task.
androidComponents.onVariants { variant ->
    variant.sources.res?.addGeneratedSourceDirectory(rootProject.tasks.named<GeneratePorchlightTokens>("generatePorchlightTokens")) {
        it.iconResourceDirectory
    }
}

dependencies {
    implementation(project(":shared"))
    implementation(libs.androidx.activity.compose)
    // LocalLifecycleOwner (QR scanner); debug got it only transitively, so release builds failed without it.
    implementation(libs.jetbrains.lifecycle.runtime.compose)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.zxing.core)
    implementation(libs.androidx.work.runtime.ktx)

    // c3b: key-screen instrumented equivalents use the same already-cached shared UI/model libraries.
    androidTestImplementation(libs.compose.foundation)
    androidTestImplementation(libs.kotlinx.datetime)
    androidTestImplementation(libs.kotlinx.serialization.json)
    androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.ext.junit)
    debugImplementation(libs.androidx.compose.ui.test.manifest)
}

// c2a: a real configured-runner assertion, also a prerequisite of every managed-device test task.
val excludedTestAnnotations = android.defaultConfig.testInstrumentationRunnerArguments["notAnnotation"].orEmpty()
val verifyManagedDeviceFilter by tasks.registering {
    inputs.property("excludedAnnotations", excludedTestAnnotations)
    doLast {
        check(inputs.properties.getValue("excludedAnnotations").toString().split(',').contains(
            "app.porchlight.android.trust.ControllerOnly")) {
            "Managed-device runs must exclude @ControllerOnly (UTP treats assumption skips as failures)"
        }
    }
}
tasks.withType<com.android.build.gradle.internal.tasks.ManagedDeviceInstrumentationTestTask>().configureEach {
    dependsOn(verifyManagedDeviceFilter)
}
tasks.withType<com.android.build.gradle.internal.tasks.ManagedDeviceInstrumentationTestResultAggregationTask>().configureEach {
    dependsOn(verifyManagedDeviceFilter)
}
tasks.withType<com.android.build.gradle.internal.tasks.ManagedDeviceTestTask>().configureEach {
    dependsOn(verifyManagedDeviceFilter)
}
