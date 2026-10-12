// Controller tool (needs network): resolves every resolvable configuration of every project, writes the lockfiles
// and downloads every artifact into the project-local Gradle home (var/gradle-home, see scripts/env.sh), so
// sandboxed agents can build offline. Not part of the build; run after changing any dependency or plugin:
//
//   cd client && . ../scripts/env.sh
//   ./gradlew -I gradle/cache-all-dependencies.init.gradle.kts cacheAllDependencies --write-locks --no-configuration-cache
//
// then run the real tasks once (assemble, tests, lint, createReleaseDistributable) so tool configurations resolved only at
// execution time (managed-device test platform, desktop packaging) are cached too.
allprojects {
    tasks.register("cacheAllDependencies") {
        group = "porchlight"
        description = "Resolves all configurations and downloads their artifacts (offline cache + lockfiles)."
        notCompatibleWithConfigurationCache("resolves arbitrary configurations at execution time")
        val configurationsToResolve = project.configurations.matching { it.isCanBeResolved }
        doLast {
            configurationsToResolve.toList().forEach { configuration -> // resolving may register more
                try {
                    // Only external modules need downloading; project dependencies would need consumer attributes.
                    val files = configuration.incoming.artifactView {
                        componentFilter { it !is org.gradle.api.artifacts.component.ProjectComponentIdentifier }
                    }.files.files
                    logger.lifecycle("cached ${project.path}:${configuration.name} (${files.size} files)")
                } catch (e: Exception) {
                    val causes = generateSequence(e as Throwable) { it.cause }.mapNotNull { it.message?.lineSequence()?.first() }
                    logger.warn("skipped ${project.path}:${configuration.name}: ${causes.distinct().joinToString(" <- ")}")
                }
            }
        }
    }
}
