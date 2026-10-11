package app.porchlight.shared.settings

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.*

const val PRIVATE_STORAGE_NOTICE = "Private device storage is unavailable. Porchlight will retry; existing device state has been retained."
enum class PrivateStorageEntry { Activity, Receiver, Worker, Live, Settings }

/** One application-owned boundary for private metadata, never the disposable cache recovery policy. */
class PrivateStorageBoundary {
    private val failures = MutableStateFlow<Set<PrivateStorageEntry>>(emptySet())
    val notices: Flow<String?> = failures.map { if (it.isEmpty()) null else PRIVATE_STORAGE_NOTICE }.distinctUntilChanged()
    val notice: String? get() = if (failures.value.isEmpty()) null else PRIVATE_STORAGE_NOTICE
    suspend fun <T> attempt(entry: PrivateStorageEntry, work: suspend () -> T): Result<T> = try {
        Result.success(work()).also { failures.update { it - entry } }
    } catch (e: CancellationException) { throw e }
    catch (e: Exception) { failures.update { it + entry }; Result.failure(e) }
    suspend fun <T> retry(entry: PrivateStorageEntry, work: suspend () -> T): T {
        var wait = 1_000L
        while (true) {
            val result = attempt(entry, work)
            if (result.isSuccess) return result.getOrThrow()
            delay(wait); wait = (wait * 2).coerceAtMost(30_000)
        }
    }
    fun <T> observe(entry: PrivateStorageEntry, source: Flow<T>): Flow<T> = source.retryWhen { failure, attempt ->
        if (failure is CancellationException || failure !is Exception) throw failure
        failures.update { it + entry }
        delay(1_000L shl attempt.coerceAtMost(4).toInt())
        true
    }.onEach { failures.update { it - entry } }
}

/** Settings reads AND writes use the same boundary as alarms, delivery, receivers and workers. */
class RecoveringSettingsStore(private val delegate: SettingsStore, private val boundary: PrivateStorageBoundary) : SettingsStore {
    override val settings = boundary.observe(PrivateStorageEntry.Settings, delegate.settings)
    override suspend fun saveRoute(route: app.porchlight.shared.navigation.AppRoute) = boundary.retry(PrivateStorageEntry.Settings) { delegate.saveRoute(route) }
    override suspend fun setTheme(mode: ThemeMode) = boundary.retry(PrivateStorageEntry.Settings) { delegate.setTheme(mode) }
    override suspend fun setStartScreen(screen: StartScreen) = boundary.retry(PrivateStorageEntry.Settings) { delegate.setStartScreen(screen) }
    override suspend fun setStayConnected(enabled: Boolean) = boundary.retry(PrivateStorageEntry.Settings) { delegate.setStayConnected(enabled) }
    override suspend fun dismissUpdate(version: String) = boundary.retry(PrivateStorageEntry.Settings) { delegate.dismissUpdate(version) }
    override suspend fun setLimitedReminders(limited: Boolean) = boundary.retry(PrivateStorageEntry.Settings) { delegate.setLimitedReminders(limited) }
}
