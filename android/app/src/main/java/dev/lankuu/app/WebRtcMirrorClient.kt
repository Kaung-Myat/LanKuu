package dev.lankuu.app

import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjection
import android.os.Build
import android.provider.Settings
import android.util.Log
import org.json.JSONObject
import org.webrtc.DataChannel
import org.webrtc.DefaultVideoDecoderFactory
import org.webrtc.DefaultVideoEncoderFactory
import org.webrtc.EglBase
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RtpParameters
import org.webrtc.RtpReceiver
import org.webrtc.SdpObserver
import org.webrtc.ScreenCapturerAndroid
import org.webrtc.SessionDescription
import org.webrtc.SurfaceTextureHelper
import org.webrtc.VideoSource
import org.webrtc.VideoTrack
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket
import java.nio.charset.StandardCharsets
import java.util.Locale
import java.util.UUID
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

internal class WebRtcMirrorClient(
    private val context: Context,
    private val projectionData: Intent,
    private val endpoint: MirrorEndpoint,
    private val onConnected: () -> Unit,
    private val onFailure: (String) -> Unit,
    private val onClosed: () -> Unit,
) : AutoCloseable {
    private val closed = AtomicBoolean(false)
    private val connected = AtomicBoolean(false)
    private val iceGathered = CountDownLatch(1)
    private val sessionId = UUID.randomUUID().toString()
    private var eglBase: EglBase? = null
    private var factory: PeerConnectionFactory? = null
    private var peerConnection: PeerConnection? = null
    private var capturer: ScreenCapturerAndroid? = null
    private var surfaceHelper: SurfaceTextureHelper? = null
    private var videoSource: VideoSource? = null
    private var videoTrack: VideoTrack? = null
    private var controlChannel: DataChannel? = null

    fun start() {
        initializeWebRtc()
        val peer = peerConnection ?: error("WebRTC peer connection is unavailable")
        val localOffer = createOffer(peer)
        val answer = exchangeOffer(localOffer)
        setDescription(peer, answer, local = false)
    }

    private fun initializeWebRtc() {
        PeerConnectionFactory.initialize(
            PeerConnectionFactory.InitializationOptions.builder(context.applicationContext)
                .setEnableInternalTracer(false)
                .createInitializationOptions(),
        )

        val egl = EglBase.create()
        eglBase = egl
        val rtcFactory = PeerConnectionFactory.builder()
            .setVideoEncoderFactory(DefaultVideoEncoderFactory(egl.eglBaseContext, true, true))
            .setVideoDecoderFactory(DefaultVideoDecoderFactory(egl.eglBaseContext))
            .createPeerConnectionFactory()
        factory = rtcFactory

        val screenCapturer = ScreenCapturerAndroid(
            projectionData,
            object : MediaProjection.Callback() {
                override fun onStop() {
                    if (!closed.get()) onClosed()
                }
            },
        )
        capturer = screenCapturer
        val helper = SurfaceTextureHelper.create("LanKuuCapture", egl.eglBaseContext)
            ?: error("Could not create the WebRTC capture thread")
        surfaceHelper = helper
        val source = rtcFactory.createVideoSource(true)
        source.setIsScreencast(true)
        videoSource = source
        screenCapturer.initialize(helper, context.applicationContext, source.capturerObserver)

        val dimensions = scaledDimensions(
            context.resources.displayMetrics.widthPixels,
            context.resources.displayMetrics.heightPixels,
        )
        screenCapturer.startCapture(dimensions.first, dimensions.second, VIDEO_FRAME_RATE)
        val track = rtcFactory.createVideoTrack(VIDEO_TRACK_ID, source)
        track.setEnabled(true)
        videoTrack = track

        val config = PeerConnection.RTCConfiguration(emptyList()).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            continualGatheringPolicy = PeerConnection.ContinualGatheringPolicy.GATHER_ONCE
            tcpCandidatePolicy = PeerConnection.TcpCandidatePolicy.DISABLED
            enableDscp = true
            screencastMinBitrate = MIN_VIDEO_BIT_RATE / 1_000
        }
        val peer = rtcFactory.createPeerConnection(config, peerObserver)
            ?: error("Could not create a WebRTC peer connection")
        peerConnection = peer
        val control = peer.createDataChannel(CONTROL_CHANNEL_ID, DataChannel.Init())
        control.registerObserver(controlObserver)
        controlChannel = control
        val sender = peer.addTrack(track, listOf(STREAM_ID))
            ?: error("Could not add the screen video track")
        sender.getParameters().also { parameters ->
            parameters.degradationPreference = RtpParameters.DegradationPreference.BALANCED
            parameters.encodings.firstOrNull()?.apply {
                minBitrateBps = MIN_VIDEO_BIT_RATE
                maxBitrateBps = MAX_VIDEO_BIT_RATE
                maxFramerate = VIDEO_FRAME_RATE
            }
            if (!sender.setParameters(parameters)) {
                Log.w(LOG_TAG, "The hardware encoder did not accept the preferred bitrate range")
            }
        }
        peer.setBitrate(MIN_VIDEO_BIT_RATE, START_VIDEO_BIT_RATE, MAX_VIDEO_BIT_RATE)
    }

    private fun createOffer(peer: PeerConnection): SessionDescription {
        val create = SdpAwaiter()
        peer.createOffer(create, MediaConstraints())
        val offer = create.awaitCreated("creating the WebRTC offer")
        setDescription(peer, offer, local = true)
        if (
            peer.iceGatheringState() != PeerConnection.IceGatheringState.COMPLETE &&
            !iceGathered.await(SIGNAL_TIMEOUT_SECONDS, TimeUnit.SECONDS)
        ) {
            error("Timed out while finding the local LAN route")
        }
        return peer.localDescription ?: error("WebRTC did not produce a local offer")
    }

    private fun setDescription(
        peer: PeerConnection,
        description: SessionDescription,
        local: Boolean,
    ) {
        val observer = SdpAwaiter()
        if (local) peer.setLocalDescription(observer, description)
        else peer.setRemoteDescription(observer, description)
        observer.awaitSet(if (local) "setting the local offer" else "setting the desktop answer")
    }

    private fun exchangeOffer(offer: SessionDescription): SessionDescription {
        val request = JSONObject()
            .put("type", offer.type.canonicalForm())
            .put("sdp", offer.description)
            .put("protocolVersion", MIRROR_PROTOCOL_VERSION)
            .put("sessionId", sessionId)
            .put("deviceId", deviceId())
            .put("deviceName", deviceName())
            .put("platform", "android")
            .toString()
            .toByteArray(StandardCharsets.UTF_8)
        require(request.size <= MAX_SIGNAL_BYTES) { "WebRTC offer is too large" }

        Socket().use { socket ->
            socket.connect(InetSocketAddress(endpoint.host, endpoint.port), CONNECT_TIMEOUT_MS)
            socket.soTimeout = (SIGNAL_TIMEOUT_SECONDS * 1_000).toInt()
            val output = DataOutputStream(socket.getOutputStream())
            output.writeInt(request.size)
            output.write(request)
            output.flush()

            val input = DataInputStream(socket.getInputStream())
            val answerSize = input.readInt()
            require(answerSize in 1..MAX_SIGNAL_BYTES) { "Invalid response from desktop" }
            val answerBytes = ByteArray(answerSize)
            input.readFully(answerBytes)
            val json = JSONObject(String(answerBytes, StandardCharsets.UTF_8))
            if (json.has("error")) error(json.getString("error"))
            return SessionDescription(
                SessionDescription.Type.fromCanonicalForm(json.getString("type")),
                json.getString("sdp"),
            )
        }
    }

    private val peerObserver = object : PeerConnection.Observer {
        override fun onSignalingChange(state: PeerConnection.SignalingState) = Unit
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState) = Unit
        override fun onIceConnectionReceivingChange(receiving: Boolean) = Unit
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState) {
            if (state == PeerConnection.IceGatheringState.COMPLETE) iceGathered.countDown()
        }
        override fun onIceCandidate(candidate: IceCandidate) = Unit
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>) = Unit
        override fun onAddStream(stream: MediaStream) = Unit
        override fun onRemoveStream(stream: MediaStream) = Unit
        override fun onDataChannel(channel: DataChannel) = Unit
        override fun onRenegotiationNeeded() = Unit
        override fun onAddTrack(receiver: RtpReceiver, streams: Array<out MediaStream>) = Unit

        override fun onConnectionChange(state: PeerConnection.PeerConnectionState) {
            when (state) {
                PeerConnection.PeerConnectionState.CONNECTED -> {
                    if (connected.compareAndSet(false, true) && !closed.get()) onConnected()
                }
                PeerConnection.PeerConnectionState.FAILED -> {
                    if (!closed.get()) onFailure("WebRTC connection failed")
                }
                PeerConnection.PeerConnectionState.CLOSED -> {
                    if (!closed.get()) onClosed()
                }
                else -> Unit
            }
        }
    }

    private val controlObserver = object : DataChannel.Observer {
        override fun onBufferedAmountChange(previousAmount: Long) = Unit

        override fun onStateChange() = Unit

        override fun onMessage(buffer: DataChannel.Buffer) {
            if (buffer.binary || closed.get()) return
            runCatching {
                val data = buffer.data.asReadOnlyBuffer()
                val bytes = ByteArray(data.remaining())
                data.get(bytes)
                JSONObject(String(bytes, StandardCharsets.UTF_8))
            }.onSuccess { message ->
                if (message.optString("type") == "stop_session") onClosed()
            }.onFailure { error ->
                Log.w(LOG_TAG, "Ignored invalid desktop control message", error)
            }
        }
    }

    override fun close() {
        if (!closed.compareAndSet(false, true)) return
        runCatching { capturer?.stopCapture() }
        runCatching { controlChannel?.unregisterObserver() }
        runCatching { controlChannel?.close() }
        runCatching { controlChannel?.dispose() }
        controlChannel = null
        runCatching { peerConnection?.close() }
        runCatching { peerConnection?.dispose() }
        peerConnection = null
        runCatching { videoTrack?.dispose() }
        videoTrack = null
        runCatching { videoSource?.dispose() }
        videoSource = null
        runCatching { capturer?.dispose() }
        capturer = null
        runCatching { surfaceHelper?.dispose() }
        surfaceHelper = null
        runCatching { factory?.dispose() }
        factory = null
        runCatching { eglBase?.release() }
        eglBase = null
    }

    private fun scaledDimensions(width: Int, height: Int): Pair<Int, Int> {
        val longest = maxOf(width, height)
        val scale = if (longest > MAX_DIMENSION) MAX_DIMENSION.toFloat() / longest else 1f
        val scaledWidth = ((width * scale).toInt() / 2) * 2
        val scaledHeight = ((height * scale).toInt() / 2) * 2
        return maxOf(2, scaledWidth) to maxOf(2, scaledHeight)
    }

    private fun deviceId(): String = Settings.Secure.getString(
        context.contentResolver,
        Settings.Secure.ANDROID_ID,
    ).orEmpty().ifBlank { "android-${Build.DEVICE}" }

    private fun deviceName(): String {
        val manufacturer = Build.MANUFACTURER.trim()
        val model = Build.MODEL.trim()
        if (manufacturer.isBlank()) return model.ifBlank { "Android device" }
        if (model.startsWith(manufacturer, ignoreCase = true)) return model
        return "${manufacturer.replaceFirstChar { it.titlecase(Locale.getDefault()) }} $model".trim()
    }

    private class SdpAwaiter : SdpObserver {
        private val latch = CountDownLatch(1)
        @Volatile private var description: SessionDescription? = null
        @Volatile private var failure: String? = null

        override fun onCreateSuccess(value: SessionDescription) {
            description = value
            latch.countDown()
        }

        override fun onSetSuccess() = latch.countDown()

        override fun onCreateFailure(error: String) {
            failure = error
            latch.countDown()
        }

        override fun onSetFailure(error: String) {
            failure = error
            latch.countDown()
        }

        fun awaitCreated(stage: String): SessionDescription {
            await(stage)
            return description ?: error("$stage failed: ${failure ?: "no description"}")
        }

        fun awaitSet(stage: String) {
            await(stage)
            failure?.let { error("$stage failed: $it") }
        }

        private fun await(stage: String) {
            if (!latch.await(SIGNAL_TIMEOUT_SECONDS, TimeUnit.SECONDS)) {
                error("Timed out while $stage")
            }
        }
    }

    private companion object {
        const val VIDEO_TRACK_ID = "lankuu-screen"
        const val STREAM_ID = "lankuu-mirror"
        const val CONTROL_CHANNEL_ID = "lankuu-control"
        const val MIRROR_PROTOCOL_VERSION = 3
        const val VIDEO_FRAME_RATE = 30
        const val MAX_DIMENSION = 1_280
        const val MIN_VIDEO_BIT_RATE = 600_000
        const val START_VIDEO_BIT_RATE = 3_500_000
        const val MAX_VIDEO_BIT_RATE = 6_000_000
        const val CONNECT_TIMEOUT_MS = 5_000
        const val SIGNAL_TIMEOUT_SECONDS = 15L
        const val MAX_SIGNAL_BYTES = 1_048_576
        const val LOG_TAG = "LanKuuWebRTC"
    }
}
