package dev.lankuu.app

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.SocketTimeoutException
import java.nio.charset.StandardCharsets

internal object LanKuuDiscovery {
    data class Device(val name: String, val host: String, val port: Int)

    fun discover(timeoutMillis: Long = 2_000): List<Device> {
        val found = linkedMapOf<String, Device>()
        DatagramSocket().use { socket ->
            socket.broadcast = true
            socket.soTimeout = 250
            val broadcast = InetAddress.getByName("255.255.255.255")
            socket.send(
                DatagramPacket(
                    LanKuuProtocol.discoveryQuery,
                    LanKuuProtocol.discoveryQuery.size,
                    broadcast,
                    LanKuuProtocol.discoveryPort,
                ),
            )

            val deadline = System.currentTimeMillis() + timeoutMillis
            val buffer = ByteArray(512)
            while (System.currentTimeMillis() < deadline) {
                val packet = DatagramPacket(buffer, buffer.size)
                try {
                    socket.receive(packet)
                    val message = String(packet.data, 0, packet.length, StandardCharsets.UTF_8)
                    parse(message, packet.address.hostAddress ?: continue)?.let { device ->
                        found["${device.host}:${device.port}"] = device
                    }
                } catch (_: SocketTimeoutException) {
                    // Keep collecting until the overall deadline.
                }
            }
        }
        return found.values.toList()
    }

    private fun parse(message: String, host: String): Device? {
        val fields = message.split('|')
        if (fields.size != 3 || fields[0] != LanKuuProtocol.discoveryResponsePrefix) return null
        val port = fields[2].toIntOrNull() ?: return null
        return Device(fields[1], host, port)
    }
}
