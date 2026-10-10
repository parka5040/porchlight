plugins {
    alias(libs.plugins.kotlin.multiplatform)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.openapi.generator)
}

val generatedDir = layout.buildDirectory.dir("generated/openapi")
openApiGenerate {
    generatorName = "kotlin"
    library = "multiplatform"
    inputSpec = rootProject.layout.projectDirectory.file("../docs/openapi.json")
    outputDir = generatedDir
    cleanupOutput = true
    packageName = "app.porchlight.contract"
    // Shared with :shared (buildSrc OpenApiClientConventions) so the drift gate and the app client cannot diverge.
    configOptions = OpenApiClientConventions.CONFIG_OPTIONS
    schemaMappings = OpenApiClientConventions.SCHEMA_MAPPINGS
    typeMappings = OpenApiClientConventions.TYPE_MAPPINGS
    importMappings = OpenApiClientConventions.IMPORT_MAPPINGS
}
kotlin {
    jvm()
    sourceSets {
        commonMain {
            kotlin.srcDir(generatedDir.map { it.dir("src/commonMain/kotlin") })
            dependencies {
                implementation(libs.ktor.client.core)
                implementation(libs.ktor.client.content.negotiation)
                implementation(libs.ktor.serialization.kotlinx.json)
                implementation(libs.kotlinx.serialization.json)
                implementation(libs.kotlinx.coroutines.core)
                implementation(libs.kotlinx.datetime)
            }
        }
        commonTest.dependencies { implementation(libs.kotlin.test) }
    }
}
// Main is entirely generator output; handwritten smoke-test warnings remain visible.
tasks.named<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>("compileKotlinJvm") {
    compilerOptions.suppressWarnings.set(true)
}

tasks.matching { it.name.startsWith("compile") && it.name.contains("Kotlin") }.configureEach {
    dependsOn("openApiGenerate")
}
