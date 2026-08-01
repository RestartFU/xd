package com.restartfu.xd.mobile

import android.app.Application
import com.restartfu.xd.XdClient
import com.restartfu.xd.credentials.AndroidCredentialStore
import com.restartfu.xd.net.AndroidSocketFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

class XdApplication : Application() {
    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    val client: XdClient by lazy {
        XdClient(
            socketFactory = AndroidSocketFactory(),
            credentials = AndroidCredentialStore(this),
            scope = applicationScope,
        )
    }
}
