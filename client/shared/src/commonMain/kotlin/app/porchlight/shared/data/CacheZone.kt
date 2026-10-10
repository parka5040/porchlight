package app.porchlight.shared.data

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.datetime.TimeZone

/** Shell banners do not observe unrelated cursor/projection changes. */
fun Flow<CacheView>.memberZoneChanges(fallback: () -> String = { TimeZone.currentSystemDefault().id }): Flow<String> =
    map { view -> view.members.firstOrNull { it.id == view.settings?.memberId }?.tzid ?: fallback() }
        .distinctUntilChanged()
