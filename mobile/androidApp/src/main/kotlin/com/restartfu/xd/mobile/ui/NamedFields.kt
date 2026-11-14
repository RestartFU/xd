@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)

package com.restartfu.xd.mobile.ui

import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.LocalTextSelectionColors
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.TextFieldColors
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.takeOrElse
import androidx.compose.ui.semantics.error
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.VisualTransformation

/*
 * Names for fields that show none of their own once they hold text.
 *
 * A content description on an edit box can make a screen reader say it in
 * place of what was typed. The name is instead a piece of text inside the
 * field, as a label is: the field's merged node keeps its typed text as its
 * text, and the name is read with it.
 */

/**
 * [field] (a field's inner text) with [name] laid over it while [shown]:
 * while it holds text, once the placeholder that names it until then is gone,
 * or always for a field with no placeholder. Draws nothing.
 */
@Composable
internal fun FieldName(name: String, shown: Boolean, field: @Composable () -> Unit) {
    Box {
        if (shown) Box(Modifier.matchParentSize().semantics { text = AnnotatedString(name) })
        field()
    }
}

/**
 * Material's outlined field, drawn as `OutlinedTextField` draws it, that keeps
 * [name] for a screen reader once it holds text (see [FieldName]). The rest
 * is `OutlinedTextField`'s, with [xdFieldColors] by default.
 */
@Composable
internal fun NamedOutlinedField(
    value: String,
    onValueChange: (String) -> Unit,
    name: String,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    textStyle: TextStyle = LocalTextStyle.current,
    placeholder: @Composable (() -> Unit)? = null,
    leadingIcon: @Composable (() -> Unit)? = null,
    trailingIcon: @Composable (() -> Unit)? = null,
    suffix: @Composable (() -> Unit)? = null,
    isError: Boolean = false,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
    singleLine: Boolean = false,
    shape: Shape = OutlinedTextFieldDefaults.shape,
    colors: TextFieldColors = xdFieldColors(),
) {
    val interactions = remember { MutableInteractionSource() }
    CompositionLocalProvider(LocalTextSelectionColors provides colors.textSelectionColors) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            modifier = modifier.outlinedField(isError),
            enabled = enabled,
            textStyle = fieldTextStyle(textStyle, colors, enabled, isError, interactions),
            cursorBrush = SolidColor(if (isError) colors.errorCursorColor else colors.cursorColor),
            keyboardOptions = keyboardOptions,
            keyboardActions = keyboardActions,
            interactionSource = interactions,
            singleLine = singleLine,
            maxLines = if (singleLine) 1 else Int.MAX_VALUE,
            decorationBox = { inner ->
                OutlinedTextFieldDefaults.DecorationBox(
                    value = value,
                    innerTextField = { FieldName(name, value.isNotEmpty() || placeholder == null, inner) },
                    enabled = enabled,
                    singleLine = singleLine,
                    visualTransformation = VisualTransformation.None,
                    interactionSource = interactions,
                    isError = isError,
                    placeholder = placeholder,
                    leadingIcon = leadingIcon,
                    trailingIcon = trailingIcon,
                    suffix = suffix,
                    colors = colors,
                    container = {
                        OutlinedTextFieldDefaults.Container(enabled, isError, interactions, colors = colors, shape = shape)
                    },
                )
            },
        )
    }
}

/** [NamedOutlinedField] for a [TextFieldValue], with its [visualTransformation]. */
@Composable
internal fun NamedOutlinedField(
    value: TextFieldValue,
    onValueChange: (TextFieldValue) -> Unit,
    name: String,
    modifier: Modifier = Modifier,
    textStyle: TextStyle = LocalTextStyle.current,
    visualTransformation: VisualTransformation = VisualTransformation.None,
    shape: Shape = OutlinedTextFieldDefaults.shape,
    colors: TextFieldColors = xdFieldColors(),
) {
    val interactions = remember { MutableInteractionSource() }
    CompositionLocalProvider(LocalTextSelectionColors provides colors.textSelectionColors) {
        BasicTextField(
            value = value,
            onValueChange = onValueChange,
            modifier = modifier.outlinedField(isError = false),
            textStyle = fieldTextStyle(textStyle, colors, enabled = true, isError = false, interactions),
            cursorBrush = SolidColor(colors.cursorColor),
            visualTransformation = visualTransformation,
            interactionSource = interactions,
            decorationBox = { inner ->
                OutlinedTextFieldDefaults.DecorationBox(
                    value = value.text,
                    innerTextField = { FieldName(name, shown = true, inner) },
                    enabled = true,
                    singleLine = false,
                    visualTransformation = visualTransformation,
                    interactionSource = interactions,
                    colors = colors,
                    container = {
                        OutlinedTextFieldDefaults.Container(true, false, interactions, colors = colors, shape = shape)
                    },
                )
            },
        )
    }
}

/** What `OutlinedTextField` puts on its field: Material's least size, and its error said. */
private fun Modifier.outlinedField(isError: Boolean): Modifier =
    then(if (isError) Modifier.semantics { error(INVALID_INPUT) } else Modifier)
        .defaultMinSize(minWidth = OutlinedTextFieldDefaults.MinWidth, minHeight = OutlinedTextFieldDefaults.MinHeight)

/** The field's text in its colours' text colour, as `OutlinedTextField` picks it. */
@Composable
private fun fieldTextStyle(
    style: TextStyle,
    colors: TextFieldColors,
    enabled: Boolean,
    isError: Boolean,
    interactions: MutableInteractionSource,
): TextStyle {
    val focused by interactions.collectIsFocusedAsState()
    val ink = style.color.takeOrElse {
        when {
            !enabled -> colors.disabledTextColor
            isError -> colors.errorTextColor
            focused -> colors.focusedTextColor
            else -> colors.unfocusedTextColor
        }
    }
    return style.merge(TextStyle(color = ink))
}

/** Material's own words for a field in error. */
private const val INVALID_INPUT = "Invalid input"
