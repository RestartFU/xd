package com.restartfu.xd.mobile.ui

import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.getValue
import androidx.compose.material3.TextButton
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
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
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

/**
 * What code is coloured with: plain text takes [text], each token its colour
 * in [syntax]. Both come from the theme, so a coloured result is kept by this
 * as well as by the code: a theme switch colours it again.
 */
@Immutable
internal data class CodeInk(val text: Color, val syntax: XdSyntax)

/** The theme's ink for code, as drawn here. */
@Composable
internal fun codeInk(): CodeInk {
    val text = LocalContentColor.current
    val syntax = LocalXdSyntax.current
    return remember(text, syntax) { CodeInk(text, syntax) }
}

/**
 * One block of code and the ink it is coloured in: the key of a coloured block.
 * Its hash is worked out once, from the text's hash and length, and two keys
 * compare their texts only once those match, so a lookup does not read a long
 * block through again; a collision still compares the text, so it is never
 * taken for another block.
 */
internal class CodeKey(val code: String, val language: SyntaxLanguage, val ink: CodeInk) {
    private val hash = 31 * (31 * (31 * code.hashCode() + code.length) + language.hashCode()) + ink.hashCode()

    override fun hashCode(): Int = hash

    override fun equals(other: Any?): Boolean = other is CodeKey && other.hash == hash &&
        other.code.length == code.length && other.language == language && other.ink == ink && other.code == code
}

/** Finished code blocks, coloured once each, in the background, for every row that shows them. */
internal val CodeColours = PreparedCache<CodeKey, AnnotatedString>(
    capacity = 600,
    maxWeight = CACHED_CHARACTERS,
    weigh = { it.code.length },
) {
    highlight(it.code, it.language, it.ink)
}

/** How many characters of text each prepared cache keeps at most. */
internal const val CACHED_CHARACTERS: Long = 2_000_000

/** A patch and the ink it is coloured in: the key of a read and coloured patch, compared as [CodeKey] is. */
internal class DiffKey(val patch: String, val ink: CodeInk) {
    private val hash = 31 * (31 * patch.hashCode() + patch.length) + ink.hashCode()

    override fun hashCode(): Int = hash

    override fun equals(other: Any?): Boolean = other is DiffKey && other.hash == hash &&
        other.patch.length == patch.length && other.ink == ink && other.patch == patch
}

/** A patch read into its lines, each coloured for its file's language. */
internal class PreparedPatch(val lines: List<DiffLine>, val coloured: List<AnnotatedString>)

/** Patches shown in approvals and tool rows, read and coloured once each, in the background. */
internal val DiffColours = PreparedCache<DiffKey, PreparedPatch>(
    capacity = 64,
    maxWeight = CACHED_CHARACTERS,
    weigh = { it.patch.length },
) { key ->
    val lines = CodeBlocks.diffLines(key.patch)
    PreparedPatch(lines, highlightDiff(lines, key.ink))
}

/** How many lines of a patch are drawn inline before "Show all" is asked for. */
internal const val DIFF_INLINE_LINES = 200

/**
 * The newest colourings of blocks still being written, so the finished reply
 * that takes over a streamed one has its code coloured on its first frame.
 */
private object StreamedCode {
    private val recent = object : LinkedHashMap<CodeKey, AnnotatedString>(16, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<CodeKey, AnnotatedString>?): Boolean = size > 16
    }

    @Synchronized fun put(key: CodeKey, text: AnnotatedString) {
        recent[key] = text
    }

    @Synchronized fun get(key: CodeKey): AnnotatedString? = recent[key]
}

/**
 * Syntax-coloured source, one scanner state threaded down the whole block so
 * a comment or string opened on one line keeps colouring the next.
 *
 * A block still being written is coloured again as it grows, away from the
 * main thread. A finished one is coloured once and kept (see [CodeColours]):
 * until its colours are ready it shows the same characters uncoloured.
 */
@Composable
internal fun CodeText(
    code: String,
    language: SyntaxLanguage,
    modifier: Modifier = Modifier,
) {
    val key = CodeKey(code, language, ink = codeInk())
    val text = if (LocalStreaming.current) {
        val (coloured, highlighted) = rememberOffMain(key) { next: CodeKey, _: Pair<CodeKey, AnnotatedString>? ->
            next to highlight(next.code, next.language, next.ink)
        }
        StreamedCode.put(coloured, highlighted)
        highlighted
    } else {
        StreamedCode.get(key)?.let { CodeColours.offer(key, it) }
        rememberPrepared(CodeColours, key) ?: AnnotatedString(code)
    }
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
    private val ink: CodeInk,
) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText = TransformedText(
        highlight(text.text, language, ink),
        OffsetMapping.Identity,
    )
}

