package app.porchlight.shared.trust

import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.UserNotAuthenticatedException
import java.security.ProviderException
import java.security.UnrecoverableKeyException
import javax.crypto.AEADBadTagException
import kotlin.test.*

class AuditBlClientAttemptReadabilityTest {
    @Test fun providerStateErrorsAreNotEvidenceOfPermanentKeyLoss() {
        assertFalse(isDefinitivelyUnreadableAttempt(IllegalStateException("provider restarting")))
        assertFalse(isDefinitivelyUnreadableAttempt(IllegalArgumentException("provider configuration")))
        assertFalse(isDefinitivelyUnreadableAttempt(ProviderException(UserNotAuthenticatedException())))
    }
    @Test fun permanentKeyLossIsRecognizedThroughProviderCauses() {
        for (cause in listOf(KeyPermanentlyInvalidatedException(), UnrecoverableKeyException(), AEADBadTagException())) {
            assertTrue(isDefinitivelyUnreadableAttempt(ProviderException(cause)), cause.javaClass.simpleName)
        }
    }
}
