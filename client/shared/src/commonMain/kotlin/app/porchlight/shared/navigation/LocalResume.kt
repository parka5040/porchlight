package app.porchlight.shared.navigation

import app.porchlight.shared.data.CacheView

/** Gate-safe: consults only locally persisted visible projections, never HTTP or trust credentials. */
fun localRouteAvailable(route: AppRoute, cache: CacheView): Boolean = when (route) {
    is AppRoute.Item -> {
        val authoritative = cache.items.find { it.id == route.id }
        if (authoritative != null) authoritative.deletedAt == null && "view" in authoritative.capabilities
        else (cache.board + cache.agenda).any { it.id == route.id && it.deletedAt == null && "view" in it.capabilities }
    }
    // C5: local-only resume uses the cached authoritative list capabilities.
    is AppRoute.Lists -> route.listId == null || cache.lists.any { it.id == route.listId && it.deletedAt == null && "view" in it.capabilities }
    is AppRoute.Board -> route.filter == null || route.filter in listOf("mine", "household") || cache.members.any { it.id == route.filter }
    else -> true
}
