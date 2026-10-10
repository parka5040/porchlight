package app.porchlight.shared.data

import app.porchlight.shared.transport.ApiProblem

/** A failed read is not necessarily visibility loss: step-up and transient failures keep offline content. */
internal fun ApiProblem.deniesVisibility() = status == 404 || (status == 403 && type == "forbidden")

/** Shared repository read-failure boundary; eviction removes the parent, projections and collaboration children,
 * and awaits registered original/thumbnail/view-file removal. Keep this wrapper around comment page fetches. */
internal suspend fun ReadCache.evictOnVisibilityFailure(itemId: String, problem: ApiProblem) {
    if (problem.deniesVisibility()) {
        try { evictItem(itemId) }
        catch (e: kotlinx.coroutines.CancellationException) { throw e }
        catch (e: Exception) { problem.addSuppressed(e) } // Keep the original read failure and continue refetch batches.
    }
}
