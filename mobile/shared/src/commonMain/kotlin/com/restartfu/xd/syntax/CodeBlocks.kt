package com.restartfu.xd.syntax

public enum class DiffKind {
    /** `diff --git`, `index`, `---`, `+++` and similar file headers. */
    META,

    /** An `@@ ... @@` hunk header. */
    HUNK,
    ADDED,
    REMOVED,
    CONTEXT,
}

/**
 * One line of a unified patch, already told apart and given the language of
 * the file it belongs to so it can be syntax coloured.
 *
 * [code] is the line without its `+`/`-`/space marker, which is what the
 * scanner should see; keeping the marker would make every added line start
 * with a stray operator.
 */
public data class DiffLine(
    val kind: DiffKind,
    val marker: String,
    val code: String,
    val language: SyntaxLanguage,
)

/** A run of prose, or a fenced code block, from an assistant message. */
public data class Segment(
    val code: Boolean,
    val text: String,
    val language: SyntaxLanguage = SyntaxLanguage.NONE,
)

public object CodeBlocks {
    private const val META_DIFF = "diff --git "
    private const val META_NEW = "+++ "
    private const val HUNK = "@@"

    /**
     * Splits a unified patch into classified lines.
     *
     * The language follows the `+++ b/path` header, so a patch touching
     * several files colours each one for what it actually is.
     */
    public fun diffLines(patch: String): List<DiffLine> {
        var language = SyntaxLanguage.NONE
        return patch.lines().map { line ->
            when {
                line.startsWith(META_NEW) -> {
                    language = Syntax.languageForPath(
                        line.removePrefix(META_NEW).removePrefix("b/").trim()
                            .takeUnless { it == "/dev/null" },
                    )
                    DiffLine(DiffKind.META, "", line, SyntaxLanguage.NONE)
                }
                line.startsWith(META_DIFF) ||
                    line.startsWith("--- ") ||
                    line.startsWith("index ") ||
                    line.startsWith("new file") ||
                    line.startsWith("deleted file") ||
                    line.startsWith("similarity index") ||
                    line.startsWith("rename ") ->
                    DiffLine(DiffKind.META, "", line, SyntaxLanguage.NONE)
                line.startsWith(HUNK) ->
                    DiffLine(DiffKind.HUNK, "", line, SyntaxLanguage.NONE)
                line.startsWith("+") ->
                    DiffLine(DiffKind.ADDED, "+", line.substring(1), language)
                line.startsWith("-") ->
                    DiffLine(DiffKind.REMOVED, "-", line.substring(1), language)
                line.startsWith(" ") ->
                    DiffLine(DiffKind.CONTEXT, " ", line.substring(1), language)
                else ->
                    DiffLine(DiffKind.CONTEXT, "", line, language)
            }
        }
    }

    /**
     * Splits assistant text into prose and fenced code.
     *
     * Only fences are recognised. The rest of Markdown is left alone, because
     * half-rendered Markdown reads worse than none.
     */
    public fun segments(text: String): List<Segment> {
        val segments = mutableListOf<Segment>()
        val buffer = StringBuilder()
        var fence: String? = null
        var language = SyntaxLanguage.NONE

        fun flush(code: Boolean) {
            if (buffer.isEmpty()) return
            segments += Segment(code, buffer.toString().removeSuffix("\n"), language)
            buffer.clear()
        }

        text.lines().forEach { line ->
            val trimmed = line.trimStart()
            val open = fence == null &&
                (trimmed.startsWith("```") || trimmed.startsWith("~~~"))
            val close = fence != null && trimmed.startsWith(fence!!)

            when {
                close -> {
                    flush(true)
                    fence = null
                    language = SyntaxLanguage.NONE
                }
                open -> {
                    flush(false)
                    fence = trimmed.take(3)
                    language = Syntax.languageForFence(trimmed.drop(3))
                }
                else -> buffer.append(line).append('\n')
            }
        }
        // An unterminated fence still shows as code: that is what it is.
        flush(fence != null)
        return segments
    }
}
