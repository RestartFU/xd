package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import com.restartfu.xd.markdown.Block
import com.restartfu.xd.markdown.Markdown
import com.restartfu.xd.markdown.Span

/** Draws a parsed Markdown document. */
@Composable
internal fun MarkdownText(
    source: String,
    modifier: Modifier = Modifier,
) {
    val blocks = remember(source) { Markdown.parse(source) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        blocks.forEach { BlockContent(it) }
    }
}

@Composable
private fun BlockContent(block: Block, depth: Int = 0) {
    when (block) {
        is Block.Paragraph -> Text(block.spans.annotated())

        is Block.Heading -> Text(
            block.spans.annotated(),
            style = when (block.level) {
                1 -> MaterialTheme.typography.titleLarge
                2 -> MaterialTheme.typography.titleMedium
                else -> MaterialTheme.typography.titleSmall
            },
            fontWeight = FontWeight.Bold,
        )

        is Block.Code -> Surface(
            color = MaterialTheme.colorScheme.surfaceVariant,
            shape = MaterialTheme.shapes.small,
        ) {
            CodeText(
                block.text,
                block.language,
                modifier = Modifier.padding(horizontal = 10.dp, vertical = 8.dp),
            )
        }

        // The bar takes its height from the quote beside it.
        is Block.Quote -> Row(
            Modifier
                .fillMaxWidth()
                .height(IntrinsicSize.Min),
        ) {
            Box(
                Modifier
                    .width(3.dp)
                    .fillMaxHeight()
                    .background(MaterialTheme.colorScheme.outline),
            )
            Column(
                modifier = Modifier.padding(start = 10.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                block.blocks.forEach { BlockContent(it, depth) }
            }
        }

        is Block.Bullets -> Column(
            modifier = Modifier.padding(start = if (depth == 0) 0.dp else 12.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            block.items.forEachIndexed { index, item ->
                Row(Modifier.fillMaxWidth()) {
                    Text(
                        if (block.ordered) "${block.start + index}." else "•",
                        modifier = Modifier.width(24.dp),
                        color = MaterialTheme.colorScheme.outline,
                    )
                    Column(
                        modifier = Modifier.weight(1f),
                        verticalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        item.forEach { BlockContent(it, depth + 1) }
                    }
                }
            }
        }

        // Tables scroll rather than wrap: a wrapped cell stops the columns
        // lining up, which is the only reason the table is a table.
        is Block.Table -> Column(
            Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
        ) {
            TableRow(block.header, header = true)
            HorizontalDivider()
            block.rows.forEach { TableRow(it, header = false) }
        }

        Block.Rule -> HorizontalDivider(Modifier.padding(vertical = 4.dp))
    }
}

@Composable
private fun TableRow(cells: List<List<Span>>, header: Boolean) {
    Row(Modifier.padding(vertical = 4.dp)) {
        cells.forEach { cell ->
            Text(
                cell.annotated(),
                modifier = Modifier
                    .width(140.dp)
                    .padding(end = 8.dp),
                fontWeight = if (header) FontWeight.Bold else null,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable
private fun List<Span>.annotated(): AnnotatedString {
    val linkColour = MaterialTheme.colorScheme.primary
    val codeColour = MaterialTheme.colorScheme.surfaceVariant
    val spans = this
    return remember(spans, linkColour, codeColour) {
        buildAnnotatedString {
            spans.forEach { span ->
                val style = SpanStyle(
                    fontWeight = if (span.bold) FontWeight.Bold else null,
                    fontStyle = if (span.italic) FontStyle.Italic else null,
                    fontFamily = if (span.code) FontFamily.Monospace else null,
                    background = if (span.code) codeColour else Color.Unspecified,
                    textDecoration = if (span.strike) TextDecoration.LineThrough else null,
                )
                val url = span.link
                if (url == null) {
                    withStyle(style) { append(span.text) }
                } else {
                    // Only links the parser judged safe reach here.
                    withLink(
                        LinkAnnotation.Url(
                            url,
                            TextLinkStyles(style.copy(color = linkColour)),
                        ),
                    ) {
                        append(span.text)
                    }
                }
            }
        }
    }
}
