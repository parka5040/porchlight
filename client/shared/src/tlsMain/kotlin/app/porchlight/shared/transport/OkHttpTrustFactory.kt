package app.porchlight.shared.transport

import app.porchlight.shared.trust.*
import io.ktor.client.HttpClient
import io.ktor.client.engine.okhttp.OkHttp
import io.ktor.client.plugins.HttpTimeout
import okhttp3.ConnectionSpec
import okhttp3.TlsVersion
import java.security.SecureRandom
import java.util.Base64
import javax.net.ssl.*

class OkHttpTrustFactory(private val keys: JavaDeviceKeyStore, private val info: ClientInfo) : TrustTransportFactory {
    private val policy = HeaderPolicy(info, { systemIanaZone() }, {
        Base64.getUrlEncoder().withoutPadding().encodeToString(ByteArray(16).also { SecureRandom().nextBytes(it) })
    })
    private fun transport(manager: HouseholdTrustManager, endpoints: List<String>, clientKeys: Array<KeyManager>?): ApiTransport {
        val context = SSLContext.getInstance("TLSv1.3").apply { init(clientKeys, arrayOf(manager), SecureRandom()) }
        val client = HttpClient(OkHttp) {
            expectSuccess = false
            followRedirects = false
            install(HttpTimeout) { connectTimeoutMillis = 10_000; socketTimeoutMillis = 300_000; requestTimeoutMillis = null }
            engine {
                config {
                    sslSocketFactory(context.socketFactory, manager)
                    connectionSpecs(listOf(ConnectionSpec.Builder(ConnectionSpec.MODERN_TLS).tlsVersions(TlsVersion.TLS_1_3).build()))
                    followRedirects(false); followSslRedirects(false); retryOnConnectionFailure(false)
                    readTimeout(300, java.util.concurrent.TimeUnit.SECONDS)
                    writeTimeout(30, java.util.concurrent.TimeUnit.SECONDS) // c5: align slow upload idle deadline
                    // Hostname verifier remains OkHttp's default, including SAN checks for IP addresses.
                }
            }
        }
        return KtorApiTransport(client, endpoints, policy, ::isDefiniteConnectFailure, ::classifyTlsFailure)
    }
    override fun enrollment(uri: EnrollUri) = transport(HouseholdTrustManager(pin = uri.caHex), listOf(EnrollmentTarget.parse(uri.host).baseUrl), null)
    override suspend fun authenticated(trust: DeviceTrust) = transport(HouseholdTrustManager(root = certificate(trust.caCertificate)), trust.endpoints, keys.keyManagers(trust))
}
