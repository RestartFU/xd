#pragma once

#include <glib.h>

G_BEGIN_DECLS

/*
 * Parses CommonMark and renders it as Pango markup suitable for GtkLabel.
 * Raw HTML stays literal; links are escaped and restricted to safe schemes.
 *
 * The result is always valid markup. That matters because the text arrives a
 * token at a time, so the converter is regularly handed a half-written span --
 * an unterminated one is shown literally rather than swallowing the rest of
 * the message.
 */
char *xd_markdown_to_pango (const char *text);

/* Escapes plain text and turns bare HTTP(S) URLs into GtkLabel links. */
char *xd_urls_to_pango     (const char *text);

G_END_DECLS
