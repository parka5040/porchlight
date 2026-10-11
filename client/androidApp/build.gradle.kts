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
// Opt-in instrumentation against release code/rules, without production signing keys or debug fixtures.
val minifiedSmoke = providers.gradleProperty("porchlight.minifiedSmoke").orNull == "true"
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
    buildTypes.getByName("release") {
        isMinifyEnabled = true
        isShrinkResources = true
        proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
    }
    buildTypes.create("minified") {
        initWith(buildTypes.getByName("release"))
        signingConfig = signingConfigs.getByName("debug")
        applicationIdSuffix = ".minified"
        matchingFallbacks += "release"
        // AndroidX instrumentation resolves this shared-library ABI from the target APK.
        // Never apply these test-only roots to the shipping release build.
        proguardFiles("proguard-minified-test.pro")
    }
    if (minifiedSmoke) {
        testBuildType = "minified"
        // Debug's instrumentation uses fixture activities that must never enter a release-like APK.
        sourceSets.getByName("androidTest") {
            java.setSrcDirs(listOf("src/minifiedAndroidTest/kotlin"))
            kotlin.setSrcDirs(listOf("src/minifiedAndroidTest/kotlin"))
        }
    }

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
    if (!minifiedSmoke) variant.androidTest?.sources?.assets?.addGeneratedSourceDirectory(generateUpdateSignerFixture) { it.outputDirectory }
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
    // Trust helper theme uses Theme.AppCompat; pin the already-locked transitive artifact explicitly.
    runtimeOnly(libs.androidx.appcompat)
    // LocalLifecycleOwner (QR scanner); debug got it only transitively, so release builds failed without it.
    implementation(libs.jetbrains.lifecycle.runtime.compose)
    implementation(libs.androidx.camera.camera2)
    implementation(libs.androidx.camera.lifecycle)
    implementation(libs.androidx.camera.view)
    implementation(libs.zxing.core)
    implementation(libs.androidx.work.runtime.ktx)
    // Already in shared's runtime; only the minified-only probe calls Json directly from this module.
    add("minifiedImplementation", libs.kotlinx.serialization.json)

    // c3b: debug key-screen instrumentation uses shared UI/model libraries. The minified smoke
    // stays black-box: Compose test internals can reference methods inlined away from release.
    if (!minifiedSmoke) {
        androidTestImplementation(libs.compose.foundation)
        androidTestImplementation(libs.kotlinx.datetime)
        androidTestImplementation(libs.kotlinx.serialization.json)
        androidTestImplementation(libs.androidx.compose.ui.test.junit4)
    }
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
// These task types are internal AGP APIs: a plugin bump requires rechecking all three filter hooks.
check(com.android.Version.ANDROID_GRADLE_PLUGIN_VERSION == "9.4.1") {
    "Revalidate the managed-device filter hooks before changing the pinned AGP version"
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
