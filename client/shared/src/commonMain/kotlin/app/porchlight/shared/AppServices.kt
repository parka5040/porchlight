package app.porchlight.shared

import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.settings.*
import kotlinx.coroutines.flow.StateFlow
import kotlin.time.Instant

fun interface AppClock { fun now(): Instant }
enum class PlatformKind { Android, Desktop }
interface PlatformInfo {
    val kind: PlatformKind
    val systemDark: StateFlow<Boolean>
}
interface AppServices {
    val updates: app.porchlight.shared.update.UpdateController? get() = null
    val recoverNotifications: (() -> Unit)? get() = null
    val recoverExactAlarms: (() -> Unit)? get() = null
    val desktopLoginConflict: StateFlow<String?>? get() = null
    val dismissDesktopLoginConflict: (() -> Unit)? get() = null
    val desktopLoginHelp: String? get() = null
    val runtimeNotice: StateFlow<String?>? get() = null
    /** Counts system wakes (desktop: logind PrepareForSleep(false)); null where the platform has no such signal. */
    val systemResumes: StateFlow<Long>? get() = null
    val pendingOpen: kotlinx.coroutines.flow.MutableStateFlow<AppRoute?>? get() = null
    // o2b: desktop-only status and C4 connection hook; Android/UI fixtures omit this.
    val backups: app.porchlight.shared.backups.DesktopBackups? get() = null
    val settings: SettingsStore
    val clock: AppClock
    val platform: PlatformInfo
    // c2b: production services own these for the application lifetime; UI-only test fixtures may omit them.
    val data: app.porchlight.shared.data.HouseholdData? get() = null
    suspend fun stepUp(password: String, action: String): String = throw UnsupportedOperationException("Sign in before sensitive actions")
    val screenSession: app.porchlight.shared.screens.ScreenSession? get() = null
    // Application-owned replay state survives a PIN relock. Production implementations create it lazily.
    val settingsWrites: app.porchlight.shared.screens.settings.HouseholdSettingsController? get() = null
    suspend fun routeAvailable(route: AppRoute): Boolean {
        val household = data ?: return app.porchlight.shared.navigation.localRouteAvailable(route, app.porchlight.shared.data.CacheView())
        return try {
            household.cache.initialize() // disk only; valid before the trust/PIN gates pass
            app.porchlight.shared.navigation.localRouteAvailable(route, household.cache.view.value)
        } catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (_: Exception) { false } // Broken resume cache falls back to Today, not a startup retry loop.
    }
}

