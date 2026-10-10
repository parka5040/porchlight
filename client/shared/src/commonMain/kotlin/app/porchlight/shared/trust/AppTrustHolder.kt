package app.porchlight.shared.trust

/** One holder per application, not per Activity. Lazy initialization is synchronized on JVM/Android. */
class AppTrustHolder<K : DeviceKeyStore>(keysFactory: () -> K, controllerFactory: (K) -> TrustController) {
    val keys: K by lazy(keysFactory)
    val controller: TrustController by lazy { controllerFactory(keys) }
}
