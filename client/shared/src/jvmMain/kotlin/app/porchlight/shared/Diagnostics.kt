package app.porchlight.shared

actual fun reportDiagnostic(context: String, exception: Exception) {
    System.err.println("Porchlight: $context failed (${exception.javaClass.simpleName})")
}
