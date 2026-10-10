package app.porchlight.shared.settings

import android.content.Context
import androidx.datastore.core.handlers.ReplaceFileCorruptionHandler
import androidx.datastore.preferences.core.*
import app.porchlight.shared.*
import java.io.File
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.collect
import kotlin.time.Clock

/** Application-scoped DataStore: activity recreation must not open a second instance on the same file. */
class AndroidAppServices private constructor(context: Context) : AppServices {
    override val settings: SettingsStore = DataStoreSettingsStore(PreferenceDataStoreFactory.create(
        corruptionHandler = ReplaceFileCorruptionHandler { emptyPreferences() },
        produceFile = { File(context.filesDir, "datastore/device.preferences_pb") },
    ))
    override val recoverNotifications: () -> Unit = {
        context.startActivity(android.content.Intent(android.provider.Settings.ACTION_APP_NOTIFICATION_SETTINGS)
            .putExtra(android.provider.Settings.EXTRA_APP_PACKAGE, context.packageName).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
    }
    override val recoverExactAlarms: () -> Unit = {
        if (android.os.Build.VERSION.SDK_INT >= 31) context.startActivity(android.content.Intent(android.provider.Settings.ACTION_REQUEST_SCHEDULE_EXACT_ALARM,
            android.net.Uri.parse("package:${context.packageName}")).addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK))
    }
    override val runtimeNotice = MutableStateFlow<String?>(null)
    override val pendingOpen = MutableStateFlow<app.porchlight.shared.navigation.AppRoute?>(null)
    override suspend fun stepUp(password: String, action: String): String = trust.controller.reauth(password, action)
    override val clock = AppClock { Clock.System.now() }
    override val screenSession = app.porchlight.shared.screens.ScreenSession()
    val trust = app.porchlight.shared.trust.AppTrustHolder(
        { app.porchlight.shared.trust.AndroidDeviceKeyStore(context.applicationContext) },
        { keys -> app.porchlight.shared.trust.TrustController(keys,
            app.porchlight.shared.transport.OkHttpTrustFactory(keys,
                app.porchlight.shared.transport.ClientInfo(BuildInfo.VERSION)),
            "android", { clock.now() }, pinRequired = true) })
    private val dataScope = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + kotlinx.coroutines.Dispatchers.IO)
    override val updates by lazy {
        app.porchlight.shared.update.UpdateController(app.porchlight.shared.update.TrustedUpdateSource(trust.controller),
            app.porchlight.shared.update.platformUpdater(context), settings, dataScope, { clock.now() })
            .also { it.bind(trust.controller, data.live) }
    }
    private val foreground = MutableStateFlow(false)
    private val streamEnabled = MutableStateFlow(false)
    fun setForeground(value: Boolean) { foreground.value = value }
    init {
        dataScope.launch {
            kotlinx.coroutines.flow.combine(settings.settings, foreground) { device, active -> device.stayConnected || active }
                .collect { streamEnabled.value = it }
        }
    }
    override val data by lazy {
        val schema = app.porchlight.shared.db.PorchlightDb.Schema
        // The read cache is disposable: a schema version mismatch either way recreates it (as on desktop) instead of
        // migrating, or crashing at start on a downgrade (SQLite's default onDowngrade throws).
        val callback = object : app.cash.sqldelight.driver.android.AndroidSqliteDriver.Callback(schema) {
            override fun onUpgrade(db: androidx.sqlite.db.SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = recreate(db)
            override fun onDowngrade(db: androidx.sqlite.db.SupportSQLiteDatabase, oldVersion: Int, newVersion: Int) = recreate(db)
            private fun recreate(db: androidx.sqlite.db.SupportSQLiteDatabase) {
                val tables = db.query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name <> 'android_metadata'").use { cursor ->
                    buildList { while (cursor.moveToNext()) add(cursor.getString(0)) }
                }
                tables.forEach { db.execSQL("DROP TABLE IF EXISTS \"$it\"") }
                onCreate(db)
            }
        }
        val driver = app.cash.sqldelight.driver.android.AndroidSqliteDriver(schema, context.applicationContext, "read-cache.db", callback = callback)
        app.porchlight.shared.data.HouseholdData(
            app.porchlight.shared.data.ReadCache(driver) { clock.now() },
            app.porchlight.shared.data.GeneratedCall.trusted(trust.controller), { clock.now() },
            attachmentStore = app.porchlight.shared.data.FileAttachmentStore(java.io.File(context.applicationContext.cacheDir, "attachments")),
        ).also { it.bindTrust(trust.controller, dataScope, screenSession::bind, onIdentityEnded = { kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) {
            app.porchlight.shared.live.AndroidAlarms(context).clear()
            app.porchlight.shared.live.AndroidDelivery(context).clear()
            context.getSharedPreferences("os-delivery", Context.MODE_PRIVATE).edit().remove("owner").remove("identity").remove("action-failure").commit()
        } }, streamEnabled = streamEnabled, refreshWhenInactive = false, resumeFromCursor = true); app.porchlight.shared.live.bindAndroidLive(context.applicationContext, it, dataScope) }
    }
    override val settingsWrites by lazy { app.porchlight.shared.screens.settings.HouseholdSettingsController(data) }
    private val dark = MutableStateFlow(isDark(context.resources.configuration))
    private fun isDark(configuration: android.content.res.Configuration): Boolean =
        configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK == android.content.res.Configuration.UI_MODE_NIGHT_YES
    init {
        context.registerComponentCallbacks(object : android.content.ComponentCallbacks {
            override fun onConfigurationChanged(configuration: android.content.res.Configuration) { dark.value = isDark(configuration) }
            @Deprecated("No longer called from API 34")
            override fun onLowMemory() = Unit
        })
    }
    override val platform = object : PlatformInfo {
        override val kind = PlatformKind.Android
        override val systemDark = dark
    }
    companion object {
        @Volatile private var instance: AndroidAppServices? = null
        fun get(context: Context): AndroidAppServices = instance ?: synchronized(this) {
            instance ?: AndroidAppServices(context.applicationContext).also { instance = it }
        }
    }
}
