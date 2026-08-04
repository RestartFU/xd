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

/** One independently collapsible file in a unified patch. */
public data class DiffFile(
    val path: String,
    val lines: List<DiffLine>,
    val additions: Int,
    val deletions: Int,
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
     * Groups a patch by its `diff --git` file headers. Patches without those
     * headers still produce one useful section instead of disappearing.
     */
    public fun diffFiles(patch: String): List<DiffFile> {
        if (patch.isBlank()) return emptyList()

        val lines = diffLines(patch)
        val starts = lines.indices.filter { lines[it].code.startsWith(META_DIFF) }
        if (starts.isEmpty()) return listOf(diffFile("Changes", lines))

        return starts.mapIndexed { index, start ->
            val finish = starts.getOrNull(index + 1) ?: lines.size
            val header = lines[start].code
            // The tappable section header replaces this raw plumbing line.
            val body = lines.subList(start + 1, finish)
            diffFile(fileTitle(header), body)
        }
    }

    private fun diffFile(path: String, lines: List<DiffLine>): DiffFile = DiffFile(
        path = path,
        lines = lines,
        additions = lines.count { it.kind == DiffKind.ADDED },
        deletions = lines.count { it.kind == DiffKind.REMOVED },
    )

    private fun fileTitle(header: String): String {
        val plainTarget = header.lastIndexOf(" b/")
        if (plainTarget >= 0) return header.substring(plainTarget + 3)

        val quotedTarget = header.lastIndexOf(" \"b/")
        if (quotedTarget >= 0) return header.substring(quotedTarget + 4).removeSuffix("\"")

        return header.removePrefix(META_DIFF)
    }
}
