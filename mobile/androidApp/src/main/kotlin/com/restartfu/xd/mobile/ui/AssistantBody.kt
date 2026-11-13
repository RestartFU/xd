package com.restartfu.xd.mobile.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.staticCompositionLocalOf
import com.restartfu.xd.markdown.Block
import com.restartfu.xd.markdown.Markdown
import com.restartfu.xd.model.AskBlock
import com.restartfu.xd.model.AssistantSectionKind
import com.restartfu.xd.model.AssistantSections
import com.restartfu.xd.model.ImageReference
import com.restartfu.xd.model.MessagePart
import com.restartfu.xd.voice.SpeakTagVisibility
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/*
 * What an assistant's reply is read into before it is drawn: its pictures and
 * prose, the prose's sections, and each section's Markdown. It is all plain
 * data worked out from the text alone, so it is done away from the main
 * thread: again and again for a reply still streaming in, and once for a
 * finished one, which is then kept for every time its row comes back.
 */

/**
 * A finished reply: the message's id and its text, which a record may
 * rewrite. Compared by the id, the text's length and hash first, and the text
 * itself only when those match, as [CodeKey] is.
 */
internal class ReplyKey(val id: String, val text: String) {
    private val hash = 31 * (31 * id.hashCode() + text.length) + text.hashCode()

    override fun hashCode(): Int = hash

    override fun equals(other: Any?): Boolean = other is ReplyKey && other.hash == hash &&
        other.text.length == text.length && other.id == id && other.text == text

    override fun toString(): String = "ReplyKey($id, ${text.length} characters)"
}

/** Finished replies read into their parts: the newest few hundred, and no more than [CACHED_CHARACTERS] of text. */
internal val ReplyBodies = PreparedCache<ReplyKey, List<BodyPart>>(
    capacity = 300,
    maxWeight = CACHED_CHARACTERS,
    weigh = { it.text.length },
) {
    assistantBody(it.text, live = false)
}

/**
 * What the reply streaming in last read as. The finished message that takes
 * its row is read again in the background; until then, if its words are the
 * same, it draws this instead of its plain text, so the handover does not
 * flicker.
 */
internal object LiveHandoff {
    @Volatile private var last: Pair<String, List<BodyPart>>? = null

    fun remember(text: String, parts: List<BodyPart>) {
        last = text to parts
    }

    fun forText(text: String): List<BodyPart>? = last?.takeIf { it.first == text }?.second
}

/** One stretch of a reply: a picture, or prose. */
internal sealed interface BodyPart {
    class Image(val path: String) : BodyPart
    class Prose(val text: String, val document: ProseDocument) : BodyPart
}

/** A reply's prose with its presentation blocks lifted out. */
internal class ProseDocument(
    /** The question of a tagged `<ask>`, shown under the prose. */
    val question: String?,
    val sections: List<ProseSection>,
)

internal class ProseSection(
    val kind: AssistantSectionKind,
    val text: String,
    /**
     * The section's Markdown. Null for a finished reply's analysis, which
     * starts closed and is only read into blocks if the reader opens it.
     */
    val blocks: List<Block>?,
)

/**
 * Reads [text] into its parts. [previous] is what the same reply read as a
 * moment ago: whatever came out the same is handed back as the same objects,
 * so drawing skips it and only the part still being written is laid out again.
 */
internal fun assistantBody(text: String, live: Boolean, previous: List<BodyPart>? = null): List<BodyPart> =
    ImageReference.parts(text, markdown = true).mapIndexed { index, part ->
        when (part) {
            is MessagePart.Image -> BodyPart.Image(part.path)
            is MessagePart.Prose -> {
                val before = previous?.getOrNull(index) as? BodyPart.Prose
                if (before != null && before.text == part.text) before
                else BodyPart.Prose(part.text, proseDocument(part.text, live, before?.document))
            }
        }
    }

internal fun proseDocument(text: String, live: Boolean, previous: ProseDocument? = null): ProseDocument {
    val ask = AskBlock.parse(text)
    val source = ask?.remainder ?: text
    val projected = if (live) AssistantSections.stream(source) else source
    val visible = SpeakTagVisibility.render(projected, live)
    val sections = AssistantSections.parse(visible).mapIndexed { index, section ->
        val before = previous?.sections?.getOrNull(index)?.takeIf { it.kind == section.kind }
        when {
            before != null && before.text == section.text -> before
            // A finished reply's analysis waits for the reader to open it.
            section.kind == AssistantSectionKind.ANALYSIS && !live -> ProseSection(section.kind, section.text, null)
            else -> ProseSection(section.kind, section.text, sameBlocks(before?.blocks, Markdown.parse(section.text)))
        }
    }
    return ProseDocument(ask?.ask?.question, sections)
}

/** [next], with each block equal to the one in its place in [previous] swapped for that one. */
internal fun sameBlocks(previous: List<Block>?, next: List<Block>): List<Block> {
    if (previous == null) return next
    return next.mapIndexed { index, block -> previous.getOrNull(index)?.takeIf { it == block } ?: block }
}

/**
 * True under a reply that is still streaming in: what is drawn there changes
 * many times a second, so work that grows with the text's length is done off
 * the main thread instead of in composition.
 */
internal val LocalStreaming = staticCompositionLocalOf { false }

/**
 * [compute] of [key], kept up with it away from the main thread.
 *
 * The first value is worked out in place, so nothing is ever drawn empty.
 * After that each new [key] is computed on a background thread and shown when
 * it is ready; keys that arrive meanwhile are skipped for the newest, which is
 * always computed last, so what is shown ends as [compute] of the final key.
 * [compute] is also given the value it last produced.
 */
@Composable
internal fun <K, T> rememberOffMain(key: K, compute: (key: K, previous: T?) -> T): T {
    val latest = rememberUpdatedState(key)
    val computing = rememberUpdatedState(compute)
    val shown = remember { mutableStateOf(compute(key, null)) }
    LaunchedEffect(Unit) {
        var done = key
        snapshotFlow { latest.value }.collect { next ->
            if (next != done) {
                val previous = shown.value
                shown.value = withContext(Dispatchers.Default) { computing.value(next, previous) }
                done = next
            }
        }
    }
    return shown.value
}
