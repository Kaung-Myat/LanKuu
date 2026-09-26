package dev.lankuu.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean

class ScreenCastService : Service() {
    private val running = AtomicBoolean(false)
    private val mainHandler = Handler(Looper.getMainLooper())
    private var mirrorClient: WebRtcMirrorClient? = null
    private var connectThread: Thread? = null

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopCasting()
            return START_NOT_STICKY
        }
        if (!running.compareAndSet(false, true)) return START_NOT_STICKY

        startForeground(
            NOTIFICATION_ID,
            createNotification(getString(R.string.mirror_notification_starting)),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION,
        )

        val host = intent?.getStringExtra(EXTRA_HOST)?.trim().orEmpty()
        val port = intent?.getIntExtra(EXTRA_PORT, MirrorProtocol.port) ?: MirrorProtocol.port
        val resultCode = intent?.getIntExtra(EXTRA_RESULT_CODE, 0) ?: 0
        val resultData = intent?.projectionData()
        if (host.isEmpty() || port !in 1..65_535 || resultCode == 0 || resultData == null) {
            fail(getString(R.string.mirror_invalid_request))
            return START_NOT_STICKY
        }

        connectThread = Thread(
            { connect(host, port, resultData) },
            "lankuu-webrtc-connect",
        ).apply { start() }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        stopCasting()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun connect(host: String, port: Int, projectionData: Intent) {
        try {
            val client = WebRtcMirrorClient(
                context = this,
                projectionData = projectionData,
                endpoint = MirrorEndpoint(host, port),
                onConnected = connected@{
                    if (!running.get()) return@connected
                    mainHandler.post {
                        if (!running.get()) return@post
                        active = true
                        broadcastState(true, null)
                        getSystemService(NotificationManager::class.java).notify(
                            NOTIFICATION_ID,
                            createNotification(getString(R.string.mirror_notification_active, "$host:$port")),
                        )
                    }
                },
                onFailure = { message ->
                    mainHandler.post { if (running.get()) fail(message) }
                },
                onClosed = {
                    mainHandler.post { if (running.get()) finishCasting() }
                },
            )
            mirrorClient = client
            client.start()
        } catch (error: Exception) {
            Log.e(LOG_TAG, "WebRTC mirroring failed", error)
            if (running.get()) {
                val detail = error.message?.takeIf(String::isNotBlank) ?: error.javaClass.simpleName
                fail(detail)
            }
        }
    }

    private fun stopCasting() {
        if (!running.getAndSet(false)) {
            stopSelf()
            return
        }
        releaseClient()
        active = false
        broadcastState(false, null)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun finishCasting() {
        if (!running.getAndSet(false)) return
        releaseClient()
        active = false
        broadcastState(false, null)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun releaseClient() {
        val client = mirrorClient
        mirrorClient = null
        runCatching { client?.close() }
        connectThread?.interrupt()
        connectThread = null
    }

    private fun fail(message: String) {
        if (!running.getAndSet(false)) return
        releaseClient()
        active = false
        broadcastState(false, message)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun broadcastState(isRunning: Boolean, error: String?) {
        sendBroadcast(Intent(ACTION_STATE).apply {
            setPackage(packageName)
            putExtra(EXTRA_RUNNING, isRunning)
            putExtra(EXTRA_ERROR, error)
        })
    }

    private fun createNotificationChannel() {
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL,
                getString(R.string.mirror_notification_channel),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    private fun createNotification(content: String): Notification {
        val stopIntent = Intent(this, ScreenCastService::class.java).setAction(ACTION_STOP)
        val stopAction = PendingIntent.getService(
            this,
            1,
            stopIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        return Notification.Builder(this, NOTIFICATION_CHANNEL)
            .setSmallIcon(R.drawable.ic_launcher)
            .setContentTitle(getString(R.string.mirror_notification_title))
            .setContentText(content)
            .setOngoing(true)
            .setCategory(Notification.CATEGORY_SERVICE)
            .addAction(R.drawable.ic_launcher, getString(R.string.stop_mirroring), stopAction)
            .build()
    }

    private fun Intent.projectionData(): Intent? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        getParcelableExtra(EXTRA_RESULT_DATA, Intent::class.java)
    } else {
        @Suppress("DEPRECATION")
        getParcelableExtra(EXTRA_RESULT_DATA)
    }

    companion object {
        @Volatile
        var active: Boolean = false
            private set
        const val ACTION_STATE = "dev.lankuu.app.MIRROR_STATE"
        const val ACTION_STOP = "dev.lankuu.app.STOP_MIRROR"
        const val EXTRA_HOST = "host"
        const val EXTRA_PORT = "port"
        const val EXTRA_RESULT_CODE = "result_code"
        const val EXTRA_RESULT_DATA = "result_data"
        const val EXTRA_RUNNING = "running"
        const val EXTRA_ERROR = "error"
        private const val NOTIFICATION_CHANNEL = "lankuu_mirroring"
        private const val NOTIFICATION_ID = 45_456
        private const val LOG_TAG = "LanKuuWebRTC"
    }
}
