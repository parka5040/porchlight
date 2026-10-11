package app.porchlight.shared.transport

import okhttp3.Interceptor
import okhttp3.Response
import java.util.concurrent.TimeUnit

internal object UploadWriteIdleTimeout : Interceptor {
    override fun intercept(chain: Interceptor.Chain): Response {
        val request = chain.request()
        val timed = if (request.body?.contentType()?.type == "multipart") chain.withWriteTimeout(30, TimeUnit.SECONDS) else chain
        return timed.proceed(request)
    }
}
