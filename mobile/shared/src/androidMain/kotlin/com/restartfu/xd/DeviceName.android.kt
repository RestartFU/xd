package com.restartfu.xd

import android.os.Build

internal actual fun automaticDeviceName(): String =
    Build.MODEL?.trim().orEmpty().ifEmpty { "Android device" }
