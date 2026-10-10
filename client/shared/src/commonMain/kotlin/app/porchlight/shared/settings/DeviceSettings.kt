package app.porchlight.shared.settings

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.*
import app.porchlight.shared.navigation.*
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.map

// Per-device only. Member color/Today/notifications belong to server-backed settings in later workstreams.
enum class ThemeMode { Evening, Daylight, System }
enum class StartScreen { LastScreen, Today, Board, Calendar, Lists, Inbox }
data class DeviceSettings(
    val currentRoute: AppRoute = AppRoute.Today,
    val themeMode: ThemeMode = ThemeMode.System,
    val startScreen: StartScreen = StartScreen.LastScreen,
    val limitedReminders: Boolean = false,
    val stayConnected: Boolean = false,
    val dismissedUpdate: String? = null,
) {
    fun launchRoute(): AppRoute = when (startScreen) {
        StartScreen.LastScreen -> currentRoute
        StartScreen.Today -> AppRoute.Today
        StartScreen.Board -> AppRoute.Board()
        StartScreen.Calendar -> AppRoute.Calendar()
        StartScreen.Lists -> AppRoute.Lists()
        StartScreen.Inbox -> AppRoute.Inbox
    }
}
interface SettingsStore {
    val settings: Flow<DeviceSettings>
    suspend fun saveRoute(route: AppRoute)
    suspend fun setTheme(mode: ThemeMode)
    suspend fun setStartScreen(screen: StartScreen)
    suspend fun setStayConnected(enabled: Boolean) {}
    suspend fun dismissUpdate(version: String) {}
    suspend fun setLimitedReminders(limited: Boolean)
}

/** Read failures never silently remember permission consent or crash the onboarding composition. */
fun Flow<DeviceSettings>.reminderSettingsOrDefault(onFailure: (Exception) -> Unit): Flow<DeviceSettings> =
    catch { failure ->
        if (failure !is Exception) throw failure
        onFailure(failure)
        emit(DeviceSettings())
    }

class DataStoreSettingsStore(private val dataStore: DataStore<Preferences>) : SettingsStore {
    private val routeKey = stringPreferencesKey("route")
    private val themeKey = stringPreferencesKey("theme")
    private val startKey = stringPreferencesKey("start")
    private val connectedKey = booleanPreferencesKey("stay_connected")
    private val limitedKey = booleanPreferencesKey("limited_reminders")
    private val dismissedKey = stringPreferencesKey("dismissed_update")
    override suspend fun dismissUpdate(version: String) { dataStore.edit { it[dismissedKey] = version } }
    override val settings: Flow<DeviceSettings> = dataStore.data
        .map { prefs -> DeviceSettings(RouteCodec.decode(prefs[routeKey]),
            enumValues<ThemeMode>().firstOrNull { it.name == prefs[themeKey] } ?: ThemeMode.System,
            enumValues<StartScreen>().firstOrNull { it.name == prefs[startKey] } ?: StartScreen.LastScreen, prefs[limitedKey] ?: false, prefs[connectedKey] ?: false, prefs[dismissedKey]) }
    override suspend fun saveRoute(route: AppRoute) { dataStore.edit { it[routeKey] = RouteCodec.encode(route) } }
    override suspend fun setTheme(mode: ThemeMode) { dataStore.edit { it[themeKey] = mode.name } }
    override suspend fun setStartScreen(screen: StartScreen) { dataStore.edit { it[startKey] = screen.name } }
    override suspend fun setStayConnected(enabled: Boolean) { dataStore.edit { it[connectedKey] = enabled } }
    override suspend fun setLimitedReminders(limited: Boolean) { dataStore.edit { it[limitedKey] = limited } }
}

/** C3 supplies the visibility/deletion check. Called before exposing restored content. */
suspend fun restoreRoute(settings: DeviceSettings, available: suspend (AppRoute) -> Boolean): AppRoute =
    settings.launchRoute().let { if (available(it)) it else AppRoute.Today }
