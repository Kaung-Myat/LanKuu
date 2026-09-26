package dev.lankuu.app

import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.InputStream
import java.io.OutputStream
import java.nio.charset.StandardCharsets

internal object LanKuuProtocol {
    val magic: ByteArray = "LANKUU01".toByteArray(StandardCharsets.US_ASCII)
    val discoveryQuery: ByteArray = "LANKUU_DISCOVER_V1".toByteArray(StandardCharsets.US_ASCII)
    const val discoveryResponsePrefix = "LANKUU_HERE_V1"
    const val transferPort = 45_454
    const val discoveryPort = 45_455
    const val maxNameBytes = 1_024
    const val maxTextBytes = 16L * 1_024L * 1_024L
    val ackOk: ByteArray = "OK".toByteArray(StandardCharsets.US_ASCII)
    val ackError: ByteArray = "ER".toByteArray(StandardCharsets.US_ASCII)

    enum class Kind(val wireValue: Int) {
        TEXT(1),
        FILE(2);

        companion object {
            fun fromWire(value: Int): Kind = entries.firstOrNull { it.wireValue == value }
                ?: throw IllegalArgumentException("Unsupported payload kind: $value")
        }
    }

    data class Header(val kind: Kind, val name: String, val payloadLength: Long)

    fun writeHeader(output: DataOutputStream, header: Header) {
        val name = header.name.toByteArray(StandardCharsets.UTF_8)
        require(name.isNotEmpty() && name.size <= maxNameBytes) { "Invalid payload name" }
        require(header.payloadLength >= 0) { "Invalid payload length" }

        output.write(magic)
        output.writeByte(header.kind.wireValue)
        output.writeShort(name.size)
        output.writeLong(header.payloadLength)
        output.write(name)
    }

    fun readHeader(input: DataInputStream): Header {
        val receivedMagic = ByteArray(magic.size)
        input.readFully(receivedMagic)
        require(receivedMagic.contentEquals(magic)) { "Not a LanKuu v1 message" }

        val kind = Kind.fromWire(input.readUnsignedByte())
        val nameLength = input.readUnsignedShort()
        require(nameLength in 1..maxNameBytes) { "Invalid payload name length" }
        val payloadLength = input.readLong()
        require(payloadLength >= 0) { "Invalid payload length" }
        val name = ByteArray(nameLength)
        input.readFully(name)
        return Header(kind, String(name, StandardCharsets.UTF_8), payloadLength)
    }

    fun copyExactly(input: InputStream, output: OutputStream, expected: Long): Long {
        var remaining = expected
        var copied = 0L
        val buffer = ByteArray(1024 * 1024)
        while (remaining > 0) {
            val count = input.read(buffer, 0, minOf(buffer.size.toLong(), remaining).toInt())
            if (count < 0) throw EOFException("Expected $expected bytes but received $copied")
            output.write(buffer, 0, count)
            remaining -= count
            copied += count
        }
        return copied
    }

    fun safeFileName(raw: String): String {
        val component = raw.substringAfterLast('/').substringAfterLast('\\')
        val cleaned = component.map { character ->
            if (character.isISOControl() || character == '/' || character == '\\') '_' else character
        }.joinToString("")
        return if (cleaned.isBlank() || cleaned == "." || cleaned == "..") "received.bin" else cleaned
    }
}
