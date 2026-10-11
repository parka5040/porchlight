package app.porchlight.shared.trust

import app.porchlight.shared.transport.*
import java.io.IOException
import java.net.SocketException
import javax.net.ssl.SSLException
import kotlin.test.*

class AuditBlDeployTlsTest {
    @Test fun resetAndBrokenPipeCannotProveRevocationOrSafeReplay() {
        for (failure in listOf(SocketException("Connection reset"), SocketException("Broken pipe"), IOException("certificate_revoked"))) {
            assertFalse(isCertificateRevoked(failure))
            assertFalse(isDefiniteConnectFailure(failure))
            assertSame(failure, classifyTlsFailure(failure))
        }
        val alert = IOException("TLS wrapper", SSLException("Received fatal alert: certificate_revoked"))
        assertIs<DeviceCertificateRevoked>(classifyTlsFailure(alert))
    }
}
