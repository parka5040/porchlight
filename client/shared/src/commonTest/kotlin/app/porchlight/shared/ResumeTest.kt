package app.porchlight.shared

import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.*
import app.porchlight.shared.navigation.*
import app.porchlight.shared.settings.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import kotlin.test.*

private class FakeDataStore : DataStore<Preferences> {
    override val data = MutableStateFlow<Preferences>(emptyPreferences())
    override suspend fun updateData(transform: suspend (Preferences) -> Preferences): Preferences =
        transform(data.value).also { data.value = it }
}

class ResumeTest {
    @Test fun routesRoundTripWithTheirContext() {
        val routes = listOf(AppRoute.Today, AppRoute.Board("mine"), AppRoute.Calendar(CalendarView.Week, "2026-10-05"),
            AppRoute.Lists("groceries"), AppRoute.Lists(), AppRoute.Inbox, AppRoute.Settings(SettingsSection.Appearance),
            AppRoute.Settings(), AppRoute.Item("item-123"))
        routes.forEach { assertEquals(it, RouteCodec.decode(RouteCodec.encode(it))) }
    }
    @Test fun invalidRoutesFallBackToToday() {
        listOf(null, "", "not json", "{\"type\":\"future-screen\"}",
            "{\"type\":\"calendar\",\"view\":\"Week\",\"date\":\"bad\"}",
            "{\"type\":\"item\",\"id\":\"\"}").forEach { assertEquals(AppRoute.Today, RouteCodec.decode(it)) }
    }
    @Test fun persistsEveryNavigationAndPerDeviceOptions() = runTest {
        val data = FakeDataStore()
        val store = DataStoreSettingsStore(data)
        assertEquals(AppRoute.Today, store.settings.first().launchRoute())
        store.saveRoute(AppRoute.Calendar(CalendarView.Day, "2026-12-01"))
        store.setTheme(ThemeMode.Daylight)
        val reloaded = DataStoreSettingsStore(data).settings.first()
        assertEquals(AppRoute.Calendar(CalendarView.Day, "2026-12-01"), reloaded.launchRoute())
        assertEquals(ThemeMode.Daylight, reloaded.themeMode)
        store.setStartScreen(StartScreen.Board)
        assertEquals(AppRoute.Board(), store.settings.first().launchRoute())
        store.setStartScreen(StartScreen.LastScreen)
        assertEquals(reloaded.currentRoute, store.settings.first().launchRoute())
    }
    @Test fun corruptPreferencesAndUnavailableItemsRecover() = runTest {
        val data = FakeDataStore()
        data.updateData { mutablePreferencesOf(stringPreferencesKey("route") to "unknown",
            stringPreferencesKey("theme") to "future", stringPreferencesKey("start") to "missing") }
        val store = DataStoreSettingsStore(data)
        assertEquals(DeviceSettings(), store.settings.first())
        store.saveRoute(AppRoute.Item("deleted"))
        assertEquals(AppRoute.Today, restoreRoute(store.settings.first()) { false })
        assertEquals(AppRoute.Item("deleted"), restoreRoute(store.settings.first()) { true })
    }
}
