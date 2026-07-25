#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * A multiple-choice question the assistant asked.
 *
 * Neither CLI can prompt for input when driven one-shot, so a question that
 * matters would otherwise arrive as prose the user has to retype an answer to.
 * Instead the assistant is asked to wrap such questions in an <ask> block,
 * which hy lifts out of the reply and renders as buttons -- the same trick
 * t3code uses for its <proposed_plan> blocks.
 */
typedef struct
{
  char *question;
  GStrv options;
} HyAsk;

void  hy_ask_free (HyAsk *self);

/*
 * Lifts the first <ask> block out of @text.
 *
 * Returns NULL when there is none, or when the block names fewer than two
 * options -- a question with one answer is not worth a row of buttons. On
 * success @remainder receives the reply with the block removed, so the block
 * markup is never shown.
 */
HyAsk *hy_ask_parse (const char  *text,
                     char       **remainder);

/*
 * How much of @text can be shown while the reply is still streaming.
 *
 * The block must never appear, not even for the instant before the turn ends
 * and the buttons replace it -- and the opening tag arrives in fragments, so
 * a trailing "<as" has to be held back too in case it becomes "<ask>".
 */
gsize hy_ask_visible_length (const char *text);

/* Told to the assistant so it knows the block exists. */
const char *hy_ask_instructions (void);

G_DEFINE_AUTOPTR_CLEANUP_FUNC (HyAsk, hy_ask_free)

G_END_DECLS
