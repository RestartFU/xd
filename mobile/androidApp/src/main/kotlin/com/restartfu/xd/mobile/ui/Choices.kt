package com.restartfu.xd.mobile.ui

import androidx.compose.material3.FilterChip
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics

/**
 * One of a few choices that exclude each other, drawn as a chip. A filter
 * chip on its own says "checkbox", as if several could be on at once; this
 * one says it is one of a set of radio buttons. Put the chips of one choice
 * in a row or column with `Modifier.selectableGroup()`, so a screen reader
 * also says how many there are and which one this is.
 */
@Composable
internal fun ChoiceChip(
    selected: Boolean,
    onClick: () -> Unit,
    label: @Composable () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = label,
        enabled = enabled,
        // Outside the chip's own semantics, so it takes the place of the chip's checkbox role.
        modifier = modifier.semantics { role = Role.RadioButton },
    )
}
