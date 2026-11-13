package com.restartfu.xd.mobile.ui

import android.database.ContentObserver
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.SemanticsPropertyReceiver
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.restartfu.xd.mobile.R
import com.restartfu.xd.model.ChatSummary
import kotlinx.coroutines.delay

/**
 * How a session or chat stands, as the mark drawn for it in session rows,
 * chat tabs, and closed project and folder rows. The shapes are the desktop's:
 * each state has its own, so none is told by colour alone.
 */
internal enum class StatusMark(val label: String) {
    /** A filled dot that pulses, and holds still when animations are off. */
    WORKING("Working"),

    /** A hollow ring. */
    WAITING("Waiting on you"),

    /** A check mark: finished, and not read yet. */
    DONE("Done"),

    /** Two bars. */
    PAUSED("Paused"),
}

@Composable
internal fun StatusMark.color(): Color {
    val signals = LocalXdSignals.current
    return when (this) {
        StatusMark.WORKING -> signals.working
        StatusMark.WAITING -> signals.waiting
        StatusMark.DONE -> signals.done
        StatusMark.PAUSED -> signals.faint
    }
}

/**
 * How one chat stands, as its tab and its row in the chat menu mark it: the
 * same order a session row uses, so the two never disagree. Null for a quiet
 * chat, and for one waiting only on its background tasks.
 */
internal fun chatStatus(chat: ChatSummary, waiting: Boolean, unread: Boolean): StatusMark? = when {
    // A chat stopped at its quota waits on the user's answer too.
    chat.quotaReached || waiting -> StatusMark.WAITING
    chat.working -> StatusMark.WORKING
    chat.waitingOn > 0 -> null
    chat.paused -> StatusMark.PAUSED
    unread -> StatusMark.DONE
    else -> null
}

/**
 * One status mark. [labelled] gives it its name for screen readers; pass
 * false where words beside it, or the row it sits in, already say it.
 * [still] holds the working dot steady, as the closed rows that gather
 * several marks draw it.
 */
@Composable
internal fun StatusMarkIcon(
    mark: StatusMark,
    modifier: Modifier = Modifier,
    size: Dp = StatusMarkSize,
    labelled: Boolean = true,
    still: Boolean = false,
) {
    val color = mark.color()
    val named = if (labelled) modifier.semantics { contentDescription = mark.label } else modifier
    when (mark) {
        StatusMark.WORKING -> Box(named.size(size), contentAlignment = Alignment.Center) {
            if (still) Box(Modifier.size(size * 0.8f).clip(CircleShape).background(color))
            else PulseDot(color, size = size * 0.8f)
        }
        StatusMark.WAITING -> Canvas(named.size(size)) {
            val stroke = 1.5.dp.toPx()
            drawCircle(color, radius = (this.size.minDimension - stroke) / 2, style = Stroke(stroke))
        }
        StatusMark.DONE -> Canvas(named.size(size)) {
            val w = this.size.width
            val h = this.size.height
            val path = Path().apply {
                moveTo(w * 0.12f, h * 0.54f)
                lineTo(w * 0.40f, h * 0.80f)
                lineTo(w * 0.90f, h * 0.22f)
            }
            drawPath(path, color, style = Stroke(1.75.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
        }
        StatusMark.PAUSED -> Canvas(named.size(size)) {
            val w = this.size.width
            val h = this.size.height
            val bar = Size(w * 0.24f, h * 0.76f)
            val radius = CornerRadius(bar.width / 2)
            drawRoundRect(color, Offset(w * 0.18f, h * 0.12f), bar, radius)
            drawRoundRect(color, Offset(w * 0.58f, h * 0.12f), bar, radius)
        }
    }
}

/** The size a status mark is drawn at in rows and tabs. */
internal val StatusMarkSize = 10.dp

/**
 * A status said in words, on a soft tint of its colour, with its mark in
 * front when it has one. The words carry the meaning for screen readers.
 */
@Composable
internal fun StatusPill(label: String, color: Color, modifier: Modifier = Modifier, mark: StatusMark? = null) {
    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(color.copy(alpha = 0.14f))
            .padding(horizontal = 8.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        mark?.let { StatusMarkIcon(it, size = 8.dp, labelled = false) }
        Text(
            label,
            color = color,
            style = MaterialTheme.typography.labelSmall,
            fontWeight = FontWeight.Medium,
            maxLines = 1,
        )
    }
}

/** A dot that breathes while something works, and holds still when animations are off. */
@Composable
internal fun PulseDot(color: Color, modifier: Modifier = Modifier, size: Dp = 7.dp) {
    if (reducedMotion()) {
        Box(modifier.size(size).clip(CircleShape).background(color))
        return
    }
    val transition = rememberInfiniteTransition(label = "pulse")
    val alpha by transition.animateFloat(
        initialValue = 0.35f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(800), RepeatMode.Reverse),
        label = "pulse-alpha",
    )
    Box(
        modifier
            .size(size)
            .graphicsLayer { this.alpha = alpha }
            .clip(CircleShape)
            .background(color),
    )
}

/**
 * Whether the reader turned animations off ("Remove animations", or an
 * animator scale of zero). Loops driven by a timer rather than an animation
 * check this themselves; Compose's own animations already stop.
 */
@Composable
internal fun reducedMotion(): Boolean {
    val resolver = LocalContext.current.contentResolver
    val read = { Settings.Global.getFloat(resolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f }
    var reduced by remember(resolver) { mutableStateOf(read()) }
    DisposableEffect(resolver) {
        val observer = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                reduced = read()
            }
        }
        resolver.registerContentObserver(
            Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, observer,
        )
        reduced = read()
        onDispose { resolver.unregisterContentObserver(observer) }
    }
    return reduced
}

/**
 * Makes a row open and close what it heads: a button of at least 48dp that
 * says whether it is expanded, so the chevron beside it needs no words.
 */
internal fun Modifier.disclosure(expanded: Boolean, enabled: Boolean = true, onToggle: () -> Unit): Modifier =
    if (!enabled) {
        this
    } else {
        clickable(
            role = Role.Button,
            onClickLabel = if (expanded) "Collapse" else "Expand",
            onClick = onToggle,
        ).semantics { stateDescription = if (expanded) "Expanded" else "Collapsed" }
    }

/** The chevron at the end of anything that opens. Its row says the state. */
@Composable
internal fun DisclosureChevron(expanded: Boolean, modifier: Modifier = Modifier, size: Dp = 14.dp) {
    Icon(
        painterResource(if (expanded) R.drawable.ic_chevron_down else R.drawable.ic_chevron_right),
        contentDescription = null,
        tint = LocalXdSignals.current.faint,
        modifier = modifier.size(size),
    )
}

/**
 * [value] as last told to a screen reader: its newest value, at most once per
 * [intervalMillis], so text that changes many times a second is not announced
 * for each change.
 */
@Composable
internal fun rememberThrottled(value: String, intervalMillis: Long): String {
    val latest = rememberUpdatedState(value)
    var shown by remember { mutableStateOf(value) }
    LaunchedEffect(intervalMillis) {
        snapshotFlow { latest.value }.collect { next ->
            shown = next
            delay(intervalMillis)
        }
    }
    return shown
}

/** Hides a decoration from screen readers. */
internal fun Modifier.decorative(): Modifier = clearAndSetSemantics {}

/**
 * Replaces everything beneath with one node of its own, which a clickable
 * ancestor does not fold into itself.
 */
internal fun Modifier.semanticsNode(properties: SemanticsPropertyReceiver.() -> Unit): Modifier =
    semantics(mergeDescendants = true) {}.clearAndSetSemantics(properties)
