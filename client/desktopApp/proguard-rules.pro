# Desktop release shrinking (createReleaseDistributable). Shrink only: obfuscation and optimization stay off
# (build.gradle.kts), so class/member names stay real. dbus-java derives D-Bus member names from Java names,
# kotlinx.serialization looks up generated serializers by name, and ServiceLoader files name implementations.
# The plugin's default rules already keep kotlin.**, Skiko and the coroutines service names.

-keepattributes Signature,InnerClasses,EnclosingMethod,*Annotation*,AnnotationDefault,Exceptions,MethodParameters,SourceFile,LineNumberTable

# Application code: serializers, D-Bus interfaces/exported objects/signals and Compose entry points.
-keep class app.porchlight.** { *; }

# Reflection-, JNI- or ServiceLoader-driven libraries are kept whole:
# dbus-java (+ native unix socket transport) marshals by reflection over generic signatures; sqlite-jdbc binds
# native code by name; protobuf-lite (DataStore) reads message fields reflectively.
-keep class org.freedesktop.** { *; }
-keep class org.sqlite.** { *; }
-keep class org.slf4j.** { *; }
-keep class androidx.datastore.** { *; }
# Network and serialization stack: failures here would surface only when enrolling or syncing.
-keep class io.ktor.** { *; }
-keep class okhttp3.** { *; }
-keep class okio.** { *; }
-keep class kotlinx.serialization.** { *; }
-keep class kotlinx.datetime.** { *; }
# Coroutines whole: shrinking dropped Job from JobSupport's interfaces as redundant (ChildJob/ParentJob extend it), but
# JobSupport.cancel() calls Job.cancel via invokespecial, which needs a direct superinterface -> VerifyError on load
# (audit-desktop.py scans every shrunk class for this); the packaged smoke's kotlinx-coroutines-test also links to
# coroutines internals; the library resolves its own internals through ServiceLoader and system properties.
-keep class kotlinx.coroutines.** { *; }

# BouncyCastle is used only through direct ASN.1/PKCS#10/X.509-builder calls (no BC provider is registered), as on
# Android where R8 shrinks it; it is left to shrinking here. Enrollment's CSR and the key store's certificate are
# exercised by the packaged smoke (deploy/tests/audit-desktop-smoke.py), whose certificate fixtures also need these
# few classes that production never references; without them the smoke cannot run against the shrunk jars.
-keep class org.bouncycastle.asn1.x509.ExtendedKeyUsage { *; }
-keep class org.bouncycastle.asn1.x509.GeneralName { *; }
-keep class org.bouncycastle.asn1.x509.GeneralNames { *; }
-keep class org.bouncycastle.asn1.x509.KeyPurposeId { *; }
-keep class org.bouncycastle.asn1.x509.KeyUsage { *; }
-keep class org.bouncycastle.operator.ContentVerifierProvider { *; }
-keep class org.bouncycastle.operator.jcajce.JcaContentVerifierProviderBuilder { *; }

# Optional platform integrations referenced but not shipped.
-dontwarn org.conscrypt.**
-dontwarn org.openjsse.**
-dontwarn org.bouncycastle.jsse.**
-dontwarn org.graalvm.**
-dontwarn com.oracle.svm.**
-dontwarn android.**
-dontwarn dalvik.**
-dontwarn javax.annotation.**
-dontwarn edu.umd.cs.findbugs.annotations.**
-dontwarn org.jetbrains.annotations.**
-dontwarn kotlin.annotations.jvm.**
