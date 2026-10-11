package app.porchlight.android

import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.graphics.Typeface
import android.os.Bundle
import androidx.lifecycle.lifecycleScope
import androidx.activity.ComponentActivity
import androidx.work.ListenableWorker
import androidx.work.WorkerParameters
import app.porchlight.shared.api.models.MemberRead
import app.porchlight.shared.navigation.AppRoute
import app.porchlight.shared.navigation.RouteCodec
import app.porchlight.shared.settings.AndroidAppServices
import app.porchlight.shared.transport.csr
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.json.Json
import kotlinx.serialization.serializer
import java.security.KeyPairGenerator
import java.security.spec.ECGenParameterSpec

/** Minified-only entry point: probe calls are optimized WITH the app, not linked to pre-R8 APIs.
 * No keep rules, trust bypass, network, or production manifest entry. Report through framework Intent. */
class AuditApkProbeActivity : ComponentActivity() {
    @OptIn(ExperimentalSerializationApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        lifecycleScope.launch {
            var stage = "serialization"
            try {
                withContext(Dispatchers.IO) {
                    val model = MemberRead(false, "member", "household", "Sam Rivera", "user", "#FABD2F", "SR", "UTC", "manual", 1)
                    // KClass lookup exercises consumer rules for generated companion discovery.
                    val serializer = serializer(MemberRead::class, emptyList(), false)
                    check(model == Json.decodeFromString(serializer, Json.encodeToString(serializer, model)))
                    stage = "routes"
                    val route: AppRoute = AppRoute.Item("item", "occurrence")
                    check(route == RouteCodec.decode(RouteCodec.encode(route)))
                    val routeSerializer = serializer(AppRoute.Item::class, emptyList(), false)
                    check(route == Json.decodeFromString(routeSerializer, Json.encodeToString(routeSerializer, route)))

                    stage = "cache"
                    AndroidAppServices.get(applicationContext).data.cache.initialize()
                    val cache = getDatabasePath("read-cache.db")
                    check(cache.isFile)
                    SQLiteDatabase.openDatabase(cache.path, null, SQLiteDatabase.OPEN_READONLY).use { db ->
                        db.rawQuery("SELECT COUNT(*) FROM CacheMember", null).use { cursor -> check(cursor.moveToFirst()) }
                    }
                    stage = "workers"
                    for (name in listOf("HouseholdSyncWorker", "NotificationActionWorker")) {
                        val worker = Class.forName("app.porchlight.shared.live.$name", true, classLoader)
                        check(ListenableWorker::class.java.isAssignableFrom(worker))
                        worker.getConstructor(Context::class.java, WorkerParameters::class.java)
                    }
                    stage = "csr"
                    val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()
                    val request = csr(pair.public, pair.private)
                    check(request.startsWith("-----BEGIN CERTIFICATE REQUEST-----\n"))
                    check(request.endsWith("-----END CERTIFICATE REQUEST-----\n"))
                    stage = "fonts"
                    for (font in listOf("literata_semibold", "source_sans_3_regular", "source_sans_3_semibold", "source_sans_3_bold")) {
                        checkNotNull(Typeface.createFromAsset(assets,
                            "composeResources/app.porchlight.shared.resources/font/$font.ttf"))
                    }
                }
                intent.putExtra("smoke-result", "passed")
            } catch (failure: Exception) {
                // No values or key material in diagnostics.
                intent.putExtra("smoke-result", "$stage: ${failure.javaClass.simpleName}")
            }
        }
    }
}
