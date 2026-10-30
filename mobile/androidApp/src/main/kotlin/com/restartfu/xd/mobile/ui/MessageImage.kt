package com.restartfu.xd.mobile.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import com.restartfu.xd.mobile.ChatViewModel
import kotlinx.coroutines.CancellationException

private sealed interface Loaded {
    data object Loading : Loaded
    data class Ready(val bitmap: ImageBitmap) : Loaded
    data class Failed(val message: String) : Loaded
}

/**
 * An image a message carries, fetched from the host.
 *
 * The bytes live on the host, not the phone, so the transcript asks for
 * them. A scaled preview is requested: a transcript never needs full
 * resolution, and tapping opens the same preview full screen.
 */
@Composable
internal fun MessageImage(
    model: ChatViewModel,
    path: String,
    modifier: Modifier = Modifier,
) {
    var state: Loaded by remember(path) { mutableStateOf(Loaded.Loading) }
    var zoomed by remember(path) { mutableStateOf(false) }

    LaunchedEffect(path) {
        state = try {
            val bytes = model.session.readImage(path)
            val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            if (bitmap == null) {
                Loaded.Failed("That image could not be decoded")
            } else {
                Loaded.Ready(bitmap.asImageBitmap())
            }
        } catch (error: CancellationException) {
            throw error
        } catch (error: Throwable) {
            // An image sent from another machine, or cleaned up since, is a
            // normal outcome rather than a failure of the transcript.
            Loaded.Failed(error.message ?: "That image is unavailable")
        }
    }

    when (val current = state) {
        Loaded.Loading -> Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = MaterialTheme.shapes.small,
            modifier = modifier.fillMaxWidth(),
        ) {
            Box(
                Modifier
                    .heightIn(min = 96.dp)
                    .fillMaxWidth(),
                contentAlignment = Alignment.Center,
            ) { CircularProgressIndicator() }
        }

        is Loaded.Failed -> Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = MaterialTheme.shapes.small,
            modifier = modifier.fillMaxWidth(),
        ) {
            Text(
                current.message,
                modifier = Modifier.padding(12.dp),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.outline,
            )
        }

        is Loaded.Ready -> {
            Image(
                bitmap = current.bitmap,
                contentDescription = "Attached image",
                modifier = modifier
                    .fillMaxWidth()
                    // Bounded, so a tall screenshot cannot push the rest of
                    // the turn off the screen.
                    .heightIn(max = 260.dp)
                    .clip(MaterialTheme.shapes.small)
                    .clickable { zoomed = true },
                contentScale = ContentScale.Fit,
            )
            if (zoomed) {
                Dialog(onDismissRequest = { zoomed = false }) {
                    Image(
                        bitmap = current.bitmap,
                        contentDescription = "Attached image",
                        modifier = Modifier
                            .fillMaxSize()
                            .clickable { zoomed = false },
                        contentScale = ContentScale.Fit,
                    )
                }
            }
        }
    }
}
