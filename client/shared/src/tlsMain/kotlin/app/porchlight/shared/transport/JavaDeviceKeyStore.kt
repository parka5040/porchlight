package app.porchlight.shared.transport

import app.porchlight.shared.trust.*
import java.security.*
import javax.net.ssl.*

interface JavaDeviceKeyStore : DeviceKeyStore {
    fun privateKey(): PrivateKey
    fun publicKey(): PublicKey
    fun keyManagers(trust: DeviceTrust): Array<KeyManager> {
        // Android's private key is non-exportable: never copy it to a software KeyStore.
        val key = privateKey()
        val chain = arrayOf(certificate(trust.certificate), certificate(trust.caCertificate))
        val manager = object : X509ExtendedKeyManager() {
            override fun getClientAliases(keyType: String?, issuers: Array<java.security.Principal>?) = if (keyType == "EC") arrayOf("device") else null
            override fun chooseClientAlias(types: Array<String>?, issuers: Array<java.security.Principal>?, socket: java.net.Socket?) = if (types?.contains("EC") == true) "device" else null
            override fun chooseEngineClientAlias(types: Array<String>?, issuers: Array<java.security.Principal>?, engine: SSLEngine?) = if (types?.contains("EC") == true) "device" else null
            override fun getServerAliases(keyType: String?, issuers: Array<java.security.Principal>?) = null
            override fun chooseServerAlias(keyType: String?, issuers: Array<java.security.Principal>?, socket: java.net.Socket?) = null
            override fun getCertificateChain(alias: String?) = if (alias == "device") chain else null
            override fun getPrivateKey(alias: String?) = if (alias == "device") key else null
        }
        return arrayOf(manager)
    }
    override fun certificateExpiry(pem: String) = certificate(pem).notAfter.toInstant().toString()
}

