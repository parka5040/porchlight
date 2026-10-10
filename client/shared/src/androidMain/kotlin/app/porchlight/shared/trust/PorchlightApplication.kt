package app.porchlight.shared.trust

import android.app.Activity
import android.app.Application
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import app.porchlight.shared.settings.AndroidAppServices

/** Central registration also covers translucent helpers, debug activities, and future activities.
 * minSdk 30: pre/post lifecycle callbacks (API 29) run outside the activity's own super calls. */
class PorchlightApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(TaskSnapshotCallbacks {
            AndroidAppServices.get(this).also { it.trust.controller.onForeground(); it.updates.onResume() }
        })
    }
}

class TaskSnapshotCallbacks(private val onForeground: () -> Unit = {}) : Application.ActivityLifecycleCallbacks {
    private val policy = TaskSnapshotPolicy(Build.VERSION.SDK_INT)
    private fun apply(activity: Activity, event: SnapshotLifecycle) {
        policy.apply(event, object : SnapshotWindow {
            override fun disableRecentsScreenshots() {
                if (Build.VERSION.SDK_INT >= 33) activity.setRecentsScreenshotEnabled(false)
            }
            override fun setSecure(secure: Boolean) {
                if (secure) activity.window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
                else activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
            }
        })
    }
    override fun onActivityPreCreated(activity: Activity, savedInstanceState: Bundle?) = apply(activity, SnapshotLifecycle.CREATED)
    override fun onActivityPostResumed(activity: Activity) {
        apply(activity, SnapshotLifecycle.RESUMED)
        onForeground()
    }
    override fun onActivityPrePaused(activity: Activity) = apply(activity, SnapshotLifecycle.PAUSED)
    override fun onActivityPreStopped(activity: Activity) = apply(activity, SnapshotLifecycle.STOPPED)
    override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
    override fun onActivityStarted(activity: Activity) = Unit
    override fun onActivityResumed(activity: Activity) = Unit
    override fun onActivityPaused(activity: Activity) = Unit
    override fun onActivityStopped(activity: Activity) = Unit
    override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
    override fun onActivityDestroyed(activity: Activity) = Unit
}
