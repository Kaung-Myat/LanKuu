package dev.lankuu.app

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicInteger

internal object MirrorProtocol {
    const val port = 45_456
    const val headerSize = 22
    const val maxPayload = 1_150
    const val flagConfig = 1
    const val flagKeyFrame = 2
    private val magic = byteArrayOf('L'.code.toByte(), 'K'.code.toByte(), 'S'.code.toByte(), 'C'.code.toByte())

    class Sender(host: String, private val destinationPort: Int = port) : AutoCloseable {
        private val destination = InetAddress.getByName(host)
        private val socket = DatagramSocket().apply {
            sendBufferSize = 2 * 1024 * 1024
        }
        private val nextFrameId = AtomicInteger(1)

        fun sendFrame(data: ByteArray, presentationTimeUs: Long, flags: Int) {
            if (data.isEmpty()) return
            val frameId = nextFrameId.getAndIncrement()
            val chunkCount = (data.size + maxPayload - 1) / maxPayload
            require(chunkCount <= 65_535) { "Encoded frame is too large" }

            var offset = 0
            for (chunkIndex in 0 until chunkCount) {
                val payloadSize = minOf(maxPayload, data.size - offset)
                val packetBytes = ByteArray(headerSize + payloadSize)
                val header = ByteBuffer.wrap(packetBytes).order(ByteOrder.BIG_ENDIAN)
                header.put(magic)
                header.put(1)
                header.put(flags.toByte())
                header.putInt(frameId)
                header.putShort(chunkIndex.toShort())
                header.putShort(chunkCount.toShort())
                header.putLong(presentationTimeUs)
                System.arraycopy(data, offset, packetBytes, headerSize, payloadSize)
                socket.send(DatagramPacket(packetBytes, packetBytes.size, destination, destinationPort))
                offset += payloadSize
            }
        }

        override fun close() = socket.close()
    }
}
