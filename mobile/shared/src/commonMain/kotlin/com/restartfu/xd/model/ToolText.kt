package com.restartfu.xd.model

/**
 * Reads the `tool` payloads the daemon stores.
 *
 * A tool row carrying an inline diff is the text `file_change`, a newline, and
 * a unified patch -- the same marker the daemon writes and the desktop reads.
 * Everything else is free text whose first line reads as
 * a summary.
 *
 * This lives in the shared module because it is protocol knowledge, not
 * presentation: an iOS client needs the identical rules.
 */
public object ToolText {
    private const val FILE_CHANGE_PREFIX = "file_change\n"
    private const val PATCH_MARKER = "diff --git "
    private const val FILE_CHANGE_TOKEN = "file_change"

    /** The unified patch when this row is an inline diff, else null. */
    public fun patch(text: String): String? {
        if (!text.startsWith(FILE_CHANGE_PREFIX)) return null
        val patch = text.substring(FILE_CHANGE_PREFIX.length)
        return patch.takeIf { it.startsWith(PATCH_MARKER) }
    }

    /**
     * The `b/` paths a patch touches, in order.
     *
     * A rename reports its destination, which is what the user is looking for
     * when scanning what changed.
     */
    public fun changedFiles(patch: String): List<String> =
        patch.lineSequence()
            .filter { it.startsWith(PATCH_MARKER) }
            .mapNotNull { line ->
                val rest = line.removePrefix(PATCH_MARKER)
                val marker = rest.indexOf(" b/")
                if (marker < 0) null else rest.substring(marker + 3).trim()
            }
            .filter { it.isNotEmpty() }
            .toList()

    /** One line describing the row, for a collapsed header. */
    public fun summary(text: String): String {
        patch(text)?.let { patch ->
            val files = changedFiles(patch)
            return when (files.size) {
                0 -> "Edited files"
                // Full repo paths rarely fit on a phone. The expanded patch
                // still carries the path when its directory matters.
                1 -> files.single().substringAfterLast('/')
                else -> "${files.size} files changed"
            }
        }
        if (text == FILE_CHANGE_TOKEN || text.startsWith("$FILE_CHANGE_TOKEN  ")) {
            return "Edited files"
        }
        return text.lineSequence().firstOrNull { it.isNotBlank() }?.trim().orEmpty()
    }

    /**
     * The detail hidden behind a collapsed header, or null when the summary
     * already says everything.
     *
     * A one-line tool row has nothing worth a disclosure control.
     */
    public fun detail(text: String): String? {
        patch(text)?.let { return it }
        val body = text.trimEnd()
        val firstBreak = body.indexOf('\n')
        if (firstBreak < 0) return null
        return body.takeIf { it.substring(firstBreak).isNotBlank() }
    }
}
