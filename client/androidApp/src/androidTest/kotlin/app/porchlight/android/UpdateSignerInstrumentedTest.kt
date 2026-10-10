package app.porchlight.android

import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import app.porchlight.shared.update.ApkIdentity
import app.porchlight.shared.update.DIFFERENT_APK_KEY
import app.porchlight.shared.update.apkDecision
import app.porchlight.shared.update.apkPreflight
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.security.MessageDigest

/** Runs in managed-device CI too: no server, enrollment, install permission or PackageInstaller session. */
@RunWith(AndroidJUnit4::class)
class UpdateSignerInstrumentedTest {
    @Suppress("DEPRECATION")
    private fun PackageInfo.identity() = ApkIdentity(packageName, longVersionCode,
        signingInfo?.apkContentsSigners?.map { signer ->
            MessageDigest.getInstance("SHA-256").digest(signer.toByteArray())
                .joinToString("") { "%02x".format(it.toInt() and 0xff) }
        }?.toSet())

    @Suppress("DEPRECATION")
    @Test fun installedAndRealArchiveSignersFailClosedOnDifferentKey() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val manager = context.packageManager
        val installed = manager.getPackageInfo(context.packageName, PackageManager.GET_SIGNING_CERTIFICATES).identity()
        val signers = checkNotNull(installed.signers)
        assertTrue(signers.isNotEmpty())
        assertTrue(signers.all { it.matches(Regex("[0-9a-f]{64}")) })
        // A real signed APK generated with an unrelated private key, not a fabricated PackageInfo.
        val file = File(context.cacheDir, "update-signer-fixture.apk")
        try {
            instrumentation.context.assets.open("wrong-signer.apk").use { input ->
                file.outputStream().use { input.copyTo(it) }
            }
            val other = checkNotNull(manager.getPackageArchiveInfo(file.path, PackageManager.GET_SIGNING_CERTIFICATES)).identity()
            assertEquals(installed.packageName, other.packageName)
            assertTrue(other.versionCode > installed.versionCode)
            assertFalse(other.signers.isNullOrEmpty())
            assertNotEquals(signers, other.signers)
            assertEquals(DIFFERENT_APK_KEY, apkPreflight(installed, other))
            // Even a matching advertised signer cannot conceal the archive's actual different certificate.
            val advertised = other.copy(signers = signers)
            assertNull(apkPreflight(installed, advertised))
            assertEquals(DIFFERENT_APK_KEY, apkDecision(installed, advertised, other))

            val own = checkNotNull(manager.getPackageArchiveInfo(context.applicationInfo.sourceDir,
                PackageManager.GET_SIGNING_CERTIFICATES)).identity()
            assertEquals(signers, own.signers)
            assertNull(apkPreflight(installed, own.copy(versionCode = installed.versionCode + 1)))
            // Model the immediately preceding version to isolate signer acceptance using unmodified archive info.
            assertNull(apkDecision(installed.copy(versionCode = own.versionCode - 1), own, own))
        } finally {
            check(file.delete() || !file.exists())
        }
    }
}
