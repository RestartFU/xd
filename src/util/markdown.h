#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Renders the small part of Markdown that shows up in chat replies as Pango
 * markup: fenced and inline code, bold, italics and headings.
 *
 * The result is always valid markup. That matters because the text arrives a
 * token at a time, so the converter is regularly handed a half-written span --
 * an unterminated one is shown literally rather than swallowing the rest of
 * the message.
 */
char *xd_markdown_to_pango (const char *text);

G_END_DECLS
