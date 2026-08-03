package com.restartfu.xd.model

/** The visible part of an assistant response after presentation wrappers. */
public enum class AssistantSectionKind {
    NORMAL,
    ANALYSIS,
}

public data class AssistantSection(
    val kind: AssistantSectionKind,
    val text: String,
)

/**
 * Parses the narrow response wrappers used for textual analysis and summaries.
 *
 * Only exact, lower-case tags on their own lines are structural. Fenced code
 * wins over wrapper parsing so examples in code remain literal.
 */
public object AssistantSections {
    private const val OPEN_ANALYSIS = "<analysis>"
    private const val CLOSE_ANALYSIS = "</analysis>"
    private const val OPEN_SUMMARY = "<summary>"
    private const val CLOSE_SUMMARY = "</summary>"

    private data class Block(
        val startLine: Int,
        val finishLine: Int,
        val kind: AssistantSectionKind,
    )

    private enum class StreamMode {
        NORMAL,
        ANALYSIS,
        SUMMARY,
    }

    public fun parse(text: String): List<AssistantSection> {
        if (text.isEmpty()) return emptyList()

        val lines = text.split('\n')
        val blocks = mutableListOf<Block>()
        var active: Pair<Int, AssistantSectionKind>? = null
        var inFence = false

        lines.forEachIndexed { index, line ->
            val marker = line.trimEnd('\r').trim()
            if (marker.startsWith("```")) {
                inFence = !inFence
                return@forEachIndexed
            }
            if (inFence) return@forEachIndexed

            when (marker) {
                OPEN_ANALYSIS -> {
                    if (active == null) {
                        active = index to AssistantSectionKind.ANALYSIS
                    } else {
                        // Nested or mismatched wrappers stay literal.
                        active = null
                    }
                }

                OPEN_SUMMARY -> {
                    if (active == null) {
                        active = index to AssistantSectionKind.NORMAL
                    } else {
                        active = null
                    }
                }

                CLOSE_ANALYSIS -> {
                    val current = active
                    if (current != null) {
                        if (current.second == AssistantSectionKind.ANALYSIS) {
                            blocks += Block(current.first, index, current.second)
                        }
                        active = null
                    }
                }

                CLOSE_SUMMARY -> {
                    val current = active
                    if (current != null) {
                        if (current.second == AssistantSectionKind.NORMAL) {
                            blocks += Block(current.first, index, current.second)
                        }
                        active = null
                    }
                }
            }
        }

        if (blocks.isEmpty()) return listOf(AssistantSection(AssistantSectionKind.NORMAL, text))

        val sections = mutableListOf<AssistantSection>()
        var cursor = 0
        blocks.sortedBy { it.startLine }.forEach { block ->
            if (block.startLine < cursor) return@forEach
            appendNormal(sections, lines.subList(cursor, block.startLine).joinToString("\n"))
            sections += AssistantSection(
                block.kind,
                lines.subList(block.startLine + 1, block.finishLine).joinToString("\n"),
            )
            cursor = block.finishLine + 1
        }
        if (cursor <= lines.size) {
            appendNormal(sections, lines.subList(cursor, lines.size).joinToString("\n"))
        }
        return sections
    }

    /**
     * Projects a live response into plain text without exposing wrapper tags.
     * Analysis is withheld until the completed response can show its disclosure;
     * summary content remains visible while it streams.
     */
    public fun stream(text: String): String {
        if (text.isEmpty()) return text

        val output = mutableListOf<String>()
        var mode = StreamMode.NORMAL
        var inFence = false
        val lines = text.split('\n')
        lines.forEachIndexed { index, line ->
            val marker = line.trimEnd('\r').trim()
            if (marker.startsWith("```")) {
                inFence = !inFence
                if (mode != StreamMode.ANALYSIS) output += line
                return@forEachIndexed
            }

            if (!inFence) {
                if (index == lines.lastIndex && partialTag(marker)) {
                    return@forEachIndexed
                }
                when (mode) {
                    StreamMode.NORMAL -> when (marker) {
                        OPEN_ANALYSIS -> {
                            mode = StreamMode.ANALYSIS
                            return@forEachIndexed
                        }

                        OPEN_SUMMARY -> {
                            mode = StreamMode.SUMMARY
                            return@forEachIndexed
                        }

                        else -> Unit
                    }

                    StreamMode.ANALYSIS -> {
                        if (marker == CLOSE_ANALYSIS) mode = StreamMode.NORMAL
                        return@forEachIndexed
                    }

                    StreamMode.SUMMARY -> {
                        if (marker == CLOSE_SUMMARY) mode = StreamMode.NORMAL
                        if (marker == CLOSE_SUMMARY) return@forEachIndexed
                    }
                }
            }

            if (mode != StreamMode.ANALYSIS) output += line
        }
        return output.joinToString("\n")
    }

    private fun partialTag(marker: String): Boolean {
        if (marker.isEmpty()) return false
        return listOf(OPEN_ANALYSIS, CLOSE_ANALYSIS, OPEN_SUMMARY, CLOSE_SUMMARY)
            .any { it.startsWith(marker) }
    }

    private fun appendNormal(sections: MutableList<AssistantSection>, text: String) {
        val normalized = text.trim()
        if (normalized.isNotEmpty()) {
            sections += AssistantSection(AssistantSectionKind.NORMAL, normalized)
        }
    }
}
