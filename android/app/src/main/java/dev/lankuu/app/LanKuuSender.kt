package dev.lankuu.app

import android.content.ContentResolver
import android.database.Cursor
import android.net.Uri
import android.provider.OpenableColumns
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetSocketAddress
import java.net.Socket

internal class LanKuuSender(private val resolver: ContentResolver) {
    data class FileFailure(val name: String, val reason: String)

    data class BatchResult(
        val deliveredNames: List<String>,
        val failures: List<FileFailure>,
    )

    fun sendText(host: String, text: String) {
        val bytes = text.toByteArray(Charsets.UTF_8)
        require(bytes.size.toLong() <= LanKuuProtocol.maxTextBytes) { "Text is too large" }
        bytes.inputStream().use { input ->
            send(
                host,
                LanKuuProtocol.Header(LanKuuProtocol.Kind.TEXT, "message.txt", bytes.size.toLong()),
                input,
            )
        }
    }

    fun sendFile(host: String, uri: Uri): String {
        return sendFile(host, uri, fileDetails(uri))
    }

    fun sendFiles(
        host: String,
        uris: List<Uri>,
        onFileStarting: (index: Int, total: Int, name: String) -> Unit,
    ): BatchResult {
        require(uris.isNotEmpty()) { "Choose at least one file" }
        val delivered = mutableListOf<String>()
        val failures = mutableListOf<FileFailure>()

        uris.forEachIndexed { index, uri ->
            val details = runCatching { fileDetails(uri) }
            val name = details.getOrNull()?.first
                ?: LanKuuProtocol.safeFileName(uri.lastPathSegment ?: "selected-file")
            onFileStarting(index + 1, uris.size, name)
            details
                .mapCatching { sendFile(host, uri, it) }
                .onSuccess(delivered::add)
                .onFailure { error ->
                    failures += FileFailure(name, error.message ?: error.javaClass.simpleName)
                }
        }
        return BatchResult(delivered, failures)
    }

    private fun sendFile(host: String, uri: Uri, details: Pair<String, Long>): String {
        val (name, size) = details
        require(size >= 0) { "The selected file does not report its size" }
        resolver.openInputStream(uri)?.use { input ->
            send(host, LanKuuProtocol.Header(LanKuuProtocol.Kind.FILE, name, size), input)
        } ?: error("Could not open the selected file")
        return name
    }

    private fun send(
        host: String,
        header: LanKuuProtocol.Header,
        source: java.io.InputStream,
    ) {
        Socket().use { socket ->
            socket.connect(InetSocketAddress(host, LanKuuProtocol.transferPort), 8_000)
            socket.soTimeout = 30_000
            socket.tcpNoDelay = true
            val output = DataOutputStream(BufferedOutputStream(socket.getOutputStream(), 1024 * 1024))
            LanKuuProtocol.writeHeader(output, header)
            LanKuuProtocol.copyExactly(BufferedInputStream(source, 1024 * 1024), output, header.payloadLength)
            output.flush()
            socket.shutdownOutput()

            val input = DataInputStream(socket.getInputStream())
            val acknowledgment = ByteArray(2)
            input.readFully(acknowledgment)
            require(acknowledgment.contentEquals(LanKuuProtocol.ackOk)) {
                "The receiver rejected the transfer"
            }
        }
    }

    private fun fileDetails(uri: Uri): Pair<String, Long> {
        var name: String? = null
        var size = -1L
        val cursor: Cursor? = resolver.query(
            uri,
            arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE),
            null,
            null,
            null,
        )
        cursor?.use {
            if (it.moveToFirst()) {
                val nameIndex = it.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val sizeIndex = it.getColumnIndex(OpenableColumns.SIZE)
                if (nameIndex >= 0 && !it.isNull(nameIndex)) name = it.getString(nameIndex)
                if (sizeIndex >= 0 && !it.isNull(sizeIndex)) size = it.getLong(sizeIndex)
            }
        }
        if (size < 0) size = resolver.openAssetFileDescriptor(uri, "r")?.use { it.length } ?: -1L
        return LanKuuProtocol.safeFileName(name ?: "shared-file") to size
    }
}
