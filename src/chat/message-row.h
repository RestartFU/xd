#pragma once

#include <adwaita.h>

G_BEGIN_DECLS

typedef enum
{
  XD_MESSAGE_USER,
  XD_MESSAGE_ASSISTANT,
  XD_MESSAGE_TOOL,
  XD_MESSAGE_ERROR,
} XdMessageKind;

#define XD_TYPE_MESSAGE_ROW (xd_message_row_get_type ())
G_DECLARE_FINAL_TYPE (XdMessageRow, xd_message_row, XD, MESSAGE_ROW, AdwBin)

/*
 * One message in the transcript.
 *
 * Rows represent complete messages. Turn progress belongs to the transcript's
 * working marker, so incomplete assistant text does not allocate blank space.
 */
XdMessageRow *xd_message_row_new        (XdMessageKind  kind,
                                         const char    *text);

/* Records what produced the message -- model and effort -- as a tooltip. */
void          xd_message_row_set_source (XdMessageRow  *self,
                                         const char    *source);

XdMessageKind xd_message_kind_from_role (const char *role);
const char   *xd_message_kind_to_role   (XdMessageKind kind);

G_END_DECLS
