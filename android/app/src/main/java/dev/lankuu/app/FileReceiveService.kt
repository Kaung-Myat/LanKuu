package dev.lankuu.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.os.IBinder

/** Keeps an explicitly enabled file receiver alive when the activity is recreated. */
class FileReceiveService : Service() {
    private lateinit var receiver: LanKuuReceiver

    override fun onCreate() {
        super.onCreate()
        receiver = LanKuuReceiver(applicationContext, ::report) {
            PairingStore.token(applicationContext)
        }
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> stopReceiver()
            else -> startReceiver()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        receiver.stop()
        active = false
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun startReceiver() {
        if (!active) {
            startForeground(NOTIFICATION_ID, createNotification())
            active = true
        }
        receiver.start()
    }

    private fun stopReceiver() {
        receiver.stop()
        active = false
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun report(message: String) {
        sendBroadcast(Intent(ACTION_EVENT).apply {
            setPackage(packageName)
            putExtra(EXTRA_MESSAGE, message)
        })
    }

    private fun createNotificationChannel() {
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                NOTIFICATION_CHANNEL,
                getString(R.string.receiver_notification_channel),
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    private fun createNotification(): Notification = Notification.Builder(this, NOTIFICATION_CHANNEL)
        .setSmallIcon(R.drawable.ic_launcher)
        .setContentTitle(getString(R.string.receiver_notification_title))
        .setContentText(getString(R.string.receiver_notification_active))
        .setOngoing(true)
        .setCategory(Notification.CATEGORY_SERVICE)
        .build()

    companion object {
        const val ACTION_START = "dev.lankuu.app.START_RECEIVER"
        const val ACTION_STOP = "dev.lankuu.app.STOP_RECEIVER"
        const val ACTION_EVENT = "dev.lankuu.app.RECEIVER_EVENT"
        const val EXTRA_MESSAGE = "message"
        private const val NOTIFICATION_CHANNEL = "lankuu_file_receive"
        private const val NOTIFICATION_ID = 45_454

        @Volatile
        var active: Boolean = false
            private set
    }
}
