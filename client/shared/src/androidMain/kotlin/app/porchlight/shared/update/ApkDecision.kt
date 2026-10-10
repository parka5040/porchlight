package app.porchlight.shared.update

const val DIFFERENT_APK_KEY = "This release is signed with a different key, so Android would refuse it. Ask your admin."
data class ApkIdentity(val packageName: String, val versionCode: Long, val signers: Set<String>?)
fun apkPreflight(installed: ApkIdentity, advertised: ApkIdentity): String? =
    if (installed.packageName != advertised.packageName || installed.signers.isNullOrEmpty() ||
        advertised.signers != installed.signers || installed.signers.any { !it.matches(Regex("[0-9a-f]{64}")) }) DIFFERENT_APK_KEY else null
fun apkDecision(installed: ApkIdentity, advertised: ApkIdentity, archive: ApkIdentity?): String? {
    apkPreflight(installed, advertised)?.let { return it }
    if (archive == null || archive.packageName != installed.packageName) return "The downloaded APK is not Porchlight; nothing was installed."
    if (archive.versionCode <= installed.versionCode || archive.versionCode != advertised.versionCode)
        return "The APK version does not match this update; nothing was installed."
    if (archive.signers != null && archive.signers != installed.signers) return DIFFERENT_APK_KEY
    return null // Missing archive signingInfo: advertised certs matched; Android enforces them again at install.
}
sealed interface InstallResult {
    data object Confirm : InstallResult
    data object Success : InstallResult
    data class Failure(val reason: String) : InstallResult
}
fun installResult(status: Int, message: String?): InstallResult = when (status) {
    -1 -> InstallResult.Confirm
    0 -> InstallResult.Success
    else -> InstallResult.Failure("Android could not install the update" +
        (message?.filter { it >= ' ' && it != '\u007f' }?.take(240)?.takeIf { it.isNotBlank() }?.let { ": $it" } ?: ". Please try again."))
}

/** Installation may replace/kill this process before its final callback arrives. */
fun restoredInstallState(installedVersion: Long, targetVersion: Long, sessionExists: Boolean): UpdateState = when {
    installedVersion >= targetVersion -> UpdateState.Idle
    sessionExists -> UpdateState.Installing
    else -> UpdateState.Failed("The Android installation was interrupted. Please try again.")
}
