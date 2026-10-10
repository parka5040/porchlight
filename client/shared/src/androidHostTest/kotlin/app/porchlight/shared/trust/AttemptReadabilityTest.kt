package app.porchlight.shared.trust

import javax.crypto.AEADBadTagException
import kotlinx.serialization.SerializationException
import java.security.KeyStoreException
import java.security.ProviderException
import kotlin.test.Test
import kotlin.test.assertFalse
import kotlin.test.assertTrue

class AttemptReadabilityTest {
    @Test fun definitiveConditionsDropPendingMetadata() {
        assertTrue(isDefinitivelyUnreadableAttempt(LocalTrustUnavailable()))
        assertTrue(isDefinitivelyUnreadableAttempt(AEADBadTagException("tag")))
        assertTrue(isDefinitivelyUnreadableAttempt(SerializationException("malformed")))
        assertTrue(isDefinitivelyUnreadableAttempt(IllegalStateException("foreign alias")))
    }
    @Test fun transientKeystoreErrorsKeepStoredAttempt() {
        assertFalse(isDefinitivelyUnreadableAttempt(KeyStoreException("keystore busy")))
        assertFalse(isDefinitivelyUnreadableAttempt(ProviderException("provider died")))
        assertFalse(isDefinitivelyUnreadableAttempt(java.io.IOException("io")))
    }
}
