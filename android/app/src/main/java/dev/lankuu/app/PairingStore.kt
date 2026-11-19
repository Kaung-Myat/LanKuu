package dev.lankuu.app

import android.content.Context

internal object PairingStore {
    private const val PREFERENCES = "lankuu-pairing"
    private const val TOKEN = "pairing-token"

    fun token(context: Context): String? = context.applicationContext
        .getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
        .getString(TOKEN, null)
        ?.takeIf { it.length in 32..LanKuuProtocol.maxPairingTokenBytes }

    fun save(context: Context, token: String) {
        require(token.length in 32..LanKuuProtocol.maxPairingTokenBytes)
        context.applicationContext
            .getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .putString(TOKEN, token)
            .apply()
    }
}
