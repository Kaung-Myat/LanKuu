package dev.lankuu.app

internal data class MirrorEndpoint(val host: String, val port: Int) {
    companion object {
        fun parse(rawValue: String): MirrorEndpoint? {
            val value = rawValue.trim()
            if (value.isEmpty()) return null

            if (value.startsWith("[")) {
                val closingBracket = value.indexOf(']')
                if (closingBracket <= 1) return null
                val host = value.substring(1, closingBracket)
                val suffix = value.substring(closingBracket + 1)
                val port = when {
                    suffix.isEmpty() -> MirrorProtocol.port
                    suffix.startsWith(":") -> suffix.substring(1).toIntOrNull()
                    else -> null
                } ?: return null
                return if (port in 1..65_535) MirrorEndpoint(host, port) else null
            }

            if (value.count { it == ':' } == 1) {
                val separator = value.lastIndexOf(':')
                val host = value.substring(0, separator).trim()
                val port = value.substring(separator + 1).toIntOrNull() ?: return null
                return if (host.isNotEmpty() && port in 1..65_535) MirrorEndpoint(host, port) else null
            }

            return MirrorEndpoint(value, MirrorProtocol.port)
        }
    }
}
