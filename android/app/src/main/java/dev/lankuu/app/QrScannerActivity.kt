package dev.lankuu.app

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.view.KeyEvent
import android.view.View
import android.widget.Toast
import com.google.zxing.BarcodeFormat
import com.google.zxing.client.android.Intents
import com.google.zxing.ResultPoint
import com.journeyapps.barcodescanner.BarcodeCallback
import com.journeyapps.barcodescanner.BarcodeResult
import com.journeyapps.barcodescanner.CameraPreview
import com.journeyapps.barcodescanner.DecoratedBarcodeView
import com.journeyapps.barcodescanner.DefaultDecoderFactory

class QrScannerActivity : Activity() {
    private var scanner: DecoratedBarcodeView? = null
    private var delivered = false

    private val callback = object : BarcodeCallback {
        override fun barcodeResult(result: BarcodeResult) {
            deliver(result)
        }

        override fun possibleResultPoints(resultPoints: List<ResultPoint>) = Unit
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED) {
            toast(R.string.camera_permission_denied)
            finish()
            return
        }
        setContentView(R.layout.activity_qr_scanner)
        val view = findViewById<DecoratedBarcodeView>(R.id.qr_scanner)
        scanner = view
        view.setStatusText("")
        view.barcodeView.decoderFactory = DefaultDecoderFactory(listOf(BarcodeFormat.QR_CODE))
        view.barcodeView.addStateListener(object : CameraPreview.StateListener {
            override fun previewSized() = Unit
            override fun previewStarted() = Unit
            override fun previewStopped() = Unit
            override fun cameraClosed() = Unit
            override fun cameraError(error: Exception) {
                if (delivered || isFinishing) return
                toast(R.string.camera_unavailable)
                finish()
            }
        })
        findViewById<View>(R.id.close_scanner).setOnClickListener { finish() }
    }

    override fun onResume() {
        super.onResume()
        val view = scanner ?: return
        view.resume()
        view.decodeSingle(callback)
    }

    override fun onPause() {
        scanner?.pause()
        super.onPause()
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        scanner?.onKeyDown(keyCode, event) == true || super.onKeyDown(keyCode, event)

    private fun deliver(result: BarcodeResult) {
        val text = result.text ?: return
        if (delivered || isFinishing) return
        delivered = true
        scanner?.pause()
        setResult(RESULT_OK, Intent().putExtra(Intents.Scan.RESULT, text))
        finish()
    }

    private fun toast(message: Int) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show()
    }
}
