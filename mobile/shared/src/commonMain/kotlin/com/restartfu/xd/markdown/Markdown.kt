package com.restartfu.xd.markdown

import com.restartfu.xd.syntax.Syntax
import com.restartfu.xd.syntax.SyntaxLanguage
import org.intellij.markdown.MarkdownElementTypes
import org.intellij.markdown.MarkdownTokenTypes
import org.intellij.markdown.ast.ASTNode
import org.intellij.markdown.ast.getTextInNode
import org.intellij.markdown.flavours.gfm.GFMElementTypes
import org.intellij.markdown.flavours.gfm.GFMFlavourDescriptor
import org.intellij.markdown.flavours.gfm.GFMTokenTypes
import org.intellij.markdown.parser.MarkdownParser

/** A run of text and how it is styled. */
public data class Span(
    val text: String,
    val bold: Boolean = false,
    val italic: Boolean = false,
    val code: Boolean = false,
    val strike: Boolean = false,
    val link: String? = null,
)

public sealed interface Block {
    public data class Paragraph(val spans: List<Span>) : Block
    public data class Heading(val level: Int, val spans: List<Span>) : Block
    public data class Code(val text: String, val language: SyntaxLanguage) : Block
    public data class Quote(val blocks: List<Block>) : Block

    public data class Bullets(
        val ordered: Boolean,
        val start: Int,
        val items: List<List<Block>>,
    ) : Block

    public data class Table(
        val header: List<List<Span>>,
        val rows: List<List<List<Span>>>,
    ) : Block

    public data object Rule : Block
}

/**
 * CommonMark, with GitHub tables and strikethrough.
 *
 * Parsing produces a document rather than styled text so each client draws it
 * natively -- Compose today, SwiftUI later -- from one interpretation of the
 * source. Parsing is the shared part; drawing is not.
 *
 * Assistant output is streamed, so this is called on half-written documents
 * constantly. Anything it cannot make sense of falls back to the literal text,
 * which is the honest thing to show mid-sentence.
 */
public object Markdown {
    private val flavour = GFMFlavourDescriptor()

    public fun parse(text: String): List<Block> {
        if (text.isBlank()) return emptyList()
        val root = runCatching {
            MarkdownParser(flavour).buildMarkdownTreeFromString(text)
        }.getOrNull() ?: return listOf(Block.Paragraph(listOf(Span(text))))
        return blocks(root, text)
    }

    /**
     * Only schemes a tap can safely follow, matching the desktop's
     * `safe_link?`. Anything else keeps its text and loses the link, so a
     * `javascript:` or `file:` target cannot be handed to the system.
     */
    public fun safeLink(url: String): String? {
        val trimmed = url.trim()
        val safe = trimmed.startsWith("https://", ignoreCase = true) ||
            trimmed.startsWith("http://", ignoreCase = true) ||
            trimmed.startsWith("mailto:", ignoreCase = true)
        return trimmed.takeIf { safe }
    }

    private fun blocks(parent: ASTNode, source: String): List<Block> =
        parent.children.mapNotNull { block(it, source) }

