plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.kmp.library) apply false
    alias(libs.plugins.kotlin.multiplatform) apply false
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.compose.multiplatform) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.kotlin.serialization) apply false
    alias(libs.plugins.sqldelight) apply false
}

// c1a: one generator shared by Kotlin compilation and AGP's generated-resource API.
tasks.register<GeneratePorchlightTokens>("generatePorchlightTokens") {
    tokensFile.set(layout.projectDirectory.file("../design/tokens.json"))
    versionFile.set(layout.projectDirectory.file("../VERSION"))
    outputDirectory.set(layout.buildDirectory.dir("generated/porchlight/commonMain"))
    iconResourceDirectory.set(layout.buildDirectory.dir("generated/porchlight/androidRes"))
}

allprojects {
    dependencyLocking { lockAllConfigurations() }
}
