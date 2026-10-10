package app.porchlight.shared.trust

interface SnapshotWindow {
    fun disableRecentsScreenshots()
    fun setSecure(secure: Boolean)
}
enum class SnapshotLifecycle { CREATED, RESUMED, PAUSED, STOPPED }

/** R78: every app-owned activity window is secure except while RESUMED, on every API. */
class TaskSnapshotPolicy(private val apiLevel: Int) {
    val disableRecentsScreenshots: Boolean get() = apiLevel >= 33
    fun secureWindow(resumed: Boolean): Boolean = !resumed
    fun apply(event: SnapshotLifecycle, window: SnapshotWindow) {
        if (event == SnapshotLifecycle.CREATED && disableRecentsScreenshots) window.disableRecentsScreenshots()
        window.setSecure(secureWindow(event == SnapshotLifecycle.RESUMED))
    }
}
