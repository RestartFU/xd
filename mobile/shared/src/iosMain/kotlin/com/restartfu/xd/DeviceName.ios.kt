package com.restartfu.xd

import platform.UIKit.UIDevice

internal actual fun automaticDeviceName(): String =
    UIDevice.currentDevice.model.trim().ifEmpty { "iOS device" }
