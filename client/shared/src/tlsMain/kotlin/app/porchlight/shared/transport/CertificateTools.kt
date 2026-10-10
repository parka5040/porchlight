package app.porchlight.shared.transport

import app.porchlight.shared.trust.DeviceTrust
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.security.*
import java.security.cert.*
import java.util.Base64
import org.bouncycastle.asn1.x500.X500Name
import org.bouncycastle.asn1.x509.AlgorithmIdentifier
import org.bouncycastle.asn1.x9.X9ObjectIdentifiers
import org.bouncycastle.operator.ContentSigner
import org.bouncycastle.pkcs.jcajce.JcaPKCS10CertificationRequestBuilder

fun certificate(pem: String) = CertificateFactory.getInstance("X.509").generateCertificate(ByteArrayInputStream(pem.toByteArray())) as X509Certificate
fun fingerprint(cert: X509Certificate) = MessageDigest.getInstance("SHA-256").digest(cert.encoded).joinToString("") { "%02x".format(it) }
fun pem(cert: X509Certificate): String = "-----BEGIN CERTIFICATE-----\n" + Base64.getMimeEncoder(64, byteArrayOf(10)).encodeToString(cert.encoded) + "\n-----END CERTIFICATE-----\n"
fun csr(public: PublicKey, private: PrivateKey): String {
    val output = ByteArrayOutputStream()
    val signer = object : ContentSigner {
        override fun getAlgorithmIdentifier() = AlgorithmIdentifier(X9ObjectIdentifiers.ecdsa_with_SHA256)
        override fun getOutputStream() = output
        override fun getSignature(): ByteArray = Signature.getInstance("SHA256withECDSA").run { initSign(private); update(output.toByteArray()); sign() }
    }
    val request = JcaPKCS10CertificationRequestBuilder(X500Name("CN=Porchlight device"), public).build(signer)
    return "-----BEGIN CERTIFICATE REQUEST-----\n" + Base64.getMimeEncoder(64, byteArrayOf(10)).encodeToString(request.encoded) + "\n-----END CERTIFICATE REQUEST-----\n"
}
fun validateTrust(trust: DeviceTrust, public: PublicKey, pin: String?) {
    val root = certificate(trust.caCertificate)
    require(root.basicConstraints >= 0 && (pin == null || fingerprint(root) == pin)) { "Household CA pin mismatch" }
    val leaf = certificate(trust.certificate)
    require(leaf.publicKey.encoded.contentEquals(public.encoded)) { "Certificate does not match local key" }
    leaf.verify(root.publicKey)
    require(leaf.issuerX500Principal == root.subjectX500Principal) { "Certificate issuer mismatch" }
    require(leaf.notAfter.toInstant().toString() == trust.certificateExpiresAt) { "Certificate expiry mismatch" }
    // Full client path validation (including key usage/EKU) runs in platform save with the current clock.
    require(leaf.extendedKeyUsage?.contains("1.3.6.1.5.5.7.3.2") == true) { "Not a device certificate" }
}