/**
 * A unified patch: added and removed lines carry the desktop's tinted row
 * backgrounds, and their contents are coloured for the file's own language.
 *
 * The patch is read and coloured away from the main thread (see
 * [DiffColours]); until then its first lines show plain. Only the first
 * [DIFF_INLINE_LINES] are drawn at first, with an action that draws the rest,
 * so a patch of thousands of lines does not lay out thousands of rows.
 */
@Composable
internal fun DiffText(
    patch: String,
    modifier: Modifier = Modifier,
) {
    val ink = codeInk()
    val key = remember(patch, ink) { DiffKey(patch, ink) }
    val prepared = rememberPrepared(DiffColours, key)
    var showAll by rememberSaveable(patch.length, patch.hashCode()) { mutableStateOf(false) }
    Column(modifier) {
        val scroll = rememberScrollState()
        if (prepared == null) {
            val plain = remember(patch) { patch.lineSequence().take(DIFF_INLINE_LINES).toList() }
            Column(Modifier.horizontalScroll(scroll)) {
                plain.forEach { line ->
                    DiffLineText(DiffKind.CONTEXT, AnnotatedString(line), Modifier.fillMaxWidth())
                }
            }
        } else {
            val count = prepared.lines.size
            val shown = if (showAll) count else minOf(count, DIFF_INLINE_LINES)
            Column(Modifier.horizontalScroll(scroll)) {
                for (index in 0 until shown) {
                    DiffLineText(prepared.lines[index].kind, prepared.coloured[index], Modifier.fillMaxWidth())
                }
            }
            if (shown < count) {
                TextButton(onClick = { showAll = true }) { Text("Show all $count lines") }
            }
        }
    }
}

/**
 * Each line of a patch, coloured for its file's language.
 *
 * One state per file: a hunk is not contiguous source, so carrying lexer
 * state across a hunk boundary would colour the rest of the file wrong.
 */
internal fun highlightDiff(lines: List<DiffLine>, ink: CodeInk): List<AnnotatedString> {
    val state = SyntaxState()
    return lines.map { line ->
        if (line.kind == DiffKind.META || line.kind == DiffKind.HUNK) state.reset()
        when (line.kind) {
            DiffKind.META, DiffKind.HUNK -> AnnotatedString(line.code)
            else -> buildAnnotatedString {
                append(line.marker)
                appendHighlighted(line.code, line.language, state, ink)
            }
        }
    }
}

/** A diff line before it is coloured: the same characters, uncoloured. */
internal fun plainDiffLine(line: DiffLine): AnnotatedString = when (line.kind) {
    DiffKind.META, DiffKind.HUNK -> AnnotatedString(line.code)
    else -> AnnotatedString(line.marker + line.code)
}

/** One line of a patch, on the desktop's tint for an added or removed line. */
@Composable
internal fun DiffLineText(
    kind: DiffKind,
    text: AnnotatedString,
    modifier: Modifier = Modifier,
) {
    val signals = LocalXdSignals.current
    val background = when (kind) {
        DiffKind.ADDED -> signals.diffAdded
        DiffKind.REMOVED -> signals.diffRemoved
        else -> Color.Transparent
    }
    Text(
        text,
        modifier = modifier
            .background(background)
            .padding(horizontal = 4.dp),
        color = when (kind) {
            DiffKind.META, DiffKind.HUNK -> LocalXdSignals.current.faint
            else -> Color.Unspecified
        },
        fontFamily = XdMonoFamily,
        style = MaterialTheme.typography.bodySmall,
        softWrap = false,
    )
}

private fun highlight(
    code: String,
    language: SyntaxLanguage,
    ink: CodeInk,
): AnnotatedString = buildAnnotatedString {
    val state = SyntaxState()
    code.lines().forEachIndexed { index, line ->
        if (index > 0) append('\n')
        appendHighlighted(line, language, state, ink)
    }
}

/** Syntax-coloured lines for a lazy large-file view. */
internal fun highlightLines(
    code: String,
    language: SyntaxLanguage,
    ink: CodeInk,
): List<AnnotatedString> {
    val state = SyntaxState()
    return code.split('\n').map { line ->
        buildAnnotatedString { appendHighlighted(line, language, state, ink) }
    }
}

private fun AnnotatedString.Builder.appendHighlighted(
    line: String,
    language: SyntaxLanguage,
    state: SyntaxState,
    ink: CodeInk,
) {
    if (language == SyntaxLanguage.NONE) {
        append(line)
        return
    }
    Syntax.scanLine(language, line, state).forEach { piece ->
        withStyle(SpanStyle(color = ink.syntax.colour(piece.token) ?: ink.text)) { append(piece.text) }
    }
}
