package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import com.restartfu.xd.syntax.CodeBlocks
import com.restartfu.xd.syntax.DiffLine
import com.restartfu.xd.syntax.DiffKind
import com.restartfu.xd.syntax.Syntax
import com.restartfu.xd.syntax.SyntaxLanguage
import com.restartfu.xd.syntax.SyntaxState

/** The desktop's token colours, parsed once. */
private fun tokenColour(hex: String?): Color? =
    hex?.let { Color(it.removePrefix("#").toLong(16) or 0xFF000000L) }

/**
 * Syntax-coloured source, one scanner state threaded down the whole block so
 * a comment or string opened on one line keeps colouring the next.
 */
@Composable
internal fun CodeText(
    code: String,
    language: SyntaxLanguage,
    modifier: Modifier = Modifier,
) {
    val fallback = LocalContentColor.current
    val text = remember(code, language) { highlight(code, language, fallback) }
    Text(
        text,
        modifier = modifier.horizontalScroll(rememberScrollState()),
        fontFamily = XdMonoFamily,
        style = MaterialTheme.typography.bodySmall,
        softWrap = false,
    )
}

/**
 * Colours editable source without changing its characters or offsets. Keeping
 * an identity mapping is important: selection handles, cursor movement and
 * IME edits continue to address the original file text.
 */
internal class SyntaxVisualTransformation(
    private val language: SyntaxLanguage,
    private val fallback: Color,
) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText = TransformedText(
        highlight(text.text, language, fallback),
        OffsetMapping.Identity,
    )
}

/**
 * A unified patch: added and removed lines carry the desktop's tinted row
 * backgrounds, and their contents are coloured for the file's own language.
 */
@Composable
internal fun DiffText(
    patch: String,
    modifier: Modifier = Modifier,
) = DiffText(CodeBlocks.diffLines(patch), modifier)

@Composable
internal fun DiffText(
    lines: List<DiffLine>,
    modifier: Modifier = Modifier,
) {
    val fallback = LocalContentColor.current
    // One state per file: a hunk is not contiguous source, so carrying lexer
    // state across a hunk boundary would colour the rest of the file wrong.
    val rendered = remember(lines, fallback) {
        val state = SyntaxState()
        lines.map { line ->
            if (line.kind == DiffKind.META || line.kind == DiffKind.HUNK) state.reset()
            line to when (line.kind) {
                DiffKind.META, DiffKind.HUNK -> AnnotatedString(line.code)
                else -> buildAnnotatedString {
                    append(line.marker)
                    appendHighlighted(line.code, line.language, state, fallback)
                }
            }
        }
    }

    val signals = LocalXdSignals.current
    Column(modifier.horizontalScroll(rememberScrollState())) {
        rendered.forEach { (line, annotated) ->
            val background = when (line.kind) {
                DiffKind.ADDED -> signals.diffAdded
                DiffKind.REMOVED -> signals.diffRemoved
                else -> Color.Transparent
            }
            Text(
                annotated,
                modifier = Modifier
                    .fillMaxWidth()
                    .background(background)
                    .padding(horizontal = 4.dp),
                color = when (line.kind) {
                    DiffKind.META, DiffKind.HUNK -> MaterialTheme.colorScheme.outline
                    else -> Color.Unspecified
                },
                fontFamily = XdMonoFamily,
                style = MaterialTheme.typography.bodySmall,
                softWrap = false,
            )
        }
    }
}

private fun highlight(
    code: String,
    language: SyntaxLanguage,
    fallback: Color,
): AnnotatedString = buildAnnotatedString {
    val state = SyntaxState()
    code.lines().forEachIndexed { index, line ->
        if (index > 0) append('\n')
        appendHighlighted(line, language, state, fallback)
    }
}

/** Syntax-coloured lines for a lazy large-file view. */
internal fun highlightLines(
    code: String,
    language: SyntaxLanguage,
    fallback: Color,
): List<AnnotatedString> {
    val state = SyntaxState()
    return code.split('\n').map { line ->
        buildAnnotatedString { appendHighlighted(line, language, state, fallback) }
    }
}

private fun AnnotatedString.Builder.appendHighlighted(
    line: String,
    language: SyntaxLanguage,
    state: SyntaxState,
    fallback: Color,
) {
    if (language == SyntaxLanguage.NONE) {
        append(line)
        return
    }
    // The shared token palette is tuned for dark grounds. Dark ink means a
    // light theme, where the same hues are darkened to keep their contrast.
    val onLight = fallback.luminance() < 0.5f
    Syntax.scanLine(language, line, state).forEach { piece ->
        val token = tokenColour(piece.token.colour)
        val colour = when {
            token == null -> fallback
            onLight -> Color(token.red * 0.5f, token.green * 0.5f, token.blue * 0.5f)
            else -> token
        }
        withStyle(SpanStyle(color = colour)) { append(piece.text) }
    }
}
