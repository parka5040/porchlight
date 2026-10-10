package app.porchlight.shared.files

/**
 * Porchlight's own private directory ([path]) cannot be used safely: a persistent local condition ([reason]), not a
 * transient failure, so retrying does not help until the directory is fixed. Paths here are local, never server data.
 */
class PrivateDirectoryUnavailable(val path: String, val reason: String) : Exception("Private directory unavailable: $reason")
