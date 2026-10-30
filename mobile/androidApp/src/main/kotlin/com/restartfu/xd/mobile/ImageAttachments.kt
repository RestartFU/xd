package com.restartfu.xd.mobile

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.os.Build
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import com.restartfu.xd.protocol.Limits
import com.restartfu.xd.protocol.PngAttachment
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** A chosen image, ready to send and to show in the composer. */
data class Attachment(
    val png: PngAttachment,
    val thumbnail: ImageBitmap,
)

/**
 * Turns a picked image into an attachment the host will accept.
 *
 * The host takes PNG only and validates the signature, but a phone gallery
 * holds JPEG and HEIC, so everything is decoded and re-encoded here. Matching
 * the desktop, images are scaled to fit 1920 first: a modern phone photo
 * encoded to PNG at full resolution runs to tens of megabytes and would be
 * refused outright.
 */
object ImageAttachments {
    private const val MAX_DIMENSION = 1920

    suspend fun load(context: Context, uri: Uri): Attachment =
        withContext(Dispatchers.IO) {
            val bitmap = decode(context, uri)
            try {
                val scaled = scale(bitmap, MAX_DIMENSION)
                val png = encode(scaled, bitmap)
                Attachment(PngAttachment(png), thumbnail(scaled).asImageBitmap())
            } finally {
                bitmap.recycle()
            }
        }

    suspend fun fromPng(png: PngAttachment): Attachment =
        withContext(Dispatchers.Default) {
            Limits.validateImages(listOf(png))
            val bitmap = BitmapFactory.decodeByteArray(png.bytes, 0, png.bytes.size)
                ?: error("That synchronized image could not be decoded")
            val preview = thumbnail(bitmap)
            if (preview !== bitmap) bitmap.recycle()
            Attachment(png, preview.asImageBitmap())
        }

    private fun decode(context: Context, uri: Uri): Bitmap =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            // ImageDecoder applies EXIF orientation itself.
            val source = ImageDecoder.createSource(context.contentResolver, uri)
            ImageDecoder.decodeBitmap(source) { decoder, _, _ ->
                decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                decoder.isMutableRequired = true
            }
        } else {
            val decoded = context.contentResolver.openInputStream(uri).use { stream ->
                BitmapFactory.decodeStream(stream)
            } ?: error("That image could not be read")
            orient(context, uri, decoded)
        }

    /** Older releases decode without EXIF, so a portrait photo arrives sideways. */
    private fun orient(context: Context, uri: Uri, bitmap: Bitmap): Bitmap {
        val orientation = context.contentResolver.openInputStream(uri).use { stream ->
            stream?.let { ExifInterface(it).getAttributeInt(
                ExifInterface.TAG_ORIENTATION,
                ExifInterface.ORIENTATION_NORMAL,
            ) }
        } ?: ExifInterface.ORIENTATION_NORMAL

        val matrix = Matrix()
        when (orientation) {
            ExifInterface.ORIENTATION_ROTATE_90 -> matrix.postRotate(90f)
            ExifInterface.ORIENTATION_ROTATE_180 -> matrix.postRotate(180f)
            ExifInterface.ORIENTATION_ROTATE_270 -> matrix.postRotate(270f)
            ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> matrix.postScale(-1f, 1f)
            ExifInterface.ORIENTATION_FLIP_VERTICAL -> matrix.postScale(1f, -1f)
            else -> return bitmap
        }
        val rotated = Bitmap.createBitmap(
            bitmap, 0, 0, bitmap.width, bitmap.height, matrix, true,
        )
        if (rotated !== bitmap) bitmap.recycle()
        return rotated
    }

    private fun scale(bitmap: Bitmap, limit: Int): Bitmap {
        val longest = maxOf(bitmap.width, bitmap.height)
        if (longest <= limit) return bitmap
        val ratio = limit.toFloat() / longest
        return Bitmap.createScaledBitmap(
            bitmap,
            (bitmap.width * ratio).toInt().coerceAtLeast(1),
            (bitmap.height * ratio).toInt().coerceAtLeast(1),
            true,
        )
    }

    /**
     * Encodes to PNG, halving the image until it fits.
     *
     * PNG is lossless, so quality cannot be traded for size the way it can
     * with JPEG. A screenshot of a photograph can still exceed 10 MiB at 1920,
     * and being refused by the host is a worse outcome than being smaller.
     */
    private fun encode(scaled: Bitmap, original: Bitmap): ByteArray {
        var current = scaled
        var limit = MAX_DIMENSION
        while (true) {
            val bytes = ByteArrayOutputStream().use { stream ->
                current.compress(Bitmap.CompressFormat.PNG, 100, stream)
                stream.toByteArray()
            }
            if (bytes.size <= Limits.MAX_IMAGE_BYTES || limit <= 320) {
                check(bytes.size <= Limits.MAX_IMAGE_BYTES) {
                    "That image is too large to send"
                }
                return bytes
            }
            limit /= 2
            val smaller = scale(original, limit)
            if (current !== scaled && current !== original) current.recycle()
            current = smaller
        }
    }

    private fun thumbnail(bitmap: Bitmap): Bitmap = scale(bitmap, THUMBNAIL)

    private const val THUMBNAIL = 256
}
