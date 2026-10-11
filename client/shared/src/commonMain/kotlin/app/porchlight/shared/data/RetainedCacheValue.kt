package app.porchlight.shared.data

/** A UI copy is usable only in the visibility generation that authorized it.
 * Resolve on every render (not just in a LaunchedEffect), so a revoked frame cannot be drawn. */
class RetainedCacheValue<T> internal constructor(private val value: T, private val revision: Long) {
    private var resources: Map<CacheResource, Any?>? = null
    private var identity: Any? = null
    fun bindResources(view: CacheView, dependencies: Set<CacheResource>): RetainedCacheValue<T> = apply {
        resources = dependencies.associateWith { view.resourceVisibility[it] }
        identity = view.identityVisibility
    }
    fun current(view: CacheView): T? = value.takeIf {
        resources?.let { bound -> identity === view.identityVisibility && bound.all { (resource, token) ->
            token === view.resourceVisibility[resource]
        } } ?: (view.visibilityRevision == revision)
    }
}

/** Local editors resolve their source against the current cache, never a previously captured row. */
fun <T : Any> CacheView.retainCurrent(resolve: CacheView.() -> T?): RetainedCacheValue<T>? =
    resolve()?.let { RetainedCacheValue(it, visibilityRevision) }

data class CacheResource(val kind: String, val id: String)

/** Drafts depend on their own resource tokens, not unrelated visibility changes. Empty dependencies are new drafts. */
fun <T : Any> CacheView.retainDraft(resources: Set<CacheResource>, value: T): RetainedCacheValue<T> =
    RetainedCacheValue(value, visibilityRevision).bindResources(this, resources)
