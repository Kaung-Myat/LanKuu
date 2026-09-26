package dev.lankuu.app

import android.Manifest
import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.text.Editable
import android.text.TextWatcher
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import java.text.DateFormat
import java.util.Date
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private enum class Screen { SHARE, CAST, INBOX, ACTIVITY }
    private enum class NetworkPermissionAction { RECEIVE, MIRROR }

    private val worker: ExecutorService = Executors.newSingleThreadExecutor()
    private lateinit var receiver: LanKuuReceiver
    private lateinit var shareScreen: View
    private lateinit var castScreen: View
    private lateinit var inboxScreen: View
    private lateinit var activityScreen: View
    private lateinit var navShare: View
    private lateinit var navCast: View
    private lateinit var navInbox: View
    private lateinit var navActivity: View
    private lateinit var inboxBadge: TextView
    private lateinit var castHost: EditText
    private lateinit var castButton: Button
    private lateinit var castStatus: TextView
    private lateinit var peerHost: EditText
    private lateinit var message: EditText
    private lateinit var characterCount: TextView
    private lateinit var status: TextView
    private lateinit var receiverTitle: TextView
    private lateinit var receiverCaption: TextView
    private lateinit var receiverToggle: Switch
    private lateinit var receiverCard: View
    private lateinit var selectedDevice: TextView
    private lateinit var discoverButton: Button
    private lateinit var sendTextButton: Button
    private lateinit var sendFileButton: Button
    private lateinit var deviceList: LinearLayout
    private lateinit var activityList: LinearLayout
    private lateinit var inboxList: LinearLayout
    private lateinit var inboxEmpty: View
    private lateinit var activityEmpty: View
    private var suppressReceiverToggle = false
    private var activeScreen = Screen.SHARE
    private var unreadItems = 0
    private var mirrorReceiverRegistered = false
    private var pendingMirrorEndpoint: MirrorEndpoint? = null
    private var pendingNetworkPermissionAction: NetworkPermissionAction? = null
    private val mirrorStateReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            val running = intent?.getBooleanExtra(ScreenCastService.EXTRA_RUNNING, false) ?: false
            val error = intent?.getStringExtra(ScreenCastService.EXTRA_ERROR)
            updateMirrorUi(running, error)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        bindViews()
        receiver = LanKuuReceiver(this, ::showStatus)
        bindActions()
        showScreen(Screen.SHARE)
        showStatus(getString(R.string.ready_trusted_network), addToActivity = false)
    }

    override fun onStart() {
        super.onStart()
        registerMirrorReceiver()
        updateMirrorUi(ScreenCastService.active, null)
    }

    override fun onStop() {
        if (mirrorReceiverRegistered) {
            unregisterReceiver(mirrorStateReceiver)
            mirrorReceiverRegistered = false
        }
        super.onStop()
    }

    override fun onDestroy() {
        receiver.stop()
        worker.shutdownNow()
        super.onDestroy()
    }

    @Deprecated("Kept for a dependency-free MVP file picker")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        when (requestCode) {
            PICK_FILE -> if (resultCode == RESULT_OK && data != null) {
                val uris = selectedFileUris(data)
                persistReadAccess(data, uris)
                if (uris.isEmpty()) toast(getString(R.string.no_files_selected))
                else sendFiles(uris)
            }
            REQUEST_SCREEN_CAPTURE -> {
                if (resultCode == RESULT_OK && data != null) {
                    val endpoint = pendingMirrorEndpoint
                    if (endpoint != null) startMirrorService(endpoint, resultCode, data)
                    else updateMirrorUi(false, getString(R.string.mirror_invalid_address))
                }
                else updateMirrorUi(false, getString(R.string.mirror_permission_denied))
            }
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQUEST_LOCAL_NETWORK_PERMISSION) return
        val action = pendingNetworkPermissionAction
        pendingNetworkPermissionAction = null
        receiverToggle.isEnabled = true
        if (grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED) {
            when (action) {
                NetworkPermissionAction.RECEIVE -> {
                    suppressReceiverToggle = true
                    receiverToggle.isChecked = true
                    suppressReceiverToggle = false
                    setReceiverRunning(true)
                }
                NetworkPermissionAction.MIRROR -> launchScreenCapturePrompt()
                null -> Unit
            }
        } else {
            when (action) {
                NetworkPermissionAction.RECEIVE -> {
                    resetReceiverUi()
                    showStatus(getString(R.string.receiver_local_network_permission_denied))
                    toast(getString(R.string.receiver_local_network_permission_denied))
                }
                NetworkPermissionAction.MIRROR ->
                    updateMirrorUi(false, getString(R.string.mirror_local_network_permission_denied))
                null -> Unit
            }
        }
    }

    private fun bindViews() {
        shareScreen = findViewById(R.id.share_screen)
        castScreen = findViewById(R.id.cast_screen)
        inboxScreen = findViewById(R.id.inbox_screen)
        activityScreen = findViewById(R.id.activity_screen)
        navShare = findViewById(R.id.nav_share)
        navCast = findViewById(R.id.nav_cast)
        navInbox = findViewById(R.id.nav_inbox)
        navActivity = findViewById(R.id.nav_activity)
        inboxBadge = findViewById(R.id.inbox_badge)
        castHost = findViewById(R.id.cast_host)
        castButton = findViewById(R.id.cast_button)
        castStatus = findViewById(R.id.cast_status)
        peerHost = findViewById(R.id.peer_host)
        message = findViewById(R.id.message_input)
        characterCount = findViewById(R.id.character_count)
        status = findViewById(R.id.status_text)
        receiverTitle = findViewById(R.id.receiver_title)
        receiverCaption = findViewById(R.id.receiver_caption)
        receiverToggle = findViewById(R.id.receiver_toggle)
        receiverCard = findViewById(R.id.receiver_card)
        selectedDevice = findViewById(R.id.selected_device)
        discoverButton = findViewById(R.id.discover_devices)
        sendTextButton = findViewById(R.id.send_text)
        sendFileButton = findViewById(R.id.send_file)
        deviceList = findViewById(R.id.device_list)
        activityList = findViewById(R.id.activity_list)
        activityEmpty = findViewById(R.id.activity_empty)
        inboxList = findViewById(R.id.inbox_list)
        inboxEmpty = findViewById(R.id.inbox_empty)
    }

    private fun bindActions() {
        navShare.setOnClickListener { showScreen(Screen.SHARE) }
        navCast.setOnClickListener { showScreen(Screen.CAST) }
        navInbox.setOnClickListener { showScreen(Screen.INBOX) }
        navActivity.setOnClickListener { showScreen(Screen.ACTIVITY) }
        findViewById<View>(R.id.clear_inbox).setOnClickListener { clearInbox() }
        discoverButton.setOnClickListener { discoverDevices() }
        sendTextButton.setOnClickListener { sendText() }
        sendFileButton.setOnClickListener { chooseFiles() }
        castButton.setOnClickListener {
            if (ScreenCastService.active) stopMirroring() else requestScreenCapture()
        }
        receiverToggle.setOnCheckedChangeListener { _, checked ->
            if (!suppressReceiverToggle) {
                if (checked && requestLocalNetworkPermission(NetworkPermissionAction.RECEIVE)) {
                    setReceiverStartingUi(getString(R.string.waiting_for_nearby_permission))
                } else {
                    setReceiverRunning(checked)
                }
            }
        }
        message.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(text: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(text: CharSequence?, start: Int, before: Int, count: Int) {
                characterCount.text = getString(R.string.character_count, text?.length ?: 0)
            }
            override fun afterTextChanged(text: Editable?) = Unit
        })
    }

    private fun showScreen(screen: Screen) {
        activeScreen = screen
        shareScreen.visibility = if (screen == Screen.SHARE) View.VISIBLE else View.GONE
        castScreen.visibility = if (screen == Screen.CAST) View.VISIBLE else View.GONE
        inboxScreen.visibility = if (screen == Screen.INBOX) View.VISIBLE else View.GONE
        activityScreen.visibility = if (screen == Screen.ACTIVITY) View.VISIBLE else View.GONE
        navShare.isSelected = screen == Screen.SHARE
        navCast.isSelected = screen == Screen.CAST
        navInbox.isSelected = screen == Screen.INBOX
        navActivity.isSelected = screen == Screen.ACTIVITY
        if (screen == Screen.INBOX) {
            unreadItems = 0
            updateInboxBadge()
        }
    }

    private fun discoverDevices() {
        discoverButton.isEnabled = false
        discoverButton.text = getString(R.string.searching)
        showStatus(getString(R.string.searching_local_network))
        worker.execute {
            runCatching { LanKuuDiscovery.discover() }
                .onSuccess { devices ->
                    runOnUiThread {
                        discoverButton.isEnabled = true
                        discoverButton.text = getString(R.string.refresh_short)
                        renderDevices(devices)
                        if (devices.isEmpty()) {
                            showStatus(getString(R.string.no_devices_status))
                        } else {
                            selectDevice(devices.first())
                            showStatus(resources.getQuantityString(R.plurals.devices_found, devices.size, devices.size))
                        }
                    }
                }
                .onFailure {
                    runOnUiThread {
                        discoverButton.isEnabled = true
                        discoverButton.text = getString(R.string.discover_short)
                    }
                    showStatus(getString(R.string.discovery_failed, it.userMessage()))
                }
        }
    }

    private fun renderDevices(devices: List<LanKuuDiscovery.Device>) {
        deviceList.removeAllViews()
        if (devices.isEmpty()) {
            deviceList.addView(TextView(this).apply {
                text = getString(R.string.no_devices_inline)
                setTextAppearance(R.style.TextAppearance_LanKuu_BodyMuted)
                gravity = Gravity.CENTER
                setPadding(dp(16), dp(18), dp(16), dp(18))
            })
            return
        }
        devices.forEachIndexed { index, device ->
            val row = layoutInflater.inflate(R.layout.item_device, deviceList, false)
            row.findViewById<TextView>(R.id.device_name).text = device.name
            row.findViewById<TextView>(R.id.device_address).text =
                getString(R.string.device_address_format, device.host, device.port)
            row.setOnClickListener { selectDevice(device) }
            deviceList.addView(row)
            if (index < devices.lastIndex) {
                deviceList.addView(View(this).apply { setBackgroundColor(getColor(R.color.divider)) }, matchWidth(dp(1)))
            }
        }
    }

    private fun selectDevice(device: LanKuuDiscovery.Device) {
        peerHost.setText(device.host)
        castHost.setText(device.host)
        selectedDevice.text = getString(R.string.selected_device_format, device.name, device.host)
        selectedDevice.setTextColor(getColor(R.color.text_primary))
    }

    private fun registerMirrorReceiver() {
        val filter = IntentFilter(ScreenCastService.ACTION_STATE)
        registerReceiver(mirrorStateReceiver, filter, RECEIVER_NOT_EXPORTED)
        mirrorReceiverRegistered = true
    }

    private fun requestScreenCapture() {
        val endpoint = MirrorEndpoint.parse(castHost.text.toString())
        if (endpoint == null) {
            castHost.requestFocus()
            toast(getString(R.string.mirror_invalid_address))
            return
        }
        pendingMirrorEndpoint = endpoint
        if (requestLocalNetworkPermission(NetworkPermissionAction.MIRROR)) return
        launchScreenCapturePrompt()
    }

    private fun requestLocalNetworkPermission(action: NetworkPermissionAction): Boolean {
        if (
            Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            checkSelfPermission(Manifest.permission.NEARBY_WIFI_DEVICES) == PackageManager.PERMISSION_GRANTED
        ) {
            return false
        }
        pendingNetworkPermissionAction = action
        if (action == NetworkPermissionAction.RECEIVE) receiverToggle.isEnabled = false
        requestPermissions(
            arrayOf(Manifest.permission.NEARBY_WIFI_DEVICES),
            REQUEST_LOCAL_NETWORK_PERMISSION,
        )
        return true
    }

    private fun launchScreenCapturePrompt() {
        val manager = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        val captureIntent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            manager.createScreenCaptureIntent()
        }
        startActivityForResult(captureIntent, REQUEST_SCREEN_CAPTURE)
    }

    private fun startMirrorService(endpoint: MirrorEndpoint, resultCode: Int, resultData: Intent) {
        val serviceIntent = Intent(this, ScreenCastService::class.java).apply {
            putExtra(ScreenCastService.EXTRA_HOST, endpoint.host)
            putExtra(ScreenCastService.EXTRA_PORT, endpoint.port)
            putExtra(ScreenCastService.EXTRA_RESULT_CODE, resultCode)
            putExtra(ScreenCastService.EXTRA_RESULT_DATA, resultData)
        }
        startForegroundService(serviceIntent)
        castButton.isEnabled = false
        castStatus.text = getString(R.string.mirror_connecting)
        showStatus(getString(R.string.mirror_connecting))
    }

    private fun stopMirroring() {
        startService(Intent(this, ScreenCastService::class.java).setAction(ScreenCastService.ACTION_STOP))
        castButton.isEnabled = false
        castStatus.text = getString(R.string.mirror_stopping)
    }

    private fun updateMirrorUi(running: Boolean, error: String?) {
        castButton.isEnabled = true
        castButton.text = getString(if (running) R.string.stop_mirroring else R.string.start_mirroring)
        castStatus.text = when {
            error != null -> getString(R.string.mirror_error, error)
            running -> getString(R.string.mirror_active)
            else -> getString(R.string.mirror_ready)
        }
        if (error != null) {
            toast(getString(R.string.mirror_error, error))
            showStatus(getString(R.string.mirror_error, error))
        } else if (running) {
            showStatus(getString(R.string.mirror_started))
        }
    }

    private fun sendText() {
        val host = validatedHost() ?: return
        val text = message.text.toString().trim()
        if (text.isEmpty()) {
            showStatus(getString(R.string.enter_text))
            message.requestFocus()
            return
        }
        setSending(true)
        showStatus(getString(R.string.sending_text, host))
        worker.execute {
            runCatching { LanKuuSender(contentResolver).sendText(host, text) }
                .onSuccess {
                    runOnUiThread {
                        message.text.clear()
                        toast(getString(R.string.text_delivered, host))
                    }
                    showStatus(getString(R.string.text_delivered, host))
                }
                .onFailure {
                    val error = getString(R.string.send_failed, it.userMessage())
                    showStatus(error)
                    runOnUiThread { toast(error) }
                }
            runOnUiThread { setSending(false) }
        }
    }

    private fun chooseFiles() {
        if (validatedHost() == null) return
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "*/*"
            putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        }
        startActivityForResult(intent, PICK_FILE)
    }

    private fun selectedFileUris(intent: Intent): List<Uri> {
        val selected = LinkedHashSet<Uri>()
        intent.clipData?.let { clipData ->
            for (index in 0 until clipData.itemCount) {
                clipData.getItemAt(index).uri?.let(selected::add)
            }
        }
        intent.data?.let(selected::add)
        return selected.toList()
    }

    private fun persistReadAccess(intent: Intent, uris: List<Uri>) {
        if (intent.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION == 0) return
        uris.forEach { uri ->
            runCatching {
                contentResolver.takePersistableUriPermission(
                    uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION,
                )
            }
        }
    }

    private fun sendFiles(uris: List<Uri>) {
        val host = validatedHost() ?: return
        setSending(true)
        showStatus(getString(R.string.preparing_files, uris.size, host), addToActivity = false)
        worker.execute {
            runCatching {
                LanKuuSender(contentResolver).sendFiles(host, uris) { index, total, name ->
                    showStatus(
                        getString(R.string.sending_file_progress, index, total, name, host),
                        addToActivity = false,
                    )
                }
            }
                .onSuccess { result -> showBatchResult(host, uris.size, result) }
                .onFailure {
                    val error = getString(R.string.send_failed, it.userMessage())
                    showStatus(error)
                    runOnUiThread { toast(error) }
                }
            runOnUiThread { setSending(false) }
        }
    }

    private fun showBatchResult(host: String, total: Int, result: LanKuuSender.BatchResult) {
        val delivered = result.deliveredNames.size
        val failed = result.failures.size
        val message = when {
            failed == 0 && total == 1 ->
                getString(R.string.file_delivered, result.deliveredNames.first(), host)
            failed == 0 ->
                resources.getQuantityString(R.plurals.files_delivered, delivered, delivered, host)
            delivered == 0 -> {
                val first = result.failures.first()
                getString(R.string.files_failed, failed, first.name, first.reason)
            }
            else -> getString(R.string.files_partially_delivered, delivered, total, host, failed)
        }
        showStatus(message)
        runOnUiThread { toast(message) }
    }

    private fun setSending(sending: Boolean) {
        sendTextButton.isEnabled = !sending
        sendFileButton.isEnabled = !sending
        sendTextButton.text = getString(if (sending) R.string.sending else R.string.send_text)
        sendFileButton.text = getString(if (sending) R.string.sending_short else R.string.choose_files)
    }

    private fun setReceiverRunning(shouldRun: Boolean) {
        if (shouldRun) {
            setReceiverStartingUi(getString(R.string.starting_receiver))
            receiver.start()
            showStatus(getString(R.string.starting_receiver))
        } else {
            receiver.stop()
            receiverToggle.isEnabled = true
            receiverTitle.text = getString(R.string.not_receiving)
            receiverCaption.text = getString(R.string.turn_on_receiver)
            receiverCard.setBackgroundResource(R.drawable.bg_card)
            showStatus(getString(R.string.receiver_stopped))
        }
    }

    private fun setReceiverStartingUi(caption: String) {
        receiverToggle.isEnabled = false
        receiverTitle.text = getString(R.string.starting_receiver)
        receiverCaption.text = caption
        receiverCard.setBackgroundResource(R.drawable.bg_receiver_active)
    }

    private fun setReceiverReadyUi() {
        suppressReceiverToggle = true
        receiverToggle.isChecked = true
        suppressReceiverToggle = false
        receiverToggle.isEnabled = true
        receiverTitle.text = getString(R.string.ready_to_receive)
        receiverCaption.text = getString(R.string.visible_nearby)
        receiverCard.setBackgroundResource(R.drawable.bg_receiver_active)
    }

    private fun validatedHost(): String? {
        val host = peerHost.text.toString().trim()
        if (host.isEmpty()) {
            showStatus(getString(R.string.enter_peer_ip))
            peerHost.requestFocus()
            return null
        }
        selectedDevice.text = getString(R.string.manual_destination, host)
        selectedDevice.setTextColor(getColor(R.color.text_primary))
        return host
    }

    private fun showStatus(text: String, addToActivity: Boolean = true) {
        runOnUiThread {
            status.text = text
            when {
                text.startsWith(TEXT_RECEIVED_PREFIX) -> {
                    addInboxItem(getString(R.string.received_text), text.removePrefix(TEXT_RECEIVED_PREFIX))
                }
                text.startsWith(FILE_RECEIVED_PREFIX) && text.endsWith(FILE_RECEIVED_SUFFIX) -> {
                    val name = text.removePrefix(FILE_RECEIVED_PREFIX).removeSuffix(FILE_RECEIVED_SUFFIX)
                    addInboxItem(getString(R.string.received_file), getString(R.string.received_file_format, name))
                }
            }
            when {
                text.startsWith(RECEIVER_READY_PREFIX) -> setReceiverReadyUi()
                text.startsWith(RECEIVER_ERROR_PREFIX) -> {
                    resetReceiverUi()
                    toast(text)
                }
            }
            if (addToActivity) addActivity(text)
        }
    }

    private fun addInboxItem(kind: String, content: String) {
        inboxEmpty.visibility = View.GONE
        val row = layoutInflater.inflate(R.layout.item_inbox, inboxList, false)
        row.findViewById<TextView>(R.id.inbox_kind).text = kind
        row.findViewById<TextView>(R.id.inbox_content).text = content
        row.findViewById<TextView>(R.id.inbox_time).text = currentTime()
        inboxList.addView(row, 1)
        while (inboxList.childCount > MAX_INBOX_ITEMS + 1) {
            inboxList.removeViewAt(inboxList.childCount - 1)
        }
        if (activeScreen != Screen.INBOX) {
            unreadItems += 1
            updateInboxBadge()
        }
    }

    private fun clearInbox() {
        while (inboxList.childCount > 1) inboxList.removeViewAt(inboxList.childCount - 1)
        inboxEmpty.visibility = View.VISIBLE
        unreadItems = 0
        updateInboxBadge()
    }

    private fun updateInboxBadge() {
        inboxBadge.visibility = if (unreadItems > 0) View.VISIBLE else View.GONE
        inboxBadge.text = if (unreadItems > 9) "9+" else unreadItems.toString()
    }

    private fun addActivity(text: String) {
        activityEmpty.visibility = View.GONE
        val row = layoutInflater.inflate(R.layout.item_activity, activityList, false)
        row.findViewById<TextView>(R.id.activity_message).text = text
        row.findViewById<TextView>(R.id.activity_time).text = currentTime()
        activityList.addView(row, 1)
        while (activityList.childCount > MAX_ACTIVITY_ITEMS + 1) {
            activityList.removeViewAt(activityList.childCount - 1)
        }
    }

    private fun resetReceiverUi() {
        suppressReceiverToggle = true
        receiverToggle.isChecked = false
        suppressReceiverToggle = false
        receiverToggle.isEnabled = true
        receiverTitle.text = getString(R.string.not_receiving)
        receiverCaption.text = getString(R.string.turn_on_receiver)
        receiverCard.setBackgroundResource(R.drawable.bg_card)
    }

    private fun currentTime(): String = DateFormat.getTimeInstance(DateFormat.SHORT).format(Date())

    private fun toast(text: String) = Toast.makeText(this, text, Toast.LENGTH_SHORT).show()

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    private fun matchWidth(height: Int): LinearLayout.LayoutParams =
        LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, height)

    private companion object {
        const val PICK_FILE = 1001
        const val REQUEST_SCREEN_CAPTURE = 1002
        const val REQUEST_LOCAL_NETWORK_PERMISSION = 1003
        const val MAX_ACTIVITY_ITEMS = 12
        const val MAX_INBOX_ITEMS = 20
        const val TEXT_RECEIVED_PREFIX = "Text received:\n"
        const val RECEIVER_READY_PREFIX = "Receiving on port "
        const val RECEIVER_ERROR_PREFIX = "Receiver error:"
        const val FILE_RECEIVED_PREFIX = "Received "
        const val FILE_RECEIVED_SUFFIX = " in Downloads/LanKuu"
    }
}

private fun Throwable.userMessage(): String = message ?: javaClass.simpleName
