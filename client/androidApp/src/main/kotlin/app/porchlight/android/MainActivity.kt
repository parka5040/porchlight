package app.porchlight.android

import android.os.Bundle
import android.content.Intent
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import androidx.lifecycle.Lifecycle
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.distinctUntilChanged
import androidx.activity.ComponentActivity
import androidx.activity.enableEdgeToEdge
import androidx.activity.compose.setContent
import app.porchlight.shared.PorchlightAndroidRoot
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.android.onboarding.AndroidQrScanner

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val services = AndroidAppServices.get(applicationContext)
        // Keys, controller and in-flight actions outlive this Activity. Credential unlock does not.
        intent.getStringExtra("item_id")?.let { services.pendingOpen.value = app.porchlight.shared.navigation.AppRoute.Item(it, intent.getStringExtra("occurrence_id")) }
        lifecycleScope.launch { repeatOnLifecycle(Lifecycle.State.STARTED) {
            services.setForeground(true)
            try { services.privateStorage.retry(app.porchlight.shared.settings.PrivateStorageEntry.Activity) { services.settings.settings.map { it.stayConnected }.distinctUntilChanged().collect { enabled ->
                services.privateStorage.retry(app.porchlight.shared.settings.PrivateStorageEntry.Activity) {
                app.porchlight.shared.live.updateForegroundConnection(this@MainActivity, enabled)
                kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { app.porchlight.shared.live.recoverLivePermissions(applicationContext, services) }
                }
            } } } finally { services.setForeground(false) }
        } }
        val controller = services.trust.controller
        setContent {
            PorchlightAndroidRoot(services, controller, scanner = { AndroidQrScanner(it) })
        }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        intent.getStringExtra("item_id")?.let { AndroidAppServices.get(applicationContext).pendingOpen.value = app.porchlight.shared.navigation.AppRoute.Item(it, intent.getStringExtra("occurrence_id")) }
    }
}
