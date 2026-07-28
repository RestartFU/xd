package com.restartfu.xd

import kotlin.test.Test
import kotlin.test.assertSame

class XdMobileTest {
    @Test
    fun sharedModuleLoads() {
        assertSame(XdMobile, XdMobile)
    }
}
