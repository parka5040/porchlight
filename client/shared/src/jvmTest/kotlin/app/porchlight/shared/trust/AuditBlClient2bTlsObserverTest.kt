package app.porchlight.shared.trust

import java.security.KeyStore
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.TrustManagerFactory
import kotlin.test.*

class AuditBlClient2bTlsObserverTest {
    @Test fun incompleteHttpRequestIsObservedBeforeHeadersOrBody() {
        val caKey = TestCertificates.key()
        val ca = TestCertificates.cert(caKey)
        val key = TestCertificates.key()
        val leaf = TestCertificates.cert(key, caKey, ca, ca = false)
        val roots = KeyStore.getInstance(KeyStore.getDefaultType()).apply { load(null); setCertificateEntry("fixture", ca) }
        val trust = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply { init(roots) }
        val ssl = SSLContext.getInstance("TLSv1.3").apply { init(null, trust.trustManagers, null) }
        LocalTlsServer(key, arrayOf(leaf, ca)).use { server ->
            ssl.socketFactory.createSocket("localhost", server.port).use { socket ->
                socket.outputStream.write('P'.code)
                socket.outputStream.flush()
                assertTrue(server.firstApplicationByte.await(5, TimeUnit.SECONDS), "First byte must be observed without waiting for a complete request")
                assertEquals(1, server.applicationRequests.get())
                assertTrue(server.requests.isEmpty())
            }
        }
    }
}
