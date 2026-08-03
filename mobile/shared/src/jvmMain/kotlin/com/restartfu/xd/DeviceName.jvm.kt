package com.restartfu.xd

import java.net.InetAddress

internal actual fun automaticDeviceName(): String =
    runCatching { InetAddress.getLocalHost().hostName.trim() }
        .getOrNull()
        ?.takeUnless { it.isEmpty() }
        ?: "JVM device"
