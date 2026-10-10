package app.porchlight.shared.navigation

import kotlinx.datetime.LocalDate
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

@Serializable enum class CalendarView { Day, Week, Month, Agenda }
@Serializable enum class SettingsSection { About, Appearance, Today, StartScreen, Notifications, Account, CalendarData, Availability, Planning, Export, Devices }

@Serializable
sealed interface AppRoute {
    @Serializable @SerialName("today") data object Today : AppRoute
    @Serializable @SerialName("board") data class Board(val filter: String? = null) : AppRoute
    @Serializable @SerialName("calendar") data class Calendar(val view: CalendarView = CalendarView.Week, val date: String? = null,
        val memberId: String? = null, val overlay: String? = null) : AppRoute
    @Serializable @SerialName("lists") data class Lists(val listId: String? = null, val occurrenceId: String? = null) : AppRoute
    @Serializable @SerialName("inbox") data object Inbox : AppRoute
    @Serializable @SerialName("settings") data class Settings(val section: SettingsSection? = null) : AppRoute
    @Serializable @SerialName("item") data class Item(val id: String, val occurrenceId: String? = null) : AppRoute
    @Serializable @SerialName("new_item") data object NewItem : AppRoute
}

object RouteCodec {
    private val json = Json { ignoreUnknownKeys = true }
    fun encode(route: AppRoute): String = json.encodeToString<AppRoute>(route)
    fun decode(saved: String?): AppRoute = runCatching {
        require(saved != null && saved.length <= 8192)
        json.decodeFromString<AppRoute>(saved).also { route ->
            when (route) {
                is AppRoute.Calendar -> route.date?.let { LocalDate.parse(it) }
                is AppRoute.Item -> require(route.id.isNotBlank() && route.id.length <= 256)
                is AppRoute.Lists -> { require(route.listId == null || route.listId.isNotBlank() && route.listId.length <= 256); require(route.occurrenceId == null || route.occurrenceId.isNotBlank() && route.occurrenceId.length <= 256) }
                is AppRoute.Board -> require(route.filter == null || route.filter.length <= 256)
                else -> Unit
            }
        }
    }.getOrDefault(AppRoute.Today)
}

val mainDestinations: List<Pair<String, AppRoute>> = listOf(
    "Today" to AppRoute.Today, "Board" to AppRoute.Board(), "Calendar" to AppRoute.Calendar(),
    "Lists" to AppRoute.Lists(), "Inbox" to AppRoute.Inbox,
)
fun AppRoute.title(): String = when (this) {
    AppRoute.Today -> "Today"
    is AppRoute.Board -> "Board"
    is AppRoute.Calendar -> "Calendar"
    is AppRoute.Lists -> "Lists"
    AppRoute.Inbox -> "Inbox"
    is AppRoute.Settings -> "Settings"
    is AppRoute.Item -> "Item"
    AppRoute.NewItem -> "New item"
}
