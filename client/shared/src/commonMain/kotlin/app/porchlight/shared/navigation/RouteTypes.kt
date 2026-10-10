package app.porchlight.shared.navigation

import androidx.navigation.NavType
import androidx.savedstate.SavedState
import androidx.savedstate.read
import androidx.savedstate.write
import kotlin.reflect.typeOf

// Navigation Compose's common NavTypes do not infer enum classes (unlike Android's reflection helper).
private object CalendarViewType : NavType<CalendarView>(isNullableAllowed = false) {
    override fun put(bundle: SavedState, key: String, value: CalendarView) { bundle.write { putString(key, value.name) } }
    override fun get(bundle: SavedState, key: String): CalendarView = bundle.read { CalendarView.valueOf(getString(key)) }
    override fun parseValue(value: String): CalendarView = CalendarView.valueOf(value)
    override fun serializeAsValue(value: CalendarView): String = value.name
}
private object SettingsSectionType : NavType<SettingsSection?>(isNullableAllowed = true) {
    override fun put(bundle: SavedState, key: String, value: SettingsSection?) { bundle.write { putString(key, value?.name ?: "null") } }
    override fun get(bundle: SavedState, key: String): SettingsSection? = bundle.read { parseValue(getString(key)) }
    override fun parseValue(value: String): SettingsSection? = if (value == "null") null else SettingsSection.valueOf(value)
    override fun serializeAsValue(value: SettingsSection?): String = value?.name ?: "null"
}
val routeTypes = mapOf(typeOf<CalendarView>() to CalendarViewType, typeOf<SettingsSection?>() to SettingsSectionType)
