package app.porchlight.shared.screens.settings

import app.porchlight.shared.api.models.*
import androidx.compose.ui.graphics.toArgb
import app.porchlight.shared.data.*
import app.porchlight.shared.transport.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement

val memberColorChoices = listOf("Yellow", "Orange", "Aqua", "Blue", "Purple", "Green", "Red")
    .zip(app.porchlight.shared.theme.PorchlightTokens.memberColors.map { color ->
        "#" + (color.toArgb() and 0xFFFFFF).toString(16).padStart(6, '0').uppercase()
    }).toMap()
fun sectionLabel(wire: String): String = when (wire) {
    "overdue" -> "Overdue"; "today" -> "Today"; "this_week" -> "This week"; "needs_owner" -> "Needs an owner"; else -> "Pinned list"
}
/** Disabled sections retain their slot, and toggling does not accidentally reset server-only fields. */
data class TodayDraft(private val original: TodaySettings, val order: List<String> = original.todaySections.map { it.value } + TodaySection.entries.map { it.value }.filter { it !in original.todaySections.map { s -> s.value } },
    val enabled: Set<String> = original.todaySections.map { it.value }.toSet(), val scope: String = original.scope.value,
    val days: String = original.thisWeekDays.toString()) {
    fun toggle(section: String) = copy(enabled = if (section in enabled) enabled - section else enabled + section)
    fun move(section: String, direction: Int): TodayDraft {
        val i = order.indexOf(section); val to = i + direction
        if (i < 0 || to !in order.indices) return this
        return copy(order = order.toMutableList().apply { add(to, removeAt(i)) })
    }
    fun withScope(value: String) = copy(scope = value)
    fun withDays(value: String) = copy(days = value)
    fun validatedValue(): TodaySettings? {
        if ("pinned_list" in enabled && original.pinnedListId == null) return null
        val count = days.toIntOrNull()?.takeIf { it in 1..14 } ?: return null
        val filter = SettingsScope.entries.find { it.value == scope } ?: return null
        return original.copy(todaySections = order.filter { it in enabled }.map { name -> TodaySection.entries.first { it.value == name } }, scope = filter, thisWeekDays = count)
    }
    fun value(): TodaySettings = requireNotNull(validatedValue())
}
enum class SettingsWriteStatus { Idle, Saving, Saved, Conflict, Ambiguous, Error }
data class SettingsWriteState(val status: SettingsWriteStatus = SettingsWriteStatus.Idle, val current: JsonElement? = null,
    val yours: JsonElement? = null, val refreshPending: Boolean = false, val fieldErrors: Map<String, String> = emptyMap())

