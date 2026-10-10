package app.porchlight.android.onboarding

import android.Manifest
import android.content.pm.PackageManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.zxing.BinaryBitmap
import com.google.zxing.MultiFormatReader
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.common.HybridBinarizer
import app.porchlight.shared.trust.EnrollUri
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

// DESIGN-PENDING: enrollment QR scanner + camera permission recovery
@Composable
fun AndroidQrScanner(onUri: (String) -> Unit) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val currentResult by rememberUpdatedState(onUri)
    var permission by remember { mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) }
    val request = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { permission = it }
    val previewView = remember { PreviewView(context).apply { contentDescription = "Enrollment QR camera preview" } }
    var error by remember { mutableStateOf(false) }
    if (!permission || error) {
        AndroidView(factory = { LinearLayout(it).apply {
            orientation = LinearLayout.VERTICAL
            addView(TextView(it).apply { text = "Allow camera access to scan a household QR code. You can always paste the enrollment URI instead." })
            addView(Button(it).apply { text = "Allow camera"; minHeight = (48 * resources.displayMetrics.density).toInt(); setOnClickListener { error = false; request.launch(Manifest.permission.CAMERA) } })
            addView(Button(it).apply { text = "Camera permission settings"; minHeight = (48 * resources.displayMetrics.density).toInt(); setOnClickListener {
                context.startActivity(android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, android.net.Uri.parse("package:${context.packageName}")))
            } })
        } })
        return
    }
    AndroidView(factory = { previewView.apply {
        layoutParams = android.view.ViewGroup.LayoutParams(android.view.ViewGroup.LayoutParams.MATCH_PARENT, (240 * resources.displayMetrics.density).toInt())
    } })
    DisposableEffect(owner, previewView) {
        val executor = Executors.newSingleThreadExecutor()
        val main = ContextCompat.getMainExecutor(context)
        val providerFuture = ProcessCameraProvider.getInstance(context)
        val preview = Preview.Builder().build().also { it.surfaceProvider = previewView.surfaceProvider }
        val delivered = AtomicBoolean(false)
        val alive = AtomicBoolean(true)
        val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
        analysis.setAnalyzer(executor) { image ->
            try {
                if (!alive.get() || delivered.get()) return@setAnalyzer
                val plane = image.planes[0]
                val sourceBuffer = plane.buffer.duplicate()
                val bytes = ByteArray(image.width * image.height)
                val origin = sourceBuffer.position()
                for (y in 0 until image.height) for (x in 0 until image.width)
                    bytes[y * image.width + x] = sourceBuffer.get(origin + y * plane.rowStride + x * plane.pixelStride)
                val source = PlanarYUVLuminanceSource(bytes, image.width, image.height, 0, 0, image.width, image.height, false)
                val text = MultiFormatReader().decode(BinaryBitmap(HybridBinarizer(source))).text
                EnrollUri.parse(text)
                if (delivered.compareAndSet(false, true)) main.execute { if (alive.get()) currentResult(text) }
            } catch (_: Exception) { /* no QR or malformed enrollment QR; never echo QR contents */ }
            finally { image.close() }
        }
        providerFuture.addListener({
            if (alive.get()) try {
                providerFuture.get().bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
            } catch (_: Exception) { error = true }
        }, main)
        onDispose {
            alive.set(false); analysis.clearAnalyzer()
            if (providerFuture.isDone) try { providerFuture.get().unbind(preview, analysis) } catch (_: Exception) {}
            executor.shutdown()
        }
    }
}
