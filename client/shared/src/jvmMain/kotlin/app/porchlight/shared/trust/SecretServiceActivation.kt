package app.porchlight.shared.trust

import java.nio.file.Files
import java.nio.file.Path

internal const val SECRET_SERVICE_NAME = "org.freedesktop.secrets"
/** KDE's ksecretd activation name; starting it is only a harmless nudge for niri/KDE sessions, never a requirement. */
internal const val KDE_SECRET_SERVICE_NAME = "org.kde.secretservicecompat"
/** After starting KDE's compat service, wait up to about four seconds for it to claim org.freedesktop.secrets. */
internal const val KDE_STANDARD_NAME_POLLS = 40
internal const val SECRET_SERVICE_POLL_MILLIS = 100L
internal val secretServicePollWait: () -> Unit = { Thread.sleep(SECRET_SERVICE_POLL_MILLIS) }

internal fun interface SecretServiceActivationBus {
    /** false only when the bus reports that this service name is not activatable. */
    fun start(name: String): Boolean
    fun hasOwner(name: String): Boolean = true
}

/** This session genuinely has no session bus (no address, socket absent): no Secret Service provider can exist. */
internal class SessionBusUnavailable(cause: Throwable?) : WalletUnavailable(cause)

/**
 * The user dismissed the provider's unlock prompt, let it time out, or it failed once shown; or this attempt already
 * used its one prompt. Final for the attempt: never re-prompted or retried.
 */
internal class WalletUnlockDismissed(cause: Throwable? = null) : WalletUnavailable(cause)

/**
 * One activation attempt: the standard name, then KDE's compatibility name only when the standard one is not
 * activatable. Returns whether any start succeeded, i.e. a provider is installed (fix round 1, I1), even if it has not
 * claimed the standard name yet. Errors other than "not activatable" mean a registered provider failed to start:
 * [WalletUnavailable].
 */
internal fun activateSecretService(bus: SecretServiceActivationBus, wait: () -> Unit): Boolean {
    try {
        if (bus.start(SECRET_SERVICE_NAME)) return true
        if (!bus.start(KDE_SECRET_SERVICE_NAME)) return false
        // KDE activation may return before the compatibility service acquires the standard name.
        for (attempt in 0 until KDE_STANDARD_NAME_POLLS) {
            if (bus.hasOwner(SECRET_SERVICE_NAME)) break
            if (attempt < KDE_STANDARD_NAME_POLLS - 1) wait()
        }
        return true
    } catch (e: WalletUnavailable) { throw e }
    catch (e: Exception) { throw WalletUnavailable(e) }
}

/** One retry after activation, never a disk/plaintext fallback. A dismissed unlock prompt is not retried. */
internal fun <T> withSecretServiceActivation(
    bus: SecretServiceActivationBus,
    wait: () -> Unit = secretServicePollWait,
    action: () -> T,
): T {
    try { return action() } catch (e: WalletUnavailable) {
        if (e is WalletUnlockDismissed) throw e // the user's answer, not a missing provider
    }
    activateSecretService(bus, wait)
    return action()
}

/**
 * Enrollment-time probe (owner decision 2026-10-07, fix round 1 I1/M5). A provider exists when either name has an
 * owner or any activation succeeds, even if the standard name never appears (then it is the unusable-provider case:
 * WalletLocked plus the explicit R85 choice). Only "nothing activatable" (both names not activatable, no owner) or a
 * genuinely absent session bus means no provider, which selects the owner-only file automatically. A registered
 * provider that fails to start, or a bus that exists but cannot be used, is [WalletUnavailable].
 */
internal fun secretServiceProviderExists(bus: SecretServiceActivationBus, wait: () -> Unit = secretServicePollWait): Boolean = try {
    bus.hasOwner(SECRET_SERVICE_NAME) || bus.hasOwner(KDE_SECRET_SERVICE_NAME) ||
        activateSecretService(bus, wait) || bus.hasOwner(SECRET_SERVICE_NAME)
} catch (_: SessionBusUnavailable) { false }
catch (e: WalletUnavailable) { throw e }
catch (e: Exception) { throw WalletUnavailable(e) }

/**
 * The session bus address, or null only when this session genuinely has none (fix round 1, M5): no
 * DBUS_SESSION_BUS_ADDRESS and no `$XDG_RUNTIME_DIR/bus` socket, or a configured address whose every entry is a
 * `unix:path=` socket that does not exist. Anything that cannot be checked (abstract sockets, TCP, escaped paths)
 * counts as present, so its connection errors are "unusable", never "absent".
 */
internal fun sessionBusAddress(env: (String) -> String?, exists: (Path) -> Boolean = { Files.exists(it) }): String? {
    val configured = env("DBUS_SESSION_BUS_ADDRESS")?.trim().orEmpty()
    if (configured.isNotEmpty()) {
        val sockets = configured.split(';').filter { it.isNotBlank() }.map(::unixSocketPath)
        return if (sockets.isNotEmpty() && sockets.all { it != null && !exists(it) }) null else configured
    }
    val runtime = env("XDG_RUNTIME_DIR")?.takeIf { it.startsWith('/') } ?: return null
    val socket = Path.of(runtime, "bus")
    return if (exists(socket)) "unix:path=$socket" else null
}

private fun unixSocketPath(address: String): Path? {
    if (!address.startsWith("unix:")) return null
    val path = address.removePrefix("unix:").split(',').firstOrNull { it.startsWith("path=") }?.removePrefix("path=")
    return path?.takeIf { it.startsWith('/') && '%' !in it }?.let { Path.of(it) }
}
