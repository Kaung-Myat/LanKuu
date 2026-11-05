package dev.lankuu.app

import android.net.Uri

internal data class LanKuuConnection(
    val host: String,
    val transferPort: Int = LanKuuProtocol.transferPort,
    val mirrorPort: Int = MirrorProtocol.port,
    val name: String = "LanKuu Desktop",
) {
    val mirrorAddress: String
        get() = if (host.contains(':')) "[$host]:$mirrorPort" else "$host:$mirrorPort"

    companion object {
        fun fromQr(rawValue: String): LanKuuConnection? {
            val uri = runCatching { Uri.parse(rawValue.trim()) }.getOrNull() ?: return null
            if (uri.scheme != "lankuu" || uri.host != "connect") return null
            if (uri.getQueryParameter("v") != "1") return null

            val host = uri.getQueryParameter("host")?.trim().orEmpty()
            val transferPort = uri.getQueryParameter("transfer")?.toIntOrNull()
                ?: LanKuuProtocol.transferPort
            val mirrorPort = uri.getQueryParameter("mirror")?.toIntOrNull()
                ?: MirrorProtocol.port
            val name = uri.getQueryParameter("name")?.trim().orEmpty()
                .ifEmpty { "LanKuu Desktop" }

            if (!isLocalIpv4(host)) return null
            if (transferPort !in 1..65_535 || mirrorPort !in 1..65_535) return null
            return LanKuuConnection(host, transferPort, mirrorPort, name.take(80))
        }

        private fun isLocalIpv4(host: String): Boolean {
            val octets = host.split('.').map { part ->
                if (part.isEmpty() || part.length > 3 || part.any { !it.isDigit() }) return false
                part.toIntOrNull()?.takeIf { it in 0..255 } ?: return false
            }
            if (octets.size != 4) return false
            return octets[0] == 10 ||
                (octets[0] == 172 && octets[1] in 16..31) ||
                (octets[0] == 192 && octets[1] == 168) ||
                (octets[0] == 169 && octets[1] == 254)
        }
    }
}
