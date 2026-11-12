package com.restartfu.xd.mobile.ui

import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.createComposeRule
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [35])
class XdRootTest {
    @get:Rule
    val compose = createComposeRule()

    /**
     * Under glass the root is clear, and Material would hand black to text and
     * icons that take their color from it; the root gives them onSurface.
     */
    @Test fun contentTakesTheThemesTextColorEvenUnderGlass() {
        var theme by mutableStateOf(MinimalThemePreset.DARK)
        var content = Color.Unspecified
        var expected = Color.Unspecified
        compose.setContent {
            XdRoot(theme) {
                content = LocalContentColor.current
                expected = MaterialTheme.colorScheme.onSurface
            }
        }
        for (preset in MinimalThemePreset.entries) {
            theme = preset
            compose.waitForIdle()
            assertEquals("$preset", expected, content)
            assertEquals("$preset", minimalColors(preset).onSurface, content)
        }
    }
}