    private fun block(node: ASTNode, source: String): Block? = when (node.type) {
        MarkdownElementTypes.PARAGRAPH ->
            Block.Paragraph(spans(node, source)).takeIf { it.spans.isNotEmpty() }

        MarkdownElementTypes.ATX_1 -> heading(node, 1, source)
        MarkdownElementTypes.ATX_2 -> heading(node, 2, source)
        MarkdownElementTypes.ATX_3 -> heading(node, 3, source)
        MarkdownElementTypes.ATX_4 -> heading(node, 4, source)
        MarkdownElementTypes.ATX_5 -> heading(node, 5, source)
        MarkdownElementTypes.ATX_6 -> heading(node, 6, source)
        MarkdownElementTypes.SETEXT_1 -> heading(node, 1, source)
        MarkdownElementTypes.SETEXT_2 -> heading(node, 2, source)

        MarkdownElementTypes.CODE_FENCE -> fence(node, source)
        MarkdownElementTypes.CODE_BLOCK -> Block.Code(
            node.children
                .filter { it.type == MarkdownTokenTypes.CODE_LINE || it.type == MarkdownTokenTypes.EOL }
                .joinToString("") { it.getTextInNode(source).toString() }
                .trim('\n'),
            SyntaxLanguage.NONE,
        )

        MarkdownElementTypes.BLOCK_QUOTE -> Block.Quote(blocks(node, source))

        MarkdownElementTypes.UNORDERED_LIST -> list(node, source, ordered = false)
        MarkdownElementTypes.ORDERED_LIST -> list(node, source, ordered = true)

        GFMElementTypes.TABLE -> table(node, source)

        MarkdownTokenTypes.HORIZONTAL_RULE -> Block.Rule

        // Whitespace between blocks, the `>` that opens a quote, and anything
        // this parser models but the renderer does not, are skipped rather
        // than shown as markup.
        MarkdownTokenTypes.EOL,
        MarkdownTokenTypes.WHITE_SPACE,
        MarkdownTokenTypes.BLOCK_QUOTE,
        -> null

        else -> spans(node, source).takeIf { it.isNotEmpty() }?.let(Block::Paragraph)
    }

    private fun heading(node: ASTNode, level: Int, source: String): Block {
        val content = node.children.firstOrNull {
            it.type == MarkdownTokenTypes.ATX_CONTENT || it.type == MarkdownElementTypes.PARAGRAPH
        } ?: node
        // ATX content keeps the space after the hashes.
        return Block.Heading(level, trim(spans(content, source)))
    }

    private fun fence(node: ASTNode, source: String): Block {
        val language = node.children
            .firstOrNull { it.type == MarkdownTokenTypes.FENCE_LANG }
            ?.getTextInNode(source)
            ?.toString()
            ?.let(Syntax::languageForFence)
            ?: SyntaxLanguage.NONE

        val body = StringBuilder()
        var started = false
        node.children.forEach { child ->
            when (child.type) {
                MarkdownTokenTypes.CODE_FENCE_CONTENT -> {
                    body.append(child.getTextInNode(source))
                    started = true
                }
                MarkdownTokenTypes.EOL -> if (started) body.append('\n')
                else -> Unit
            }
        }
        return Block.Code(body.toString().trim('\n'), language)
    }

    private fun list(node: ASTNode, source: String, ordered: Boolean): Block {
        val items = node.children
            .filter { it.type == MarkdownElementTypes.LIST_ITEM }
            .map { item ->
                item.children
                    .filterNot { it.type == MarkdownTokenTypes.LIST_BULLET }
                    .filterNot { it.type == MarkdownTokenTypes.LIST_NUMBER }
                    .mapNotNull { block(it, source) }
            }
        val start = node.children
            .firstOrNull { it.type == MarkdownElementTypes.LIST_ITEM }
            ?.children
            ?.firstOrNull { it.type == MarkdownTokenTypes.LIST_NUMBER }
            ?.getTextInNode(source)
            ?.toString()
            ?.trim()
            ?.trimEnd('.', ')')
            ?.toIntOrNull()
            ?: 1
        return Block.Bullets(ordered, start, items)
    }

    private fun table(node: ASTNode, source: String): Block {
        val rows = node.children.filter {
            it.type == GFMElementTypes.HEADER || it.type == GFMElementTypes.ROW
        }
        val cells = rows.map { row ->
            row.children
                .filter { it.type == GFMTokenTypes.CELL }
                .map { spans(it, source) }
        }
        if (cells.isEmpty()) return Block.Paragraph(spans(node, source))
        return Block.Table(cells.first(), cells.drop(1))
    }

    private fun spans(node: ASTNode, source: String): List<Span> {
        val collected = mutableListOf<Span>()
        collect(node, source, Span(""), collected)
        return merge(collected).filter { it.text.isNotEmpty() }
    }

