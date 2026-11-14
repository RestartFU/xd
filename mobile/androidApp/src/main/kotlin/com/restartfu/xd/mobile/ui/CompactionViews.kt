package com.restartfu.xd.mobile.ui

import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.restartfu.xd.mobile.ChatViewModel
import com.restartfu.xd.mobile.MainViewModel
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.ChatState
import com.restartfu.xd.model.CompactBeforeSend
import com.restartfu.xd.model.CompactionState
import com.restartfu.xd.model.ContextCompaction
import com.restartfu.xd.model.IdleCompactionMode
import com.restartfu.xd.model.formatTokens
import kotlinx.coroutines.delay

internal const val IDLE_COMPACTION_TITLE = "Compact idle chats"
internal const val IDLE_COMPACTION_DETAIL =
    "When to compact an idle Claude chat over 200,000 tokens: after you return, just before its prompt cache expires, or never."

/** Each mode's choice in Settings, with the desktop's wording. */
internal fun idleCompactionLabel(mode: IdleCompactionMode): String = when (mode) {
    IdleCompactionMode.ON_RETURN -> "When you return"
    IdleCompactionMode.BEFORE_CACHE_EXPIRES -> "Before the cache expires (beta)"
    IdleCompactionMode.OFF -> "Never"
}

/** What the composer's chip reads: compact first, or keep the full history for this send. */
internal fun compactChipLabel(tokens: Long, keep: Boolean): String =
    (if (keep) "Keep " else "Compact ") + formatTokens(tokens)

/** The chip's spoken label, whichever way it is set; its on/off state says which. */
internal fun compactChipDescription(tokens: Long): String =
    "Compact ${formatTokens(tokens)} tokens of history before sending"

/** The send button's spoken label while the next send compacts first. */
internal const val COMPACT_AND_SEND = "Compact and send"

/**
 * One compaction of the chat's context, as the desktop draws it: a quiet
 * divider across the transcript with the host's sentence in the middle.
 * It breathes while the compaction runs, and turns the error colour if it
 * failed.
 */
@Composable
internal fun CompactionDivider(compaction: ContextCompaction) {
    val failed = compaction.state == CompactionState.FAILED
    val tint = if (failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant
    val line = MaterialTheme.colorScheme.outlineVariant
    BoxWithConstraints(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
        // The sentence wraps before the lines on either side vanish.
        val labelRoom = maxWidth - 64.dp
        Row(
            Modifier.fillMaxWidth().semantics(mergeDescendants = true) {},
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            HorizontalDivider(Modifier.weight(1f), color = line)
            Row(
                Modifier
                    .widthIn(max = labelRoom)
                    .let { if (compaction.state == CompactionState.RUNNING) it.breathing() else it },
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Icon(
                    painterResource(if (failed) R.drawable.ic_alert else R.drawable.ic_minimize),
                    contentDescription = null,
                    tint = tint,
                    modifier = Modifier.size(14.dp),
                )
                Text(
                    compaction.label,
                    color = tint,
                    style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center,
                )
            }
            HorizontalDivider(Modifier.weight(1f), color = line)
        }
    }
}

/** The working pulse, as on the working dot: dim, bright, dim. */
@Composable
private fun Modifier.breathing(): Modifier {
    val transition = rememberInfiniteTransition(label = "compacting")
    val alpha by transition.animateFloat(
        initialValue = 0.45f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(800), RepeatMode.Reverse),
        label = "compacting-alpha",
    )
    return graphicsLayer { this.alpha = alpha }
}

/**
 * The composer's offer to compact this idle chat before the next send, or
 * null while it makes none. Offered only once the host says a send may
 * compact, the host compacts on return, nothing runs or waits on the reader,
 * and the message is not a compaction itself. The time it becomes due is
 * checked while the screen is open, so the chip can appear without a reload.
 */
@Composable
internal fun rememberCompactOffer(state: ChatState, text: String, model: ChatViewModel): CompactBeforeSend? {
    val offer = state.compactBeforeSend
    val settings by model.idleCompaction.collectAsStateWithLifecycle()
    val known = settings != null
    LaunchedEffect(offer != null, known) {
        if (offer != null && !known) model.loadIdleCompaction()
    }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(offer?.afterMillis) {
        val after = offer?.afterMillis ?: return@LaunchedEffect
        now = System.currentTimeMillis()
        // Coarse steps, so a phone asleep in between catches up when it wakes.
        while (now < after) {
            delay((after - now).coerceIn(1_000L, 30_000L))
            now = System.currentTimeMillis()
        }
    }
    val waiting = remember(state) { dockedAsk(state) != null }
    return offer?.takeIf { it.offered(now, settings?.mode, state.working, waiting, text) }
}

/**
 * Beside the send button while a send would compact first: "Compact 251K"
 * compacts, and a tap makes it "Keep 251K", which sends with the full
 * history this once.
 */
@Composable
internal fun CompactChip(tokens: Long, keep: Boolean, onToggle: () -> Unit, modifier: Modifier = Modifier) {
    val accent = MaterialTheme.colorScheme.primary
    val color = if (keep) MaterialTheme.colorScheme.onSurfaceVariant else accent
    // Drawn at 32dp in the composer's control line; the platform still gives it a 48dp touch area.
    CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides 0.dp) {
        Surface(
            checked = !keep,
            onCheckedChange = { onToggle() },
            modifier = modifier
                .height(32.dp)
                .semantics {
                    role = Role.Switch
                    contentDescription = compactChipDescription(tokens)
                },
            shape = RoundedCornerShape(16.dp),
            color = if (keep) Color.Transparent else accent.copy(alpha = 0.14f),
            border = BorderStroke(1.dp, if (keep) MaterialTheme.colorScheme.outlineVariant else accent.copy(alpha = 0.45f)),
        ) {
            Row(
                Modifier.padding(horizontal = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(5.dp),
            ) {
                Icon(
                    painterResource(R.drawable.ic_minimize),
                    contentDescription = null,
                    tint = color,
                    modifier = Modifier.size(12.dp),
                )
                Text(
                    compactChipLabel(tokens, keep),
                    // The spoken label and switch state say this already.
                    modifier = Modifier.clearAndSetSemantics {},
                    color = color,
                    style = MaterialTheme.typography.labelMedium,
                    maxLines = 1,
                )
            }
        }
    }
}

/**
 * When the host compacts an idle Claude chat. Shown only on hosts that
 * answer with a mode: older ones refuse the read and have nothing to set.
 */
@Composable
internal fun IdleCompactionSection(model: MainViewModel, mode: IdleCompactionMode) {
    val error by model.idleCompactionError.collectAsStateWithLifecycle()
    Column {
        Text(IDLE_COMPACTION_TITLE, fontWeight = FontWeight.SemiBold)
        Text(
            IDLE_COMPACTION_DETAIL,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            style = MaterialTheme.typography.bodySmall,
        )
        IdleCompactionMode.entries.forEach { choice ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .selectable(selected = choice == mode, role = Role.RadioButton) {
                        if (choice != mode) model.setIdleCompaction(choice)
                    }
                    .padding(vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                RadioButton(selected = choice == mode, onClick = null, modifier = Modifier.size(40.dp))
                Text(idleCompactionLabel(choice), style = MaterialTheme.typography.bodyMedium)
            }
        }
        error?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        }
    }
}