/** Application-owned, so a PIN relock cannot lose an ambiguous operation's original body/version/key. */
class HouseholdSettingsController(private val newKey: () -> String,
    private val put: suspend (Int, TodaySettings, String) -> WriteResult<SettingsRead>,
    private val patch: suspend (String, Int, MemberPatch, String) -> WriteResult<MemberRead>, private val epoch: () -> Long = { 0L }) {
    constructor(data: HouseholdData) : this(data.writes.newKey, { v, b, k -> data.household.putSettings(v, b, k) }, { id, v, b, k -> data.household.patchMember(id, v, b, k) }, { data.cache.sessionEpoch() })
    private var identity = epoch()
    fun checkIdentity() {
        if (identity != epoch()) { identity = epoch(); pending = null; mutable.value = SettingsWriteState() }
    }
    private sealed interface Operation {
        val key: String
        data class Today(val version: Int, val body: TodaySettings, override val key: String) : Operation
        data class Color(val id: String, val version: Int, val body: MemberPatch, override val key: String) : Operation
    }
    private var pending: Operation? = null
    private val mutable = MutableStateFlow(SettingsWriteState())
    val state = mutable.asStateFlow()
    val unresolved get() = pending != null
    suspend fun saveToday(base: SettingsRead, body: TodaySettings) {
        checkIdentity()
        if (pending != null) return
        pending = Operation.Today(base.version, body, newKey()); execute()
    }
    suspend fun saveColor(base: MemberRead, color: String) {
        checkIdentity()
        if (pending != null) return
        pending = Operation.Color(base.id, base.version, MemberPatch(color = color), newKey()); execute()
    }
    suspend fun retry() {
        if (state.value.status in listOf(SettingsWriteStatus.Ambiguous, SettingsWriteStatus.Error)) execute()
    }
    /** Explicit reapply, never automatic replacement of the user's expected version. */
    suspend fun reapply() {
        if (state.value.status != SettingsWriteStatus.Conflict) return
        val current = state.value.current ?: return
        pending = when (val op = pending) {
            is Operation.Today -> runCatching { WireJson.decodeFromJsonElement<SettingsRead>(current) }.getOrNull()?.let { op.copy(version = it.version, body = op.body.copy(pinnedListId = it.settings.pinnedListId), key = newKey()) }
            is Operation.Color -> runCatching { WireJson.decodeFromJsonElement<MemberRead>(current) }.getOrNull()?.takeIf { it.id == op.id }?.let { op.copy(version = it.version, key = newKey()) }
            null -> null
        } ?: return
        execute()
    }
    /** An ambiguous request cannot be discarded and replaced: replay it after unlocking first. */
    fun discard() {
        if (state.value.status == SettingsWriteStatus.Conflict || state.value.status == SettingsWriteStatus.Error) {
            pending = null; mutable.value = SettingsWriteState()
        }
    }
    private suspend fun execute() {
        checkIdentity()
        val operationEpoch = identity
        val op = pending ?: return
        if (state.value.status == SettingsWriteStatus.Saving) return
        val yours = when (op) { is Operation.Today -> WireJson.encodeToJsonElement(TodaySettings.serializer(), op.body); is Operation.Color -> WireJson.encodeToJsonElement(MemberPatch.serializer(), op.body) }
        mutable.value = SettingsWriteState(SettingsWriteStatus.Saving, yours = yours)
        try {
            val result = when (op) { is Operation.Today -> put(op.version, op.body, op.key); is Operation.Color -> patch(op.id, op.version, op.body, op.key) }
            if (operationEpoch != epoch()) { checkIdentity(); return }
            mutable.value = when (result) {
                is WriteResult.Success -> { pending = null; SettingsWriteState(SettingsWriteStatus.Saved, refreshPending = result.refreshPending) }
                is WriteResult.VersionConflict -> SettingsWriteState(SettingsWriteStatus.Conflict, current = result.current, yours = yours)
            }
        } catch (e: CancellationException) {
            if (operationEpoch == epoch()) mutable.value = SettingsWriteState(SettingsWriteStatus.Ambiguous, yours = yours) else checkIdentity()
            throw e
        } catch (e: HouseholdLockedAfterReply) {
            // Repository used our key; don't generate a replacement while the gate was closed.
            if (operationEpoch != epoch()) { checkIdentity(); return }
            mutable.value = SettingsWriteState(SettingsWriteStatus.Ambiguous, yours = yours)
        } catch (e: ApiProblem) {
            if (operationEpoch != epoch()) { checkIdentity(); return }
            val errors = (e.extras["errors"] as? kotlinx.serialization.json.JsonArray).orEmpty().mapNotNull { value ->
                val obj = value as? kotlinx.serialization.json.JsonObject ?: return@mapNotNull null
                val field = (obj["field"] as? kotlinx.serialization.json.JsonPrimitive)?.content ?: return@mapNotNull null
                val message = (obj["message"] as? kotlinx.serialization.json.JsonPrimitive)?.content ?: return@mapNotNull null
                field to message
            }.toMap().ifEmpty { if (e.status in listOf(400, 403, 404, 422)) mapOf("settings" to (e.detail ?: e.title)) else emptyMap() }
            mutable.value = SettingsWriteState(SettingsWriteStatus.Error, yours = yours, fieldErrors = errors)
        } catch (_: Exception) {
            if (operationEpoch != epoch()) { checkIdentity(); return }
            mutable.value = SettingsWriteState(SettingsWriteStatus.Ambiguous, yours = yours)
        }
    }
}
