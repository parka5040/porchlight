package app.porchlight.shared.transport

import okhttp3.*
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.ResponseBody.Companion.toResponseBody
import java.lang.reflect.Proxy
import java.util.concurrent.TimeUnit
import kotlin.test.*

class AuditBlClient2bUploadEngineTest {
    @Test fun productionUploadInterceptorLimitsOnlyWriteIdle() {
        var write = 600_000L
        val request = Request.Builder().url("https://lan/v1/items/item/attachments")
            .post("fixture".toRequestBody("multipart/form-data".toMediaType())).build()
        // Every unrequested Chain operation fails: the interceptor must not alter read/connect budgets.
        val chain = Proxy.newProxyInstance(Interceptor.Chain::class.java.classLoader, arrayOf(Interceptor.Chain::class.java)) { proxy, method, args ->
            when (method.name) {
                "request" -> request
                "withWriteTimeout" -> { write = (args!![1] as TimeUnit).toMillis((args[0] as Int).toLong()); proxy }
                "proceed" -> {
                    assertEquals(30_000L, write)
                    Response.Builder().request(request).protocol(Protocol.HTTP_1_1).code(200).message("OK").body("{}".toResponseBody()).build()
                }
                else -> error("Unexpected Chain operation: ${method.name}")
            }
        } as Interceptor.Chain
        UploadWriteIdleTimeout.intercept(chain).close()
    }
}
