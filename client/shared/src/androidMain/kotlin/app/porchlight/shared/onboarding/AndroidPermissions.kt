package app.porchlight.shared.onboarding

import android.Manifest
import android.app.Activity
import android.app.AlarmManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.porchlight.shared.trust.TrustButton
import app.porchlight.shared.trust.TrustForm
import app.porchlight.shared.settings.SettingsStore
import app.porchlight.shared.settings.reminderSettingsOrDefault
import app.porchlight.shared.reportDiagnostic
import kotlinx.coroutines.launch

class TrustNotificationPermissionActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1) else finish()
    }
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults); finish()
    }
}

// DESIGN-PENDING: permission onboarding + notification/exact-alarm/battery denial recovery
@Composable
fun AndroidPermissionOnboarding(settings: SettingsStore, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    var revision by remember { mutableIntStateOf(0) }
    var error by remember { mutableStateOf(false) }
    var readRetry by remember { mutableIntStateOf(0) }
    var readFailed by remember { mutableStateOf(false) }
    val preferences = remember(settings, readRetry) {
        settings.settings.reminderSettingsOrDefault {
            reportDiagnostic("Reading reminder preferences", it)
            readFailed = true
        }
    }
    val stored by preferences.collectAsState(initial = null)
    val scope = rememberCoroutineScope()
    var saving by remember { mutableStateOf(false) }
    var requested by remember { mutableStateOf(false) }
    DisposableEffect(owner) {
        val observer = LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) revision++ }
        owner.lifecycle.addObserver(observer); onDispose { owner.lifecycle.removeObserver(observer) }
    }
    val notifications = remember(revision) { Build.VERSION.SDK_INT < 33 || ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED }
    val exact = remember(revision) {
        if (Build.VERSION.SDK_INT >= 31) exactAlarmsGranted(Build.VERSION.SDK_INT,
            context.getSystemService(AlarmManager::class.java).canScheduleExactAlarms())
        else true
    }
    val battery = remember(revision) { context.getSystemService(PowerManager::class.java).isIgnoringBatteryOptimizations(context.packageName) }
    fun open(intent: Intent) { try { context.startActivity(intent); error = false } catch (_: Exception) { error = true } }
    val packageUri = Uri.parse("package:${context.packageName}")
    if (readFailed) {
        TrustForm("Reminder preferences unavailable") {
            Text("Device settings could not be read. Your limited-reminder choice has not been assumed.")
            TrustButton("Try reading preferences again") { readFailed = false; readRetry++ }
        }
        return
    }
    if (stored == null) { Text("Checking reminder preferences…"); return }
    if (stored!!.limitedReminders || (notifications && exact && battery)) { content(); return }
    TrustForm("Keep household reminders working") {
        Text("Porchlight needs notifications, exact alarms and a battery exemption for timely household reminders. If you deny a request, use the settings links below to recover.")
        if (error) Text("Android could not open this settings page. Open Porchlight app settings manually, then retry.")
        if (!notifications) {
            Text(if (requested) "Notifications were not granted. Enable Porchlight notifications in Android settings." else "Allow notifications for household reminders and changes.")
            TrustButton("Allow notifications") { requested = true; open(Intent(context, TrustNotificationPermissionActivity::class.java)) }
            TrustButton("Notification settings") { open(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)) }
        }
        if (!exact) {
            Text("Exact alarms are unavailable. Enable alarms and reminders for Porchlight.")
            TrustButton("Alarms and reminders settings") {
                if (Build.VERSION.SDK_INT >= 31 && needsExactAlarmSpecialPermission(Build.VERSION.SDK_INT))
                    open(Intent(Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM, packageUri))
                else open(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri))
            }
        }
        if (!battery) {
            Text("Battery optimization can delay updates. Exempt Porchlight to keep the household stream available.")
            TrustButton("Allow background reminders") { open(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, packageUri)) }
            TrustButton("Battery optimization settings") { open(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
        }
        // OEM restrictions/settings failures must not permanently block enrollment.
        TrustButton("Continue with limited reminders", enabled = !saving, onClick = {
            saving = true
            scope.launch {
                try { settings.setLimitedReminders(true) }
                catch (e: kotlinx.coroutines.CancellationException) { throw e }
                catch (_: Exception) { error = true }
                finally { saving = false }
            }
        })
        TrustButton("Check permissions again") { revision++ }
        TrustButton("Open Porchlight app settings") { open(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, packageUri)) }
    }
}
