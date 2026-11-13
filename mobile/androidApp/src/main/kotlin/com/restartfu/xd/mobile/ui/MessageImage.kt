package com.restartfu.xd.mobile.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.material3.FilledTonalIconButton
import androidx.compose.material3.Icon
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.window.DialogProperties
import com.restartfu.xd.mobile.R
import kotlin.math.roundToInt
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * About what a phone can show, and small enough to draw. A message composes all
 * of its images at once, and a reply can name dozens of them.
 */
private const val MAX_BITMAP_SIDE = 1280

/**
 * Decodes at a size that fits. A small file can still describe an enormous
 * picture, and a bitmap too large to draw takes the whole app down.
 */
private fun decodeBounded(bytes: ByteArray): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (maxOf(bounds.outWidth, bounds.outHeight) / sample > MAX_BITMAP_SIDE) sample *= 2
    val options = BitmapFactory.Options().apply { inSampleSize = sample }
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
}

/**
 * Images already decoded, so one scrolled back into view is not fetched and
 * decoded again. Bounded by the bitmaps' size, not their number.
 */
private val decodedImages = object : android.util.LruCache<String, ImageBitmap>(24 * 1024 * 1024) {
    override fun sizeOf(key: String, value: ImageBitmap): Int = value.width * value.height * 4
}

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
    // Paths are the host's, and chats are the host's too, so the chat tells hosts apart.
    val cacheKey = "${model.state.value.chatId}\u0000$path"
    var state: Loaded by remember(cacheKey) {
        mutableStateOf(decodedImages.get(cacheKey)?.let(Loaded::Ready) ?: Loaded.Loading)
    }
    // Saved, so the picture stays open when the phone is turned to look at it.
    var zoomed by rememberSaveable(path) { mutableStateOf(false) }

    LaunchedEffect(cacheKey) {
        if (state is Loaded.Ready) return@LaunchedEffect
        state = try {
            val bytes = model.session.readImage(path)
            val bitmap = withContext(Dispatchers.Default) { decodeBounded(bytes)?.asImageBitmap() }
            if (bitmap == null) {
                Loaded.Failed("That image could not be decoded")
            } else {
                decodedImages.put(cacheKey, bitmap)
                Loaded.Ready(bitmap)
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
            // Named by its file: the picture itself cannot be put into words here.
            val described = "Image ${path.substringAfterLast('/')}"
            Image(
                bitmap = current.bitmap,
                contentDescription = described,
                modifier = modifier
                    .fillMaxWidth()
                    // Bounded, so a tall screenshot cannot push the rest of
                    // the turn off the screen.
                    .heightIn(max = 260.dp)
                    .clip(MaterialTheme.shapes.small)
                    .clickable(onClickLabel = "View larger") { zoomed = true },
                contentScale = ContentScale.Fit,
            )
            if (zoomed) ImageViewer(current.bitmap, described) { zoomed = false }
        }
    }
}

/**
 * A picture on its own, over the whole screen: pinch or double-tap to zoom,
 * drag to move around it once zoomed, and Close or Back to leave.
 */
@Composable
private fun ImageViewer(bitmap: ImageBitmap, description: String, onClose: () -> Unit) {
    Dialog(
        onDismissRequest = onClose,
        properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
    ) {
        // Its own window, so its bars are set here: the activity's do not reach it.
        DialogSystemBars()
        ImageViewerContent(bitmap, description, onClose)
    }
}

/** The viewer's picture, for tests. */
internal const val IMAGE_VIEWER_TAG = "image-viewer"

@Composable
internal fun ImageViewerContent(bitmap: ImageBitmap, description: String, onClose: () -> Unit) {
    var zoom by remember { mutableStateOf(ImageZoom()) }
    var size by remember { mutableStateOf(IntSize.Zero) }
    // The theme's own ground, said outright: a dialog starts with no surface under it.
    Surface(
        Modifier.fillMaxSize(),
        color = LocalXdGlass.current.backdrop,
        contentColor = MaterialTheme.colorScheme.onSurface,
    ) {
        Box(Modifier.fillMaxSize()) {
            Image(
                bitmap = bitmap,
                contentDescription = description,
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .fillMaxSize()
                    .testTag(IMAGE_VIEWER_TAG)
                    .onSizeChanged { size = it }
                    // Zooming without a pinch, for a switch, a keyboard or a screen reader.
                    .semantics {
                        stateDescription = zoom.label
                        customActions = listOf(
                            CustomAccessibilityAction("Zoom in") {
                                zoom = zoom.moved(1.5f, 0f, 0f, size.width, size.height)
                                true
                            },
                            CustomAccessibilityAction("Zoom out") {
                                zoom = zoom.moved(1 / 1.5f, 0f, 0f, size.width, size.height)
                                true
                            },
                        )
                    }
                    .pointerInput(Unit) {
                        detectTransformGestures { _, pan, zoomBy, _ ->
                            zoom = zoom.moved(zoomBy, pan.x, pan.y, size.width, size.height)
                        }
                    }
                    .pointerInput(Unit) {
                        detectTapGestures(
                            onDoubleTap = { zoom = if (zoom.scale > 1f) ImageZoom() else ImageZoom(DOUBLE_TAP_ZOOM) },
                        )
                    }
                    .graphicsLayer {
                        scaleX = zoom.scale
                        scaleY = zoom.scale
                        translationX = zoom.x
                        translationY = zoom.y
                    },
            )
            // On a tone of its own, so it reads over whatever part of the picture is under it.
            FilledTonalIconButton(
                onClick = onClose,
                modifier = Modifier
                    .align(Alignment.TopStart)
                    .windowInsetsPadding(WindowInsets.safeDrawing)
                    .padding(8.dp)
                    .size(48.dp),
            ) {
                Icon(painterResource(R.drawable.ic_close), contentDescription = "Close", modifier = Modifier.size(20.dp))
            }
        }
    }
}

/** How far a double tap zooms in. */
private const val DOUBLE_TAP_ZOOM = 2.5f

/**
 * How far into a picture the viewer is, and where: [scale] from the picture
 * fitted to the screen (1) to [MAX_SCALE], and how far its middle has been
 * dragged from the screen's, in pixels.
 */
internal data class ImageZoom(val scale: Float = 1f, val x: Float = 0f, val y: Float = 0f) {
    /** Said to a screen reader: the picture looks the same to it at any zoom. */
    val label: String get() = if (scale == 1f) "Fitted to the screen" else "Zoomed to ${(scale * 100).roundToInt()}%"

    /**
     * This, zoomed by [zoomBy] and dragged by [panX] and [panY] on a screen
     * [width] by [height]. Zoom stays between fitted and [MAX_SCALE], and the
     * drag stops where the picture's edge would leave the screen's, so it can
     * never be lost off to one side; fitted, it does not move at all.
     */
    fun moved(zoomBy: Float, panX: Float, panY: Float, width: Int, height: Int): ImageZoom {
        val next = (scale * zoomBy).coerceIn(1f, MAX_SCALE)
        val reachX = width * (next - 1f) / 2f
        val reachY = height * (next - 1f) / 2f
        return ImageZoom(next, within(x + panX, reachX), within(y + panY, reachY))
    }

    /** [offset] no further than [reach] either way; with no reach, exactly the middle. */
    private fun within(offset: Float, reach: Float): Float = if (reach <= 0f) 0f else offset.coerceIn(-reach, reach)

    companion object {
        const val MAX_SCALE = 5f
    }
}
