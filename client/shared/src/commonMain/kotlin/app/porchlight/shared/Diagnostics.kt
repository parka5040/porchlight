package app.porchlight.shared

/** Platform diagnostic; context is a fixed operation label, never a settings payload or credential. */
expect fun reportDiagnostic(context: String, exception: Exception)
