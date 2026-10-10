package app.porchlight.shared

actual fun reportDiagnostic(context: String, exception: Exception) {
    android.util.Log.w("Porchlight", "$context failed (${exception.javaClass.simpleName})")
}
