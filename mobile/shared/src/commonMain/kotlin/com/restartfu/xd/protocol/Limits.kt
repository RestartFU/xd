package com.restartfu.xd.protocol

public object Limits {
    public const val MAX_IMAGES: Int = 4
    public const val MAX_IMAGE_BYTES: Int = 10 * 1024 * 1024
    public const val MAX_IMAGES_BYTES: Int = 20 * 1024 * 1024
    public const val PNG_MIME: String = "image/png"

    private val pngSignature = byteArrayOf(
        0x89.toByte(),
        0x50,
        0x4e,
        0x47,
        0x0d,
        0x0a,
        0x1a,
        0x0a,
    )

    public fun validateImages(images: List<PngAttachment>) {
        require(images.size <= MAX_IMAGES) { "A message can contain at most 4 images" }
        var total = 0
        for (image in images) {
            require(image.bytes.size <= MAX_IMAGE_BYTES) {
                "PNG images must not exceed 10 MiB"
            }
            require(
                image.bytes.size >= pngSignature.size &&
                    pngSignature.indices.all { image.bytes[it] == pngSignature[it] },
            ) {
                "Attachment is not a PNG"
            }
            require(total <= MAX_IMAGES_BYTES - image.bytes.size) {
                "Attached images must not exceed 20 MiB"
            }
            total += image.bytes.size
        }
    }

    public fun validateEncodedImageLength(length: Int) {
        val limit = ((MAX_IMAGE_BYTES + 2) / 3) * 4
        require(length <= limit) { "Encoded PNG exceeds the wire limit" }
    }
}

public data class PngAttachment(
    val bytes: ByteArray,
)
