package app.porchlight.shared.transport

import java.security.KeyStore
import java.security.cert.*
import javax.net.ssl.*

/** Pin selects one CA in the presented chain; then PKIX checks signatures, validity, constraints and server usage. */
class HouseholdTrustManager(private val root: X509Certificate? = null, private val pin: String? = null) : X509TrustManager {
    init { require((root == null) != (pin == null)); root?.let { require(it.basicConstraints >= 0) } }
    private fun anchor(chain: Array<X509Certificate>): X509Certificate = root ?: chain.firstOrNull { fingerprint(it) == pin && it.basicConstraints >= 0 }
        ?: throw CertificateException("Pinned household CA absent from chain")
    override fun getAcceptedIssuers(): Array<X509Certificate> = root?.let { arrayOf(it) } ?: emptyArray()
    override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) { throw CertificateException("Server-only trust manager") }
    override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
        val selected = anchor(chain)
        val store = KeyStore.getInstance(KeyStore.getDefaultType()).apply { load(null); setCertificateEntry("household", selected) }
        val delegate = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm()).apply { init(store) }.trustManagers.filterIsInstance<X509TrustManager>().single()
        delegate.checkServerTrusted(chain.filter { it != selected }.toTypedArray(), authType)
    }
}
