package dev.lankuu.app

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Log
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.BindException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.net.SocketTimeoutException
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

internal class LanKuuReceiver(
    private val context: Context,
    private val report: (String) -> Unit,
) {
    private val running = AtomicBoolean(false)
    private val generation = AtomicInteger(0)
    @Volatile
    private var serverSocket: ServerSocket? = null
    @Volatile
    private var discoverySocket: DatagramSocket? = null

    fun start() {
        if (!running.compareAndSet(false, true)) return
        val session = generation.incrementAndGet()
        Thread({ receiveLoop(session) }, "lankuu-receiver").start()
    }

    fun stop() {
        generation.incrementAndGet()
        running.set(false)
        runCatching { serverSocket?.close() }
        runCatching { discoverySocket?.close() }
        serverSocket = null
        discoverySocket = null
    }

    fun isRunning(): Boolean = running.get()

    private fun isSessionActive(session: Int): Boolean =
        running.get() && generation.get() == session

    private fun receiveLoop(session: Int) {
        var localServer: ServerSocket? = null
        try {
            val server = bindServerSocket()
            localServer = server
            if (!isSessionActive(session)) {
                server.close()
                return
            }
            serverSocket = server
            val activePort = server.localPort
            Thread(
                { discoveryLoop(session, activePort) },
                "lankuu-discovery-responder",
            ).start()
            report("Receiving on port $activePort")
            while (isSessionActive(session)) {
                val socket = try {
                    server.accept()
                } catch (error: Exception) {
                    if (!isSessionActive(session)) break else throw error
                }
                handle(socket)
            }
        } catch (error: Exception) {
            if (isSessionActive(session)) {
                Log.e(LOG_TAG, "Receiver failed to start or stopped unexpectedly", error)
                report("Receiver error: ${error.userMessage()}")
            }
        } finally {
            runCatching { localServer?.close() }
            if (serverSocket === localServer) serverSocket = null
            if (generation.get() == session) running.set(false)
        }
    }

    private fun bindServerSocket(): ServerSocket {
        return try {
            newServerSocket(LanKuuProtocol.transferPort)
        } catch (error: BindException) {
            Log.w(LOG_TAG, "Port ${LanKuuProtocol.transferPort} is busy; selecting a free port", error)
            newServerSocket(0)
        }
    }

    private fun newServerSocket(port: Int): ServerSocket {
        val server = ServerSocket()
        return try {
            server.reuseAddress = true
            server.bind(InetSocketAddress(port))
            server
        } catch (error: Exception) {
            runCatching { server.close() }
            throw error
        }
    }

    private fun handle(socket: Socket) {
        socket.use {
            it.soTimeout = 30_000
            val input = DataInputStream(BufferedInputStream(it.getInputStream(), 1024 * 1024))
            val output = DataOutputStream(it.getOutputStream())
            try {
                val header = LanKuuProtocol.readHeader(input)
                when (header.kind) {
                    LanKuuProtocol.Kind.TEXT -> receiveText(input, header)
                    LanKuuProtocol.Kind.FILE -> receiveFile(input, header)
                }
                output.write(LanKuuProtocol.ackOk)
                output.flush()
            } catch (error: Exception) {
                runCatching {
                    output.write(LanKuuProtocol.ackError)
                    output.flush()
                }
                report("Transfer failed: ${error.userMessage()}")
            }
        }
    }

    private fun receiveText(input: DataInputStream, header: LanKuuProtocol.Header) {
        require(header.payloadLength <= LanKuuProtocol.maxTextBytes) { "Text payload is too large" }
        val bytes = ByteArray(header.payloadLength.toInt())
        input.readFully(bytes)
        report("Text received:\n${String(bytes, StandardCharsets.UTF_8)}")
    }

    private fun receiveFile(input: DataInputStream, header: LanKuuProtocol.Header) {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, LanKuuProtocol.safeFileName(header.name))
            put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
            put(MediaStore.MediaColumns.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/LanKuu")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: error("Could not create the destination file")
        try {
            resolver.openOutputStream(uri, "w")?.use { rawOutput ->
                val output = BufferedOutputStream(rawOutput, 1024 * 1024)
                LanKuuProtocol.copyExactly(input, output, header.payloadLength)
                output.flush()
            } ?: error("Could not open the destination file")

            val complete = ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }
            resolver.update(uri, complete, null, null)
            report("Received ${header.name} in Downloads/LanKuu")
        } catch (error: Exception) {
            resolver.delete(uri, null, null)
            throw error
        }
    }

    private fun discoveryLoop(session: Int, transferPort: Int) {
        var localSocket: DatagramSocket? = null
        try {
            val socket = DatagramSocket(null).also {
                it.reuseAddress = true
                it.bind(InetSocketAddress(LanKuuProtocol.discoveryPort))
                it.soTimeout = 300
            }
            localSocket = socket
            if (!isSessionActive(session)) {
                socket.close()
                return
            }
            discoverySocket = socket
            val buffer = ByteArray(128)
            val deviceName = Build.MODEL.replace('|', '_')
            val response = (
                "${LanKuuProtocol.discoveryResponsePrefix}|$deviceName|$transferPort"
            ).toByteArray(StandardCharsets.UTF_8)

            while (isSessionActive(session)) {
                val request = DatagramPacket(buffer, buffer.size)
                try {
                    socket.receive(request)
                    if (request.data.copyOf(request.length).contentEquals(LanKuuProtocol.discoveryQuery)) {
                        socket.send(DatagramPacket(response, response.size, request.address, request.port))
                    }
                } catch (_: SocketTimeoutException) {
                    // Re-check the running flag.
                }
            }
        } catch (error: Exception) {
            if (isSessionActive(session)) {
                Log.w(LOG_TAG, "Discovery responder is unavailable", error)
                report("Discovery unavailable: ${error.userMessage()}")
            }
        } finally {
            runCatching { localSocket?.close() }
            if (discoverySocket === localSocket) discoverySocket = null
        }
    }

    private companion object {
        const val LOG_TAG = "LanKuuReceiver"
    }
}

private fun Throwable.userMessage(): String = message ?: javaClass.simpleName