    private fun collect(
        node: ASTNode,
        source: String,
        style: Span,
        into: MutableList<Span>,
    ) {
        when (node.type) {
            MarkdownElementTypes.EMPH ->
                return inlineChildren(node, source, style.copy(italic = true), into)
            MarkdownElementTypes.STRONG ->
                return inlineChildren(node, source, style.copy(bold = true), into)
            GFMElementTypes.STRIKETHROUGH ->
                return inlineChildren(node, source, style.copy(strike = true), into)
            MarkdownElementTypes.CODE_SPAN -> {
                val text = node.children
                    .filterNot { it.type == MarkdownTokenTypes.BACKTICK }
                    .joinToString("") { it.getTextInNode(source).toString() }
                into += style.copy(text = text, code = true)
                return
            }
            MarkdownElementTypes.INLINE_LINK,
            MarkdownElementTypes.FULL_REFERENCE_LINK,
            MarkdownElementTypes.SHORT_REFERENCE_LINK,
            -> return link(node, source, style, into)

            MarkdownElementTypes.AUTOLINK, GFMTokenTypes.GFM_AUTOLINK -> {
                val text = node.getTextInNode(source).toString().trim('<', '>')
                into += style.copy(text = text, link = safeLink(text))
                return
            }
            MarkdownElementTypes.IMAGE -> {
                // There is nowhere to draw a remote image in a transcript, so
                // it reads as its alt text rather than vanishing. The label
                // sits under the nested link, not directly on the image.
                val alt = find(node, MarkdownElementTypes.LINK_TEXT)
                    ?.let { plain(it, source).trim('[', ']') }
                    .orEmpty()
                into += style.copy(text = if (alt.isEmpty()) "Image" else "Image: $alt")
                return
            }
        }

        if (node.children.isEmpty()) {
            val text = when (node.type) {
                MarkdownTokenTypes.EOL -> " "
                MarkdownTokenTypes.HARD_LINE_BREAK -> "\n"
                else -> node.getTextInNode(source).toString()
            }
            into += style.copy(text = text)
            return
        }
        node.children.forEach { collect(it, source, style, into) }
    }

    private fun inlineChildren(
        node: ASTNode,
        source: String,
        style: Span,
        into: MutableList<Span>,
    ) {
        // The delimiters are leaf children of the styled node, so they would
        // otherwise be drawn: *bold* would render with its own asterisks.
        node.children
            .filterNot { it.children.isEmpty() && delimiter(plain(it, source)) }
            .forEach { collect(it, source, style, into) }
    }

    private fun delimiter(text: String): Boolean =
        text.isNotEmpty() && text.all { it == '*' || it == '_' || it == '~' }

    private fun link(
        node: ASTNode,
        source: String,
        style: Span,
        into: MutableList<Span>,
    ) {
        val destination = node.children
            .firstOrNull { it.type == MarkdownElementTypes.LINK_DESTINATION }
            ?.getTextInNode(source)
            ?.toString()
            ?.trim('<', '>')
        val label = node.children.firstOrNull {
            it.type == MarkdownElementTypes.LINK_TEXT
        }
        val text = label?.let { plain(it, source).trim('[', ']') }
            ?: node.getTextInNode(source).toString()
        into += style.copy(text = text, link = destination?.let(::safeLink))
    }

    private fun plain(node: ASTNode, source: String): String =
        node.getTextInNode(source).toString()

    private fun find(node: ASTNode, type: org.intellij.markdown.IElementType): ASTNode? {
        if (node.type == type) return node
        node.children.forEach { child -> find(child, type)?.let { return it } }
        return null
    }

    private fun trim(spans: List<Span>): List<Span> {
        if (spans.isEmpty()) return spans
        val trimmed = spans.toMutableList()
        trimmed[0] = trimmed[0].copy(text = trimmed[0].text.trimStart())
        val last = trimmed.lastIndex
        trimmed[last] = trimmed[last].copy(text = trimmed[last].text.trimEnd())
        return trimmed.filter { it.text.isNotEmpty() }
    }

    /** Adjacent runs sharing a style are one span, so renderers do less work. */
    private fun merge(spans: List<Span>): List<Span> {
        val merged = mutableListOf<Span>()
        spans.forEach { span ->
            val last = merged.lastOrNull()
            if (last != null && last.copy(text = "") == span.copy(text = "")) {
                merged[merged.lastIndex] = last.copy(text = last.text + span.text)
            } else {
                merged += span
            }
        }
        return merged
    }
}
