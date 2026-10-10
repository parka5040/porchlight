package app.porchlight.shared.trust

import kotlin.test.*

class SecretServiceActivationTest {
    private class Bus(private val activatable: Set<String>) : SecretServiceActivationBus {
        val starts = mutableListOf<String>()
        override fun start(name: String): Boolean { starts += name; return name in activatable }
    }
    @Test fun standardActivationRetriesExactlyOnce() {
        val bus = Bus(setOf("org.freedesktop.secrets"))
        var tries = 0
        withSecretServiceActivation(bus) { if (++tries == 1) throw WalletUnavailable() }
        assertEquals(2, tries)
        assertEquals(listOf("org.freedesktop.secrets"), bus.starts)
    }
    @Test fun kdeFallbackOnlyWhenStandardNameIsNotActivatable() {
        val bus = Bus(setOf("org.kde.secretservicecompat"))
        var tries = 0
        withSecretServiceActivation(bus) { if (++tries == 1) throw WalletUnavailable() }
        assertEquals(2, tries)
        assertEquals(listOf("org.freedesktop.secrets", "org.kde.secretservicecompat"), bus.starts)
    }
    @Test fun unavailableWalletRetriesOnceAndAbsentServicesDoNotLoop() {
        val bus = Bus(setOf("org.freedesktop.secrets"))
        var tries = 0
        assertFailsWith<WalletUnavailable> { withSecretServiceActivation(bus) { tries++; throw WalletUnavailable() } }
        assertEquals(2, tries)
        assertEquals(listOf("org.freedesktop.secrets"), bus.starts)
        val absent = Bus(emptySet())
        assertFailsWith<WalletUnavailable> { withSecretServiceActivation(absent) { throw WalletUnavailable() } }
        assertEquals(listOf("org.freedesktop.secrets", "org.kde.secretservicecompat"), absent.starts)
    }
    @Test fun throwingBusKeepsCauseAndNeverStartsKdeOrRetriesWallet() {
        val starts = mutableListOf<String>()
        val cause = IllegalStateException("Spawn.ChildExited")
        val bus = SecretServiceActivationBus { starts += it; throw cause }
        var tries = 0
        val failure = assertFailsWith<WalletUnavailable> {
            withSecretServiceActivation(bus) { tries++; throw WalletUnavailable() }
        }
        assertSame(cause, failure.cause)
        assertEquals(1, tries)
        assertEquals(listOf("org.freedesktop.secrets"), starts)
    }
    @Test fun kdeWaitsForStandardNameBeforeSingleRetry() {
        var polls = 0
        val bus = object : SecretServiceActivationBus {
            override fun start(name: String) = name == "org.kde.secretservicecompat"
            override fun hasOwner(name: String): Boolean {
                assertEquals("org.freedesktop.secrets", name)
                return ++polls == 3
            }
        }
        var tries = 0
        assertEquals("ready", withSecretServiceActivation(bus, wait = {}) {
            if (++tries == 1) throw WalletUnavailable()
            assertEquals(3, polls)
            "ready"
        })
        assertEquals(2, tries)
    }
    @Test fun kdeOwnerPollingIsBounded() {
        var polls = 0
        val bus = object : SecretServiceActivationBus {
            override fun start(name: String) = name == "org.kde.secretservicecompat"
            override fun hasOwner(name: String): Boolean { polls++; return false }
        }
        var tries = 0
        assertFailsWith<WalletUnavailable> { withSecretServiceActivation(bus, wait = {}) { tries++; throw WalletUnavailable() } }
        assertEquals(KDE_STANDARD_NAME_POLLS, polls)
        assertTrue(KDE_STANDARD_NAME_POLLS * SECRET_SERVICE_POLL_MILLIS >= 3_000, "a starting ksecretd gets a few seconds (fix round 1, I1)")
        assertEquals(2, tries)
    }
    @Test fun workingWalletDoesNotStartServices() {
        val bus = Bus(emptySet())
        assertEquals("ok", withSecretServiceActivation(bus) { "ok" })
        assertTrue(bus.starts.isEmpty())
    }
    // deskkey: a dismissed unlock prompt is the user's answer, not a missing provider — never activate and re-prompt.
    @Test fun dismissedUnlockIsNotRetried() {
        val bus = Bus(setOf("org.freedesktop.secrets"))
        var tries = 0
        assertFailsWith<WalletUnlockDismissed> { withSecretServiceActivation(bus) { tries++; throw WalletUnlockDismissed() } }
        assertEquals(1, tries)
        assertTrue(bus.starts.isEmpty())
    }
    // deskkey: enrollment-time provider probe ("no owner of org.freedesktop.secrets after one activation attempt").
    private class Probe(var owner: Boolean, private val activatable: Set<String> = emptySet(), private val ownerAfterKde: Boolean = true,
                        private val kdeOwner: Boolean = false) : SecretServiceActivationBus {
        val starts = mutableListOf<String>()
        override fun hasOwner(name: String): Boolean = when (name) {
            "org.freedesktop.secrets" -> owner
            "org.kde.secretservicecompat" -> kdeOwner
            else -> fail("unexpected name $name")
        }
        override fun start(name: String): Boolean {
            starts += name
            val ok = name in activatable
            if (ok && (name == "org.freedesktop.secrets" || ownerAfterKde)) owner = true
            return ok
        }
    }
    @Test fun runningProviderIsFoundWithoutActivation() {
        val bus = Probe(owner = true)
        assertTrue(secretServiceProviderExists(bus) {})
        assertTrue(bus.starts.isEmpty())
    }
    @Test fun providerProbeActivatesExactlyOnce() {
        val standard = Probe(owner = false, activatable = setOf("org.freedesktop.secrets"))
        assertTrue(secretServiceProviderExists(standard) {})
        assertEquals(listOf("org.freedesktop.secrets"), standard.starts)
        val kde = Probe(owner = false, activatable = setOf("org.kde.secretservicecompat"))
        assertTrue(secretServiceProviderExists(kde) {})
        assertEquals(listOf("org.freedesktop.secrets", "org.kde.secretservicecompat"), kde.starts)
        val absent = Probe(owner = false)
        assertFalse(secretServiceProviderExists(absent) {})
        assertEquals(listOf("org.freedesktop.secrets", "org.kde.secretservicecompat"), absent.starts, "one attempt, no loop")
        // Fix round 1 (I1): a successful activation means a provider exists even if it never claims the standard name;
        // it is then the unusable-provider case (WalletLocked + the R85 choice), never a silent file backend.
        val kdeWithoutStandardName = Probe(owner = false, activatable = setOf("org.kde.secretservicecompat"), ownerAfterKde = false)
        assertTrue(secretServiceProviderExists(kdeWithoutStandardName) {}, "activated provider that is late or crashed")
        assertEquals(listOf("org.freedesktop.secrets", "org.kde.secretservicecompat"), kdeWithoutStandardName.starts)
        val standardWithoutOwner = object : SecretServiceActivationBus {
            override fun hasOwner(name: String) = false
            override fun start(name: String) = name == "org.freedesktop.secrets"
        }
        assertTrue(secretServiceProviderExists(standardWithoutOwner) {})
        val kdeAlreadyRunning = Probe(owner = false, kdeOwner = true)
        assertTrue(secretServiceProviderExists(kdeAlreadyRunning) {})
        assertTrue(kdeAlreadyRunning.starts.isEmpty())
    }
    @Test fun noSessionBusMeansNoProvider() {
        val bus = object : SecretServiceActivationBus {
            override fun hasOwner(name: String): Boolean = throw SessionBusUnavailable(IllegalStateException("no bus"))
            override fun start(name: String): Boolean = throw SessionBusUnavailable(IllegalStateException("no bus"))
        }
        assertFalse(secretServiceProviderExists(bus) {})
    }
    // Fix round 1 (M5): only a genuinely absent session bus selects the file automatically.
    @Test fun sessionBusIsAbsentOnlyWithoutAddressAndSocket() {
        fun address(env: Map<String, String>, sockets: Set<String> = emptySet()) =
            sessionBusAddress({ env[it] }) { it.toString() in sockets }
        assertNull(address(emptyMap()))
        assertNull(address(mapOf("XDG_RUNTIME_DIR" to "/run/user/1000")), "no address and no socket")
        assertEquals("unix:path=/run/user/1000/bus", address(mapOf("XDG_RUNTIME_DIR" to "/run/user/1000"), setOf("/run/user/1000/bus")))
        assertNull(address(mapOf("DBUS_SESSION_BUS_ADDRESS" to "unix:path=/run/user/1000/bus")), "configured socket missing")
        assertEquals("unix:path=/run/user/1000/bus", address(mapOf("DBUS_SESSION_BUS_ADDRESS" to "unix:path=/run/user/1000/bus"), setOf("/run/user/1000/bus")))
        assertEquals("unix:abstract=/tmp/dbus-x,guid=1", address(mapOf("DBUS_SESSION_BUS_ADDRESS" to "unix:abstract=/tmp/dbus-x,guid=1")), "cannot be checked: present")
        assertEquals("tcp:host=localhost,port=1", address(mapOf("DBUS_SESSION_BUS_ADDRESS" to "tcp:host=localhost,port=1")))
        assertEquals("unix:path=/a%2db", address(mapOf("DBUS_SESSION_BUS_ADDRESS" to "unix:path=/a%2db")), "escaped path: not guessed")
    }
    @Test fun connectionFailureIsUnusableNotAbsent() {
        val refused = DbusSecretServiceBus { throw java.io.IOException("authentication rejected") }
        val failure = assertFailsWith<WalletUnavailable> { secretServiceProviderExists(refused) {} }
        assertFalse(failure is SessionBusUnavailable)
        assertFalse(secretServiceProviderExists(DbusSecretServiceBus { throw SessionBusUnavailable(null) }) {})
    }
    @Test fun registeredProviderThatFailsToStartIsUnavailableNotAbsent() {
        val cause = IllegalStateException("Spawn.ChildExited")
        val bus = object : SecretServiceActivationBus {
            override fun hasOwner(name: String) = false
            override fun start(name: String): Boolean = throw cause
        }
        val failure = assertFailsWith<WalletUnavailable> { secretServiceProviderExists(bus) {} }
        assertFalse(failure is SessionBusUnavailable)
        assertSame(cause, failure.cause)
    }
}
