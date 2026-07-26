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
 * Assistant rows are created empty, hold a spinner while the backend is
 * talking, and are given their text by xd_message_row_set_text() once the
 * message is complete.
 */
XdMessageRow *xd_message_row_new        (XdMessageKind  kind,
                                         const char    *text);

void          xd_message_row_append     (XdMessageRow  *self,
                                         const char    *delta);

const char   *xd_message_row_get_text   (XdMessageRow  *self);

/* Replaces what the row shows. */
void          xd_message_row_set_text   (XdMessageRow  *self,
                                         const char    *text);

/* Records what produced the message -- model and effort -- as a tooltip. */
void          xd_message_row_set_source (XdMessageRow  *self,
                                         const char    *source);

/* Shows the working dots while the row is waiting for its first token. */
void          xd_message_row_set_waiting (XdMessageRow *self,
                                          gboolean      waiting);

XdMessageKind xd_message_kind_from_role (const char *role);
const char   *xd_message_kind_to_role   (XdMessageKind kind);

G_END_DECLS
