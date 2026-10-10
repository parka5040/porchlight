package app.porchlight.android.trust

/** Requires private live-server configuration; excluded from deterministic managed-device suites. */
@Target(AnnotationTarget.CLASS, AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class ControllerOnly
