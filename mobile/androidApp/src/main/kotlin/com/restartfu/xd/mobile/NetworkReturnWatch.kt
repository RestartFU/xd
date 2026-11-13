package com.restartfu.xd.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network

/**
 * Calls [onReturn] each time the device gains a network while started. The
 * connection waits longer after each failed attempt, up to two minutes, so a
 * phone back in signal would otherwise sit offline until that wait ran out.
 * What [onReturn] does when nothing is waiting to reconnect is up to it.
 */
internal class NetworkReturnWatch(
    context: Context,
    private val onReturn: () -> Unit,
) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private var callback: ConnectivityManager.NetworkCallback? = null

    fun start() {
        if (callback != null || connectivity == null) return
        val watching = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = onReturn()
        }
        // Without the permission or the service there is only the backoff, as before.
        runCatching { connectivity.registerDefaultNetworkCallback(watching) }
            .onSuccess { callback = watching }
    }

    fun stop() {
        val watching = callback ?: return
        callback = null
        runCatching { connectivity?.unregisterNetworkCallback(watching) }
    }
}
