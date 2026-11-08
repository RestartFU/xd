package com.restartfu.xd.mobile

import android.app.Application
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.AndroidCredentialStore
import com.restartfu.xd.net.AndroidDaemonSocketFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

open class XdApplication : Application() {
    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val settings: MobileSettings by lazy { MobileSettings(this) }

    val client: XdClient by lazy { createClient(applicationScope) }

    /** How the app reaches its host; screen tests answer from a script instead. */
    protected open fun createClient(scope: CoroutineScope): XdClient = XdClient(
        socketFactory = AndroidDaemonSocketFactory(),
        credentials = AndroidCredentialStore(this),
        scope = scope,
    )
}
