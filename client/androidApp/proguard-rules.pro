# Serialization's consumer rules retain serializer/Companion discovery for generated API
# models and typed AppRoute navigation (all use unnamed companions). Preserve the metadata
# used by that discovery; do not keep every model or serializer alive.
-keepattributes Signature,InnerClasses,EnclosingMethod,RuntimeVisibleAnnotations,AnnotationDefault

# AAPT keeps manifest entry points (Application, activities, services, receivers, providers).
# WorkManager's consumer rules keep ListenableWorker names and (Context, WorkerParameters)
# constructors, including CoroutineWorker subclasses. No duplicate application-wide keep.
# SQLDelight queries and AndroidSqliteDriver are direct calls; no reflective query keep.
# OkHttp's Android public-suffix asset is packaged unchanged (checked by audit-apk.py);
# its consumer rules cover optional platforms. No additional platform suppressions here.
# Compose Res assets are packaged by the resource plugin; audit-apk.py checks fonts/drawables.
# BC is used only through direct ASN.1/PKCS#10 calls in CertificateTools. No BC provider is
# registered, so provider service classes must NOT be kept wholesale. Exercise CSR in smoke.
